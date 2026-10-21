//! Windows autorun sources: Run / RunOnce (machine, 32-bit, user), Startup folders, services,
//! logon / boot scheduled tasks. Enable/disable uses the `StartupApproved` flags Task Manager
//! uses for Run entries and Startup items, `sc config` for services and `schtasks` for tasks.

use super::windows::{get_str, get_u32, join, open, read_raw, reg_string, subkeys, values, write_raw};
use crate::autorun::{AddMethod, AutorunEntry, Source};
use crate::model::{Fix, Issue};
use crate::tuner::RawValue;
use anyhow::{bail, Context, Result};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const HKLM: &str = "HKEY_LOCAL_MACHINE";
const HKCU: &str = "HKEY_CURRENT_USER";
const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUNONCE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce";
const RUN32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const RUNONCE32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
const NO_WINDOW: u32 = 0x0800_0000;

fn env(v: &str) -> Option<PathBuf> {
    std::env::var_os(v).map(PathBuf::from)
}

fn startup_dirs() -> Vec<(PathBuf, bool)> {
    let mut v = Vec::new();
    if let Some(a) = env("APPDATA") {
        v.push((a.join(r"Microsoft\Windows\Start Menu\Programs\Startup"), false));
    }
    if let Some(p) = env("ProgramData") {
        v.push((p.join(r"Microsoft\Windows\Start Menu\Programs\StartUp"), true));
    }
    v
}

/// `StartupApproved` key + value name for an entry, and whether it is currently disabled.
fn approved(hive: &str, sub: &str, name: &str) -> (String, bool) {
    let key = join(hive, &format!(r"{APPROVED}\{sub}"));
    let disabled = read_raw(&key, name).is_some_and(|r| r.bytes.first().is_some_and(|b| b & 1 == 1));
    (key, disabled)
}

fn is_microsoft(publisher: &Option<String>) -> bool {
    publisher.as_deref().is_some_and(|p| p.starts_with("Microsoft"))
}

fn entry(id: String, name: String, command: String, location: String, source: Source, all_users: bool, enabled: bool, removable: bool) -> AutorunEntry {
    AutorunEntry { id, name, command, location, source, all_users, enabled, publisher: None, description: None, exe: None, running: false, valid: true, removable, system: false }
}

fn task_entries(show_system: bool, out: &mut Vec<AutorunEntry>) {
    let Some(root) = env("SystemRoot").map(|r| r.join(r"System32\Tasks")) else {
        return;
    };
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let xml = if bytes.starts_with(&[0xFF, 0xFE]) {
                let w: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&w)
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            if !xml.contains("<LogonTrigger") && !xml.contains("<BootTrigger") {
                continue;
            }
            let tag = |t: &str| xml.split(&format!("<{t}>")).nth(1).and_then(|r| r.split(&format!("</{t}>")).next()).map(|s| s.trim().to_string());
            let rel = format!(r"\{}", p.strip_prefix(&root).unwrap_or(&p).display());
            let author = tag("Author");
            let system = rel.starts_with(r"\Microsoft\") || author.as_deref().is_some_and(|a| a.contains("Microsoft"));
            if system && !show_system {
                continue;
            }
            let command = format!("{} {}", tag("Command").unwrap_or_default(), tag("Arguments").unwrap_or_default()).trim().to_string();
            let mut en = entry(
                format!("task:{rel}"),
                rel.rsplit('\\').next().unwrap_or(&rel).to_string(),
                command,
                rel.clone(),
                Source::ScheduledTask,
                true,
                tag("Enabled").as_deref() != Some("false"),
                false,
            );
            en.description = tag("Description");
            en.system = system;
            out.push(en);
        }
    }
}

pub fn autorun_list(show_system: bool) -> Vec<AutorunEntry> {
    let mut v = Vec::new();
    // Run / RunOnce
    for (hive, sub, once, approved_sub, all) in [
        (HKLM, RUN, false, "Run", true),
        (HKLM, RUN32, false, "Run32", true),
        (HKLM, RUNONCE, true, "", true),
        (HKLM, RUNONCE32, true, "", true),
        (HKCU, RUN, false, "Run", false),
        (HKCU, RUNONCE, true, "", false),
    ] {
        let key = join(hive, sub);
        for (name, val) in values(&key) {
            let Some(cmd) = reg_string(&val) else {
                continue;
            };
            let enabled = approved_sub.is_empty() || !approved(hive, approved_sub, &name).1;
            v.push(entry(format!("run:{key}|{name}"), name.clone(), cmd, format!("{key} | {name}"), if once { Source::RunOnce } else { Source::Run }, all, enabled, true));
        }
    }
    // Startup folders
    for (dir, common) in startup_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let hive = if common { HKLM } else { HKCU };
            let enabled = !approved(hive, "StartupFolder", &n).1;
            v.push(entry(
                format!("startup:{}", e.path().display()),
                Path::new(&n).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(n.clone()),
                e.path().display().to_string(),
                e.path().display().to_string(),
                Source::StartupFolder,
                common,
                enabled,
                true,
            ));
        }
    }
    // Services that start automatically (Win32 services, not drivers)
    let services = join(HKLM, r"SYSTEM\CurrentControlSet\Services");
    for svc in subkeys(&services) {
        let key = join(&services, &svc);
        let start = get_u32(&key, "Start");
        let ty = get_u32(&key, "Type").unwrap_or(0);
        if ty & 0x30 == 0 || !matches!(start, Some(2) | Some(4)) {
            continue;
        }
        let Some(image) = get_str(&key, "ImagePath") else {
            continue;
        };
        let mut en = entry(
            format!("service:{svc}"),
            get_str(&key, "DisplayName").filter(|d| !d.starts_with('@')).unwrap_or(svc.clone()),
            image,
            svc.clone(),
            Source::Service,
            true,
            start == Some(2),
            false,
        );
        en.description = get_str(&key, "Description").filter(|d| !d.starts_with('@'));
        if let Some(exe) = crate::autorun::exe_of(&en.command) {
            en.publisher = super::file_publisher(&exe);
            en.exe = Some(exe);
        }
        en.system = is_microsoft(&en.publisher);
        // Disabled Microsoft services are part of Windows' defaults.
        if en.system && (!show_system || !en.enabled) {
            continue;
        }
        v.push(en);
    }
    task_entries(show_system, &mut v);
    v
}

