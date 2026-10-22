//! Registry Watcher: records every change under watched registry keys (Windows) or
//! configuration paths / dconf directories (Linux), with the process responsible when the
//! OS reports it.
//!
//! The watch list and logs live in `<data_dir>/watcher/`. `cleancore watch run` is the
//! long-running recorder; the other `watch` commands edit the list and read the logs, so they
//! work whether or not the recorder is running.

use crate::config::{data_dir, ensure_dir, Settings};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------- events

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    CreatedKey,
    DeletedKey,
    RenamedKey,
    ValueSet,
    DeletedValue,
    SetInformationKey,
    QueryKey,
    QueryValue,
    QueryMultipleValue,
    EnumerateKey,
    EnumerateValue,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Self::CreatedKey => "Created Key",
            Self::DeletedKey => "Deleted Key",
            Self::RenamedKey => "Renamed Key",
            Self::ValueSet => "Value Set",
            Self::DeletedValue => "Deleted Value",
            Self::SetInformationKey => "Set Information Key",
            Self::QueryKey => "Query Key",
            Self::QueryValue => "Query Value",
            Self::QueryMultipleValue => "Query Multiple Value",
            Self::EnumerateKey => "Enumerate Key",
            Self::EnumerateValue => "Enumerate Value",
        }
    }

    pub fn is_read(self) -> bool {
        matches!(self, Self::QueryKey | Self::QueryValue | Self::QueryMultipleValue | Self::EnumerateKey | Self::EnumerateValue)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchEvent {
    pub time: DateTime<Local>,
    /// Process image name; empty when the OS doesn't say (inotify, change notifications).
    #[serde(default)]
    pub process: String,
    #[serde(default)]
    pub pid: Option<u32>,
    pub op: Op,
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub new_data: Option<String>,
    #[serde(default)]
    pub old_data: Option<String>,
    #[serde(default)]
    pub data_type: Option<String>,
    /// Which watch recorded it (filled by the recorder).
    #[serde(default)]
    pub watch: String,
}

impl WatchEvent {
    pub fn new(op: Op, key: impl Into<String>) -> Self {
        Self { time: Local::now(), process: String::new(), pid: None, op, key: key.into(), value: None, new_data: None, old_data: None, data_type: None, watch: String::new() }
    }

    pub fn value(mut self, name: &str, ty: Option<&str>, old: Option<&str>, new: Option<&str>) -> Self {
        self.value = Some(name.to_string());
        self.data_type = ty.map(str::to_string);
        self.old_data = old.map(str::to_string);
        self.new_data = new.map(str::to_string);
        self
    }

    /// Every searchable column, for `--filter` and `--highlight`.
    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {}",
            self.process,
            self.pid.map(|p| p.to_string()).unwrap_or_default(),
            self.op.label(),
            self.key,
            self.value.as_deref().unwrap_or(""),
            self.new_data.as_deref().unwrap_or(""),
            self.old_data.as_deref().unwrap_or(""),
            self.data_type.as_deref().unwrap_or("")
        )
        .to_lowercase()
    }
}

// ---------------------------------------------------------------- snapshots and diffing

/// A data value with its type label (`String`, `DWord`, `Line`, ...).
pub type Typed = (String, String);
/// key path → value name → typed data.
pub type Snapshot = BTreeMap<String, BTreeMap<String, Typed>>;

/// Events that turn `old` into `new`, for keys under `scope` (pass "" for everything).
/// This is what backends without per-operation detail use (inotify, change notifications, dconf).
pub fn diff(old: &Snapshot, new: &Snapshot) -> Vec<WatchEvent> {
    let mut out = Vec::new();
    for (key, vals) in new {
        match old.get(key) {
            None => {
                out.push(WatchEvent::new(Op::CreatedKey, key));
                for (name, (ty, data)) in vals {
                    out.push(WatchEvent::new(Op::ValueSet, key).value(name, Some(ty), None, Some(data)));
                }
            }
            Some(before) => {
                for (name, (ty, data)) in vals {
                    if before.get(name).is_none_or(|(_, d)| d != data) {
                        let old_data = before.get(name).map(|(_, d)| d.as_str());
                        out.push(WatchEvent::new(Op::ValueSet, key).value(name, Some(ty), old_data, Some(data)));
                    }
                }
                for (name, (ty, data)) in before {
                    if !vals.contains_key(name) {
                        out.push(WatchEvent::new(Op::DeletedValue, key).value(name, Some(ty), Some(data), None));
                    }
                }
            }
        }
    }
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        out.push(WatchEvent::new(Op::DeletedKey, key));
    }
    out
}

/// Case-insensitive "is `path` the watch target or below it", with `\` or `/` separators.
pub fn under(path: &str, target: &str) -> bool {
    let (p, t) = (path.to_lowercase(), target.to_lowercase().trim_end_matches(['\\', '/']).to_string());
    p == t || (p.starts_with(&t) && matches!(p.as_bytes().get(t.len()), Some(b'\\') | Some(b'/')))
}

