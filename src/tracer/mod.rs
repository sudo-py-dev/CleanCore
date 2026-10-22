//! Real-time installation monitor ("Traced Programs").
//!
//! A trace is a pair of snapshots taken before and after an installation. The snapshots cover
//! the places installers write to: the settings store (Windows registry / Linux dconf), the
//! program, data and config folders, and the package database. The diff becomes a [`TraceLog`]
//! of everything created, which can later be undone exactly (Uninstall), item by item (Custom
//! Uninstall) or followed by a leftover scan (Advanced Uninstall).
//!
//! Snapshots keep only 64-bit hashes of paths and data, so even a full registry fits in memory;
//! the "after" pass walks the live system again to recover names.

use crate::config::{data_dir, ensure_dir, Settings};
use crate::model::{Fix, Issue};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- hashing

/// FNV-1a: stable across runs (snapshots can be saved and compared later by the CLI).
pub fn h64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

fn hpath(p: &str) -> u64 {
    h64(p.to_lowercase().as_bytes())
}

// ---------------------------------------------------------------- log model

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    /// Registry key (Windows) or dconf folder (Linux).
    SettingsKey,
    /// Registry value or dconf key.
    SettingsValue,
    Folder,
    File,
    /// A package installed during the trace (`manager:name`), removed with the package manager.
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemStatus {
    /// Did not exist before the installation: removed on uninstall.
    Created,
    /// Existed and was changed: shown, never removed.
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceItem {
    pub kind: ItemKind,
    pub status: ItemStatus,
    /// Key / folder / file path, or `manager:name` for packages.
    pub path: String,
    /// Value name for [`ItemKind::SettingsValue`].
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// For a modified file / settings value: its content before the installation (hex), so
    /// uninstalling can put it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
}

impl TraceItem {
    pub fn label(&self) -> String {
        match &self.value {
            Some(v) => format!("{} | {}", self.path, if v.is_empty() { "(Default)" } else { v }),
            None => self.path.clone(),
        }
    }

    fn dedup_key(&self) -> (ItemKind, String, Option<String>) {
        (self.kind, self.path.to_lowercase(), self.value.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceLog {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// Group in the Traced Programs list.
    #[serde(default)]
    pub group: Option<String>,
    pub created: DateTime<Local>,
    pub modified: DateTime<Local>,
    /// `windows` / `linux` and `x86_64` / `aarch64`, so logs from elsewhere are recognisable.
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub installer: Option<String>,
    #[serde(default)]
    pub imported: bool,
    pub items: Vec<TraceItem>,
}

impl TraceLog {
    pub fn count(&self, settings: bool) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ItemStatus::Created)
            .filter(|i| matches!(i.kind, ItemKind::SettingsKey | ItemKind::SettingsValue) == settings)
            .filter(|i| i.kind != ItemKind::Package || !settings)
            .count()
    }

    pub fn size(&self) -> u64 {
        self.items.iter().filter(|i| i.status == ItemStatus::Created).filter_map(|i| i.size).sum()
    }

    /// Created files and folders: the program's install paths, for an Advanced Uninstall scan.
    pub fn target(&self) -> crate::uninstaller::Target {
        let folders: Vec<String> = self.items.iter().filter(|i| i.kind == ItemKind::Folder && i.status == ItemStatus::Created).map(|i| i.path.clone()).collect();
        let exes: Vec<String> = self
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::File && i.status == ItemStatus::Created)
            .filter(|i| {
                let l = i.path.to_lowercase();
                l.ends_with(".exe") || l.contains("/bin/") || l.ends_with(".appimage")
            })
            .map(|i| i.path.clone())
            .collect();
        crate::uninstaller::Target { names: vec![self.name.clone()], paths: folders, executables: exes, ..Default::default() }
    }
}

// ---------------------------------------------------------------- snapshots

/// What the platform walkers report for one settings key.
pub struct SettingsEntry<'a> {
    pub key: &'a str,
    /// (value name, hash of type + data)
    pub values: Vec<(String, u64)>,
    /// Raw data of each value, same order (may be empty when the walker doesn't provide it).
    pub raw: Vec<Vec<u8>>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub taken: Option<DateTime<Local>>,
    files: HashMap<u64, (u64, i64)>,
    dirs: HashSet<u64>,
    keys: HashSet<u64>,
    values: HashMap<u64, u64>,
    packages: BTreeSet<String>,
    /// Content of small configuration files and settings values, to restore what an installer
    /// changes.
    #[serde(default)]
    file_data: HashMap<u64, Vec<u8>>,
    #[serde(default)]
    value_data: HashMap<u64, Vec<u8>>,
}

