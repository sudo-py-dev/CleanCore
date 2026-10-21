//! Linux watcher backend.
//!
//! * Files and folders: inotify. After each burst of events the touched paths are re-read and
//!   diffed against a snapshot, so atomic saves (write temp file, rename over) show up as plain
//!   value changes. INI-style files are tracked per `section/key`, JSON per flattened field,
//!   anything else as one content fingerprint.
//! * `dconf:/path/` targets: a `dconf watch` child process per target; old values come from a
//!   `dconf dump` snapshot.
//!
//! inotify and dconf don't report which process made a change, so Process/PID stay empty.

use crate::watcher::{diff, under, Op, Snapshot, Target, Typed, WatchEvent};
use anyhow::{Context, Result};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::ATTRIB)
    .union(WatchMask::DONT_FOLLOW);
const MAX_FILES_PER_TARGET: usize = 20_000;
const MAX_DEPTH: usize = 12;
const MAX_PARSE_BYTES: u64 = 512 * 1024;

pub struct WatchBackend {
    tx: Sender<WatchEvent>,
    fs: Option<FsWatcher>,
    dconf: HashMap<String, Child>,
}

impl WatchBackend {
    pub fn new(tx: Sender<WatchEvent>) -> Result<Self> {
        Ok(Self { tx, fs: None, dconf: HashMap::new() })
    }

    pub fn mode(&self) -> &'static str {
        "inotify + dconf watch (Linux does not report the process that made a change)"
    }

    pub fn set_targets(&mut self, targets: &[Target]) -> Result<()> {
        let (dconf, files): (Vec<&Target>, Vec<&Target>) = targets.iter().partition(|t| t.path.starts_with("dconf:"));

        let wanted: HashSet<&str> = dconf.iter().map(|t| t.path.as_str()).collect();
        self.dconf.retain(|path, child| {
            let keep = wanted.contains(path.as_str());
            if !keep {
                let _ = child.kill();
                let _ = child.wait();
            }
            keep
        });
        let mut errors = Vec::new();
        for t in dconf {
            if !self.dconf.contains_key(&t.path) {
                match spawn_dconf(&t.path, self.tx.clone()) {
                    Ok(c) => {
                        self.dconf.insert(t.path.clone(), c);
                    }
                    Err(e) => errors.push(format!("{}: {e:#}", t.path)),
                }
            }
        }

        let paths: Vec<PathBuf> = files.iter().map(|t| PathBuf::from(&t.path)).collect();
        if self.fs.is_none() && !paths.is_empty() {
            self.fs = Some(FsWatcher::start(self.tx.clone())?);
        }
        if let Some(fs) = &self.fs {
            errors.extend(fs.set_targets(&paths));
        }
        if !errors.is_empty() {
            anyhow::bail!("{}", errors.join("; "));
        }
        Ok(())
    }
}

impl Drop for WatchBackend {
    fn drop(&mut self) {
        for c in self.dconf.values_mut() {
            let _ = c.kill();
        }
    }
}

// ---------------------------------------------------------------- inotify

struct FsState {
    targets: Vec<PathBuf>,
    /// Watched directory for each descriptor.
    dirs: HashMap<WatchDescriptor, PathBuf>,
    snapshot: Snapshot,
    watches: Watches,
}

struct FsWatcher {
    state: Arc<Mutex<FsState>>,
}

impl FsWatcher {
    fn start(tx: Sender<WatchEvent>) -> Result<Self> {
        let inotify = Inotify::init().context("inotify_init")?;
        let state = Arc::new(Mutex::new(FsState { targets: Vec::new(), dirs: HashMap::new(), snapshot: Snapshot::new(), watches: inotify.watches() }));
        let st = state.clone();
        std::thread::spawn(move || event_loop(inotify, st, tx));
        Ok(Self { state })
    }

    fn set_targets(&self, targets: &[PathBuf]) -> Vec<String> {
        let mut st = self.state.lock().unwrap();
        let mut errors = Vec::new();
        let removed: Vec<PathBuf> = st.targets.iter().filter(|t| !targets.contains(t)).cloned().collect();
        let added: Vec<PathBuf> = targets.iter().filter(|t| !st.targets.contains(t)).cloned().collect();
        st.targets = targets.to_vec();
        let covered = |st: &FsState, p: &Path| st.targets.iter().any(|t| p.starts_with(t) || t.parent() == Some(p));

        for r in removed {
            let gone: Vec<WatchDescriptor> =
                st.dirs.iter().filter(|(_, d)| (d.starts_with(&r) || r.parent() == Some(d.as_path())) && !covered(&st, d)).map(|(w, _)| w.clone()).collect();
            for wd in gone {
                st.dirs.remove(&wd);
                let _ = st.watches.remove(wd);
            }
            let rs = r.to_string_lossy().to_string();
            let tracked: Vec<String> = st.targets.iter().map(|t| t.to_string_lossy().to_string()).collect();
            st.snapshot.retain(|k, _| !under(k, &rs) || tracked.iter().any(|t| under(k, t)));
        }
        for a in added {
            if !a.exists() {
                errors.push(format!("{} does not exist (it will not be watched until re-added)", a.display()));
                continue;
            }
            // A file target is watched through its folder, so atomic saves (rename over) are seen.
            let watch_root = if a.is_dir() { a.clone() } else { a.parent().unwrap_or(&a).to_path_buf() };
            if a.is_dir() {
                add_tree(&mut st, &watch_root, &mut errors);
            } else {
                add_dir(&mut st, &watch_root, &mut errors);
            }
            let snap = read_tree(&a);
            st.snapshot.extend(snap);
        }
        errors
    }
}