fn approved_bytes(on: bool) -> Vec<u8> {
    let mut b = vec![if on { 2 } else { 3 }, 0, 0, 0];
    let ft = if on { 0u64 } else { (chrono::Utc::now().timestamp() as u64 + 11_644_473_600) * 10_000_000 };
    b.extend_from_slice(&ft.to_le_bytes());
    b
}

pub fn autorun_set_enabled(e: &AutorunEntry, on: bool) -> Result<()> {
    match e.source {
        Source::Run => {
            let (key, name) = e.location.split_once(" | ").context("bad location")?;
            let hive = if key.starts_with(HKLM) { HKLM } else { HKCU };
            let sub = if key.contains("WOW6432Node") { "Run32" } else { "Run" };
            write_raw(&approved(hive, sub, name).0, name, &RawValue { vtype: 3, bytes: approved_bytes(on) })
        }
        Source::StartupFolder => {
            let name = Path::new(&e.location).file_name().context("bad path")?.to_string_lossy().to_string();
            let hive = if e.all_users { HKLM } else { HKCU };
            write_raw(&approved(hive, "StartupFolder", &name).0, &name, &RawValue { vtype: 3, bytes: approved_bytes(on) })
        }
        Source::Service => {
            let o = Command::new("sc").args(["config", &e.location, "start=", if on { "auto" } else { "disabled" }]).creation_flags(NO_WINDOW).output()?;
            if !o.status.success() {
                bail!("sc config failed (run as administrator): {}", String::from_utf8_lossy(&o.stdout).trim());
            }
            Ok(())
        }
        Source::ScheduledTask => {
            let o = Command::new("schtasks").args(["/Change", "/TN", &e.location, if on { "/ENABLE" } else { "/DISABLE" }]).creation_flags(NO_WINDOW).output()?;
            if !o.status.success() {
                bail!("schtasks failed (run as administrator): {}", String::from_utf8_lossy(&o.stderr).trim());
            }
            Ok(())
        }
        Source::RunOnce => bail!("RunOnce entries run once at the next start; remove it instead"),
        _ => bail!("not a Windows autorun entry"),
    }
}

pub fn autorun_run(e: &AutorunEntry) -> Result<()> {
    let mut c = match e.source {
        Source::Service => {
            let mut c = Command::new("sc");
            c.args(["start", &e.location]);
            c
        }
        Source::ScheduledTask => {
            let mut c = Command::new("schtasks");
            c.args(["/Run", "/TN", &e.location]);
            c
        }
        _ => {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]).raw_arg(&e.command);
            c
        }
    };
    let o = c.creation_flags(NO_WINDOW).output().context("starting the program")?;
    if !o.status.success() {
        bail!("{}", String::from_utf8_lossy(&o.stdout).trim());
    }
    Ok(())
}

pub fn autorun_removal(e: &AutorunEntry) -> Result<Vec<Issue>> {
    match e.source {
        Source::Run | Source::RunOnce => {
            let (key, name) = e.location.split_once(" | ").context("bad location")?;
            Ok(vec![Issue::new("autorun", e.location.clone(), format!("autorun entry {}", e.name), Fix::DeleteRegValue { key: key.into(), value: name.into() })])
        }
        Source::StartupFolder => Ok(vec![Issue::new("autorun", e.location.clone(), format!("startup item {}", e.name), Fix::DeleteFile { path: PathBuf::from(&e.location) })]),
        _ => bail!("\"{}\" can't be deleted, only disabled", e.name),
    }
}

pub fn autorun_add(name: &str, command: &str, all_users: bool, method: AddMethod) -> Result<()> {
    let hive = if all_users { HKLM } else { HKCU };
    match method {
        AddMethod::Run | AddMethod::RunOnce => {
            let key = join(hive, if method == AddMethod::Run { RUN } else { RUNONCE });
            if open(&key).is_some_and(|k| k.get_raw_value(name).is_ok()) {
                bail!("an entry named {name} already exists");
            }
            write_raw(&key, name, &crate::tuner::RegData::Sz(command.into()).to_raw())
        }
        AddMethod::StartupFolder => {
            let dir = startup_dirs().into_iter().find(|(_, c)| *c == all_users).map(|(d, _)| d).context("Startup folder not found")?;
            let path = dir.join(format!("{name}.cmd"));
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            std::fs::write(&path, format!("@echo off\r\nstart \"\" {command}\r\n"))?;
            Ok(())
        }
    }
}
