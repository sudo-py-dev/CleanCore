//! File and folder backup, restore and deletion shared by both platforms.

use crate::backup::BackupEntry;
use crate::config::ensure_dir;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Copy `path` into the backup payload folder and describe how to put it back.
pub fn backup_path(path: &Path, payload: &Path, n: usize) -> Result<BackupEntry> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(BackupEntry::Symlink { original: path.into(), target: fs::read_link(path)? });
    }
    if meta.is_dir() && fs::read_dir(path)?.next().is_none() {
        return Ok(BackupEntry::EmptyDir { original: path.into() });
    }
    let stored = format!("{n:05}");
    ensure_dir(payload)?;
    copy_tree(path, &payload.join(&stored))?;
    Ok(BackupEntry::FileCopy { original: path.into(), stored: format!("payload/{stored}"), is_dir: meta.is_dir() })
}

fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        make_symlink(&fs::read_link(src)?, dst)?;
    } else if meta.is_dir() {
        ensure_dir(dst)?;
        for e in fs::read_dir(src)?.flatten() {
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        fs::copy(src, dst).with_context(|| format!("copying {}", src.display()))?;
    }
    Ok(())
}

/// Restore one file-type backup entry. Returns false for entries it doesn't handle.
pub fn restore_entry(dir: &Path, e: &BackupEntry) -> Result<bool> {
    match e {
        BackupEntry::FileCopy { original, stored, is_dir } => {
            if let Some(parent) = original.parent() {
                ensure_dir(parent)?;
            }
            if *is_dir {
                if original.exists() {
                    fs::remove_dir_all(original)?;
                }
            } else if fs::symlink_metadata(original).is_ok() {
                fs::remove_file(original)?;
            }
            copy_tree(&dir.join(stored), original)?;
        }
        BackupEntry::Symlink { original, target } => {
            if fs::symlink_metadata(original).is_ok() {
                fs::remove_file(original)?;
            }
            if let Some(parent) = original.parent() {
                ensure_dir(parent)?;
            }
            make_symlink(target, original)?;
        }
        BackupEntry::EmptyDir { original } => ensure_dir(original)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// Delete a file, symlink or folder tree; to the Trash / Recycle Bin when `to_trash` is set
/// (falling back to permanent deletion when the item can't be trashed, e.g. another drive).
pub fn delete_path(path: &Path, to_trash: bool) -> Result<()> {
    if to_trash && trash(path).is_ok() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("deleting {}", path.display()))
    }
}

/// freedesktop.org Trash: `$XDG_DATA_HOME/Trash/{files,info}`.
#[cfg(not(windows))]
pub fn trash(path: &Path) -> Result<()> {
    use std::time::SystemTime;
    let base = dirs::data_dir().context("no data dir")?.join("Trash");
    let (files, info) = (base.join("files"), base.join("info"));
    ensure_dir(&files)?;
    ensure_dir(&info)?;
    let name = path.file_name().context("no file name")?.to_string_lossy().to_string();
    let mut n = 1;
    let mut cand = name.clone();
    while files.join(&cand).exists() || info.join(format!("{cand}.trashinfo")).exists() {
        n += 1;
        cand = format!("{name}.{n}");
    }
    let abs = std::path::absolute(path)?;
    let enc: String =
        abs.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let now = chrono::DateTime::<chrono::Local>::from(SystemTime::now()).format("%Y-%m-%dT%H:%M:%S");
    fs::write(info.join(format!("{cand}.trashinfo")), format!("[Trash Info]\nPath={enc}\nDeletionDate={now}\n"))?;
    if let Err(e) = fs::rename(path, files.join(&cand)) {
        let _ = fs::remove_file(info.join(format!("{cand}.trashinfo")));
        return Err(e).context("moving to Trash (different filesystem?)");
    }
    Ok(())
}

/// Windows Recycle Bin via `SHFileOperationW(FO_DELETE, FOF_ALLOWUNDO)`.
#[cfg(windows)]
pub fn trash(path: &Path) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    let abs = std::path::absolute(path)?;
    // Double-NUL-terminated list of paths.
    let mut from: Vec<u16> = abs.as_os_str().to_string_lossy().encode_utf16().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
    // SAFETY: `op` is fully initialized and `from` outlives the call.
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted != 0 {
        anyhow::bail!("Recycle Bin operation failed ({r})");
    }
    Ok(())
}

/// Apply a [`crate::model::Fix::EditJson`]: remove object keys / matching array elements.
pub fn edit_json(file: &Path, remove: &[String]) -> Result<()> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let mut v: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    for path in remove {
        remove_path(&mut v, path);
    }
    let tmp = file.with_extension("cleancore-tmp");
    fs::write(&tmp, serde_json::to_string(&v)?)?;
    fs::rename(&tmp, file)?;
    Ok(())
}

fn remove_path(v: &mut serde_json::Value, path: &str) {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut cur = v;
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        // `name[key=value]`: array `name`, element whose `key` equals `value`
        if let Some((name, rest)) = seg.split_once('[') {
            let (k, val) = rest.trim_end_matches(']').split_once('=').unwrap_or(("", ""));
            let Some(arr) = cur.get_mut(name).and_then(|a| a.as_array_mut()) else {
                return;
            };
            if last {
                arr.retain(|e| e.get(k).and_then(|x| x.as_str()) != Some(val));
                return;
            }
            let Some(next) = arr.iter_mut().find(|e| e.get(k).and_then(|x| x.as_str()) == Some(val)) else {
                return;
            };
            cur = next;
            continue;
        }
        if last {
            if let Some(o) = cur.as_object_mut() {
                o.remove(*seg);
            }
            return;
        }
        let Some(next) = cur.get_mut(*seg) else {
            return;
        };
        cur = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_paths() {
        let mut v: serde_json::Value = serde_json::from_str(r#"{"extensions":{"settings":{"a":1,"b":2}},"addons":[{"id":"x"},{"id":"y"}]}"#).unwrap();
        remove_path(&mut v, "/extensions/settings/a");
        remove_path(&mut v, "/addons[id=y]");
        remove_path(&mut v, "/missing/key");
        assert_eq!(v.to_string(), r#"{"addons":[{"id":"x"}],"extensions":{"settings":{"b":2}}}"#);
    }
}
