//! Linux scheduling: systemd user units (`~/.config/systemd/user`) and desktop notifications.
//!
//! Each task becomes `cleancore-task-<id>.service` (oneshot) + `.timer` (`OnCalendar=`,
//! `Persistent=true` so a run missed while the PC was off happens at the next login).
//! The recorder is `cleancore-recorder.service`, started with the user session.
//! `CLEANCORE_SERVICE_DIR` writes the units elsewhere without activating them (for inspection).

use crate::scheduler::{on_calendar, Task};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const RECORDER: &str = "cleancore-recorder.service";

fn unit_dir() -> (PathBuf, bool) {
    match std::env::var_os("CLEANCORE_SERVICE_DIR") {
        Some(d) => (PathBuf::from(d), false),
        None => (dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join("systemd/user"), true),
    }
}

fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl").arg("--user").args(args).output().context("running systemctl --user")?;
    if !out.status.success() {
        bail!("systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Quote one ExecStart argument (systemd.syntax: double quotes, `\` escapes, `%` and `$` doubled).
fn sd_quote(s: &str) -> String {
    let e = s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%").replace('$', "$$");
    format!("\"{e}\"")
}

fn exec_line(exe: &Path, args: &[String]) -> String {
    std::iter::once(exe.to_string_lossy().to_string()).chain(args.iter().cloned()).map(|a| sd_quote(&a)).collect::<Vec<_>>().join(" ")
}

fn task_unit(id: &str) -> String {
    format!("cleancore-task-{id}")
}

pub fn service_unit(task: &Task, exe: &Path, args: &[String]) -> String {
    format!(
        "[Unit]\nDescription=CleanCore scheduled task: {name}\n\n[Service]\nType=oneshot\nExecStart={exec}\nNice=10\nIOSchedulingClass=idle\n",
        name = task.name.replace('\n', " "),
        exec = exec_line(exe, args)
    )
}

pub fn timer_unit(task: &Task) -> String {
    format!(
        "[Unit]\nDescription=CleanCore schedule ({when}): {name}\n\n[Timer]\nOnCalendar={cal}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n",
        when = task.schedule,
        name = task.name.replace('\n', " "),
        cal = on_calendar(&task.schedule)
    )
}

pub fn sched_register(task: &Task, exe: &Path, args: &[String]) -> Result<()> {
    let (dir, activate) = unit_dir();
    crate::config::ensure_dir(&dir)?;
    let unit = task_unit(&task.id);
    std::fs::write(dir.join(format!("{unit}.service")), service_unit(task, exe, args))?;
    std::fs::write(dir.join(format!("{unit}.timer")), timer_unit(task))?;
    if activate {
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", &format!("{unit}.timer")])?;
        // Re-registering an edited task: make the timer pick up the new OnCalendar.
        systemctl(&["restart", &format!("{unit}.timer")])?;
    }
    Ok(())
}

pub fn sched_unregister(id: &str) -> Result<()> {
    let (dir, activate) = unit_dir();
    let unit = task_unit(id);
    if activate {
        let _ = systemctl(&["disable", "--now", &format!("{unit}.timer")]);
    }
    for ext in ["timer", "service"] {
        let p = dir.join(format!("{unit}.{ext}"));
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
    }
    if activate {
        systemctl(&["daemon-reload"])?;
    }
    Ok(())
}

/// Human-readable OS state of a task's timer.
pub fn sched_status(id: &str) -> String {
    let (dir, activate) = unit_dir();
    let timer = format!("{}.timer", task_unit(id));
    if !dir.join(&timer).exists() {
        return crate::scheduler::NOT_REGISTERED.into();
    }
    if !activate {
        return "unit written (not activated)".into();
    }
    systemctl(&["is-active", &timer]).unwrap_or_else(|_| "inactive".into())
}

pub fn recorder_enable(exe: &Path, args: &[String]) -> Result<()> {
    let (dir, activate) = unit_dir();
    crate::config::ensure_dir(&dir)?;
    let unit = format!(
        "[Unit]\nDescription=CleanCore watch recorder\n\n[Service]\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exec_line(exe, args)
    );
    std::fs::write(dir.join(RECORDER), unit)?;
    if activate {
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", RECORDER])?;
    }
    Ok(())
}

pub fn recorder_disable() -> Result<()> {
    let (dir, activate) = unit_dir();
    if activate {
        let _ = systemctl(&["disable", "--now", RECORDER]);
    }
    let p = dir.join(RECORDER);
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    if activate {
        systemctl(&["daemon-reload"])?;
    }
    Ok(())
}

/// Stop and delete the task timers and recorder unit written under an earlier name of the app
/// (`<prefix>-task-*`, `<prefix>-recorder.service`). Returns whether a recorder was set up.
pub fn sched_remove_legacy(prefix: &str) -> bool {
    let (dir, activate) = unit_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return false };
    let recorder = format!("{prefix}-recorder.service");
    let task = format!("{prefix}-task-");
    let mut had_recorder = false;
    let mut changed = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name == recorder || (name.starts_with(&task) && (name.ends_with(".timer") || name.ends_with(".service")));
        if !ours {
            continue;
        }
        had_recorder |= name == recorder;
        if activate && (name.ends_with(".timer") || name == recorder) {
            let _ = systemctl(&["disable", "--now", &name]);
        }
        changed |= std::fs::remove_file(e.path()).is_ok();
    }
    if changed && activate {
        let _ = systemctl(&["daemon-reload"]);
    }
    had_recorder
}

pub fn recorder_status() -> String {
    let (dir, activate) = unit_dir();
    if !dir.join(RECORDER).exists() {
        return "not set up to start at login".into();
    }
    if !activate {
        return format!("unit written to {} (not activated)", dir.display());
    }
    let enabled = systemctl(&["is-enabled", RECORDER]).unwrap_or_else(|_| "disabled".into());
    let active = systemctl(&["is-active", RECORDER]).unwrap_or_else(|_| "inactive".into());
    format!("starts at login: {enabled}; now: {active}")
}

/// Desktop notification through the freedesktop notification service.
pub fn notify(title: &str, body: &str) -> Result<()> {
    if let Ok(st) = Command::new("notify-send").args(["--app-name=CleanCore", "--icon=edit-clear", title, body]).status() {
        if st.success() {
            return Ok(());
        }
    }
    // notify-send isn't installed everywhere; the D-Bus call works on any freedesktop desktop.
    let q = |s: &str| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"));
    let st = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.Notifications",
            "--object-path",
            "/org/freedesktop/Notifications",
            "--method",
            "org.freedesktop.Notifications.Notify",
            "CleanCore",
            "0",
            "edit-clear",
            &q(title),
            &q(body),
            "[]",
            "{}",
            "8000",
        ])
        .output()
        .context("no notify-send or gdbus available")?;
    if !st.status.success() {
        bail!("{}", String::from_utf8_lossy(&st.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sd_quote(r#"/a b/c"d%$"#), r#""/a b/c\"d%%$$""#);
    }
}