/// Files whose old content is kept: small configuration files.
const KEEP_FILE_MAX: u64 = 64 * 1024;
const KEEP_FILES_BUDGET: u64 = 64 << 20;
const KEEP_VALUE_MAX: usize = 4096;
const KEEP_VALUES_BUDGET: usize = 32 << 20;

fn is_config_file(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    if s.starts_with("/etc/") || s.contains("/.config/") {
        return true;
    }
    if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
        return true; // dotfiles
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "ini" | "conf" | "cfg" | "config" | "json" | "xml" | "plist" | "toml" | "yaml" | "yml" | "rc" | "properties" | "desktop" | "list" | "reg" | "inf")
}

fn value_id(key: &str, name: &str) -> u64 {
    hpath(&format!("{key}\u{1}{name}"))
}

/// Compiled exclusion patterns: case-insensitive substrings with `/` and `\` treated alike.
pub struct Excludes(Vec<String>);

impl Excludes {
    pub fn new(settings: &Settings) -> Self {
        let mut v: Vec<String> = crate::platform::default_trace_excludes().into_iter().map(String::from).collect();
        v.extend(settings.trace_excludes.iter().cloned());
        Self(v.into_iter().map(|p| p.to_lowercase().replace('\\', "/")).filter(|p| !p.is_empty()).collect())
    }

    pub fn hit(&self, path: &str) -> bool {
        // Runs for every file of a snapshot: build the lowercase, `/`-separated form in one
        // pass, with a trailing "/" so a pattern like "/.cache/" also matches the folder itself.
        let trimmed = path.trim_end_matches(['/', '\\']);
        let mut p = String::with_capacity(trimmed.len() + 1);
        for c in trimmed.chars() {
            if c == '\\' {
                p.push('/');
            } else {
                p.extend(c.to_lowercase());
            }
        }
        p.push('/');
        self.0.iter().any(|e| p.contains(e.as_str()))
    }
}

struct FileEntry {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

/// All files and folders below the roots, in parallel per top-level folder.
fn walk_files(ex: &Excludes) -> Vec<FileEntry> {
    let mut tops: Vec<PathBuf> = Vec::new();
    for root in crate::platform::trace_file_roots() {
        if root.is_file() {
            tops.push(root);
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&root) {
            tops.extend(rd.flatten().map(|e| e.path()));
        }
    }
    tops.sort();
    tops.dedup();
    tops.par_iter()
        .flat_map_iter(|t| {
            let mut out = Vec::new();
            walk_one(t, ex, &mut out, 0);
            out
        })
        .collect()
}

fn walk_one(p: &Path, ex: &Excludes, out: &mut Vec<FileEntry>, depth: usize) {
    if depth > 24 || ex.hit(&p.to_string_lossy()) {
        return;
    }
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return;
    };
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    let is_dir = m.is_dir() && !m.file_type().is_symlink();
    out.push(FileEntry { path: p.to_path_buf(), is_dir, size: if is_dir { 0 } else { m.len() }, mtime });
    if is_dir {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                walk_one(&e.path(), ex, out, depth + 1);
            }
        }
    }
}

