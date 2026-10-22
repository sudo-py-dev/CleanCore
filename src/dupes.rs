//! Duplicate Finder: files with the same content in the chosen folders, and deleting the extra
//! copies (to the Recycle Bin / Trash or permanently), always keeping at least one.
//!
//! Fast because almost nothing is read in full: files are first grouped by size (sizes found
//! once are dropped), then by a hash of a few KB from their start and end, and only files
//! still alike after that are hashed whole (BLAKE3). Every stage runs on all cores.

use crate::config::Settings;
use crate::junk::{builtin_excludes, excluded, norm, DeleteReport};
use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Bytes read from each end of a file for the quick comparison.
const SAMPLE: u64 = 8 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DupeFile {
    pub path: PathBuf,
    pub modified: Option<chrono::DateTime<chrono::Local>>,
}

/// Files with identical content.
#[derive(Debug, Clone, Serialize)]
pub struct DupeGroup {
    pub size: u64,
    /// BLAKE3 of the content, hex.
    pub hash: String,
    pub files: Vec<DupeFile>,
}

impl DupeGroup {
    /// Space freed by keeping one copy.
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64).saturating_sub(1)
    }
}

/// Scan progress shared with the GUI.
#[derive(Default)]
pub struct Progress {
    pub folders: AtomicUsize,
    pub files: AtomicUsize,
    /// 0 listing files, 1 comparing samples, 2 comparing whole files.
    pub stage: AtomicUsize,
    /// Files left to compare in the current stage, and those done.
    pub to_check: AtomicUsize,
    pub checked: AtomicUsize,
    pub bytes_read: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Which copy of each group stays when duplicates are selected automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Oldest,
    Newest,
    ShortestPath,
}

impl Keep {
    pub const ALL: [Keep; 3] = [Keep::Oldest, Keep::Newest, Keep::ShortestPath];
}

/// Where a scan looks when no locations are set: the home folder.
pub fn default_locations() -> Vec<PathBuf> {
    dirs::home_dir().into_iter().collect()
}

struct Entry {
    path: PathBuf,
    size: u64,
    modified: Option<std::time::SystemTime>,
    /// (device, inode): hard links to one file are the same data, not duplicates.
    id: Option<(u64, u64)>,
}

/// Find duplicate files, biggest waste first. Cancelled scans return nothing.
pub fn scan(settings: &Settings, progress: &Progress) -> Vec<DupeGroup> {
    let locations: Vec<PathBuf> =
        if settings.dupes_locations.is_empty() { default_locations() } else { settings.dupes_locations.iter().map(|l| PathBuf::from(crate::pathutil::expand_env(l))).collect() };
    let mut excludes = builtin_excludes();
    excludes.extend(settings.dupes_exclude.iter().map(|e| crate::pathutil::expand_env(e)));
    let excludes: Vec<String> = excludes.iter().map(|e| norm(e)).collect();
    let opts = Walk { min_size: settings.dupes_min_size.max(1), skip_hidden: settings.dupes_skip_hidden, excludes: &excludes, progress };
    let mut files: Vec<Entry> = locations.par_iter().flat_map(|l| opts.walk(l)).collect();
    if progress.cancelled() {
        return Vec::new();
    }
    // Overlapping locations list a file twice, hard links list it under several names.
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| f.id.is_none_or(|id| seen.insert(id)));

    // 1. Same size.
    let mut by_size: HashMap<u64, Vec<Entry>> = HashMap::new();
    for f in files {
        by_size.entry(f.size).or_default().push(f);
    }
    let candidates: Vec<Vec<Entry>> = by_size.into_values().filter(|g| g.len() > 1).collect();
    // Windows tells hard links apart only through an open file: done here, for the files that
    // share a size, rather than for every file walked.
    #[cfg(windows)]
    let candidates: Vec<Vec<Entry>> = candidates
        .into_par_iter()
        .filter_map(|mut g| {
            let mut seen = std::collections::HashSet::new();
            g.retain_mut(|f| {
                f.id = windows_file_id(&f.path);
                f.id.is_none_or(|id| seen.insert(id))
            });
            (g.len() > 1).then_some(g)
        })
        .collect();

    // 2. Same start and end. Small files are read whole here, so this hash is final for them.
    progress.stage.store(1, Ordering::Relaxed);
    let groups = refine(candidates, progress, |f| sample_hash(&f.path, f.size, progress));
    if progress.cancelled() {
        return Vec::new();
    }

    // 3. Same content.
    progress.stage.store(2, Ordering::Relaxed);
    let (small, large): (Vec<_>, Vec<_>) = groups.into_iter().partition(|(_, g)| g[0].size <= 2 * SAMPLE);
    let mut done: Vec<(blake3::Hash, Vec<Entry>)> = small;
    done.extend(refine(large.into_iter().map(|(_, g)| g).collect(), progress, |f| full_hash(&f.path, progress)));
    if progress.cancelled() {
        return Vec::new();
    }

    let mut out: Vec<DupeGroup> = done
        .into_iter()
        .map(|(hash, mut g)| {
            g.sort_by(|a, b| a.path.cmp(&b.path));
            DupeGroup {
                size: g[0].size,
                hash: hash.to_hex().to_string(),
                files: g.into_iter().map(|f| DupeFile { path: f.path, modified: f.modified.map(chrono::DateTime::from) }).collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| b.wasted().cmp(&a.wasted()).then_with(|| a.files[0].path.cmp(&b.files[0].path)));
    out
}

/// Split each group by `hash`, keeping the parts with more than one file. Files that can't be
/// read drop out.
fn refine(groups: Vec<Vec<Entry>>, progress: &Progress, hash: impl Fn(&Entry) -> Option<blake3::Hash> + Sync) -> Vec<(blake3::Hash, Vec<Entry>)> {
    progress.to_check.store(groups.iter().map(Vec::len).sum(), Ordering::Relaxed);
    progress.checked.store(0, Ordering::Relaxed);
    groups
        .into_par_iter()
        .flat_map_iter(|g| {
            let mut parts: HashMap<blake3::Hash, Vec<Entry>> = HashMap::new();
            for f in g {
                if progress.cancelled() {
                    break;
                }
                if let Some(h) = hash(&f) {
                    parts.entry(h).or_default().push(f);
                }
                progress.checked.fetch_add(1, Ordering::Relaxed);
            }
            parts.into_iter().filter(|(_, p)| p.len() > 1)
        })
        .collect()
}

fn sample_hash(path: &Path, size: u64, progress: &Progress) -> Option<blake3::Hash> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; (2 * SAMPLE) as usize];
    if size <= 2 * SAMPLE {
        f.read_exact(&mut buf[..size as usize]).ok()?;
        h.update(&buf[..size as usize]);
    } else {
        let (head, tail) = buf.split_at_mut(SAMPLE as usize);
        f.read_exact(head).ok()?;
        f.seek(SeekFrom::End(-(SAMPLE as i64))).ok()?;
        f.read_exact(tail).ok()?;
        h.update(&buf);
    }
    progress.bytes_read.fetch_add(size.min(2 * SAMPLE), Ordering::Relaxed);
    Some(h.finalize())
}

