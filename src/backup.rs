//! Backup Center: on-disk backup store shared by all platforms.
//!
//! Layout: `<backup_dir>/<id>/manifest.json` plus payload files written by the platform layer.

use crate::config::{ensure_dir, Settings};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupKind {
    Full,
    Partial,
    AutoFull,
    AutoPartial,
    FullBeforeClean,
    DeletedItems,
}

impl BackupKind {
    pub fn is_auto(self) -> bool {
        matches!(self, Self::AutoFull | Self::AutoPartial)
    }
}

/// One restorable unit inside a backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupEntry {
    /// Windows: a `.reg` file (regedit format) holding one key or value. `label` is the key path.
    RegFile {
        label: String,
        file: String,
    },
    /// Windows: a binary hive saved with `RegSaveKeyEx` (full backup).
    Hive {
        label: String,
        file: String,
    },
    /// A regular file (or a whole directory tree) copied into the backup.
    FileCopy {
        original: PathBuf,
        stored: String,
        is_dir: bool,
    },
    Symlink {
        original: PathBuf,
        target: PathBuf,
    },
    EmptyDir {
        original: PathBuf,
    },
    /// Linux: a `dconf dump` of a settings folder, restored with `dconf load`.
    Dconf {
        dir: String,
        file: String,
    },
    /// Linux: one dconf key and its GVariant value, restored with `dconf write`.
    DconfKey {
        key: String,
        value: String,
    },
    /// Something that can't be restored automatically (e.g. purged system config); kept for the record.
    Note {
        label: String,
        text: String,
    },
}

impl BackupEntry {
    pub fn label(&self) -> String {
        match self {
            Self::RegFile { label, .. } | Self::Hive { label, .. } => label.clone(),
            Self::FileCopy { original, .. } | Self::Symlink { original, .. } | Self::EmptyDir { original } => original.display().to_string(),
            Self::Dconf { dir, .. } => format!("dconf:{dir}"),
            Self::DconfKey { key, .. } => format!("dconf:{key}"),
            Self::Note { label, .. } => label.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub note: String,
    pub kind: BackupKind,
    pub created: DateTime<Local>,
    pub entries: Vec<BackupEntry>,
    #[serde(default)]
    pub size_bytes: u64,
}

pub struct Backup {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Backup {
    /// Create an empty backup folder; fill it with `platform::backup_*` and then call [`Backup::save`].
    pub fn create(settings: &Settings, name: &str, note: &str, kind: BackupKind) -> Result<Self> {
        let created = Local::now();
        let base = created.format("%Y%m%d-%H%M%S").to_string();
        let root = settings.backup_dir();
        ensure_dir(&root)?;
        let mut id = base.clone();
        let mut n = 1;
        while root.join(&id).exists() {
            n += 1;
            id = format!("{base}-{n}");
        }
        let dir = root.join(&id);
        ensure_dir(&dir)?;
        let manifest = Manifest { id, name: name.into(), note: note.into(), kind, created, entries: vec![], size_bytes: 0 };
        Ok(Self { dir, manifest })
    }

    pub fn save(&mut self) -> Result<()> {
        self.manifest.size_bytes = dir_size(&self.dir);
        std::fs::write(self.dir.join("manifest.json"), serde_json::to_string_pretty(&self.manifest)?)?;
        Ok(())
    }

    pub fn open(settings: &Settings, id: &str) -> Result<Self> {
        let dir = settings.backup_dir().join(id);
        let f = dir.join("manifest.json");
        if !f.exists() {
            bail!("backup '{id}' not found in {}", settings.backup_dir().display());
        }
        let manifest = serde_json::from_str(&std::fs::read_to_string(&f)?).with_context(|| format!("parsing {}", f.display()))?;
        Ok(Self { dir, manifest })
    }

    /// Restore all entries, or only those whose label contains one of `filter`.
    pub fn restore(&self, filter: Option<&[String]>) -> Result<usize> {
        let entries: Vec<&BackupEntry> = self.manifest.entries.iter().filter(|e| filter.is_none_or(|f| f.iter().any(|s| e.label().contains(s.as_str())))).collect();
        crate::platform::restore_entries(&self.dir, &entries)?;
        Ok(entries.len())
    }
}

/// All backups, newest first.
pub fn list(settings: &Settings) -> Result<Vec<Manifest>> {
    let root = settings.backup_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let f = e.path().join("manifest.json");
            if let Ok(s) = std::fs::read_to_string(&f) {
                if let Ok(m) = serde_json::from_str::<Manifest>(&s) {
                    out.push(m);
                }
            }
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.created));
    Ok(out)
}

pub fn delete(settings: &Settings, id: &str) -> Result<()> {
    let dir = settings.backup_dir().join(id);
    if !dir.join("manifest.json").exists() {
        bail!("backup '{id}' not found");
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Apply the auto-delete rules from settings. Returns the ids that were removed.
pub fn prune(settings: &Settings) -> Result<Vec<String>> {
    let mut all = list(settings)?; // newest first
    let eligible = |m: &Manifest| !settings.auto_delete_only_auto_backups || m.kind.is_auto();
    let mut removed = Vec::new();
    if let Some(days) = settings.auto_delete_backups_older_than_days {
        let cutoff = Local::now() - chrono::Duration::days(days as i64);
        for m in all.iter().filter(|m| m.created < cutoff && eligible(m)) {
            delete(settings, &m.id)?;
            removed.push(m.id.clone());
        }
        all.retain(|m| !removed.contains(&m.id));
    }
    if let Some(max_mb) = settings.auto_delete_backups_over_mb {
        let limit = max_mb * 1024 * 1024;
        let mut total: u64 = all.iter().map(|m| m.size_bytes).sum();
        // Drop the oldest first.
        for m in all.iter().rev() {
            if total <= limit {
                break;
            }
            if eligible(m) {
                delete(settings, &m.id)?;
                total -= m.size_bytes;
                removed.push(m.id.clone());
            }
        }
    }
    Ok(removed)
}

/// Delete every backup created within `[from, to]` (Revo: Backup Manager › Delete by time range).
/// Returns the ids that were removed.
pub fn delete_range(settings: &Settings, from: DateTime<Local>, to: DateTime<Local>) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for m in list(settings)?.into_iter().filter(|m| m.created >= from && m.created <= to) {
        delete(settings, &m.id)?;
        removed.push(m.id);
    }
    Ok(removed)
}

/// Back up the whole registry (Windows) / config store (Linux) into a new backup.
pub fn create_full(settings: &Settings, name: &str, note: &str, kind: BackupKind) -> Result<String> {
    let mut b = Backup::create(settings, name, note, kind)?;
    match crate::platform::backup_full(&b.dir) {
        Ok(e) => b.manifest.entries = e,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&b.dir);
            return Err(e.context("Backup creation failed!"));
        }
    }
    b.save()?;
    Ok(b.manifest.id)
}

