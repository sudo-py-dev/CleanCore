//! Windows scheduling: Task Scheduler entries under the `\CleanCore\` folder, created with
//! `schtasks.exe`, and toast notifications.
//!
//! Tasks run through `conhost.exe --headless` so no console window flashes on screen.
//! They are created with highest privileges when CleanCore itself runs elevated, which an
//! Auto Clean of HKEY_LOCAL_MACHINE and a full hive backup need.

use crate::scheduler::{schtasks_args, Task};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

const RECORDER: &str = r"CleanCore\Recorder";

fn task_name(id: &str) -> String {
    format!(r"CleanCore\{id}")
}

/// Quote one argument for a Windows command line.
fn win_quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.to_string();
    }
    format!("\"{}\"", s.replace('"', "\\\""))
}

fn task_run_line(exe: &Path, args: &[String]) -> String {
    let mut parts = vec!["conhost.exe".to_string(), "--headless".to_string(), win_quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| win_quote(a)));
    parts.join(" ")
}

fn schtasks(args: &[String]) -> Result<String> {
    let out = Command::new("schtasks").args(args).output().context("running schtasks")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        bail!("{msg}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Create (or replace) a task; elevated if possible, otherwise as a normal-rights task.
fn create(name: &str, run: &str, schedule: Vec<String>) -> Result<()> {
    let mut base = vec!["/Create".to_string(), "/F".into(), "/TN".into(), name.into(), "/TR".into(), run.into()];
    if run.len() > 261 {
        bail!("command line is longer than Task Scheduler allows (261 chars); move cleancore.exe to a shorter path");
    }
    base.extend(schedule);
    let mut elevated = base.clone();
    elevated.extend(["/RL".to_string(), "HIGHEST".into()]);
    match schtasks(&elevated) {
        Ok(_) => Ok(()),
        Err(e) => {
            schtasks(&base).with_context(|| format!("creating task {name} (elevated attempt: {e})"))?;
            eprintln!("warning: created '{name}' without administrator rights (run CleanCore as administrator to fix); machine-wide items will be skipped");
            Ok(())
        }
    }
}

pub fn sched_register(task: &Task, exe: &Path, args: &[String]) -> Result<()> {
    create(&task_name(&task.id), &task_run_line(exe, args), schtasks_args(&task.schedule))
}

pub fn sched_unregister(id: &str) -> Result<()> {
    match schtasks(&["/Delete".into(), "/F".into(), "/TN".into(), task_name(id)]) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().to_lowercase().contains("cannot find") => Ok(()),
        Err(e) => Err(e),
    }
}

fn query(name: &str) -> Option<(String, String)> {
    let out = schtasks(&["/Query".into(), "/TN".into(), name.into(), "/FO".into(), "LIST".into()]).ok()?;
    let field = |k: &str| out.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string()).unwrap_or_default();
    Some((field("Status:"), field("Next Run Time:")))
}

pub fn sched_status(id: &str) -> String {
    match query(&task_name(id)) {
        Some((status, next)) => format!("{status}, next run {next}"),
        None => "not registered".into(),
    }
}

pub fn recorder_enable(exe: &Path, args: &[String]) -> Result<()> {
    let run = task_run_line(exe, args);
    create(RECORDER, &run, vec!["/SC".into(), "ONLOGON".into()])?;
    // Start it now too, not only at the next login.
    schtasks(&["/Run".into(), "/TN".into(), RECORDER.into()])?;
    Ok(())
}

pub fn recorder_disable() -> Result<()> {
    let _ = schtasks(&["/End".into(), "/TN".into(), RECORDER.into()]);
    match schtasks(&["/Delete".into(), "/F".into(), "/TN".into(), RECORDER.into()]) {
        Err(e) if !e.to_string().to_lowercase().contains("cannot find") => Err(e),
        _ => Ok(()),
    }
}

/// Delete the tasks created under an earlier name of the app (`\<folder>\<id>` and
/// `\<folder>\Recorder`). Returns whether a recorder task was set up.
pub fn sched_remove_legacy(folder: &str, ids: &[String]) -> bool {
    for id in ids {
        let _ = schtasks(&["/Delete".into(), "/F".into(), "/TN".into(), format!(r"{folder}\{id}")]);
    }
    let recorder = format!(r"{folder}\Recorder");
    let had = query(&recorder).is_some();
    if had {
        let _ = schtasks(&["/End".into(), "/TN".into(), recorder.clone()]);
        let _ = schtasks(&["/Delete".into(), "/F".into(), "/TN".into(), recorder]);
    }
    had
}

pub fn recorder_status() -> String {
    match query(RECORDER) {
        Some((status, _)) => format!("starts at login; now: {status}"),
        None => "not set up to start at login".into(),
    }
}

/// Toast notification, shown under the Windows PowerShell app identity (always registered).
pub fn notify(title: &str, body: &str) -> Result<()> {
    let esc = |s: &str| s.replace('\'', "''");
    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
         $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $x = $t.GetElementsByTagName('text'); \
         $x.Item(0).AppendChild($t.CreateTextNode('{}')) > $null; \
         $x.Item(1).AppendChild($t.CreateTextNode('{}')) > $null; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
        esc(title),
        esc(body)
    );
    let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_line() {
        let l = task_run_line(Path::new(r"C:\Program Files\CleanCore\cleancore.exe"), &["schedule".into(), "exec".into(), "x".into()]);
        assert_eq!(l, r#"conhost.exe --headless "C:\Program Files\CleanCore\cleancore.exe" schedule exec x"#);
    }
}
