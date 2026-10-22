//! Windows scheduling: Task Scheduler entries under the `\CleanCore\` folder, created with
//! `schtasks.exe`, and toast notifications.
//!
//! Tasks run through `conhost.exe --headless` so no console window flashes on screen.
//! They are created from task XML (`scheduler::task_xml`) so a run missed while the PC was off
//! or asleep still happens and laptops run them on battery, and with highest privileges when
//! CleanCore itself runs elevated, which an Auto Clean of HKEY_LOCAL_MACHINE and a full hive
//! backup need.

use crate::scheduler::{logon_trigger, task_trigger, task_xml, Task};
use anyhow::{bail, Context, Result};
use std::path::Path;

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

/// conhost's arguments: headless, then CleanCore's command line.
fn task_arguments(exe: &Path, args: &[String]) -> String {
    let mut parts = vec!["--headless".to_string(), win_quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| win_quote(a)));
    parts.join(" ")
}

/// The signed-in account, as Task Scheduler names it.
fn user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!(r"{d}\{name}"),
        _ => name,
    }
}

fn schtasks(args: &[String]) -> Result<String> {
    let out = crate::platform::hidden("schtasks").args(args).output().context("running schtasks")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        bail!("{msg}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Create (or replace) a task; elevated if possible, otherwise as a normal-rights task.
fn create(name: &str, trigger: &str, exe: &Path, args: &[String], time_limit: &str) -> Result<()> {
    let file = std::env::temp_dir().join(format!("cleancore-task-{}.xml", std::process::id()));
    let arguments = task_arguments(exe, args);
    let attempt = |highest: bool| -> Result<String> {
        // schtasks reads task XML as UTF-16 with a byte order mark.
        let xml = task_xml(name, trigger, &user(), highest, "conhost.exe", &arguments, time_limit);
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&file, bytes).context("writing the task definition")?;
        schtasks(&["/Create".into(), "/F".into(), "/TN".into(), name.into(), "/XML".into(), file.to_string_lossy().into_owned()])
    };
    let r = match attempt(true) {
        Ok(_) => Ok(()),
        Err(e) => attempt(false)
            .map(|_| eprintln!("warning: created '{name}' without administrator rights (run CleanCore as administrator to fix); machine-wide items will be skipped"))
            .with_context(|| format!("creating task {name} (elevated attempt: {e})")),
    };
    let _ = std::fs::remove_file(&file);
    r
}

pub fn sched_register(task: &Task, exe: &Path, args: &[String]) -> Result<()> {
    create(&task_name(&task.id), &task_trigger(&task.schedule, chrono::Local::now().date_naive()), exe, args, "PT4H")
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
        None => crate::scheduler::NOT_REGISTERED.into(),
    }
}

pub fn recorder_enable(exe: &Path, args: &[String]) -> Result<()> {
    // Runs for the whole session: no time limit.
    create(RECORDER, &logon_trigger(&user()), exe, args, "PT0S")?;
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
    let out = crate::platform::hidden("powershell").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).output()?;
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
        let l = task_arguments(Path::new(r"C:\Program Files\CleanCore\cleancore.exe"), &["schedule".into(), "exec".into(), "x".into()]);
        assert_eq!(l, r#"--headless "C:\Program Files\CleanCore\cleancore.exe" schedule exec x"#);
    }
}