// ---------------------------------------------------------------- Windows key names

/// `HKCU\x`, `HKEY_CURRENT_USER\x`, `hklm/x` → `HKEY_CURRENT_USER\x` / `HKEY_LOCAL_MACHINE\x`.
pub fn canonical_key(k: &str) -> String {
    let k = k.trim().trim_matches('"').replace('/', "\\");
    let k = k.trim_end_matches('\\');
    let (root, rest) = k.split_once('\\').unwrap_or((k, ""));
    let root = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => "HKEY_LOCAL_MACHINE",
        "HKCU" | "HKEY_CURRENT_USER" => "HKEY_CURRENT_USER",
        "HKU" | "HKEY_USERS" => "HKEY_USERS",
        "HKCR" | "HKEY_CLASSES_ROOT" => "HKEY_CLASSES_ROOT",
        "HKCC" | "HKEY_CURRENT_CONFIG" => "HKEY_CURRENT_CONFIG",
        _ => return k.to_string(),
    };
    if rest.is_empty() {
        root.to_string()
    } else {
        format!("{root}\\{rest}")
    }
}

/// Friendly key → kernel object name (`\REGISTRY\MACHINE\...`), as ETW reports it.
pub fn to_kernel_path(key: &str, user_sid: &str) -> String {
    let key = canonical_key(key);
    let (root, rest) = key.split_once('\\').unwrap_or((&key, ""));
    let base = match root {
        "HKEY_LOCAL_MACHINE" => r"\REGISTRY\MACHINE".to_string(),
        "HKEY_USERS" => r"\REGISTRY\USER".to_string(),
        "HKEY_CURRENT_USER" => format!(r"\REGISTRY\USER\{user_sid}"),
        "HKEY_CLASSES_ROOT" => r"\REGISTRY\MACHINE\SOFTWARE\Classes".to_string(),
        "HKEY_CURRENT_CONFIG" => r"\REGISTRY\MACHINE\SYSTEM\CurrentControlSet\Hardware Profiles\Current".to_string(),
        _ => return key.to_string(),
    };
    if rest.is_empty() {
        base
    } else {
        format!(r"{base}\{rest}")
    }
}

/// Kernel object name → friendly key. The current user's hive is shown as HKEY_CURRENT_USER.
pub fn from_kernel_path(kpath: &str, user_sid: &str) -> String {
    let lower = kpath.to_ascii_lowercase();
    let cur = format!(r"\registry\user\{}", user_sid.to_ascii_lowercase());
    let map = |prefix_len: usize, root: &str| {
        let rest = &kpath[prefix_len..];
        format!("{root}{rest}")
    };
    if !user_sid.is_empty() && (lower == cur || lower.starts_with(&format!("{cur}\\"))) {
        map(cur.len(), "HKEY_CURRENT_USER")
    } else if lower.starts_with(r"\registry\machine") {
        map(r"\registry\machine".len(), "HKEY_LOCAL_MACHINE")
    } else if lower.starts_with(r"\registry\user") {
        map(r"\registry\user".len(), "HKEY_USERS")
    } else {
        kpath.to_string()
    }
}

// ---------------------------------------------------------------- watch list

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    /// Registry key (Windows), file/dir path, or `dconf:/path/` (Linux).
    pub path: String,
    pub active: bool,
    /// Also record reads (query / enumerate). Very noisy; ETW only.
    #[serde(default)]
    pub include_reads: bool,
    pub created: DateTime<Local>,
}

impl Watch {
    pub fn log_file(&self) -> PathBuf {
        logs_dir().join(format!("{}.jsonl", log_id(&self.path)))
    }
}

/// Stable file name for a watch's log.
fn log_id(path: &str) -> String {
    // FNV-1a: tiny, stable across runs and platforms.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let tail: String = path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or("root").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(32).collect();
    format!("{tail}-{h:016x}")
}

pub fn watcher_dir() -> PathBuf {
    data_dir().join("watcher")
}

pub fn logs_dir() -> PathBuf {
    watcher_dir().join("logs")
}

fn list_file() -> PathBuf {
    watcher_dir().join("watches.json")
}

fn heartbeat_file() -> PathBuf {
    watcher_dir().join("recorder.heartbeat")
}

