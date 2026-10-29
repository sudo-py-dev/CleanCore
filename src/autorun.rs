//! Autorun Manager: programs, services and tasks that start
//! with the system or at login. Enable/disable is reversible; Remove deletes the entry (backed
//! up) where the OS allows it, and otherwise disables it.

use crate::model::Issue;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Source {
    /// Windows `Run` / `RunOnce` registry values.
    Run,
    RunOnce,
    /// Windows Startup folder / Linux XDG autostart.
    StartupFolder,
    Service,
    ScheduledTask,
    /// systemd user unit (Linux).
    UserService,
    /// cron `@reboot` (Linux).
    Cron,
    /// A program started in the background from `~/.xprofile`, `~/.xsessionrc` or `~/.profile` (Linux).
    LoginScript,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "Registry Run",
            Self::RunOnce => "Registry RunOnce",
            Self::StartupFolder => {
                if cfg!(windows) {
                    "Startup folder"
                } else {
                    "Autostart"
                }
            }
            Self::Service => "Service",
            Self::ScheduledTask => "Scheduled task",
            Self::UserService => "User service",
            Self::Cron => "Cron @reboot",
            Self::LoginScript => "Login script",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AutorunEntry {
    /// Stable id: `<source>:<location>`.
    pub id: String,
    pub name: String,
    pub command: String,
    /// Registry `key | value`, file path, unit name or crontab line.
    pub location: String,
    pub source: Source,
    /// For all users (machine-wide) rather than the current user.
    pub all_users: bool,
    pub enabled: bool,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub exe: Option<String>,
    pub running: bool,
    /// The program it starts exists.
    pub valid: bool,
    /// Can be deleted (otherwise Remove disables it).
    pub removable: bool,
    /// Part of the operating system.
    pub system: bool,
    /// What it costs at startup, a friendly name and whether it should stay on.
    pub startup: Startup,
}

/// How much an entry slows down startup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Impact {
    #[default]
    Unknown,
    Low,
    Medium,
    High,
}

