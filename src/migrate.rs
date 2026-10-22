//! One-time move from the app's former name, cleancore, to CleanCore: the data folder (settings,
//! backups, logs, tuner history), the scheduled tasks and recorder, the file-manager menu
//! entries and the marker on disabled cron lines. It runs at start-up, only when the old data
//! folder exists and the new one doesn't, so it happens once and costs nothing afterwards.

use std::path::Path;

/// The former name, as used in folder names and menu texts.
const OLD: &str = "cleancore";

/// Move everything over from cleancore, if this is the first start since the rename. Problems
/// are printed, not fatal: the app works either way.
pub fn from_old_name() {
    // A data folder given on the command line or in the environment is never touched.
    if std::env::var_os("CLEANCORE_DATA_DIR").is_some() {
        return;
    }
    let Some(base) = dirs::data_local_dir() else { return };
    let (old, new) = (base.join(OLD), crate::config::data_dir());
    if !old.is_dir() || new.exists() {
        return;
    }
    if let Err(e) = std::fs::rename(&old, &new) {
        eprintln!("warning: could not move {} to {}: {e}", old.display(), new.display());
        return;
    }
    fix_settings_paths(&new.join("settings.json"), &old, &new);

    // Scheduled tasks and the recorder ran the old executable: make them again under the new name.
    let tasks = crate::scheduler::load_tasks().unwrap_or_default();
    #[cfg(windows)]
    let had_recorder = crate::platform::sched_remove_legacy(OLD, &tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>());
    #[cfg(not(windows))]
    let had_recorder = crate::platform::sched_remove_legacy(&OLD.to_lowercase());
    for t in &tasks {
        if let Err(e) = crate::scheduler::register(t) {
            eprintln!("warning: could not re-create scheduled task {}: {e:#}", t.id);
        }
    }
    if had_recorder {
        if let Err(e) = crate::scheduler::exe_args(&["watch", "run", "--quiet"]).and_then(|(exe, args)| crate::platform::recorder_enable(&exe, &args)) {
            eprintln!("warning: could not re-create the watch recorder: {e:#}");
        }
    }

    // File-manager menu: "… with cleancore" becomes "… with CleanCore".
    if crate::shell::remove_legacy(OLD) {
        if let Err(e) = crate::shell::integrate(true) {
            eprintln!("warning: could not add the file manager menu: {e:#}");
        }
    }

    #[cfg(not(windows))]
    if let Err(e) = crate::platform::cron_migrate_marker(&format!("#{}-disabled# ", OLD.to_lowercase())) {
        eprintln!("warning: could not update disabled cron entries: {e:#}");
    }
}

/// The window was its own program, `cleancore-gui`, until 0.2.8; it is now `cleancore`. Point
/// the file-manager menu at it. (Scheduled tasks and the recorder always ran `cleancore`.)
pub fn from_gui_program() {
    if crate::shell::uses_old_program() {
        if let Err(e) = crate::shell::integrate(true) {
            eprintln!("warning: could not update the file manager menu: {e:#}");
        }
    }
}

/// Settings may name folders inside the old data folder (a custom backup folder, say).
fn fix_settings_paths(file: &Path, old: &Path, new: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    // Paths are stored as JSON strings, so compare in their escaped form.
    let esc = |p: &Path| serde_json::to_string(&p.display().to_string()).unwrap_or_default().trim_matches('"').to_string();
    let (o, n) = (esc(old), esc(new));
    if !o.is_empty() && text.contains(&o) {
        let _ = std::fs::write(file, text.replace(&o, &n));
    }
}
