//! Unrecoverable Delete and Evidence Remover (Revo: Tools › Unrecoverable Delete / Evidence
//! Remover). Files are overwritten in place (zeros, ones, random; the last pass is always
//! random), truncated, renamed to random names and deleted. Free space is wiped by filling
//! the drive with random-data files, then deleting them.
//!
//! On SSDs and copy-on-write filesystems (btrfs, ZFS, APFS) the drive may keep old copies of
//! blocks elsewhere, so overwriting can't guarantee old data is gone there; the GUI says so.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{ErrorKind, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const CHUNK: usize = 1 << 20;

/// Shared progress and controls (pause / resume / cancel) for a running job.
#[derive(Default)]
pub struct Control {
    pub pause: AtomicBool,
    pub cancel: AtomicBool,
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub pass: AtomicU32,
    pub current: Mutex<String>,
}

impl Control {
    /// Block while paused; `Err` once cancelled.
    fn checkpoint(&self) -> Result<()> {
        while self.pause.load(Ordering::Relaxed) && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }

    fn set_current(&self, s: impl Into<String>) {
        if let Ok(mut c) = self.current.lock() {
            *c = s.into();
        }
    }

    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            0.0
        } else {
            (self.done.load(Ordering::Relaxed) as f64 / t as f64).min(1.0) as f32
        }
    }
}

/// xorshift64*: fast, and good enough for overwrite data (this isn't cryptography).
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Self(t ^ (std::process::id() as u64) << 32 | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for c in buf.chunks_mut(8) {
            let x = self.next().to_le_bytes();
            c.copy_from_slice(&x[..c.len()]);
        }
    }
    fn name(&mut self, len: usize) -> String {
        const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..len.clamp(4, 40)).map(|_| A[(self.next() % A.len() as u64) as usize] as char).collect()
    }
}

/// Byte pattern for pass `i` of `n`: 0x00, 0xFF, random, … and always random last.
fn pattern(i: u32, n: u32) -> Option<u8> {
    if i + 1 == n {
        return None;
    }
    match i % 3 {
        0 => Some(0x00),
        1 => Some(0xFF),
        _ => None,
    }
}