impl Impact {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Not measured",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// Why an entry is better left on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Need {
    #[default]
    No,
    /// Part of Windows / the desktop.
    System,
    /// A driver helper (sound, touchpad, graphics).
    Hardware,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Startup {
    /// A name a person recognises ("Microsoft OneDrive" for `OneDrive`), when the program has one.
    pub friendly: Option<String>,
    pub impact: Impact,
    /// Read from what the system recorded at startup; otherwise estimated from the program's size.
    pub measured: bool,
    /// CPU time and disk reads at startup.
    pub cpu_ms: Option<u64>,
    pub disk_bytes: Option<u64>,
    /// How long it delayed startup.
    pub delay_ms: Option<u64>,
    /// Size of the program file (the estimate).
    pub size: Option<u64>,
    pub need: Need,
}

/// What the system recorded about the last startups.
#[derive(Debug, Default)]
pub struct BootData {
    /// Program path (lower case) → CPU ms and disk bytes at startup (Task Manager's data).
    pub usage: HashMap<String, (u64, u64)>,
    /// Program file name or unit name (lower case) → how long it delayed startup, ms.
    pub delay: HashMap<String, u64>,
}

const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;

/// Task Manager's rule: High above 1 s of CPU or 3 MB of disk, Low below 300 ms and 300 KB.
pub fn classify_usage(cpu_ms: u64, disk_bytes: u64) -> Impact {
    if cpu_ms > 1000 || disk_bytes > 3 * MB {
        Impact::High
    } else if cpu_ms >= 300 || disk_bytes >= 300 * KB {
        Impact::Medium
    } else {
        Impact::Low
    }
}

/// Time it added to startup.
pub fn classify_delay(ms: u64) -> Impact {
    match ms {
        1000.. => Impact::High,
        300.. => Impact::Medium,
        _ => Impact::Low,
    }
}

/// The estimate: big programs (browsers, chat apps built on Electron) load more at startup.
pub fn classify_size(bytes: u64) -> Impact {
    match bytes {
        b if b >= 50 * MB => Impact::High,
        b if b >= 5 * MB => Impact::Medium,
        _ => Impact::Low,
    }
}

/// Measured figures, if any, else the size estimate. `early` is the CPU ms and disk bytes the
/// entry's process used since it started with this login, when CleanCore runs shortly after it.
pub fn measure(e: &AutorunEntry, boot: &BootData, early: Option<(u64, u64)>, size: Option<u64>) -> Startup {
    let mut s = Startup { size, ..Default::default() };
    let exe = e.exe.as_deref().map(str::to_lowercase);
    let file = exe.as_deref().and_then(|x| x.rsplit(['\\', '/']).next()).map(str::to_string);
    let mut found = Vec::new();
    if let Some(&(cpu, disk)) = exe.as_ref().and_then(|x| boot.usage.get(x)) {
        s.cpu_ms = Some(cpu);
        s.disk_bytes = Some(disk);
        found.push(classify_usage(cpu, disk));
    } else if let Some((cpu, disk)) = early {
        s.cpu_ms = Some(cpu);
        s.disk_bytes = Some(disk);
        found.push(classify_usage(cpu, disk));
    }
    let unit = matches!(e.source, Source::Service | Source::UserService).then(|| e.location.to_lowercase());
    if let Some(&ms) = file.iter().chain(unit.iter()).find_map(|k| boot.delay.get(k)) {
        s.delay_ms = Some(ms);
        found.push(classify_delay(ms));
    }
    if let Some(&worst) = found.iter().max() {
        s.impact = worst;
        s.measured = true;
    } else if let Some(b) = size {
        s.impact = classify_size(b);
    }
    s
}

/// Makers of drivers whose startup helpers keep sound, touchpads and graphics working.
const HARDWARE: &[&str] = &["realtek", "intel", "nvidia", "advanced micro devices", "synaptics", "elan microelectronics", "dolby", "waves audio", "conexant", "cirrus logic"];
/// Windows programs started from Run keys that Windows itself needs.
const WINDOWS_NEEDS: &[&str] = &["securityhealthsystray.exe", "ctfmon.exe"];

pub fn need_of(e: &AutorunEntry) -> Need {
    let file = e.exe.as_deref().and_then(|x| x.rsplit(['\\', '/']).next()).map(str::to_lowercase).unwrap_or_default();
    if e.system || WINDOWS_NEEDS.contains(&file.as_str()) {
        Need::System
    } else if e.publisher.as_deref().map(str::to_lowercase).is_some_and(|p| HARDWARE.iter().any(|h| p.contains(h))) {
        Need::Hardware
    } else {
        Need::No
    }
}

/// CPU ms and disk bytes of the entry's running process, if it started with this login and the
/// system started less than 20 minutes ago: then its use so far is what it cost at startup.
fn early_usage(pids: &[u32], uptime_ms: u64) -> Option<(u64, u64)> {
    const WINDOW: u64 = 20 * 60 * 1000;
    if uptime_ms > WINDOW {
        return None;
    }
    let mut total: Option<(u64, u64)> = None;
    for &pid in pids {
        let Some(u) = crate::platform::process_usage(pid) else { continue };
        // Started within 5 minutes of boot: part of startup, not opened later by hand.
        if uptime_ms.saturating_sub(u.age_ms) > 5 * 60 * 1000 {
            continue;
        }
        let t = total.get_or_insert((0, 0));
        t.0 += u.cpu_ms;
        t.1 += u.io_bytes;
    }
    total
}

/// Resource use of a running process.
#[derive(Debug, Clone, Copy)]
pub struct ProcUsage {
    /// Time since the process started.
    pub age_ms: u64,
    pub cpu_ms: u64,
    pub io_bytes: u64,
}

impl AutorunEntry {
    /// The name shown to people: the program's own name when it has one.
    pub fn display_name(&self) -> &str {
        self.startup.friendly.as_deref().unwrap_or(&self.name)
    }
}

/// How a new entry is added: Registry Run / RunOnce / Startup folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddMethod {
    Run,
    RunOnce,
    StartupFolder,
}

/// All entries with exe, publisher, running and validity filled in.
pub fn list(show_system: bool) -> Vec<AutorunEntry> {
    let mut v = crate::platform::autorun_list(show_system);
    let mut running: HashMap<String, Vec<u32>> = HashMap::new();
    for p in crate::platform::list_processes() {
        if let Some(exe) = p.exe {
            running.entry(exe.to_lowercase()).or_default().push(p.pid);
        }
    }
    for e in &mut v {
        if e.exe.is_none() {
            e.exe = exe_of(&e.command);
        }
    }
    let exes: Vec<String> = v.iter().filter(|e| e.publisher.is_none()).filter_map(|e| e.exe.clone()).collect();
    let publishers = crate::platform::file_publishers(&exes);
    for e in &mut v {
        if let Some(exe) = &e.exe {
            e.valid = Path::new(exe).exists();
            e.running = running.contains_key(&exe.to_lowercase());
            if e.publisher.is_none() {
                e.publisher = publishers.get(exe).cloned();
            }
        } else {
            e.valid = true; // can't tell (e.g. a shell command)
        }
    }
    let boot = crate::platform::boot_data();
    let uptime = crate::platform::uptime_ms();
    for e in &mut v {
        let pids = e.exe.as_deref().and_then(|x| running.get(&x.to_lowercase()));
        let early = pids.zip(uptime).and_then(|(p, up)| early_usage(p, up));
        let size = e.exe.as_deref().and_then(|x| std::fs::metadata(x).ok()).filter(|m| m.is_file()).map(|m| m.len());
        let mut s = measure(e, &boot, early, size);
        s.need = need_of(e);
        // Services and tasks already have a display name; Run values and shortcuts often don't.
        if matches!(e.source, Source::Run | Source::RunOnce | Source::StartupFolder) {
            s.friendly = e.exe.as_deref().and_then(crate::platform::file_description).filter(|d| d.chars().count() >= 3 && *d != e.name);
        }
        e.startup = s;
    }
    v.sort_by(|a, b| (!a.enabled, a.source.label(), a.name.to_lowercase()).cmp(&(!b.enabled, b.source.label(), b.name.to_lowercase())));
    v
}