pub fn take_snapshot(settings: &Settings) -> Snapshot {
    let ex = Excludes::new(settings);
    let mut s = Snapshot { taken: Some(Local::now()), ..Default::default() };
    let (files, (keys, packages)) = rayon::join(
        || walk_files(&ex),
        || {
            let mut keys: Vec<(u64, Vec<(u64, u64)>)> = Vec::new();
            let mut budget = KEEP_VALUES_BUDGET;
            let mut data = HashMap::new();
            crate::platform::walk_settings(&ex, &mut |e: SettingsEntry| {
                for (i, (n, _)) in e.values.iter().enumerate() {
                    if let Some(r) = e.raw.get(i).filter(|r| r.len() <= KEEP_VALUE_MAX && r.len() <= budget) {
                        budget -= r.len();
                        data.insert(value_id(e.key, n), r.clone());
                    }
                }
                keys.push((hpath(e.key), e.values.iter().map(|(n, h)| (value_id(e.key, n), *h)).collect()));
            });
            ((keys, data), crate::platform::package_snapshot())
        },
    );
    let (keys, value_data) = keys;
    s.value_data = value_data;
    // Content of small config files, within a budget.
    let mut budget = KEEP_FILES_BUDGET;
    let keep: Vec<&FileEntry> = files
        .iter()
        .filter(|f| !f.is_dir && f.size <= KEEP_FILE_MAX && is_config_file(&f.path))
        .filter(|f| {
            let ok = f.size <= budget;
            budget = budget.saturating_sub(f.size);
            ok
        })
        .collect();
    s.file_data = keep.par_iter().filter_map(|f| Some((hpath(&f.path.to_string_lossy()), std::fs::read(&f.path).ok()?))).collect();
    for f in files {
        let id = hpath(&f.path.to_string_lossy());
        if f.is_dir {
            s.dirs.insert(id);
        } else {
            s.files.insert(id, (f.size, f.mtime));
        }
    }
    for (k, vals) in keys {
        s.keys.insert(k);
        s.values.extend(vals);
    }
    s.packages = packages;
    s
}

/// Compare the live system with `before`. Only the topmost created folder / key is kept;
/// everything below it goes with it.
pub fn diff(settings: &Settings, before: &Snapshot) -> Vec<TraceItem> {
    let ex = Excludes::new(settings);
    let mut items = Vec::new();

    // Files and folders
    let mut entries = walk_files(&ex);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    for f in entries {
        let p = f.path.to_string_lossy().to_string();
        let id = hpath(&p);
        if created_dirs.last().is_some_and(|d| f.path.starts_with(d)) {
            continue;
        }
        if f.is_dir {
            if !before.dirs.contains(&id) {
                items.push(TraceItem { kind: ItemKind::Folder, status: ItemStatus::Created, path: p, value: None, size: Some(crate::backup::dir_size(&f.path)), original: None });
                created_dirs.push(f.path);
            }
        } else {
            match before.files.get(&id) {
                None => items.push(TraceItem { kind: ItemKind::File, status: ItemStatus::Created, path: p, value: None, size: Some(f.size), original: None }),
                Some(&(size, mtime)) if size != f.size || mtime != f.mtime => {
                    let original = before.file_data.get(&id).map(|d| to_hex(d));
                    items.push(TraceItem { kind: ItemKind::File, status: ItemStatus::Modified, path: p, value: None, size: Some(f.size), original })
                }
                _ => {}
            }
        }
    }

    // Settings store
    let mut created_keys: Vec<String> = Vec::new();
    let mut found: Vec<(String, Vec<(String, u64)>)> = Vec::new();
    crate::platform::walk_settings(&ex, &mut |e: SettingsEntry| found.push((e.key.to_string(), e.values)));
    found.sort_by_key(|a| a.0.to_lowercase());
    for (key, vals) in found {
        let lk = key.to_lowercase();
        if created_keys.last().is_some_and(|c| lk.starts_with(c.as_str()) && lk.as_bytes().get(c.len()).is_some_and(|b| *b == b'\\' || *b == b'/')) {
            continue;
        }
        if !before.keys.contains(&hpath(&key)) {
            items.push(TraceItem { kind: ItemKind::SettingsKey, status: ItemStatus::Created, path: key.clone(), value: None, size: None, original: None });
            created_keys.push(lk.trim_end_matches(['/', '\\']).to_string());
            continue;
        }
        for (name, h) in vals {
            match before.values.get(&value_id(&key, &name)) {
                None => items.push(TraceItem { kind: ItemKind::SettingsValue, status: ItemStatus::Created, path: key.clone(), value: Some(name), size: None, original: None }),
                Some(old) if *old != h => {
                    let original = before.value_data.get(&value_id(&key, &name)).map(|d| to_hex(d));
                    items.push(TraceItem { kind: ItemKind::SettingsValue, status: ItemStatus::Modified, path: key.clone(), value: Some(name), size: None, original })
                }
                _ => {}
            }
        }
    }

    // Packages
    for p in crate::platform::package_snapshot().difference(&before.packages) {
        items.push(TraceItem { kind: ItemKind::Package, status: ItemStatus::Created, path: p.clone(), value: None, size: None, original: None });
    }
    items
}