fn fill(buf: &mut [u8], pat: Option<u8>, rng: &mut Rng) {
    match pat {
        Some(b) => buf.fill(b),
        None => rng.fill(buf),
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ShredReport {
    pub files: usize,
    pub folders: usize,
    pub bytes: u64,
    pub failed: Vec<(String, String)>,
}

fn total_size(p: &Path) -> u64 {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::read_dir(p).map(|rd| rd.flatten().map(|e| total_size(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

/// Rename a few times to random names of the same length, so the old name isn't left in the
/// directory entry either. Returns the final path.
fn scramble_name(p: &Path, rng: &mut Rng) -> PathBuf {
    let mut cur = p.to_path_buf();
    let len = p.file_name().map(|n| n.len()).unwrap_or(8);
    for _ in 0..3 {
        let next = cur.with_file_name(rng.name(len));
        if !next.exists() && fs::rename(&cur, &next).is_ok() {
            cur = next;
        }
    }
    cur
}

fn shred_file(p: &Path, passes: u32, ctl: &Control, rng: &mut Rng, buf: &mut [u8]) -> Result<u64> {
    ctl.set_current(p.display().to_string());
    let meta = fs::symlink_metadata(p)?;
    if meta.permissions().readonly() {
        let mut perm = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(p, perm)?;
    }
    let len = meta.len();
    let mut f = File::options().write(true).open(p).with_context(|| format!("opening {}", p.display()))?;
    for i in 0..passes {
        f.rewind()?;
        let pat = pattern(i, passes);
        let mut left = len;
        while left > 0 {
            ctl.checkpoint()?;
            let n = left.min(buf.len() as u64) as usize;
            fill(&mut buf[..n], pat, rng);
            f.write_all(&buf[..n])?;
            left -= n as u64;
            ctl.done.fetch_add(n as u64, Ordering::Relaxed);
        }
        f.sync_all()?;
    }
    f.set_len(0)?;
    f.sync_all()?;
    drop(f);
    let last = scramble_name(p, rng);
    fs::remove_file(&last).with_context(|| format!("deleting {}", p.display()))?;
    Ok(len)
}

fn shred_tree(p: &Path, passes: u32, ctl: &Control, rng: &mut Rng, buf: &mut [u8], r: &mut ShredReport) -> Result<()> {
    ctl.checkpoint()?;
    let meta = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) => {
            r.failed.push((p.display().to_string(), e.to_string()));
            return Ok(());
        }
    };
    if meta.file_type().is_symlink() {
        // Never follow links: only the link itself goes.
        if let Err(e) = fs::remove_file(p) {
            r.failed.push((p.display().to_string(), e.to_string()));
        }
        return Ok(());
    }
    if meta.is_dir() {
        for e in fs::read_dir(p)?.flatten() {
            shred_tree(&e.path(), passes, ctl, rng, buf, r)?;
        }
        let last = scramble_name(p, rng);
        match fs::remove_dir(&last) {
            Ok(()) => r.folders += 1,
            Err(e) => r.failed.push((p.display().to_string(), e.to_string())),
        }
        return Ok(());
    }
    match shred_file(p, passes, ctl, rng, buf) {
        Ok(n) => {
            r.files += 1;
            r.bytes += n;
        }
        Err(e) if ctl.cancel.load(Ordering::Relaxed) => return Err(e),
        Err(e) => r.failed.push((p.display().to_string(), format!("{e:#}"))),
    }
    Ok(())
}

/// Unrecoverable Delete: overwrite and delete files and folders.
pub fn shred(paths: &[PathBuf], passes: u32, ctl: &Control) -> Result<ShredReport> {
    let passes = passes.clamp(1, 35);
    for p in paths {
        if is_protected(p) {
            bail!("{} is a system or home folder and can't be shredded", p.display());
        }
    }
    ctl.total.store(paths.iter().map(|p| total_size(p)).sum::<u64>() * passes as u64, Ordering::Relaxed);
    let mut r = ShredReport::default();
    let mut rng = Rng::new();
    let mut buf = vec![0u8; CHUNK];
    for p in paths {
        shred_tree(p, passes, ctl, &mut rng, &mut buf, &mut r)?;
    }
    Ok(r)
}

/// Roots, the home folder and OS folders are refused outright.
fn is_protected(p: &Path) -> bool {
    let Ok(abs) = std::path::absolute(p) else {
        return true;
    };
    if abs.parent().is_none() || dirs::home_dir().is_some_and(|h| h == abs) {
        return true;
    }
    let s = abs.display().to_string().to_lowercase();
    let sys: &[&str] = if cfg!(windows) {
        &[r"c:\windows", r"c:\program files", r"c:\program files (x86)", r"c:\programdata", r"c:\users"]
    } else {
        &["/usr", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/var", "/opt", "/home", "/root", "/proc", "/sys", "/dev"]
    };
    sys.contains(&s.trim_end_matches(['/', '\\']))
}

fn disk_full(e: &std::io::Error) -> bool {
    // ENOSPC / ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL
    e.kind() == ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(28) | Some(112) | Some(39))
}

/// Evidence Remover: overwrite the free space of the drive holding `dir`.
/// `limit` caps the bytes written per pass (tests, or a partial wipe).
pub fn wipe_free_space(dir: &Path, passes: u32, limit: Option<u64>, ctl: &Control) -> Result<u64> {
    let passes = passes.clamp(1, 7);
    let free = crate::platform::free_space(dir).context("reading the free space")?;
    let per_pass = limit.map_or(free, |l| l.min(free));
    ctl.total.store(per_pass * passes as u64, Ordering::Relaxed);
    let work = dir.join(format!(".cleancore-wipe-{}", std::process::id()));
    crate::config::ensure_dir(&work)?;
    let result = (|| -> Result<u64> {
        let mut rng = Rng::new();
        let mut buf = vec![0u8; CHUNK];
        let mut written_total = 0;
        for i in 0..passes {
            ctl.pass.store(i + 1, Ordering::Relaxed);
            let pat = pattern(i, passes);
            let mut written = 0u64;
            let mut n = 0;
            'files: loop {
                let path = work.join(format!("fill{n:05}.bin"));
                n += 1;
                ctl.set_current(path.display().to_string());
                let mut f = File::create(&path)?;
                // 1 GiB per file keeps every filesystem (FAT32: 4 GiB max) happy.
                for _ in 0..1024 {
                    ctl.checkpoint()?;
                    let want = (per_pass - written).min(CHUNK as u64) as usize;
                    if want == 0 {
                        let _ = f.sync_all();
                        break 'files;
                    }
                    fill(&mut buf[..want], pat, &mut rng);
                    match f.write_all(&buf[..want]) {
                        Ok(()) => {
                            written += want as u64;
                            ctl.done.fetch_add(want as u64, Ordering::Relaxed);
                        }
                        Err(e) if disk_full(&e) => {
                            let _ = f.sync_all();
                            break 'files;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                f.sync_all()?;
            }
            // Small files take over directory / MFT slots of deleted small files too.
            if limit.is_none() {
                for k in 0..20_000 {
                    if ctl.checkpoint().is_err() {
                        break;
                    }
                    let mut small = [0u8; 512];
                    rng.fill(&mut small);
                    if fs::write(work.join(format!("s{k:05}")), small).is_err() {
                        break;
                    }
                }
            }
            written_total += written;
            // Free the space before the next pass.
            for e in fs::read_dir(&work)?.flatten() {
                let _ = fs::remove_file(e.path());
            }
        }
        Ok(written_total)
    })();
    // Whatever happened, give the space back.
    let _ = fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shreds_tree_and_refuses_roots() {
        let root = std::env::temp_dir().join(format!("cleancore-shred-{}", std::process::id()));
        fs::create_dir_all(root.join("d/e")).unwrap();
        fs::write(root.join("d/a.txt"), vec![7u8; 3 * CHUNK + 5]).unwrap();
        fs::write(root.join("d/e/b.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", root.join("d/link")).unwrap();
        let ctl = Control::default();
        let r = shred(&[root.join("d")], 3, &ctl).unwrap();
        assert_eq!((r.files, r.folders), (2, 2));
        assert!(r.failed.is_empty(), "{:?}", r.failed);
        assert!(!root.join("d").exists());
        // The symlink was removed, not the file it points to.
        #[cfg(unix)]
        assert!(Path::new("/etc/hostname").exists());
        assert_eq!(ctl.done.load(Ordering::Relaxed), ctl.total.load(Ordering::Relaxed));
        assert!(shred(&[PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })], 1, &ctl).is_err());
        assert!(shred(&[dirs::home_dir().unwrap()], 1, &ctl).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn wipe_with_limit_cleans_up() {
        let root = std::env::temp_dir().join(format!("cleancore-wipe-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let ctl = Control::default();
        let n = wipe_free_space(&root, 2, Some(3 * CHUNK as u64), &ctl).unwrap();
        assert_eq!(n, 6 * CHUNK as u64);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }
}
