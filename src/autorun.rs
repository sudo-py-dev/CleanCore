//! Autorun Manager: programs, services and tasks that start
//! with the system or at login. Enable/disable is reversible; Remove deletes the entry (backed
//! up) where the OS allows it, and otherwise disables it.

use crate::model::Issue;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashSet;
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
    let running: HashSet<String> = crate::platform::list_processes().into_iter().filter_map(|p| p.exe).map(|e| e.to_lowercase()).collect();
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
            e.running = running.contains(&exe.to_lowercase());
            if e.publisher.is_none() {
                e.publisher = publishers.get(exe).cloned();
            }
        } else {
            e.valid = true; // can't tell (e.g. a shell command)
        }
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
    const HEAD: [&str; 7] = ["Name", "State", "Type", "Command", "Publisher", "Location", "Running"];
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
        _ => entries.iter().map(|e| format!("{} [{}] {}\n    {}\n", e.name, row(e)[1], e.source.label(), e.command)).collect(),
    };
    std::fs::write(path, body)?;
    Ok(())
}
