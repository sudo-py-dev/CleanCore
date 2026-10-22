use crate::model::ScanMode;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One entry of the exclude or include list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListEntry {
    /// Registry key (Windows) or filesystem path (Linux).
    pub path: String,
    #[serde(default = "yes")]
    pub include_subkeys: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub exclude_external_drives: bool,
    pub exclude_os_defaults: bool,
    pub force_delete: bool,
    pub restore_point_before_clean: bool,
    pub full_backup_before_clean: bool,
    pub quick_clean_mode: ScanMode,
    pub skip_warnings: bool,
    pub exclude_list: Vec<ListEntry>,
    pub include_list: Vec<ListEntry>,
    /// Overrides the default backup folder.
    pub backup_dir: Option<PathBuf>,
    pub auto_delete_backups_older_than_days: Option<u32>,
    pub auto_delete_backups_over_mb: Option<u64>,
    pub auto_delete_only_auto_backups: bool,
    /// Registry Watcher: pause all watches when the log folder exceeds this size (0 = no limit).
    pub watcher_max_log_mb: u64,
    /// Show a desktop notification after a scheduled Auto Clean / Auto Backup (failures always notify).
    pub notify_auto_clean: bool,
    pub notify_auto_backup: bool,
    /// GUI color theme (default, dark, green, gray, black); `None` until chosen on first start.
    pub gui_theme: Option<String>,
    /// GUI accent color (blue, indigo, violet, teal, green, rose).
    pub gui_accent: Option<String>,
    /// Uninstaller: also list system components, updates and libraries.
    pub show_system_components: bool,
    /// Uninstaller: restore point / full registry backup before uninstalling.
    pub uninstall_restore_point: bool,
    pub uninstall_full_backup: bool,
    /// Uninstaller: default leftover scan depth (Quick Uninstall uses it too).
    pub uninstall_scan_mode: ScanMode,
    /// Uninstaller: programs installed within this many days form the "New Programs" group.
    pub new_program_days: u32,
    /// Deleted files go to the Recycle Bin / Trash (they are backed up either way).
    pub delete_to_trash: bool,
    /// Uninstaller: leftovers start out selected.
    pub leftovers_checked_by_default: bool,
    /// Installation monitor: extra path / key patterns to ignore (case-insensitive substrings).
    pub trace_excludes: Vec<String>,
    /// Delete a trace log after the program was uninstalled with it.
    pub delete_trace_after_uninstall: bool,
    /// Logs database: folder of shared `.cptrace` logs (default: `<data>/logs-db`).
    pub logs_db_folder: Option<std::path::PathBuf>,
    /// Logs database: optional URL serving `index.json` + the log files, synced by "Update".
    pub logs_db_url: Option<String>,
    /// Autorun Manager: also list entries that belong to the operating system.
    pub autorun_show_system: bool,
    /// Junk Files Cleaner: file name patterns (`*.tmp`, `~*`, …) that are junk.
    pub junk_patterns: Vec<Toggle>,
    /// Junk Files Cleaner: where to look (empty: the home folder on Linux, all fixed drives on Windows).
    pub junk_locations: Vec<String>,
    /// Junk Files Cleaner: folders never scanned.
    pub junk_exclude: Vec<String>,
    /// Junk Files Cleaner: leave files changed in the last 24 hours alone.
    pub junk_skip_recent: bool,
    /// Junk Files Cleaner: delete to the Recycle Bin / Trash instead of permanently.
    pub junk_to_trash: bool,
    /// Duplicate Finder: where to look (empty: the home folder).
    pub dupes_locations: Vec<String>,
    /// Duplicate Finder: folders never scanned.
    pub dupes_exclude: Vec<String>,
    /// Duplicate Finder: smaller files are ignored (bytes).
    pub dupes_min_size: u64,
    /// Duplicate Finder: leave hidden files and folders (dot names, hidden attribute) alone.
    pub dupes_skip_hidden: bool,
    /// Duplicate Finder: delete to the Recycle Bin / Trash instead of permanently.
    pub dupes_to_trash: bool,
    /// History Cleaner: the items ticked last time (ids).
    pub history_selected: Vec<String>,
    /// Unrecoverable Delete / Evidence Remover: overwrite passes.
    pub shred_passes: u32,
    /// System Tools: user-added tools.
    pub custom_tools: Vec<CustomTool>,
    /// Backups: make a full automatic backup the first time CleanCore runs each day.
    pub daily_full_backup: bool,
    /// Uninstaller: close the program's running processes before its uninstaller starts.
    pub uninstall_kill_running: bool,
    /// Uninstaller: only run the built-in uninstaller, no leftover scan afterwards.
    pub uninstall_builtin_only: bool,
    /// Installation monitor: keep only files the installer's own processes wrote (strace on
    /// Linux, ETW on Windows), so other programs' changes during the trace are left out.
    pub trace_follow_process: bool,
    /// Interface language: a code from `i18n::LANGUAGES` ("he", "de", …) or "system".
    pub language: String,
    /// Look for a newer release when the app starts (at most once a day).
    pub check_updates: bool,
    /// When updates were last looked for.
    pub last_update_check: Option<chrono::DateTime<chrono::Local>>,
}