fn full_hash(path: &Path, progress: &Progress) -> Option<blake3::Hash> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if progress.cancelled() {
            return None;
        }
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        h.update(&buf[..n]);
        progress.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
    }
    Some(h.finalize())
}

struct Walk<'a> {
    min_size: u64,
    skip_hidden: bool,
    excludes: &'a [String],
    progress: &'a Progress,
}

impl Walk<'_> {
    fn walk(&self, root: &Path) -> Vec<Entry> {
        if excluded(root, self.excludes) {
            return Vec::new();
        }
        let Ok(rd) = std::fs::read_dir(root) else {
            return Vec::new();
        };
        self.progress.folders.fetch_add(1, Ordering::Relaxed);
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            if self.progress.cancelled() {
                return files;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() || (self.skip_hidden && e.file_name().to_string_lossy().starts_with('.')) {
                continue;
            }
            if ft.is_dir() {
                dirs.push(e.path());
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.len() < self.min_size || hidden_attr(&meta, self.skip_hidden) {
                continue;
            }
            self.progress.files.fetch_add(1, Ordering::Relaxed);
            files.push(Entry { path: e.path(), size: meta.len(), modified: meta.modified().ok(), id: file_id(&meta) });
        }
        files.extend(dirs.par_iter().flat_map(|d| self.walk(d)).collect::<Vec<_>>());
        files
    }
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

/// Windows: the walk doesn't open files, so this is read later (`windows_file_id`).
#[cfg(not(unix))]
fn file_id(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// (volume serial number, file index): the same for every hard link to a file.
#[cfg(windows)]
fn windows_file_id(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let f = std::fs::File::open(path).ok()?;
    // SAFETY: an open file's handle, and a zeroed plain-data struct for the answer.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
        return None;
    }
    Some((info.dwVolumeSerialNumber as u64, (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64))
}

/// Windows hidden and system files (the dot-name rule covers Linux).
#[cfg(windows)]
fn hidden_attr(meta: &std::fs::Metadata, skip_hidden: bool) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
    skip_hidden && meta.file_attributes() & 0x6 != 0
}

#[cfg(not(windows))]
fn hidden_attr(_: &std::fs::Metadata, _: bool) -> bool {
    false
}

/// Tick every copy except the one `keep` picks, per group.
pub fn auto_select(groups: &[DupeGroup], keep: Keep) -> Vec<Vec<bool>> {
    groups
        .iter()
        .map(|g| {
            let keeper = match keep {
                Keep::Oldest => (0..g.files.len()).min_by_key(|&i| g.files[i].modified),
                Keep::Newest => (0..g.files.len()).max_by_key(|&i| g.files[i].modified),
                Keep::ShortestPath => (0..g.files.len()).min_by_key(|&i| g.files[i].path.as_os_str().len()),
            }
            .unwrap_or(0);
            (0..g.files.len()).map(|i| i != keeper).collect()
        })
        .collect()
}

/// Delete the ticked copies (to the Recycle Bin / Trash when `settings.dupes_to_trash`). A group
/// is skipped when no unticked copy is still there with the same size, so one always survives;
/// a ticked file whose size changed since the scan is left alone.
pub fn delete(settings: &Settings, groups: &[DupeGroup], checked: &[Vec<bool>]) -> DeleteReport {
    let mut r = DeleteReport::default();
    let same = |p: &Path, size: u64| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.len() == size);
    for (g, c) in groups.iter().zip(checked) {
        let doomed: Vec<&DupeFile> = g.files.iter().zip(c).filter(|(_, c)| **c).map(|(f, _)| f).collect();
        if doomed.is_empty() {
            continue;
        }
        if !g.files.iter().zip(c).any(|(f, c)| !c && same(&f.path, g.size)) {
            for f in doomed {
                r.failed.push((f.path.display().to_string(), "no other copy of this file would be left".into()));
            }
            continue;
        }
        for f in doomed {
            if !same(&f.path, g.size) {
                r.failed.push((f.path.display().to_string(), "the file changed since the scan".into()));
                continue;
            }
            match crate::platform::files::delete_path(&f.path, settings.dupes_to_trash) {
                Ok(()) => {
                    r.deleted += 1;
                    r.bytes += g.size;
                }
                Err(e) => r.failed.push((f.path.display().to_string(), format!("{e:#}"))),
            }
        }
    }
    r
}