pub fn load_watches() -> Result<Vec<Watch>> {
    match std::fs::read_to_string(list_file()) {
        Ok(s) => serde_json::from_str(&s).context("parsing watches.json"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_watches(w: &[Watch]) -> Result<()> {
    ensure_dir(&watcher_dir())?;
    let tmp = list_file().with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(w)?)?;
    std::fs::rename(tmp, list_file())?;
    Ok(())
}

/// Normalize what the user typed into the form stored in the watch list.
pub fn normalize_target(path: &str) -> String {
    if cfg!(windows) {
        return canonical_key(path);
    }
    let p = path.trim();
    if let Some(rest) = p.strip_prefix("dconf:") {
        let rest = rest.trim();
        let rest = if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") };
        return format!("dconf:{}", if rest.ends_with('/') { rest } else { format!("{rest}/") });
    }
    let expanded = PathBuf::from(crate::pathutil::expand_env(p));
    let abs = if expanded.is_absolute() { expanded } else { std::env::current_dir().unwrap_or_default().join(expanded) };
    abs.to_string_lossy().trim_end_matches('/').to_string()
}

pub fn find<'a>(watches: &'a mut [Watch], path: &str) -> Option<&'a mut Watch> {
    let n = normalize_target(path);
    watches.iter_mut().find(|w| w.path.eq_ignore_ascii_case(&n))
}

/// Is a recorder process alive? It touches its heartbeat file every second.
/// Whether the background recorder is alive (its heartbeat is fresh). The answer is reused for
/// half a second, since the GUI asks every frame.
pub fn recorder_running() -> bool {
    static LAST: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *last {
        if at.elapsed() < Duration::from_millis(500) {
            return v;
        }
    }
    let v = std::fs::metadata(heartbeat_file()).and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(5));
    *last = Some((Instant::now(), v));
    v
}

// ---------------------------------------------------------------- logs

pub fn read_log(w: &Watch) -> Result<Vec<WatchEvent>> {
    let f = match std::fs::File::open(w.log_file()) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect())
}

pub fn clear_log(w: &Watch) -> Result<()> {
    match std::fs::remove_file(w.log_file()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn logs_size() -> u64 {
    crate::backup::dir_size(&logs_dir())
}

pub fn export_events(events: &[WatchEvent], out: &Path) -> Result<()> {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_ascii_lowercase();
    let cols = |e: &WatchEvent| -> [String; 9] {
        [
            e.time.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            e.process.clone(),
            e.pid.map(|p| p.to_string()).unwrap_or_default(),
            e.op.label().to_string(),
            e.key.clone(),
            e.value.clone().unwrap_or_default(),
            e.new_data.clone().unwrap_or_default(),
            e.old_data.clone().unwrap_or_default(),
            e.data_type.clone().unwrap_or_default(),
        ]
    };
    const HEAD: [&str; 9] = ["Time", "Process Name", "PID", "Operation", "Registry Key", "Value", "New Data", "Old Data", "Data Type"];
    if let Some(p) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        ensure_dir(p)?;
    }
    if ext == "xlsx" {
        return crate::xlsx::write(out, "Registry changes", &HEAD, &events.iter().map(|e| cols(e).to_vec()).collect::<Vec<_>>());
    }
    let body = match ext.as_str() {
        "csv" => {
            let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            let mut s = HEAD.join(",") + "\r\n";
            for e in events {
                s += &cols(e).iter().map(|c| q(c)).collect::<Vec<_>>().join(",");
                s += "\r\n";
            }
            s
        }
        "json" => serde_json::to_string_pretty(events)?,
        "txt" => {
            let mut s = HEAD.join("\t") + "\n";
            for e in events {
                s += &cols(e).join("\t");
                s += "\n";
            }
            s
        }
        other => bail!("unsupported format '.{other}' (txt, csv, xlsx, json)"),
    };
    std::fs::write(out, body).with_context(|| format!("writing {}", out.display()))
}

// ---------------------------------------------------------------- recorder

/// What a platform backend reports on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub include_reads: bool,
}

