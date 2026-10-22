//! Scheduler: Auto Clean and Auto Backup tasks, plus starting the watch recorder at login.
//!
//! Tasks are stored in `<data_dir>/scheduler/tasks.json` and registered with the OS scheduler
//! (systemd user timers on Linux, Task Scheduler on Windows). The OS runs
//! `cleancore schedule exec <id>`, which does the work, records history and shows a notification.

use crate::backup::{Backup, BackupKind};
use crate::config::{data_dir, ensure_dir, Settings};
use crate::engine::{self, Selection};
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

/// `schtasks /Create` schedule arguments.
pub fn schtasks_args(s: &Schedule) -> Vec<String> {
    let st = |t: &NaiveTime| t.format("%H:%M").to_string();
    let day3 = |d: &Weekday| format!("{d}").to_uppercase();
    match s {
        Schedule::Daily { time } => vec!["/SC".into(), "DAILY".into(), "/ST".into(), st(time)],
        Schedule::Weekly { day, time } => vec!["/SC".into(), "WEEKLY".into(), "/D".into(), day3(day), "/ST".into(), st(time)],
        Schedule::Monthly { day: MonthDay::Day(d), time } => vec!["/SC".into(), "MONTHLY".into(), "/D".into(), d.to_string(), "/ST".into(), st(time)],
        Schedule::Monthly { day: MonthDay::Last, time } => {
            vec!["/SC".into(), "MONTHLY".into(), "/MO".into(), "LASTDAY".into(), "/M".into(), "*".into(), "/ST".into(), st(time)]
        }
    }
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
        Err(e) => (false, format!("failed: {e:#}")),
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
        Action::AutoClean { .. } => (if ok { "Auto Clean Completed" } else { "Auto Clean Failed" }, settings.notify_auto_clean),
        Action::AutoBackup { .. } => (if ok { "Auto Backup Completed" } else { "Auto Backup Failed" }, settings.notify_auto_backup),
        Action::RemoveSlop { .. } | Action::KeepAway => (if ok { "Automatic App Removal Completed" } else { "Automatic App Removal Failed" }, settings.notify_auto_clean),
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
                return Ok("nothing to clean".into());
            }
            let r = crate::cleaner::clean(settings, &issues)?;
            let mut s = format!("removed {} item(s), backup {}", r.cleaned, r.backup_id);
            if !r.failed.is_empty() {
                s.push_str(&format!(", {} failed", r.failed.len()));
            }
            Ok(s)
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
            let mut s = format!("created backup {} ({} entries)", b.manifest.id, b.manifest.entries.len());
            if !pruned.is_empty() {
                s.push_str(&format!(", auto-deleted {} old backup(s)", pruned.len()));
            }
            Ok(s)
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
        assert_eq!(schtasks_args(&parse_when("weekly fri 08:15").unwrap()).join(" "), "/SC WEEKLY /D FRI /ST 08:15");
        assert_eq!(schtasks_args(&parse_when("monthly last 08:15").unwrap()).join(" "), "/SC MONTHLY /MO LASTDAY /M * /ST 08:15");
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