/// A pattern or value that can be switched off without removing it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Toggle {
    pub value: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// A program added to System Tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomTool {
    pub name: String,
    pub command: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            exclude_external_drives: true,
            exclude_os_defaults: true,
            force_delete: false,
            restore_point_before_clean: false,
            full_backup_before_clean: false,
            quick_clean_mode: ScanMode::Safe,
            skip_warnings: false,
            exclude_list: Vec::new(),
            include_list: Vec::new(),
            backup_dir: None,
            auto_delete_backups_older_than_days: None,
            auto_delete_backups_over_mb: None,
            auto_delete_only_auto_backups: false,
            watcher_max_log_mb: 100,
            notify_auto_clean: true,
            notify_auto_backup: true,
            gui_theme: None,
            gui_accent: None,
            show_system_components: false,
            uninstall_restore_point: cfg!(windows),
            uninstall_full_backup: false,
            uninstall_scan_mode: ScanMode::Moderate,
            new_program_days: 7,
            delete_to_trash: true,
            leftovers_checked_by_default: true,
            trace_excludes: Vec::new(),
            delete_trace_after_uninstall: false,
            logs_db_folder: None,
            logs_db_url: None,
            autorun_show_system: false,
            junk_patterns: crate::junk::DEFAULT_PATTERNS.iter().map(|p| Toggle { value: p.to_string(), enabled: true }).collect(),
            junk_locations: Vec::new(),
            junk_exclude: Vec::new(),
            junk_skip_recent: true,
            junk_to_trash: true,
            dupes_locations: Vec::new(),
            dupes_exclude: Vec::new(),
            dupes_min_size: 64 * 1024,
            dupes_skip_hidden: true,
            dupes_to_trash: true,
            history_selected: Vec::new(),
            shred_passes: 3,
            custom_tools: Vec::new(),
            daily_full_backup: false,
            uninstall_kill_running: false,
            uninstall_builtin_only: false,
            trace_follow_process: true,
            language: "system".into(),
            check_updates: true,
            last_update_check: None,
        }
    }
}

/// Root folder for settings, backups and logs.
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("CLEANCORE_DATA_DIR") {
        return PathBuf::from(p);
    }
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("CleanCore")
}

impl Settings {
    fn file() -> PathBuf {
        data_dir().join("settings.json")
    }

    pub fn load() -> Result<Self> {
        let f = Self::file();
        if !f.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parsing {}", f.display()))
    }

    pub fn save(&self) -> Result<()> {
        let f = Self::file();
        std::fs::create_dir_all(f.parent().unwrap())?;
        std::fs::write(&f, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.backup_dir.clone().unwrap_or_else(|| data_dir().join("Backups"))
    }

    /// True if `location` is covered by an enabled exclude-list entry.
    pub fn is_excluded(&self, location: &str) -> bool {
        let mut entries = self.exclude_list.iter().filter(|e| e.enabled).peekable();
        if entries.peek().is_none() {
            return false;
        }
        // The location is normalized once, not once per entry.
        let loc = norm_location(location);
        entries.any(|e| list_matches_normalized(e, &loc))
    }
}

/// Case-insensitive prefix match on path components (`\` and `/` both act as separators).
pub fn list_matches(e: &ListEntry, location: &str) -> bool {
    list_matches_normalized(e, &norm_location(location))
}

/// Lowercase, `\` separators, no trailing separator.
fn norm_path(s: &str) -> String {
    s.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// A location as `list_matches_normalized` wants it: a "Key | Value" suffix dropped, then normalized.
fn norm_location(location: &str) -> String {
    norm_path(location.split(" | ").next().unwrap_or(location))
}

fn list_matches_normalized(e: &ListEntry, loc: &str) -> bool {
    let pat = norm_path(&e.path);
    if loc == pat {
        return true;
    }
    // Wildcards: `*` and `?` over the whole path (`*\\Temp`, `C:\\Users\\*\\AppData\\Local\\Foo*`).
    if pat.contains(['*', '?']) {
        return crate::pathutil::wildcard(&pat, loc) || e.include_subkeys && crate::pathutil::wildcard(&format!("{pat}\\*"), loc);
    }
    e.include_subkeys && loc.starts_with(&pat) && loc.as_bytes().get(pat.len()) == Some(&b'\\')
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("creating {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, sub: bool) -> ListEntry {
        ListEntry { path: path.into(), include_subkeys: sub, enabled: true }
    }

    #[test]
    fn exclude_matching() {
        let e = entry(r"HKEY_LOCAL_MACHINE\SOFTWARE\Classes", true);
        assert!(list_matches(&e, r"hkey_local_machine\software\classes\.txt"));
        assert!(list_matches(&e, r"HKEY_LOCAL_MACHINE\SOFTWARE\Classes | Foo"));
        assert!(!list_matches(&e, r"HKEY_LOCAL_MACHINE\SOFTWARE\ClassesX"));
        let e = entry(r"HKEY_LOCAL_MACHINE\SOFTWARE\Classes", false);
        assert!(!list_matches(&e, r"HKEY_LOCAL_MACHINE\SOFTWARE\Classes\.txt"));
        let e = entry("/home/u/.config/foo", true);
        assert!(list_matches(&e, "/home/u/.config/foo/bar.desktop"));
    }
}

#[cfg(test)]
mod wildcard_tests {
    use super::*;

    #[test]
    fn exclude_wildcards() {
        let e = |p: &str, sub: bool| ListEntry { path: p.into(), include_subkeys: sub, enabled: true };
        assert!(list_matches(&e(r"HKEY_CURRENT_USER\Software\Foo*", true), r"HKEY_CURRENT_USER\Software\FooBar\x | v"));
        assert!(list_matches(&e("/home/*/.config/app", false), "/home/me/.config/app"));
        assert!(!list_matches(&e("/home/*/.config/app", false), "/home/me/.config/app/sub"));
        assert!(list_matches(&e("/home/*/.config/app", true), "/home/me/.config/app/sub"));
        assert!(!list_matches(&e("/home/*/.config/ap?", true), "/home/me/.config/apps"));
        assert!(list_matches(&e("/plain/path", true), "/plain/path/x"));
    }
}