/// Run the recorder until interrupted: reconcile the watch list every second, write events
/// to per-watch logs, pause everything when the log folder exceeds the configured size.
pub fn run_recorder(settings: &Settings, mut on_event: impl FnMut(&WatchEvent)) -> Result<()> {
    ensure_dir(&logs_dir())?;
    let (tx, rx): (Sender<WatchEvent>, Receiver<WatchEvent>) = std::sync::mpsc::channel();
    let mut backend = crate::platform::WatchBackend::new(tx)?;
    eprintln!("Recording with {}. Press Ctrl+C to stop.", backend.mode());

    let max_bytes = settings.watcher_max_log_mb.saturating_mul(1024 * 1024);
    let mut targets: Vec<Target> = Vec::new();
    let mut watches: Vec<Watch> = Vec::new();
    let mut last_list: Option<SystemTime> = None;
    let mut last_tick = Instant::now() - Duration::from_secs(10);
    let mut written_since_check: u64 = 0;
    let mut size = logs_size();

    loop {
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
            std::fs::write(heartbeat_file(), b"")?;
            let mtime = std::fs::metadata(list_file()).and_then(|m| m.modified()).ok();
            if mtime != last_list {
                last_list = mtime;
                watches = load_watches()?;
                let want: Vec<Target> = watches.iter().filter(|w| w.active).map(|w| Target { path: w.path.clone(), include_reads: w.include_reads }).collect();
                if want != targets {
                    for t in &want {
                        if !targets.contains(t) {
                            eprintln!("watching {}", t.path);
                        }
                    }
                    if let Err(e) = backend.set_targets(&want) {
                        eprintln!("warning: {e:#}");
                    }
                    targets = want;
                }
            }
        }

        let ev = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ev) => ev,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => bail!("watch backend stopped"),
        };
        // Batch everything that is already queued into one write per log.
        let mut batch = vec![ev];
        batch.extend(rx.try_iter());
        let mut per_log: BTreeMap<PathBuf, String> = BTreeMap::new();
        for mut ev in batch {
            // The most specific active watch owns the event.
            let Some(w) = watches.iter().filter(|w| w.active && under(&ev.key, &w.path)).max_by_key(|w| w.path.len()) else {
                continue;
            };
            if ev.op.is_read() && !w.include_reads {
                continue;
            }
            ev.watch = w.path.clone();
            on_event(&ev);
            let line = serde_json::to_string(&ev)? + "\n";
            written_since_check += line.len() as u64;
            per_log.entry(w.log_file()).or_default().push_str(&line);
        }
        for (file, text) in per_log {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file)?;
            f.write_all(text.as_bytes())?;
        }
        if max_bytes > 0 && size + written_since_check > max_bytes {
            size = logs_size();
            written_since_check = 0;
            if size > max_bytes {
                let mut all = load_watches()?;
                all.iter_mut().for_each(|w| w.active = false);
                save_watches(&all)?;
                eprintln!("Log folder reached {} MB (limit {} MB): all watches paused. Export or clear logs, then resume.", size / (1024 * 1024), settings.watcher_max_log_mb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(entries: &[(&str, &[(&str, &str)])]) -> Snapshot {
        entries.iter().map(|(k, vals)| (k.to_string(), vals.iter().map(|(n, d)| (n.to_string(), ("String".to_string(), d.to_string()))).collect())).collect()
    }

    #[test]
    fn diff_detects_all_kinds() {
        let old = snap(&[("A", &[("x", "1"), ("y", "2")]), ("B", &[])]);
        let new = snap(&[("A", &[("x", "1"), ("y", "3"), ("z", "9")]), ("C", &[("q", "0")])]);
        let ev: Vec<(Op, String, Option<String>, Option<String>, Option<String>)> = diff(&old, &new).into_iter().map(|e| (e.op, e.key, e.value, e.old_data, e.new_data)).collect();
        let s = |x: &str| Some(x.to_string());
        assert!(ev.contains(&(Op::ValueSet, "A".into(), s("y"), s("2"), s("3"))));
        assert!(ev.contains(&(Op::ValueSet, "A".into(), s("z"), None, s("9"))));
        assert!(ev.contains(&(Op::CreatedKey, "C".into(), None, None, None)));
        assert!(ev.contains(&(Op::DeletedKey, "B".into(), None, None, None)));
        assert_eq!(ev.len(), 5); // + ValueSet C|q
        let old = snap(&[("A", &[("x", "1")])]);
        let new = snap(&[("A", &[])]);
        assert_eq!(diff(&old, &new)[0].op, Op::DeletedValue);
    }

    #[test]
    fn key_names() {
        assert_eq!(canonical_key("hkcu/Software/X\\"), r"HKEY_CURRENT_USER\Software\X");
        let sid = "S-1-5-21-1-2-3-1001";
        assert_eq!(to_kernel_path(r"HKCU\Software\X", sid), r"\REGISTRY\USER\S-1-5-21-1-2-3-1001\Software\X");
        assert_eq!(to_kernel_path(r"HKLM\SOFTWARE", sid), r"\REGISTRY\MACHINE\SOFTWARE");
        assert_eq!(from_kernel_path(r"\REGISTRY\USER\S-1-5-21-1-2-3-1001\Software\X", sid), r"HKEY_CURRENT_USER\Software\X");
        assert_eq!(from_kernel_path(r"\Registry\Machine\SOFTWARE\Y", sid), r"HKEY_LOCAL_MACHINE\SOFTWARE\Y");
        assert_eq!(from_kernel_path(r"\REGISTRY\USER\.DEFAULT\Z", sid), r"HKEY_USERS\.DEFAULT\Z");
        assert!(under(r"HKEY_CURRENT_USER\Software\X\Sub", r"hkey_current_user\software\x"));
        assert!(!under(r"HKEY_CURRENT_USER\Software\Xy", r"HKEY_CURRENT_USER\Software\X"));
        assert!(under("/home/u/.config/app/x.conf", "/home/u/.config"));
    }
}