/// Export the groups as TXT (a blank line between groups) or CSV.
pub fn export(groups: &[DupeGroup], path: &Path) -> Result<()> {
    let csv = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    let mut s = if csv { "Group,Path,Size,Modified,Hash\r\n".to_string() } else { String::new() };
    for (n, g) in groups.iter().enumerate() {
        if !csv {
            s += &format!("# {} copies of {} ({} wasted)\n", g.files.len(), crate::backup::human_size(g.size), crate::backup::human_size(g.wasted()));
        }
        for f in &g.files {
            let m = f.modified.map(|m| m.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            if csv {
                s += &format!("{},\"{}\",{},{},{}\r\n", n + 1, f.path.display().to_string().replace('"', "\"\""), g.size, m, g.hash);
            } else {
                s += &format!("{}\t{}\n", f.path.display(), m);
            }
        }
        if !csv {
            s += "\n";
        }
    }
    std::fs::write(path, s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_cleans_duplicates() {
        let root = std::env::temp_dir().join(format!("cleancore-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        // Same size, start and end as `big`, different middle: only the full hash tells them apart.
        let mut near = big.clone();
        near[50_000] ^= 1;
        let write = |p: &str, d: &[u8]| std::fs::write(root.join(p), d).unwrap();
        write("one.bin", &big);
        write("a/two.bin", &big);
        write("a/b/three.bin", &big);
        write("near.bin", &near);
        write("small1.txt", b"hello");
        write("a/small2.txt", b"hello");
        write("other.txt", b"world");
        write(".hidden/four.bin", &big);
        std::fs::hard_link(root.join("one.bin"), root.join("a/link.bin")).unwrap();

        let s = Settings { dupes_locations: vec![root.display().to_string()], dupes_min_size: 1, dupes_to_trash: false, ..Settings::default() };
        let groups = scan(&s, &Progress::default());
        let rel = |g: &DupeGroup| -> Vec<String> { g.files.iter().map(|f| f.path.strip_prefix(&root).unwrap().display().to_string().replace('\\', "/")).collect() };
        assert_eq!(groups.len(), 2);
        let big_group = rel(&groups[0]);
        assert_eq!(big_group.len(), 3, "{big_group:?}");
        assert!(big_group.contains(&"a/b/three.bin".to_string()) && !big_group.iter().any(|p| p.contains("near") || p.contains("hidden")));
        assert_eq!(rel(&groups[1]), ["a/small2.txt", "small1.txt"]);
        assert_eq!(groups[0].wasted(), 200_000);

        // Ticking every copy deletes nothing.
        let r = delete(&s, &groups[1..], &[vec![true, true]]);
        assert_eq!((r.deleted, r.failed.len()), (0, 2));
        let sel = auto_select(&groups, Keep::ShortestPath);
        let r = delete(&s, &groups, &sel);
        assert_eq!(r.deleted, 3);
        assert!(r.failed.is_empty());
        assert!(root.join("a/two.bin").exists() && !root.join("a/b/three.bin").exists() && root.join("small1.txt").exists() && !root.join("a/small2.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