fn add_dir(st: &mut FsState, dir: &Path, errors: &mut Vec<String>) {
    if st.dirs.values().any(|d| d == dir) {
        return;
    }
    match st.watches.add(dir, MASK) {
        Ok(wd) => {
            st.dirs.insert(wd, dir.to_path_buf());
        }
        Err(e) => errors.push(format!("cannot watch {}: {e}{}", dir.display(), if e.raw_os_error() == Some(28) { " (raise fs.inotify.max_user_watches)" } else { "" })),
    }
}

fn add_tree(st: &mut FsState, root: &Path, errors: &mut Vec<String>) {
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        add_dir(st, &d, errors);
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push((e.path(), depth + 1));
                }
            }
        }
    }
}

fn event_loop(mut inotify: Inotify, state: Arc<Mutex<FsState>>, tx: Sender<WatchEvent>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut raw: Vec<(WatchDescriptor, EventMask, u32, Option<std::ffi::OsString>)> = Vec::new();
        match inotify.read_events_blocking(&mut buf) {
            Ok(evs) => raw.extend(evs.map(|e| (e.wd, e.mask, e.cookie, e.name.map(|n| n.to_os_string())))),
            Err(_) => return,
        }
        // Coalesce a burst (editors write a temp file, then rename it) into one diff.
        std::thread::sleep(Duration::from_millis(60));
        while let Ok(evs) = inotify.read_events(&mut buf) {
            let before = raw.len();
            raw.extend(evs.map(|e| (e.wd, e.mask, e.cookie, e.name.map(|n| n.to_os_string()))));
            if raw.len() == before {
                break;
            }
        }

        let mut st = state.lock().unwrap();
        let mut touched: Vec<PathBuf> = Vec::new();
        let mut moved_from: HashMap<u32, PathBuf> = HashMap::new();
        let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut new_dirs: Vec<PathBuf> = Vec::new();
        let mut overflow = false;
        for (wd, mask, cookie, name) in raw {
            if mask.contains(EventMask::Q_OVERFLOW) {
                overflow = true;
                continue;
            }
            if mask.contains(EventMask::IGNORED) {
                st.dirs.remove(&wd);
                continue;
            }
            let Some(dir) = st.dirs.get(&wd) else {
                continue;
            };
            let path = match name {
                Some(n) => dir.join(n),
                None => dir.clone(),
            };
            if mask.contains(EventMask::MOVED_FROM) {
                moved_from.insert(cookie, path.clone());
            }
            if mask.contains(EventMask::MOVED_TO) {
                if let Some(from) = moved_from.remove(&cookie) {
                    renames.push((from, path.clone()));
                }
            }
            if mask.contains(EventMask::ISDIR) && mask.intersects(EventMask::CREATE | EventMask::MOVED_TO) {
                new_dirs.push(path.clone());
            }
            touched.push(path);
        }
        let mut errors = Vec::new();
        for d in &new_dirs {
            if st.targets.iter().any(|t| d.starts_with(t)) {
                add_tree(&mut st, d, &mut errors);
            }
        }
        if overflow {
            // Too many events to follow individually: re-read every target.
            touched = st.targets.clone();
        }
        for ev in process_touched(&mut st, touched, &renames) {
            if tx.send(ev).is_err() {
                return;
            }
        }
    }
}