/// A name for the new log: the most common folder created under a program location.
pub fn suggest_name(items: &[TraceItem], installer: Option<&str>) -> String {
    if let Some(pkg) = items.iter().find(|i| i.kind == ItemKind::Package) {
        return pkg.path.split(':').nth(1).unwrap_or(&pkg.path).to_string();
    }
    let folder = items
        .iter()
        .filter(|i| i.kind == ItemKind::Folder && i.status == ItemStatus::Created)
        .filter_map(|i| Path::new(&i.path).file_name().map(|n| n.to_string_lossy().to_string()))
        .find(|n| !matches!(n.to_lowercase().as_str(), "bin" | "lib" | "share" | "temp" | "tmp"));
    if let Some(f) = folder {
        return f;
    }
    if let Some(inst) = installer {
        if let Some(stem) = Path::new(inst.split_whitespace().next().unwrap_or(inst)).file_stem() {
            let s = stem.to_string_lossy();
            return s.trim_end_matches("-setup").trim_end_matches("_setup").trim_end_matches("setup").trim_end_matches(['-', '_', ' ']).to_string();
        }
    }
    format!("Traced program {}", Local::now().format("%Y-%m-%d %H:%M"))
}

// ---------------------------------------------------------------- tracing session

/// An in-progress trace: snapshot taken, installer (optionally) running.
pub struct Session {
    pub before: Snapshot,
    pub installer: Option<String>,
    pub started: DateTime<Local>,
    /// What the installer's own processes touched, when it was started through the session.
    watches: std::sync::Mutex<Vec<crate::platform::InstallWatch>>,
}

impl Session {
    pub fn start(settings: &Settings, installer: Option<String>) -> Self {
        Self { before: take_snapshot(settings), installer, started: Local::now(), watches: Default::default() }
    }

    /// Launch the session's installer (not waited for).
    pub fn run_installer(&self, settings: &Settings) -> Result<std::process::Child> {
        let cmd = self.installer.clone().context("no installer given")?;
        self.launch(settings, &cmd)
    }

    /// Launch an installer, following its processes when the settings ask for it.
    pub fn launch(&self, settings: &Settings, cmd: &str) -> Result<std::process::Child> {
        if settings.trace_follow_process {
            if let Some((child, w)) = crate::platform::launch_watched(cmd)? {
                self.watches.lock().unwrap().push(w);
                return Ok(child);
            }
        }
        crate::platform::launch(cmd)
    }

    pub fn finish(&self, settings: &Settings) -> Vec<TraceItem> {
        self.finish_report(settings).0
    }

    /// The diff, and how many file items were left out because no installer process wrote them.
    pub fn finish_report(&self, settings: &Settings) -> (Vec<TraceItem>, usize) {
        let items = diff(settings, &self.before);
        let watches: Vec<_> = std::mem::take(&mut *self.watches.lock().unwrap());
        if watches.is_empty() {
            return (items, 0);
        }
        let mut touched = HashSet::new();
        for w in watches {
            match w.touched() {
                Some(t) => touched.extend(t),
                None => return (items, 0), // can't tell: keep everything
            }
        }
        filter_touched(items, &touched)
    }
}

