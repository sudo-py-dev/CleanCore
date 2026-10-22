//! Scheduler: Auto Clean and Auto Backup tasks, plus starting the watch recorder at login.
//!
//! Tasks are stored in `<data_dir>/scheduler/tasks.json` and registered with the OS scheduler
//! (systemd user timers on Linux, Task Scheduler on Windows). The OS runs
//! `cleancore schedule exec <id>`, which does the work, records history and shows a notification.

use crate::backup::{Backup, BackupKind};
use crate::config::{data_dir, ensure_dir, Settings};
use crate::engine::{self, Selection};
use crate::i18n::tr;
use crate::model::{ScanMode, Scope};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonthDay {
    Day(u32),
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "every", rename_all = "lowercase")]
pub enum Schedule {
    Daily { time: NaiveTime },
    Weekly { day: Weekday, time: NaiveTime },
    Monthly { day: MonthDay, time: NaiveTime },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    AutoClean {
        mode: ScanMode,
        all_users: bool,
    },
    /// `key: None` = whole registry (Windows) / config store (Linux).
    AutoBackup {
        key: Option<String>,
    },
    /// Windows: remove these Microsoft slop apps (catalog names, `slop::CATALOG`) whenever
    /// Windows has brought them back.
    RemoveSlop {
        apps: Vec<String>,
    },
    /// Windows: remove Edge and OneDrive again if they came back after being removed and
    /// blocked (`keepaway`).
    KeepAway,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRun {
    pub time: DateTime<Local>,
    pub ok: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: String,
    pub schedule: Schedule,
    pub action: Action,
    pub created: DateTime<Local>,
    #[serde(default)]
    pub last_run: Option<LastRun>,
}

impl Task {
    /// Same schedule and same action: the original refuses such duplicates.
    pub fn same_params(&self, other: &Task) -> bool {
        self.schedule == other.schedule && self.action == other.action
    }
}

// ---------------------------------------------------------------- parsing and display

/// `daily 09:00`, `weekly mon 09:00`, `monthly 15 09:00`, `monthly last 21:30`.
pub fn parse_when(s: &str) -> Result<Schedule> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let time = |t: &str| NaiveTime::parse_from_str(t, "%H:%M").with_context(|| format!("bad time '{t}' (use HH:MM, 24-hour)"));
    match parts.as_slice() {
        [every, t] if every.eq_ignore_ascii_case("daily") => Ok(Schedule::Daily { time: time(t)? }),
        [every, d, t] if every.eq_ignore_ascii_case("weekly") => {
            let day: Weekday = d.parse().map_err(|_| anyhow::anyhow!("bad weekday '{d}' (mon..sun)"))?;
            Ok(Schedule::Weekly { day, time: time(t)? })
        }
        [every, d, t] if every.eq_ignore_ascii_case("monthly") => {
            let day = if d.eq_ignore_ascii_case("last") {
                MonthDay::Last
            } else {
                match d.parse::<u32>() {
                    Ok(n @ 1..=28) => MonthDay::Day(n),
                    Ok(29..=31) => bail!("day {d} doesn't exist in every month; use 1-28 or `last`"),
                    _ => bail!("bad day of month '{d}' (1-28 or `last`)"),
                }
            };
            Ok(Schedule::Monthly { day, time: time(t)? })
        }
        _ => bail!("bad schedule '{s}': use `daily HH:MM`, `weekly <mon..sun> HH:MM` or `monthly <1-28|last> HH:MM`"),
    }
}

impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Daily { time } => write!(f, "daily {}", time.format("%H:%M")),
            Self::Weekly { day, time } => write!(f, "weekly {} {}", day, time.format("%H:%M")),
            Self::Monthly { day: MonthDay::Day(d), time } => write!(f, "monthly {d} {}", time.format("%H:%M")),
            Self::Monthly { day: MonthDay::Last, time } => write!(f, "monthly last {}", time.format("%H:%M")),
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::AutoClean { mode, all_users } => {
                write!(f, "Auto Clean, {} scan, {}", format!("{mode:?}").to_lowercase(), if *all_users { "all users" } else { "current user" })
            }
            Self::AutoBackup { key: None } => write!(f, "Auto Backup, whole {}", if cfg!(windows) { "registry" } else { "config store" }),
            Self::AutoBackup { key: Some(k) } => write!(f, "Auto Backup of {k}"),
            Self::RemoveSlop { apps } => write!(f, "Remove Microsoft slop ({} app(s))", apps.len()),
            Self::KeepAway => write!(f, "Keep Edge and OneDrive away"),
        }
    }
}