/// The program a command line starts.
pub fn exe_of(command: &str) -> Option<String> {
    if cfg!(windows) {
        return crate::pathutil::extract_windows_path(command);
    }
    let prog = crate::pathutil::desktop_exec_program(command)?;
    let prog = if prog == "sh" || prog == "bash" || prog.ends_with("/sh") || prog.ends_with("/bash") {
        // sh -c "real command"
        return command.split_whitespace().skip_while(|w| *w != "-c").nth(1).map(|s| s.trim_matches(['"', '\'']).to_string()).and_then(|c| exe_of(&c));
    } else {
        prog
    };
    if prog.starts_with('/') {
        return Some(prog);
    }
    crate::pathutil::which(&prog, &crate::pathutil::PathCache::default()).map(|p| p.display().to_string())
}

pub fn set_enabled(e: &AutorunEntry, on: bool) -> Result<()> {
    crate::platform::autorun_set_enabled(e, on)
}

/// Remove an entry (backed up). Entries the OS doesn't let us delete are disabled instead.
pub fn remove(settings: &crate::config::Settings, entries: &[AutorunEntry]) -> Result<(usize, usize)> {
    let mut issues: Vec<Issue> = Vec::new();
    let mut disabled = 0;
    // Entries removed by the platform itself (crontab, login scripts: their file is copied first).
    let mut direct = 0;
    for e in entries {
        if e.removable {
            let found = crate::platform::autorun_removal(e)?;
            if found.is_empty() {
                direct += 1;
            }
            issues.extend(found);
        } else if e.enabled {
            crate::platform::autorun_set_enabled(e, false)?;
            disabled += 1;
        }
    }
    if issues.is_empty() {
        return Ok((direct, disabled));
    }
    let mut s = settings.clone();
    s.restore_point_before_clean = false;
    s.full_backup_before_clean = false;
    let rep = crate::cleaner::clean(&s, &issues)?;
    if let Some((loc, err)) = rep.failed.first() {
        bail!("{loc}: {err}");
    }
    Ok((rep.cleaned + direct, disabled))
}

/// Start the entry's program now.
pub fn run_now(e: &AutorunEntry) -> Result<()> {
    crate::platform::autorun_run(e)
}

/// Stop the entry's running program. Returns how many processes were stopped.
pub fn kill(e: &AutorunEntry) -> Result<usize> {
    let exe = e.exe.as_deref().map(str::to_lowercase).ok_or_else(|| anyhow::anyhow!("CleanCore can't tell which program \"{}\" starts", e.name))?;
    let mut n = 0;
    for p in crate::platform::list_processes() {
        if p.exe.as_deref().map(str::to_lowercase).as_ref() == Some(&exe) && p.pid != std::process::id() {
            crate::platform::kill_process(p.pid)?;
            n += 1;
        }
    }
    Ok(n)
}

pub fn add(name: &str, command: &str, all_users: bool, method: AddMethod) -> Result<()> {
    if name.trim().is_empty() || command.trim().is_empty() {
        bail!("a name and a command are needed");
    }
    crate::platform::autorun_add(name.trim(), command.trim(), all_users, method)
}