/// Re-read touched paths, diff against the snapshot, update the snapshot.
fn process_touched(st: &mut FsState, mut touched: Vec<PathBuf>, renames: &[(PathBuf, PathBuf)]) -> Vec<WatchEvent> {
    let targets: Vec<String> = st.targets.iter().map(|t| t.to_string_lossy().to_string()).collect();
    let in_scope = |p: &str| targets.iter().any(|t| under(p, t));
    touched.retain(|p| in_scope(&p.to_string_lossy()));
    touched.sort();
    touched.dedup();
    // Keep only top-most paths; a re-read of a folder covers everything inside it.
    let mut tops: Vec<PathBuf> = Vec::new();
    for p in touched {
        if !tops.iter().any(|t| p.starts_with(t) && st.snapshot.contains_key(&*t.to_string_lossy())) {
            tops.push(p);
        }
    }

    let mut events = Vec::new();
    let mut old_part = Snapshot::new();
    let mut new_part = Snapshot::new();
    for p in &tops {
        let ps = p.to_string_lossy().to_string();
        let keys: Vec<String> = st.snapshot.keys().filter(|k| under(k, &ps)).cloned().collect();
        for k in keys {
            if let Some(v) = st.snapshot.remove(&k) {
                old_part.insert(k, v);
            }
        }
        for (k, v) in read_tree(p) {
            if in_scope(&k) {
                new_part.insert(k, v);
            }
        }
    }
    // Renames: report them, then compare the old content under its new name.
    for (from, to) in renames {
        let (fs_, ts) = (from.to_string_lossy().to_string(), to.to_string_lossy().to_string());
        if !old_part.contains_key(&fs_) || old_part.contains_key(&ts) {
            continue; // temp file renamed over an existing file: just a content change
        }
        let mut ev = WatchEvent::new(Op::RenamedKey, &fs_);
        (ev.old_data, ev.new_data) = (Some(fs_.clone()), Some(ts.clone()));
        events.push(ev);
        let moved: Vec<String> = old_part.keys().filter(|k| under(k, &fs_)).cloned().collect();
        for k in moved {
            let v = old_part.remove(&k).unwrap();
            old_part.insert(format!("{ts}{}", &k[fs_.len()..]), v);
        }
    }
    events.extend(diff(&old_part, &new_part));
    st.snapshot.extend(new_part);
    events
}

/// Snapshot of a file or a folder tree.
fn read_tree(root: &Path) -> Snapshot {
    let mut snap = Snapshot::new();
    let meta = match std::fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(_) => return snap,
    };
    if !meta.is_dir() {
        snap.insert(root.to_string_lossy().to_string(), file_values(root, &meta));
        return snap;
    }
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        snap.insert(d.to_string_lossy().to_string(), BTreeMap::new());
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            if snap.len() >= MAX_FILES_PER_TARGET {
                return snap;
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((e.path(), depth + 1));
                }
            } else {
                snap.insert(e.path().to_string_lossy().to_string(), file_values(&e.path(), &m));
            }
        }
    }
    snap
}

/// The "values" of a file: INI keys, flattened JSON fields, or a content fingerprint.
fn file_values(path: &Path, meta: &std::fs::Metadata) -> BTreeMap<String, Typed> {
    let mut out = BTreeMap::new();
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path).map(|t| t.display().to_string()).unwrap_or_default();
        out.insert("(link)".into(), ("Link".into(), target));
        return out;
    }
    let text = (meta.len() <= MAX_PARSE_BYTES).then(|| std::fs::read(path).ok()).flatten();
    let Some(bytes) = text else {
        out.insert("(content)".into(), ("File".into(), format!("{} bytes", meta.len())));
        return out;
    };
    if let Ok(s) = std::str::from_utf8(&bytes) {
        if let Some(v) = parse_json(s) {
            return v;
        }
        if let Some(v) = parse_ini(s) {
            return v;
        }
    }
    out.insert("(content)".into(), ("File".into(), fingerprint(&bytes)));
    out
}

fn fingerprint(b: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{} bytes, hash {h:016x}", b.len())
}

/// `key=value` files (desktop entries, .conf, .ini, kde rc files) → `section/key`.
fn parse_ini(s: &str) -> Option<BTreeMap<String, Typed>> {
    let mut out = BTreeMap::new();
    let (mut section, mut kv, mut other) = (String::new(), 0, 0);
    for line in s.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') {
            section = l[1..l.len() - 1].to_string();
            continue;
        }
        match l.split_once('=') {
            Some((k, v)) if k.split_whitespace().count() == 1 => {
                kv += 1;
                let name = if section.is_empty() { k.trim().to_string() } else { format!("{section}/{}", k.trim()) };
                out.insert(name, ("String".into(), v.trim().to_string()));
            }
            _ => other += 1,
        }
    }
    (kv > 0 && kv >= other).then_some(out)
}

fn parse_json(s: &str) -> Option<BTreeMap<String, Typed>> {
    let t = s.trim_start();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let mut out = BTreeMap::new();
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut BTreeMap<String, Typed>) {
        match v {
            serde_json::Value::Object(m) if !m.is_empty() => {
                for (k, x) in m {
                    walk(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, x, out);
                }
            }
            serde_json::Value::Array(a) if !a.is_empty() => {
                for (i, x) in a.iter().enumerate() {
                    walk(&format!("{prefix}[{i}]"), x, out);
                }
            }
            other => {
                let name = if prefix.is_empty() { "(root)".to_string() } else { prefix.to_string() };
                out.insert(name, ("JSON".into(), other.to_string()));
            }
        }
    }
    walk("", &v, &mut out);
    Some(out)
}