/// systemd `OnCalendar=` expression.
pub fn on_calendar(s: &Schedule) -> String {
    match s {
        Schedule::Daily { time } => format!("*-*-* {}:00", time.format("%H:%M")),
        Schedule::Weekly { day, time } => format!("{} *-*-* {}:00", day, time.format("%H:%M")),
        Schedule::Monthly { day: MonthDay::Day(d), time } => format!("*-*-{d:02} {}:00", time.format("%H:%M")),
        Schedule::Monthly { day: MonthDay::Last, time } => format!("*-*~01 {}:00", time.format("%H:%M")),
    }
}

/// When a schedule runs, in plain words for the interface.
pub fn describe(s: &Schedule) -> String {
    match s {
        Schedule::Daily { time } => crate::tf!("Every day at {}", time.format("%H:%M")),
        Schedule::Weekly { day, time } => crate::tf!("Every {} at {}", day_name(*day), time.format("%H:%M")),
        Schedule::Monthly { day: MonthDay::Day(d), time } => crate::tf!("On day {d} of every month at {}", time.format("%H:%M")),
        Schedule::Monthly { day: MonthDay::Last, time } => crate::tf!("On the last day of every month at {}", time.format("%H:%M")),
    }
}

/// The day's full name, in the interface language.
pub fn day_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => tr("Monday"),
        Weekday::Tue => tr("Tuesday"),
        Weekday::Wed => tr("Wednesday"),
        Weekday::Thu => tr("Thursday"),
        Weekday::Fri => tr("Friday"),
        Weekday::Sat => tr("Saturday"),
        Weekday::Sun => tr("Sunday"),
    }
}

/// The day's English name, as Task Scheduler's XML spells it.
fn weekday_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Task Scheduler trigger for a schedule, starting from `today`.
pub fn task_trigger(s: &Schedule, today: NaiveDate) -> String {
    let start = |t: &NaiveTime| format!("<StartBoundary>{}T{}</StartBoundary><Enabled>true</Enabled>", today.format("%Y-%m-%d"), t.format("%H:%M:%S"));
    let body = match s {
        Schedule::Daily { time } => format!("{}<ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>", start(time)),
        Schedule::Weekly { day, time } => {
            format!("{}<ScheduleByWeek><WeeksInterval>1</WeeksInterval><DaysOfWeek><{}/></DaysOfWeek></ScheduleByWeek>", start(time), weekday_name(*day))
        }
        Schedule::Monthly { day, time } => {
            let day = match day {
                MonthDay::Day(d) => d.to_string(),
                MonthDay::Last => "Last".into(),
            };
            let months: String = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
                .iter()
                .map(|m| format!("<{m}/>"))
                .collect();
            format!("{}<ScheduleByMonth><DaysOfMonth><Day>{day}</Day></DaysOfMonth><Months>{months}</Months></ScheduleByMonth>", start(time))
        }
    };
    format!("<CalendarTrigger>{body}</CalendarTrigger>")
}

/// Task Scheduler trigger for "when `user` signs in".
pub fn logon_trigger(user: &str) -> String {
    format!("<LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId></LogonTrigger>", xml_escape(user))
}

/// A whole Task Scheduler task, for `schtasks /Create /XML`. It sets what plain `schtasks /Create`
/// can't: a run missed while the PC was off or asleep happens as soon as it can, and tasks run
/// on battery too (Windows' defaults skip both, so a 03:00 clean on a laptop never ran).
/// `time_limit` is an ISO 8601 duration (`PT0S` = none, for the recorder).
pub fn task_xml(description: &str, trigger: &str, user: &str, highest: bool, command: &str, arguments: &str, time_limit: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>{desc}</Description></RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>{level}</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>{time_limit}</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments></Exec></Actions>
</Task>
"#,
        desc = xml_escape(description),
        user = xml_escape(user),
        level = if highest { "HighestAvailable" } else { "LeastPrivilege" },
        command = xml_escape(command),
        arguments = xml_escape(arguments),
    )
}

fn at(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    // `earliest` resolves DST gaps/overlaps instead of skipping the run.
    Local.from_local_datetime(&date.and_time(time)).earliest()
}

fn last_day_of_month(y: i32, m: u32) -> NaiveDate {
    let first_next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) };
    first_next.unwrap() - Duration::days(1)
}

