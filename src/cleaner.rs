//! Clean selected issues: back everything up first, then apply fixes.

use crate::backup::{Backup, BackupKind};
use crate::config::Settings;
use crate::model::{Fix, Issue};
use anyhow::Result;

pub struct CleanReport {
    pub backup_id: String,
    pub cleaned: usize,
    pub failed: Vec<(String, String)>,
    pub restore_point: Option<Result<(), String>>,
    pub full_backup_id: Option<String>,
}

pub fn clean(settings: &Settings, issues: &[Issue]) -> Result<CleanReport> {
    let restore_point = settings.restore_point_before_clean.then(|| crate::platform::create_restore_point("CleanCore: before registry clean").map_err(|e| e.to_string()));

    let full_backup_id = if settings.full_backup_before_clean {
        let mut b = Backup::create(settings, "Full before cleaning", "", BackupKind::FullBeforeClean)?;
        b.manifest.entries = crate::platform::backup_full(&b.dir)?;
        b.save()?;
        Some(b.manifest.id.clone())
    } else {
        None
    };

    // Every deleted item goes to its own backup, written in full before anything is touched.
    let fixes: Vec<Fix> = issues.iter().map(|i| i.fix.clone()).collect();
    let mut backup = Backup::create(settings, "Deleted items", &format!("{} item(s)", fixes.len()), BackupKind::DeletedItems)?;
    backup.manifest.entries = crate::platform::backup_fixes(&backup.dir, &fixes)?;
    backup.save()?;

    let mut cleaned = 0;
    let mut failed = Vec::new();
    for issue in issues {
        match crate::platform::apply_fix(&issue.fix, settings) {
            Ok(()) => cleaned += 1,
            Err(e) => failed.push((issue.location.clone(), format!("{e:#}"))),
        }
    }
    let _ = crate::backup::prune(settings);
    Ok(CleanReport { backup_id: backup.manifest.id, cleaned, failed, restore_point, full_backup_id })
}