// ---------------------------------------------------------------- dconf

/// `dconf dump /dir/` → full key path → value.
fn dconf_dump(dir: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(o) = Command::new("dconf").args(["dump", dir]).output() else {
        return out;
    };
    let mut section = String::new();
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim_matches('/').to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            let base = if section.is_empty() { dir.to_string() } else { format!("{dir}{section}/") };
            out.insert(format!("{base}{k}"), v.to_string());
        }
    }
    out
}

fn dconf_read(key: &str) -> Option<String> {
    let o = Command::new("dconf").args(["read", key]).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Turn one `dconf watch` notification into events, updating the snapshot.
fn dconf_change(path: &str, new_value: Option<String>, snap: &mut HashMap<String, String>) -> Vec<WatchEvent> {
    let mut out = Vec::new();
    if path.ends_with('/') {
        // A whole directory changed (e.g. reset): re-dump and compare.
        let now = dconf_dump(path);
        let before: Vec<(String, String)> = snap.iter().filter(|(k, _)| k.starts_with(path)).map(|(k, v)| (k.clone(), v.clone())).collect();
        for (k, old) in &before {
            if !now.contains_key(k) {
                snap.remove(k);
                out.push(event_for(k, Some(old), None));
            }
        }
        for (k, v) in now {
            let old = snap.insert(k.clone(), v.clone());
            if old.as_deref() != Some(v.as_str()) {
                out.push(event_for(&k, old.as_deref(), Some(&v)));
            }
        }
        return out;
    }
    let old = match &new_value {
        Some(v) => snap.insert(path.to_string(), v.clone()),
        None => snap.remove(path),
    };
    if old != new_value {
        out.push(event_for(path, old.as_deref(), new_value.as_deref()));
    }
    out
}

fn event_for(key: &str, old: Option<&str>, new: Option<&str>) -> WatchEvent {
    let (dir, name) = key.rsplit_once('/').unwrap_or(("", key));
    let op = if new.is_some() { Op::ValueSet } else { Op::DeletedValue };
    WatchEvent::new(op, format!("dconf:{dir}")).value(name, Some("GVariant"), old, new)
}

fn spawn_dconf(target: &str, tx: Sender<WatchEvent>) -> Result<Child> {
    let dir = target.trim_start_matches("dconf:").to_string();
    let mut child = Command::new("dconf").args(["watch", &dir]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("starting `dconf watch` (is dconf installed?)")?;
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let mut snap = dconf_dump(&dir);
        let mut lines = std::io::BufReader::new(stdout).lines().map_while(std::result::Result::ok).peekable();
        while let Some(line) = lines.next() {
            if !line.starts_with('/') {
                continue;
            }
            // Format: the changed path, then "  <value>" ("  unset" after a reset), then a blank line.
            // GVariant strings are quoted, so a bare `unset` can't be a real value.
            let printed = match lines.peek() {
                Some(v) if v.starts_with("  ") => {
                    let v = v.trim().to_string();
                    lines.next();
                    Some((!v.is_empty() && v != "unset").then_some(v))
                }
                _ => None,
            };
            let value = if line.ends_with('/') { None } else { printed.unwrap_or_else(|| dconf_read(&line)) };
            for ev in dconf_change(&line, value, &mut snap) {
                if tx.send(ev).is_err() {
                    return;
                }
            }
        }
    });
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_and_json_values() {
        let ini = parse_ini("# c\n[Desktop Entry]\nName=X\nExec=/bin/x %U\n").unwrap();
        assert_eq!(ini["Desktop Entry/Exec"].1, "/bin/x %U");
        assert!(parse_ini("just some\nplain text\nwithout pairs\n").is_none());
        let j = parse_json(r#"{"editor":{"fontSize":14},"list":[1,"a"]}"#).unwrap();
        assert_eq!(j["editor.fontSize"].1, "14");
        assert_eq!(j["list[1]"].1, "\"a\"");
    }

    #[test]
    fn dconf_events() {
        let mut snap: HashMap<String, String> = [("/org/x/a".to_string(), "1".to_string())].into();
        let ev = dconf_change("/org/x/a", Some("2".into()), &mut snap);
        assert_eq!((ev[0].op, ev[0].key.as_str(), ev[0].value.as_deref()), (Op::ValueSet, "dconf:/org/x", Some("a")));
        assert_eq!((ev[0].old_data.as_deref(), ev[0].new_data.as_deref()), (Some("1"), Some("2")));
        let ev = dconf_change("/org/x/a", None, &mut snap);
        assert_eq!(ev[0].op, Op::DeletedValue);
        assert!(dconf_change("/org/x/a", None, &mut snap).is_empty());
    }
}