/// Revo's "daily registry backup": the first time CleanCore runs on a day, make a full
/// automatic backup (and apply the auto-delete rules). `None` when one exists for today or
/// the option is off.
pub fn daily_full_if_due(settings: &Settings) -> Result<Option<String>> {
    if !settings.daily_full_backup {
        return Ok(None);
    }
    let today = Local::now().date_naive();
    if list(settings)?.iter().any(|m| m.kind == BackupKind::AutoFull && m.name == DAILY_NAME && m.created.date_naive() == today) {
        return Ok(None);
    }
    let id = create_full(settings, DAILY_NAME, "Made automatically the first time CleanCore ran today", BackupKind::AutoFull)?;
    let _ = prune(settings);
    Ok(Some(id))
}

pub const DAILY_NAME: &str = "Daily backup";

/// Parse `YYYY-MM-DD` or `YYYY-MM-DD HH:MM` in local time; a bare date means its start
/// (`end_of_day` = its end).
pub fn parse_local(s: &str, end_of_day: bool) -> Result<DateTime<Local>> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone};
    let s = s.trim();
    let naive = if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        dt
    } else {
        let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").with_context(|| format!("\"{s}\" isn't a date like 2026-10-21 or 2026-10-21 14:30"))?;
        if end_of_day {
            d.and_hms_opt(23, 59, 59).unwrap()
        } else {
            d.and_hms_opt(0, 0, 0).unwrap()
        }
    };
    Local.from_local_datetime(&naive).earliest().context("that time doesn't exist in this time zone")
}

pub fn human_size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Size of a file, or of everything under a folder (symbolic links aren't followed).
pub fn dir_size(p: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(p) else {
            return 0;
        };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => walk(p),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}