/// Keep settings and packages (they are written by system services, not the installer), and
/// files / folders the installer's processes wrote (a folder counts when anything inside it was).
fn filter_touched(items: Vec<TraceItem>, touched: &HashSet<String>) -> (Vec<TraceItem>, usize) {
    let norm = |p: &str| {
        if cfg!(windows) {
            p.to_lowercase()
        } else {
            p.to_string()
        }
    };
    let touched: Vec<String> = touched.iter().map(|t| norm(t)).collect();
    let sep = std::path::MAIN_SEPARATOR;
    let before = items.len();
    let kept: Vec<TraceItem> = items
        .into_iter()
        .filter(|i| match i.kind {
            ItemKind::File => touched.contains(&norm(&i.path)),
            ItemKind::Folder => {
                let f = norm(&i.path);
                let prefix = format!("{f}{sep}");
                touched.iter().any(|t| *t == f || t.starts_with(&prefix))
            }
            _ => true,
        })
        .collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

// ---------------------------------------------------------------- storage

fn logs_dir() -> PathBuf {
    data_dir().join("traces")
}

fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() {
        "trace".into()
    } else {
        s.chars().take(40).collect()
    }
}

pub fn new_log(name: &str, installer: Option<String>, items: Vec<TraceItem>) -> TraceLog {
    let now = Local::now();
    TraceLog {
        id: format!("{}-{}", slug(name), now.format("%Y%m%d%H%M%S")),
        name: name.to_string(),
        comment: String::new(),
        author: whoami(),
        icon: None,
        group: None,
        created: now,
        modified: now,
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        installer,
        imported: false,
        items,
    }
}

fn whoami() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default()
}

pub fn save(log: &TraceLog) -> Result<()> {
    ensure_dir(&logs_dir())?;
    let tmp = logs_dir().join(format!("{}.tmp", log.id));
    std::fs::write(&tmp, serde_json::to_string_pretty(log)?)?;
    std::fs::rename(tmp, logs_dir().join(format!("{}.json", log.id)))?;
    Ok(())
}

/// All logs, newest first.
pub fn list() -> Vec<TraceLog> {
    let mut v: Vec<TraceLog> = std::fs::read_dir(logs_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|s| serde_json::from_str(&s).ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|l: &TraceLog| std::cmp::Reverse(l.created));
    v
}

/// By id, or by name (case-insensitive).
pub fn find(id_or_name: &str) -> Result<TraceLog> {
    let all = list();
    if let Some(l) = all.iter().find(|l| l.id == id_or_name) {
        return Ok(l.clone());
    }
    let named: Vec<&TraceLog> = all.iter().filter(|l| l.name.eq_ignore_ascii_case(id_or_name)).collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no trace log named \"{id_or_name}\" (see `cleancore trace list`)"),
        _ => {
            bail!("several logs are named \"{id_or_name}\"; use the id from `cleancore trace list`")
        }
    }
}

/// Logs whose name matches a program, looked up before uninstalling it.
fn name_matcher(names: &[String]) -> impl Fn(&str) -> bool {
    use crate::uninstaller::{normalize, strip_version};
    let wanted: HashSet<String> = names.iter().flat_map(|n| [normalize(n), normalize(&strip_version(n))]).filter(|n| n.len() >= 3).collect();
    move |name| wanted.contains(&normalize(name)) || wanted.contains(&normalize(&strip_version(name)))
}

pub fn matching(names: &[String]) -> Vec<TraceLog> {
    let hit = name_matcher(names);
    list().into_iter().filter(|l| hit(&l.name)).collect()
}

/// Logs database entries for this system whose name matches (Forced Uninstall).
pub fn db_matching(settings: &Settings, names: &[String]) -> Vec<DbLog> {
    let hit = name_matcher(names);
    db_list(settings).into_iter().filter(|d| d.compatible() && hit(&d.log.name)).collect()
}

pub fn delete(id: &str) -> Result<()> {
    std::fs::remove_file(logs_dir().join(format!("{id}.json"))).with_context(|| format!("deleting log {id}"))
}

pub fn rename(id: &str, name: &str) -> Result<()> {
    let mut l = find(id)?;
    l.name = name.to_string();
    l.modified = Local::now();
    save(&l)
}