/// The first run strictly after `now`.
pub fn next_run(s: &Schedule, now: DateTime<Local>) -> DateTime<Local> {
    let today = now.date_naive();
    match s {
        Schedule::Daily { time } => (0..3).filter_map(|i| at(today + Duration::days(i), *time)).find(|t| *t > now).unwrap(),
        Schedule::Weekly { day, time } => (0..15).map(|i| today + Duration::days(i)).filter(|d| d.weekday() == *day).filter_map(|d| at(d, *time)).find(|t| *t > now).unwrap(),
        Schedule::Monthly { day, time } => {
            let (mut y, mut m) = (today.year(), today.month());
            loop {
                let date = match day {
                    MonthDay::Day(d) => NaiveDate::from_ymd_opt(y, m, *d),
                    MonthDay::Last => Some(last_day_of_month(y, m)),
                };
                if let Some(t) = date.and_then(|d| at(d, *time)).filter(|t| *t > now) {
                    return t;
                }
                (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            }
        }
    }
}

// ---------------------------------------------------------------- store

fn sched_dir() -> PathBuf {
    data_dir().join("scheduler")
}

fn tasks_file() -> PathBuf {
    sched_dir().join("tasks.json")
}

fn history_file() -> PathBuf {
    sched_dir().join("history.jsonl")
}

pub fn load_tasks() -> Result<Vec<Task>> {
    match std::fs::read_to_string(tasks_file()) {
        Ok(s) => serde_json::from_str(&s).context("parsing tasks.json"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_tasks(tasks: &[Task]) -> Result<()> {
    ensure_dir(&sched_dir())?;
    let tmp = tasks_file().with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(tasks)?)?;
    std::fs::rename(tmp, tasks_file())?;
    Ok(())
}

/// URL-ish id from the name, unique among `existing`.
pub fn make_id(name: &str, existing: &[Task]) -> String {
    let mut base: String =
        name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    base.truncate(40);
    if base.is_empty() {
        base = "task".into();
    }
    let mut id = base.clone();
    let mut n = 1;
    while existing.iter().any(|t| t.id == id) {
        n += 1;
        id = format!("{base}-{n}");
    }
    id
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub task: String,
    pub name: String,
    pub time: DateTime<Local>,
    pub ok: bool,
    pub summary: String,
}

pub fn history() -> Result<Vec<HistoryEntry>> {
    let Ok(f) = std::fs::File::open(history_file()) else {
        return Ok(Vec::new());
    };
    Ok(std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect())
}

// ---------------------------------------------------------------- OS registration

/// The command line the OS scheduler should run (absolute exe, plus the data folder if overridden).
pub fn exe_args(args: &[&str]) -> Result<(PathBuf, Vec<String>)> {
    let mut exe = std::env::current_exe().context("locating the cleancore executable")?;
    // The GUI schedules work for the command-line binary that ships next to it.
    if exe.file_stem().is_some_and(|s| s == "cleancore-gui") {
        exe = exe.with_file_name(if cfg!(windows) { "cleancore.exe" } else { "cleancore" });
    }
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("CLEANCORE_DATA_DIR") {
        v.push("--data-dir".to_string());
        v.push(d.to_string_lossy().to_string());
    }
    v.extend(args.iter().map(|s| s.to_string()));
    Ok((exe, v))
}

/// What `platform::sched_status` says about a task the system scheduler doesn't have.
pub const NOT_REGISTERED: &str = "not registered";

pub fn register(task: &Task) -> Result<()> {
    let (exe, args) = exe_args(&["schedule", "exec", &task.id])?;
    crate::platform::sched_register(task, &exe, &args)
}

pub fn unregister(id: &str) -> Result<()> {
    crate::platform::sched_unregister(id)
}

// ---------------------------------------------------------------- running a task

/// Run one task now: do the work, record the result, notify. Returns the summary.
pub fn execute(settings: &Settings, id: &str) -> Result<String> {
    let mut tasks = load_tasks()?;
    let task = tasks.iter().find(|t| t.id == id).cloned().with_context(|| format!("no scheduled task '{id}'"))?;
    let result = run_action(settings, &task);
    let (ok, summary) = match &result {
        Ok(s) => (true, s.clone()),
        Err(e) => (false, crate::tf!("Failed: {e:#}")),
    };

    let now = Local::now();
    if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
        t.last_run = Some(LastRun { time: now, ok, summary: summary.clone() });
    }
    save_tasks(&tasks)?;
    ensure_dir(&sched_dir())?;
    let entry = HistoryEntry { task: task.id.clone(), name: task.name.clone(), time: now, ok, summary: summary.clone() };
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(history_file())?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;

    let (title, wanted) = match task.action {
        Action::AutoClean { .. } => (if ok { tr("Auto Clean Completed") } else { tr("Auto Clean Failed") }, settings.notify_auto_clean),
        Action::AutoBackup { .. } => (if ok { tr("Auto Backup Completed") } else { tr("Auto Backup Failed") }, settings.notify_auto_backup),
        Action::RemoveSlop { .. } | Action::KeepAway => (if ok { tr("Automatic App Removal Completed") } else { tr("Automatic App Removal Failed") }, settings.notify_auto_clean),
    };
    // Failures are always shown: a silent failing schedule is worse than an extra popup.
    if wanted || !ok {
        if let Err(e) = crate::platform::notify(&format!("CleanCore: {title}"), &format!("{}: {summary}", task.name)) {
            eprintln!("warning: notification failed: {e:#}");
        }
    }
    result.map(|_| summary)
}

fn run_action(settings: &Settings, task: &Task) -> Result<String> {
    match &task.action {
        Action::AutoClean { mode, all_users } => {
            let scope = if *all_users { Scope::AllUsers } else { Scope::CurrentUser };
            let issues = engine::run_scan(settings, &Selection::Mode(*mode), scope);
            if issues.is_empty() {
                return Ok(tr("Nothing to clean").to_string());
            }
            let r = crate::cleaner::clean(settings, &issues)?;
            Ok(if r.failed.is_empty() {
                crate::tf!("Removed {} item(s), backup {}", r.cleaned, r.backup_id)
            } else {
                crate::tf!("Removed {} item(s), backup {}, {} could not be removed", r.cleaned, r.backup_id, r.failed.len())
            })
        }
        Action::KeepAway => crate::keepaway::enforce(),
        Action::RemoveSlop { apps } => {
            let scan = crate::slop::scan()?;
            let back: Vec<crate::slop::Found> = scan.found.into_iter().filter(|f| apps.iter().any(|a| a == f.app.name)).collect();
            if back.is_empty() {
                return Ok("none of the apps came back".into());
            }
            // As administrator (the task runs with the highest rights it can): for every user,
            // and the preinstalled copies too, so they stay away.
            let r = crate::slop::remove(settings, &back, scan.admin, scan.admin);
            let mut s = format!("removed {}", if r.removed.is_empty() { "nothing".to_string() } else { r.removed.join(", ") });
            if !r.skipped.is_empty() {
                s.push_str(&format!("; already gone: {}", r.skipped.join(", ")));
            }
            if !r.failed.is_empty() {
                s.push_str(&format!("; failed: {}", r.failed.iter().map(|(n, e)| format!("{n} ({e})")).collect::<Vec<_>>().join("; ")));
                anyhow::bail!(s);
            }
            Ok(s)
        }
        Action::AutoBackup { key } => {
            let kind = if key.is_none() { BackupKind::AutoFull } else { BackupKind::AutoPartial };
            let mut b = Backup::create(settings, &task.name, &task.note, kind)?;
            let entries = match key {
                None => crate::platform::backup_full(&b.dir),
                Some(k) => crate::platform::backup_key(&b.dir, k),
            };
            match entries {
                Ok(e) => b.manifest.entries = e,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&b.dir);
                    return Err(e);
                }
            }
            b.save()?;
            let pruned = crate::backup::prune(settings)?;
            Ok(if pruned.is_empty() {
                crate::tf!("Created backup {} ({} entries)", b.manifest.id, b.manifest.entries.len())
            } else {
                crate::tf!("Created backup {} ({} entries), deleted {} old backup(s)", b.manifest.id, b.manifest.entries.len(), pruned.len())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slop_removal_task_round_trips() {
        let a = Action::RemoveSlop { apps: vec!["News".into(), "Xbox".into()] };
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, r#"{"kind":"remove_slop","apps":["News","Xbox"]}"#);
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
        assert_eq!(a.to_string(), "Remove Microsoft slop (2 app(s))");
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        at(NaiveDate::from_ymd_opt(y, mo, d).unwrap(), t(h, mi)).unwrap()
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_when("daily 09:30").unwrap(), Schedule::Daily { time: t(9, 30) });
        assert_eq!(parse_when("weekly Mon 18:00").unwrap(), Schedule::Weekly { day: Weekday::Mon, time: t(18, 0) });
        assert_eq!(parse_when("monthly last 23:59").unwrap(), Schedule::Monthly { day: MonthDay::Last, time: t(23, 59) });
        assert!(parse_when("monthly 31 10:00").is_err());
        assert!(parse_when("daily 25:00").is_err());
        assert!(parse_when("hourly 10:00").is_err());
        let s = parse_when("monthly 5 07:05").unwrap();
        assert_eq!(parse_when(&s.to_string()).unwrap(), s);
    }

    #[test]
    fn next_runs() {
        let now = local(2026, 10, 17, 10, 0); // a Saturday
        assert_eq!(next_run(&parse_when("daily 11:00").unwrap(), now), local(2026, 10, 17, 11, 0));
        assert_eq!(next_run(&parse_when("daily 09:00").unwrap(), now), local(2026, 10, 18, 9, 0));
        assert_eq!(next_run(&parse_when("weekly sat 10:00").unwrap(), now), local(2026, 10, 24, 10, 0));
        assert_eq!(next_run(&parse_when("weekly mon 08:00").unwrap(), now), local(2026, 10, 19, 8, 0));
        assert_eq!(next_run(&parse_when("monthly 15 09:00").unwrap(), now), local(2026, 11, 15, 9, 0));
        assert_eq!(next_run(&parse_when("monthly last 09:00").unwrap(), now), local(2026, 10, 31, 9, 0));
        let dec = local(2026, 12, 31, 12, 0);
        assert_eq!(next_run(&parse_when("monthly last 09:00").unwrap(), dec), local(2027, 1, 31, 9, 0));
        let feb = local(2028, 2, 1, 0, 0);
        assert_eq!(next_run(&parse_when("monthly last 09:00").unwrap(), feb), local(2028, 2, 29, 9, 0));
    }

    #[test]
    fn os_formats() {
        assert_eq!(on_calendar(&parse_when("weekly mon 09:00").unwrap()), "Mon *-*-* 09:00:00");
        assert_eq!(on_calendar(&parse_when("monthly 5 09:00").unwrap()), "*-*-05 09:00:00");
        assert_eq!(on_calendar(&parse_when("monthly last 21:30").unwrap()), "*-*~01 21:30:00");
        let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        assert_eq!(
            task_trigger(&parse_when("weekly fri 08:15").unwrap(), today),
            "<CalendarTrigger><StartBoundary>2026-10-22T08:15:00</StartBoundary><Enabled>true</Enabled><ScheduleByWeek><WeeksInterval>1</WeeksInterval><DaysOfWeek><Friday/></DaysOfWeek></ScheduleByWeek></CalendarTrigger>"
        );
        assert!(task_trigger(&parse_when("monthly last 08:15").unwrap(), today).contains("<DaysOfMonth><Day>Last</Day></DaysOfMonth><Months><January/>"));
        assert!(task_trigger(&parse_when("daily 03:00").unwrap(), today).contains("<ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>"));
    }

    #[test]
    fn windows_task_runs_when_missed_and_on_battery() {
        let xml = task_xml(
            "Weekly <clean>",
            &logon_trigger(r"PC\me"),
            r"PC\me",
            true,
            "conhost.exe",
            r#"--headless "C:\Program Files\CleanCore\cleancore.exe" schedule exec weekly"#,
            "PT4H",
        );
        for want in [
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<RunLevel>HighestAvailable</RunLevel>",
            "<ExecutionTimeLimit>PT4H</ExecutionTimeLimit>",
            "<Description>Weekly &lt;clean&gt;</Description>",
            "&quot;C:\\Program Files\\CleanCore\\cleancore.exe&quot;",
        ] {
            assert!(xml.contains(want), "{want}");
        }
    }

    #[test]
    fn ids() {
        let mk = |id: &str| Task {
            id: id.into(),
            name: String::new(),
            note: String::new(),
            schedule: Schedule::Daily { time: t(1, 0) },
            action: Action::AutoBackup { key: None },
            created: Local::now(),
            last_run: None,
        };
        assert_eq!(make_id("Weekly Clean!", &[]), "weekly-clean");
        assert_eq!(make_id("Weekly Clean", &[mk("weekly-clean")]), "weekly-clean-2");
        assert_eq!(make_id("***", &[]), "task");
    }
}