pub fn export(entries: &[AutorunEntry], path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_ascii_lowercase();
    let row = |e: &AutorunEntry| -> [String; 7] {
        [
            e.name.clone(),
            if e.enabled { "Enabled".into() } else { "Disabled".into() },
            e.source.label().into(),
            e.command.clone(),
            e.publisher.clone().unwrap_or_default(),
            e.location.clone(),
            if e.running { "Running".into() } else { String::new() },
        ]
    };
    const HEAD: [&str; 8] = ["Name", "Startup impact", "State", "Type", "Command", "Publisher", "Location", "Running"];
    if ext == "xlsx" {
        return crate::xlsx::write(path, "Autorun", &HEAD, &entries.iter().map(|e| row(e).to_vec()).collect::<Vec<_>>());
    }
    let body = match ext.as_str() {
        "csv" | "xls" => {
            let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            let mut s = HEAD.join(",") + "\r\n";
            for e in entries {
                s += &row(e).iter().map(|c| q(c)).collect::<Vec<_>>().join(",");
                s += "\r\n";
            }
            s
        }
        "html" | "htm" => {
            let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let mut s = String::from("<!doctype html><meta charset=utf-8><title>Autorun</title><style>body{font:14px system-ui;margin:2em}td,th{padding:4px 8px;border-bottom:1px solid #ddd;text-align:left}</style><table><tr>");
            for h in HEAD {
                s += &format!("<th>{h}</th>");
            }
            s += "</tr>";
            for e in entries {
                s += "<tr>";
                for c in row(e) {
                    s += &format!("<td>{}</td>", esc(&c));
                }
                s += "</tr>";
            }
            s + "</table>"
        }
        "json" => serde_json::to_string_pretty(entries)?,
        _ => entries.iter().map(|e| format!("{} [{}] {}\n    {}\n", e.name, row(e)[2], e.source.label(), e.command)).collect(),
    };
    std::fs::write(path, body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(exe: &str, source: Source, location: &str) -> AutorunEntry {
        AutorunEntry {
            id: "x".into(),
            name: "x".into(),
            command: exe.into(),
            location: location.into(),
            source,
            all_users: false,
            enabled: true,
            publisher: None,
            description: None,
            exe: Some(exe.into()),
            running: false,
            valid: true,
            removable: true,
            system: false,
            startup: Startup::default(),
        }
    }

    #[test]
    fn usage_follows_task_manager() {
        assert_eq!(classify_usage(100, 100 * KB), Impact::Low);
        assert_eq!(classify_usage(300, 0), Impact::Medium);
        assert_eq!(classify_usage(0, 300 * KB), Impact::Medium);
        assert_eq!(classify_usage(1000, 3 * MB), Impact::Medium);
        assert_eq!(classify_usage(1001, 0), Impact::High);
        assert_eq!(classify_usage(0, 3 * MB + 1), Impact::High);
    }

    #[test]
    fn delay_and_size() {
        assert_eq!(classify_delay(299), Impact::Low);
        assert_eq!(classify_delay(300), Impact::Medium);
        assert_eq!(classify_delay(2500), Impact::High);
        assert_eq!(classify_size(MB), Impact::Low);
        assert_eq!(classify_size(5 * MB), Impact::Medium);
        assert_eq!(classify_size(150 * MB), Impact::High);
        assert!(Impact::High > Impact::Medium && Impact::Low > Impact::Unknown);
    }

    #[test]
    fn measured_data_wins_over_the_estimate() {
        let e = entry(r"C:\Apps\Chat\Chat.exe", Source::Run, "");
        // No data: estimated from the size, or unknown without a file.
        let s = measure(&e, &BootData::default(), None, Some(120 * MB));
        assert_eq!((s.impact, s.measured), (Impact::High, false));
        assert_eq!(measure(&e, &BootData::default(), None, None).impact, Impact::Unknown);
        // Task Manager's figures, matched by path in any case.
        let mut boot = BootData::default();
        boot.usage.insert(r"c:\apps\chat\chat.exe".into(), (120, 10 * KB));
        let s = measure(&e, &boot, Some((5000, 0)), Some(120 * MB));
        assert_eq!((s.impact, s.measured, s.cpu_ms), (Impact::Low, true, Some(120)));
        // A logged startup delay, matched by file name, raises it.
        boot.delay.insert("chat.exe".into(), 4200);
        let s = measure(&e, &boot, None, None);
        assert_eq!((s.impact, s.delay_ms), (Impact::High, Some(4200)));
    }

    #[test]
    fn early_use_and_units_count_as_measured() {
        let e = entry("/usr/bin/syncd", Source::StartupFolder, "");
        let s = measure(&e, &BootData::default(), Some((400, 0)), Some(KB));
        assert_eq!((s.impact, s.measured), (Impact::Medium, true));
        let mut boot = BootData::default();
        boot.delay.insert("docker.service".into(), 1800);
        let svc = entry("/usr/bin/dockerd", Source::Service, "docker.service");
        assert_eq!(measure(&svc, &boot, None, None).impact, Impact::High);
    }

    #[test]
    fn needs() {
        let mut e = entry(r"C:\Windows\System32\SecurityHealthSystray.exe", Source::Run, "");
        assert_eq!(need_of(&e), Need::System);
        e.exe = Some(r"C:\Program Files\Realtek\Audio\RtkAudUService64.exe".into());
        e.publisher = Some("Realtek Semiconductor".into());
        assert_eq!(need_of(&e), Need::Hardware);
        e.publisher = Some("Spotify AB".into());
        assert_eq!(need_of(&e), Need::No);
    }
}