/// Put a log in a group (`None`: ungrouped).
pub fn set_group(id: &str, group: Option<&str>) -> Result<()> {
    let mut l = find(id)?;
    l.group = group.map(str::trim).filter(|g| !g.is_empty()).map(str::to_string);
    save(&l)
}

/// Choose a log's icon (a key of the GUI's icon palette; `None`: the default).
pub fn set_icon(id: &str, icon: Option<&str>) -> Result<()> {
    let mut l = find(id)?;
    l.icon = icon.map(str::to_string);
    save(&l)
}

/// Log groups in use, sorted.
pub fn groups() -> Vec<String> {
    let mut g: Vec<String> = list().into_iter().filter_map(|l| l.group).collect();
    g.sort_by_key(|x| x.to_lowercase());
    g.dedup();
    g
}

/// Rename a log group (`new` = None removes the group from every log).
pub fn rename_group(old: &str, new: Option<&str>) -> Result<usize> {
    let mut n = 0;
    for mut l in list().into_iter().filter(|l| l.group.as_deref() == Some(old)) {
        l.group = new.map(str::to_string);
        save(&l)?;
        n += 1;
    }
    Ok(n)
}

pub fn export(log: &TraceLog, path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let body = match ext.as_str() {
        "html" | "htm" => to_html(log),
        "txt" => to_text(log),
        _ => serde_json::to_string_pretty(log)?,
    };
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

/// Import a `.cptrace` file; it gets a fresh id so it never overwrites an existing log.
pub fn import(path: &Path) -> Result<TraceLog> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut l: TraceLog = serde_json::from_str(&text).context("not a CleanCore trace log")?;
    l.id = format!("{}-{}", slug(&l.name), Local::now().format("%Y%m%d%H%M%S%3f"));
    l.imported = true;
    save(&l)?;
    Ok(l)
}

/// Merge logs into the oldest one (it is the base); the others are deleted.
pub fn merge(ids: &[String], name: &str) -> Result<TraceLog> {
    let mut logs: Vec<TraceLog> = ids.iter().map(|i| find(i)).collect::<Result<_>>()?;
    if logs.len() < 2 {
        bail!("select at least two logs to merge");
    }
    logs.sort_by_key(|l| l.created);
    let mut base = logs[0].clone();
    let mut seen: HashSet<_> = base.items.iter().map(|i| i.dedup_key()).collect();
    for l in &logs[1..] {
        for it in &l.items {
            if seen.insert(it.dedup_key()) {
                base.items.push(it.clone());
            }
        }
    }
    base.name = name.to_string();
    base.modified = Local::now();
    save(&base)?;
    for l in &logs[1..] {
        delete(&l.id)?;
    }
    Ok(base)
}

/// Back up every log into one file / restore them all.
pub fn backup_all(path: &Path) -> Result<usize> {
    let all = list();
    std::fs::write(path, serde_json::to_string_pretty(&all)?)?;
    Ok(all.len())
}

pub fn restore_all(path: &Path) -> Result<usize> {
    let all: Vec<TraceLog> = serde_json::from_str(&std::fs::read_to_string(path)?).context("not a CleanCore logs backup")?;
    let existing: HashSet<String> = list().into_iter().map(|l| l.id).collect();
    let mut n = 0;
    for l in all.into_iter().filter(|l| !existing.contains(&l.id)) {
        save(&l)?;
        n += 1;
    }
    Ok(n)
}

pub fn to_text(log: &TraceLog) -> String {
    let mut s = format!("{}\nCreated {} by {} on {} {}\n", log.name, log.created.format("%Y-%m-%d %H:%M"), log.author, log.os, log.arch);
    if !log.comment.is_empty() {
        s += &format!("{}\n", log.comment);
    }
    for (k, title) in
        [(ItemKind::Package, "Packages"), (ItemKind::SettingsKey, "Keys"), (ItemKind::SettingsValue, "Values"), (ItemKind::Folder, "Folders"), (ItemKind::File, "Files")]
    {
        let items: Vec<&TraceItem> = log.items.iter().filter(|i| i.kind == k).collect();
        if items.is_empty() {
            continue;
        }
        s += &format!("\n{title} ({})\n", items.len());
        for i in items {
            s += &format!("  {} {}\n", if i.status == ItemStatus::Created { "+" } else { "~" }, i.label());
        }
    }
    s
}

