use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Scan depth. Each mode includes every check of the lower modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    Safe,
    Moderate,
    Advanced,
}

impl std::str::FromStr for ScanMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.trim_start_matches('-').to_ascii_lowercase().as_str() {
            "safe" => Ok(Self::Safe),
            "moderate" => Ok(Self::Moderate),
            "advanced" => Ok(Self::Advanced),
            other => anyhow::bail!("unknown scan mode '{other}' (safe|moderate|advanced)"),
        }
    }
}

/// Which accounts to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    CurrentUser,
    AllUsers,
}

/// Concrete repair operation for one issue. Every variant must be backed up before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fix {
    /// Windows: delete a registry key recursively. `key` is a full path, e.g. `HKEY_CURRENT_USER\Software\Foo`.
    DeleteRegKey { key: String },
    /// Windows: delete one value of a registry key (empty name = default value).
    DeleteRegValue { key: String, value: String },
    /// Delete a file or a symlink.
    DeleteFile { path: PathBuf },
    /// Put back a file's earlier content (hex), e.g. a config file an installer changed.
    WriteFile { path: PathBuf, data: String },
    /// Put back a settings value's earlier data (hex): Windows: 4-byte type + data;
    /// Linux: the dconf GVariant text of key `key` + `value`.
    WriteSetting { key: String, value: String, data: String },
    /// Delete an empty directory.
    DeleteEmptyDir { path: PathBuf },
    /// Delete a directory tree (include list / orphaned config).
    DeleteDirTree { path: PathBuf },
    /// Linux: remove one `<bookmark href=...>` from an XBEL recent-files list.
    RemoveXbelBookmark { file: PathBuf, href: String },
    /// Linux: remove a `.desktop` id from every association line of a mimeapps.list.
    RemoveMimeHandler { file: PathBuf, desktop_id: String },
    /// Linux: reset a dconf folder (GNOME settings of a removed app), e.g. `/org/gnome/gedit/`.
    DconfResetDir { dir: String },
    /// Linux: reset one dconf key, e.g. `/org/gnome/shell/favorite-apps`.
    DconfResetKey { key: String },
    /// Remove entries from a JSON file (browser preferences). Each entry is a path like
    /// `/extensions/settings/<id>` or `/addons[id=<id>]` (array element whose `id` matches).
    EditJson { file: PathBuf, remove: Vec<String> },
    /// Linux: purge the configuration a removed package left behind (`apt-get purge`, …).
    PurgePackage { manager: String, name: String },
}

/// One problem found by a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    /// Stable check id (e.g. `startup`, `shared_dlls_missing`).
    pub category: &'static str,
    /// Human-readable location: `Key | Value` for registry values, file path otherwise.
    pub location: String,
    /// Why it was flagged (usually the missing target).
    pub detail: String,
    pub fix: Fix,
}

impl Issue {
    pub fn new(category: &'static str, location: impl Into<String>, detail: impl Into<String>, fix: Fix) -> Self {
        Self { category, location: location.into(), detail: detail.into(), fix }
    }
}

/// Static description of a check, shown in the managed-scan picker.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CheckInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Lowest scan mode in which this check runs.
    pub min_mode: ScanMode,
}
