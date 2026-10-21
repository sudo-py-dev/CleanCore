//! Junk Files Cleaner (Revo: Tools › Junk Files Cleaner): find files whose names match junk
//! patterns (temporary files, editor backups, crash dumps, …) on the chosen drives or folders,
//! then delete them to the Recycle Bin / Trash or permanently.

use crate::config::Settings;
use crate::pathutil::wildcard;
use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// Revo's default list, minus patterns that often match real data (`*.log` in projects).
pub const DEFAULT_PATTERNS: &[&str] = &[
    "*.tmp",
    "*.temp",
    "*._mp",
    "*.$$$",
    "*.---",
    "*.~*",
    "~*.*",
    "*~",
    "*.bak",
    "*.old",
    "*.chk",
    "*.gid",
    "*.syd",
    "*.prv",
    "*.dmp",
    "*.mdmp",
    "thumbs.db",
    "ehthumbs.db",
    ".DS_Store",
    "*.crdownload",
    "*.part",
    "core",
    "core.?*",
];

#[derive(Debug, Clone, Serialize)]
pub struct JunkFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<chrono::DateTime<chrono::Local>>,
    /// The pattern it matched.
    pub pattern: String,
}

/// Scan progress shared with the GUI.
#[derive(Default)]
pub struct Progress {
    pub folders: AtomicUsize,
    pub found: AtomicUsize,
    pub cancel: AtomicBool,
}

/// Where a scan looks when no locations are set.
pub fn default_locations() -> Vec<PathBuf> {
    if cfg!(windows) {
        crate::platform::drives().into_iter().filter(|d| !d.removable).map(|d| d.path).collect()
    } else {
        dirs::home_dir().into_iter().collect()
    }
}

/// Folders a scan never enters: the OS itself, package-managed trees and virtual filesystems.
fn builtin_excludes() -> Vec<String> {
    let mut v: Vec<String> = if cfg!(windows) {
        let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        vec![format!(r"{win}\WinSxS"), format!(r"{win}\Installer"), format!(r"{win}\servicing"), r"\$Recycle.Bin".into(), r"\System Volume Information".into()]
    } else {
        ["/proc", "/sys", "/dev", "/run", "/snap", "/usr", "/boot", "/var/lib", "/lost+found"].map(String::from).to_vec()
    };
    // Our own backups and the Trash.
    v.push(crate::config::data_dir().display().to_string());
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("Trash").display().to_string());
    }
    v
}

fn norm(p: &str) -> String {
    let s = p.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        s.to_lowercase().replace('/', "\\")
    } else {
        s.to_string()
    }
}

fn excluded(path: &Path, excludes: &[String]) -> bool {
    let p = norm(&path.display().to_string());
    excludes.iter().any(|e| {
        // Drive-less Windows excludes (`\$Recycle.Bin`) apply on every drive.
        if cfg!(windows) && e.starts_with('\\') {
            p.get(2..).is_some_and(|rest| rest == e || rest.starts_with(&format!("{e}\\")))
        } else {
            p == *e || p.starts_with(&format!("{e}{}", std::path::MAIN_SEPARATOR))
        }
    })
}

/// Find junk files. `locations` empty → [`default_locations`].
pub fn scan(settings: &Settings, progress: &Progress) -> Vec<JunkFile> {
    let patterns: Vec<String> = settings.junk_patterns.iter().filter(|p| p.enabled && !p.value.trim().is_empty()).map(|p| p.value.trim().to_string()).collect();
    if patterns.is_empty() {
        return Vec::new();
    }
    let locations: Vec<PathBuf> =
        if settings.junk_locations.is_empty() { default_locations() } else { settings.junk_locations.iter().map(|l| PathBuf::from(crate::pathutil::expand_env(l))).collect() };
    let mut excludes: Vec<String> = builtin_excludes();
    excludes.extend(settings.junk_exclude.iter().map(|e| crate::pathutil::expand_env(e)));
    let excludes: Vec<String> = excludes.iter().map(|e| norm(e)).collect();
    let cutoff = settings.junk_skip_recent.then(|| SystemTime::now() - Duration::from_secs(24 * 3600));
    let mut out: Vec<JunkFile> = locations.par_iter().flat_map(|l| walk(l, &patterns, &excludes, cutoff, progress)).collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn walk(root: &Path, patterns: &[String], excludes: &[String], cutoff: Option<SystemTime>, progress: &Progress) -> Vec<JunkFile> {
    if excluded(root, excludes) {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    progress.folders.fetch_add(1, Ordering::Relaxed);
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in rd.flatten() {
        if progress.cancel.load(Ordering::Relaxed) {
            return files;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            dirs.push(e.path());
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pat) = patterns.iter().find(|p| wildcard(p, &name)) else {
            continue;
        };
        let Ok(meta) = e.metadata() else { continue };
        let modified = meta.modified().ok();
        if let (Some(c), Some(m)) = (cutoff, modified) {
            if m > c {
                continue;
            }
        }
        progress.found.fetch_add(1, Ordering::Relaxed);
        files.push(JunkFile { path: e.path(), size: meta.len(), modified: modified.map(chrono::DateTime::from), pattern: pat.clone() });
    }
    // Subfolders in parallel; deep trees spread over the pool.
    files.extend(dirs.par_iter().flat_map(|d| walk(d, patterns, excludes, cutoff, progress)).collect::<Vec<_>>());
    files
}

#[derive(Debug, Default, Serialize)]
pub struct DeleteReport {
    pub deleted: usize,
    pub bytes: u64,
    pub failed: Vec<(String, String)>,
}

/// Delete the files (to the Recycle Bin / Trash when `settings.junk_to_trash`).
pub fn delete(settings: &Settings, files: &[JunkFile]) -> DeleteReport {
    let mut r = DeleteReport::default();
    for f in files {
        match crate::platform::files::delete_path(&f.path, settings.junk_to_trash) {
            Ok(()) => {
                r.deleted += 1;
                r.bytes += f.size;
            }
            Err(e) => r.failed.push((f.path.display().to_string(), format!("{e:#}"))),
        }
    }
    r
}

/// Export the found files as TXT or CSV.
pub fn export(files: &[JunkFile], path: &Path) -> Result<()> {
    let csv = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    let mut s = if csv { "Path,Size,Modified,Pattern\r\n".to_string() } else { String::new() };
    for f in files {
        let m = f.modified.map(|m| m.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
        if csv {
            s += &format!("\"{}\",{},{},\"{}\"\r\n", f.path.display().to_string().replace('"', "\"\""), f.size, m, f.pattern);
        } else {
            s += &format!("{}\t{}\t{}\t{}\n", f.path.display(), crate::backup::human_size(f.size), m, f.pattern);
        }
    }
    std::fs::write(path, s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_old_junk_only() {
        let root = std::env::temp_dir().join(format!("cleancore-junk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub/skip")).unwrap();
        for f in ["a.tmp", "sub/b.bak", "sub/keep.txt", "sub/skip/c.tmp", "new.tmp"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
        for f in ["a.tmp", "sub/b.bak", "sub/keep.txt", "sub/skip/c.tmp"] {
            std::fs::File::options().write(true).open(root.join(f)).unwrap().set_modified(old).unwrap();
        }
        let s = Settings { junk_locations: vec![root.display().to_string()], junk_exclude: vec![root.join("sub/skip").display().to_string()], ..Settings::default() };
        let found: Vec<String> = scan(&s, &Progress::default()).into_iter().map(|f| f.path.strip_prefix(&root).unwrap().display().to_string()).collect();
        assert_eq!(found, ["a.tmp", "sub/b.bak"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