fn to_html(log: &TraceLog) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut s = format!(
        "<!doctype html><meta charset=utf-8><title>{0}</title><style>body{{font:14px system-ui;margin:2em}}td,th{{padding:3px 8px;border-bottom:1px solid #ddd;text-align:left}}.m{{color:#888}}</style><h1>{0}</h1><p>{1} · {2}</p><table><tr><th>Type</th><th>Item</th><th>Status</th></tr>",
        esc(&log.name),
        log.created.format("%Y-%m-%d %H:%M"),
        esc(&log.comment)
    );
    for i in &log.items {
        s += &format!("<tr class='{}'><td>{:?}</td><td>{}</td><td>{:?}</td></tr>", if i.status == ItemStatus::Modified { "m" } else { "" }, i.kind, esc(&i.label()), i.status);
    }
    s + "</table>"
}

// ---------------------------------------------------------------- uninstall from a log

/// The log's created items that still exist, as removable issues (packages first).
pub fn to_issues(log: &TraceLog) -> Vec<Issue> {
    let mut v: Vec<Issue> = log
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Created)
        .filter(|i| crate::platform::trace_item_exists(i))
        .map(|i| {
            let (cat, fix) = match i.kind {
                ItemKind::Folder => ("leftover_files", Fix::DeleteDirTree { path: i.path.clone().into() }),
                ItemKind::File => ("leftover_files", Fix::DeleteFile { path: i.path.clone().into() }),
                ItemKind::Package => {
                    let (m, n) = i.path.split_once(':').unwrap_or(("dpkg", &i.path));
                    ("leftover_registry", Fix::PurgePackage { manager: m.into(), name: n.into() })
                }
                _ => ("leftover_registry", crate::platform::trace_settings_fix(i)),
            };
            let what = match i.kind {
                ItemKind::Package => "package installed by the setup".to_string(),
                ItemKind::Folder | ItemKind::File => format!("created during installation · {}", crate::backup::human_size(i.size.unwrap_or(0))),
                _ => "created during installation".to_string(),
            };
            Issue::new(cat, i.label(), what, fix)
        })
        .collect();
    // Items the installer changed: put back what they contained before (backed up first).
    for i in log.items.iter().filter(|i| i.status == ItemStatus::Modified) {
        let Some(data) = &i.original else { continue };
        let fix = match i.kind {
            ItemKind::File if Path::new(&i.path).is_file() => Fix::WriteFile { path: i.path.clone().into(), data: data.clone() },
            ItemKind::SettingsValue => Fix::WriteSetting { key: i.path.clone(), value: i.value.clone().unwrap_or_default(), data: data.clone() },
            _ => continue,
        };
        let cat = if i.kind == ItemKind::File { "leftover_files" } else { "leftover_registry" };
        v.push(Issue::new(cat, i.label(), "changed by the setup · restore the earlier content", fix));
    }
    v.sort_by_key(|i| !matches!(i.fix, Fix::PurgePackage { .. }));
    v
}

// ---------------------------------------------------------------- logs database

/// The Logs Database is a folder of shared `.cptrace` logs (a network share, a synced folder…), optionally filled from a URL that serves
/// `index.json` (a list of file names) next to the log files.
pub fn db_folder(settings: &Settings) -> PathBuf {
    settings.logs_db_folder.clone().unwrap_or_else(|| data_dir().join("logs-db"))
}

#[derive(Debug, Clone)]
pub struct DbLog {
    pub log: TraceLog,
    pub file: PathBuf,
}

impl DbLog {
    /// Recorded on this OS and CPU architecture.
    pub fn compatible(&self) -> bool {
        self.log.os == std::env::consts::OS && self.log.arch == std::env::consts::ARCH
    }
}

pub fn db_list(settings: &Settings) -> Vec<DbLog> {
    let mut out = Vec::new();
    let mut stack = vec![(db_folder(settings), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            } else if p.extension().is_some_and(|x| x == "cptrace" || x == "json") && p.file_name().is_some_and(|n| n != "index.json") {
                if let Some(log) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<TraceLog>(&t).ok()) {
                    out.push(DbLog { log, file: p });
                }
            }
        }
    }
    out.sort_by_key(|d| d.log.name.to_lowercase());
    out
}

/// Download new logs from the database URL. Returns how many files were fetched.
pub fn db_update(settings: &Settings) -> Result<usize> {
    let url = settings.logs_db_url.clone().filter(|u| !u.trim().is_empty()).context("no logs database URL set (Settings › Logs database)")?;
    let base = url.trim_end_matches('/').to_string();
    let folder = db_folder(settings);
    ensure_dir(&folder)?;
    let fetch = |u: &str| -> Result<Vec<u8>> {
        let o = crate::platform::hidden(if cfg!(windows) { "curl.exe" } else { "curl" })
            .args(["-fsSL", "--max-time", "60", u])
            .output()
            .context("curl is needed to update the logs database")?;
        if !o.status.success() {
            bail!("downloading {u} failed: {}", String::from_utf8_lossy(&o.stderr).trim());
        }
        Ok(o.stdout)
    };
    let index: Vec<String> = serde_json::from_slice(&fetch(&format!("{base}/index.json"))?).context("index.json must be a list of file names")?;
    let mut n = 0;
    for f in index {
        let name = Path::new(&f).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        if name.is_empty() || folder.join(&name).exists() {
            continue;
        }
        let data = fetch(&format!("{base}/{f}"))?;
        if serde_json::from_slice::<TraceLog>(&data).is_ok() {
            std::fs::write(folder.join(&name), data)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Share a local log through the database folder.
pub fn db_add(settings: &Settings, log: &TraceLog) -> Result<PathBuf> {
    let folder = db_folder(settings);
    ensure_dir(&folder)?;
    let path = folder.join(format!("{}.cptrace", log.id));
    let mut l = log.clone();
    l.imported = false;
    std::fs::write(&path, serde_json::to_string_pretty(&l)?)?;
    Ok(path)
}

/// Take a database log into the local Traced Programs, before uninstalling with it.
pub fn db_import(d: &DbLog) -> Result<TraceLog> {
    import(&d.file)
}

/// Uninstall with a log: remove the given items (all created items for a plain Uninstall, the
/// user's selection for Custom Uninstall), record the history, and delete the log if configured.
pub fn uninstall(settings: &Settings, log: &TraceLog, items: &[Issue]) -> Result<crate::cleaner::CleanReport> {
    let rep = crate::uninstaller::remove(settings, &log.name, "", crate::uninstaller::Operation::TracedUninstall, items)?;
    if settings.delete_trace_after_uninstall && rep.failed.is_empty() {
        let _ = delete(&log.id);
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, path: &str) -> TraceItem {
        TraceItem { kind, status: ItemStatus::Created, path: path.into(), value: None, size: Some(10), original: None }
    }

    #[test]
    fn names_and_counts() {
        let items = vec![item(ItemKind::Folder, "/opt/myapp"), item(ItemKind::Folder, "/home/u/.config/myapp"), item(ItemKind::SettingsKey, "/org/myapp/")];
        assert_eq!(suggest_name(&items, None), "myapp");
        assert_eq!(suggest_name(&[], Some("/tmp/Foo-Setup.exe /S")), "Foo-Setup");
        assert_eq!(suggest_name(&[], Some("/tmp/foo_setup.sh")), "foo");
        let log = new_log("x", None, items);
        assert_eq!((log.count(true), log.count(false)), (1, 2));
        assert_eq!(log.target().paths.len(), 2);
    }

    #[test]
    fn excludes() {
        let ex = Excludes(vec!["/.cache/".into(), "appdata/local/temp".into()]);
        assert!(ex.hit("/home/u/.cache/x"));
        assert!(ex.hit(r"C:\Users\u\AppData\Local\Temp\a.tmp"));
        assert!(!ex.hit("/home/u/.config/x"));
    }
}
