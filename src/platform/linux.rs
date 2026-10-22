//! Linux backend. Linux has no registry, so the cleaner targets the equivalent
//! per-user configuration stores: XDG desktop entries, autostart, MIME associations,
//! recent-file lists, systemd user units and leftover application data folders.

use crate::backup::BackupEntry;
use crate::config::{ensure_dir, list_matches, Settings};
use crate::engine::{Check, ScanContext};
use crate::model::{CheckInfo, Fix, Issue, ScanMode, Scope};
use crate::pathutil::{desktop_exec_program, which};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- helpers

/// Home folders to scan for the given scope.
fn homes(ctx: &ScanContext) -> Vec<PathBuf> {
    let me = dirs::home_dir().into_iter().collect::<Vec<_>>();
    if ctx.scope == Scope::CurrentUser {
        return me;
    }
    let Ok(passwd) = fs::read_to_string("/etc/passwd") else {
        return me;
    };
    let mut out: Vec<PathBuf> = passwd
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let uid: u32 = f.get(2)?.parse().ok()?;
            let home = PathBuf::from(f.get(5)?);
            ((1000..60000).contains(&uid) && home.is_dir()).then_some(home)
        })
        .collect();
    for h in me {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Visit entries under `root` up to `depth` levels, never following symlinks.
fn walk(root: &Path, depth: usize, f: &mut dyn FnMut(&Path, &fs::FileType)) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        f(&p, &ft);
        if ft.is_dir() && depth > 1 {
            walk(&p, depth - 1, f);
        }
    }
}

#[derive(Default)]
struct DesktopEntry {
    kind: String,
    exec: Option<String>,
    try_exec: Option<String>,
    hidden: bool,
}

fn parse_desktop(path: &Path) -> Option<DesktopEntry> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_main = false;
    let mut d = DesktopEntry::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "Type" => d.kind = v.trim().to_string(),
            "Exec" => d.exec = Some(v.trim().to_string()),
            "TryExec" => d.try_exec = Some(v.trim().to_string()),
            "Hidden" => d.hidden = v.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    Some(d)
}

/// The program a desktop entry would launch, if it is missing.
fn missing_program(ctx: &ScanContext, d: &DesktopEntry) -> Option<String> {
    // `Hidden=true` files are user overrides that hide a system entry; they are intentional.
    if d.hidden || (!d.kind.is_empty() && d.kind != "Application") {
        return None;
    }
    let prog = d.try_exec.clone().or_else(|| d.exec.as_deref().and_then(desktop_exec_program))?;
    let expanded = crate::pathutil::expand_env(&prog);
    if expanded.starts_with('/') {
        return (!ctx.target_exists(&expanded)).then_some(expanded);
    }
    which(&expanded, &ctx.paths).is_none().then_some(expanded)
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into()).split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    for extra in ["/var/lib/flatpak/exports/share", "/var/lib/snapd/desktop"] {
        let p = PathBuf::from(extra);
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// All installed desktop-file ids (`foo/bar.desktop` → `foo-bar.desktop`).
fn installed_desktop_ids(home: &Path) -> HashSet<String> {
    let mut dirs: Vec<PathBuf> = xdg_data_dirs().into_iter().map(|d| d.join("applications")).collect();
    dirs.push(home.join(".local/share/applications"));
    dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    let mut ids = HashSet::new();
    for d in dirs {
        walk(&d, 4, &mut |p, _| {
            if p.extension().is_some_and(|e| e == "desktop") {
                if let Ok(rel) = p.strip_prefix(&d) {
                    ids.insert(rel.to_string_lossy().replace('/', "-"));
                }
            }
        });
    }
    ids
}

// ---------------------------------------------------------------- checks

macro_rules! check {
    ($ty:ident, $id:literal, $name:literal, $desc:literal, $mode:expr) => {
        struct $ty;
        impl Check for $ty {
            fn info(&self) -> CheckInfo {
                CheckInfo { id: $id, name: $name, description: $desc, min_mode: $mode }
            }
            fn scan(&self, ctx: &ScanContext) -> Vec<Issue> {
                homes(ctx).iter().flat_map(|h| $ty::scan_home(ctx, h)).collect()
            }
        }
    };
}

check!(BrokenDesktopEntries, "desktop_entries", "Broken application launchers", "Desktop entries whose program no longer exists.", ScanMode::Safe);
impl BrokenDesktopEntries {
    fn scan_home(ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let mut out = Vec::new();
        walk(&home.join(".local/share/applications"), 4, &mut |p, ft| {
            if ft.is_file() && p.extension().is_some_and(|e| e == "desktop") {
                if let Some(missing) = parse_desktop(p).and_then(|d| missing_program(ctx, &d)) {
                    out.push(Issue::new("desktop_entries", p.display().to_string(), format!("missing program: {missing}"), Fix::DeleteFile { path: p.into() }));
                }
            }
        });
        out
    }
}

check!(BrokenAutostart, "startup", "Missing start-up items", "Autostart entries and systemd user services whose program no longer exists.", ScanMode::Safe);
impl BrokenAutostart {
    fn scan_home(ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let mut out = Vec::new();
        walk(&home.join(".config/autostart"), 1, &mut |p, ft| {
            if ft.is_file() && p.extension().is_some_and(|e| e == "desktop") {
                if let Some(missing) = parse_desktop(p).and_then(|d| missing_program(ctx, &d)) {
                    out.push(Issue::new("startup", p.display().to_string(), format!("missing program: {missing}"), Fix::DeleteFile { path: p.into() }));
                }
            }
        });
        walk(&home.join(".config/systemd/user"), 1, &mut |p, ft| {
            if !ft.is_file() || !p.extension().is_some_and(|e| e == "service") {
                return;
            }
            let Ok(text) = fs::read_to_string(p) else {
                return;
            };
            for line in text.lines() {
                let Some(cmd) = line.trim().strip_prefix("ExecStart=") else {
                    continue;
                };
                // Strip systemd prefixes (`-`, `@`, `+`, `!`, `:`).
                let cmd = cmd.trim_start_matches(['-', '@', '+', '!', ':']);
                let Some(prog) = desktop_exec_program(cmd) else {
                    continue;
                };
                let prog = crate::pathutil::expand_env(&prog.replace("%h", &home.to_string_lossy()));
                let missing = if prog.starts_with('/') { !ctx.target_exists(&prog) } else { which(&prog, &ctx.paths).is_none() };
                if missing {
                    out.push(Issue::new("startup", p.display().to_string(), format!("ExecStart missing: {prog}"), Fix::DeleteFile { path: p.into() }));
                }
                break;
            }
        });
        out
    }
}

check!(StaleMimeHandlers, "file_associations", "Invalid file associations", "mimeapps.list entries that point to applications that are not installed.", ScanMode::Safe);
impl StaleMimeHandlers {
    fn scan_home(ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let lists = [home.join(".config/mimeapps.list"), home.join(".local/share/applications/mimeapps.list")];
        let lists: Vec<_> = lists.into_iter().filter(|p| p.is_file()).collect();
        if lists.is_empty() {
            return vec![];
        }
        let mut installed = installed_desktop_ids(home);
        // Launchers reported as broken in this same scan count as not installed, so one clean is enough.
        let apps = home.join(".local/share/applications");
        for broken in BrokenDesktopEntries::scan_home(ctx, home) {
            if let Ok(rel) = Path::new(&broken.location).strip_prefix(&apps) {
                installed.remove(&rel.to_string_lossy().replace('/', "-"));
            }
        }
        let mut out = Vec::new();
        for list in lists {
            let Ok(text) = fs::read_to_string(&list) else {
                continue;
            };
            let mut reported = HashSet::new();
            for line in text.lines() {
                let Some((mime, vals)) = line.split_once('=') else {
                    continue;
                };
                for id in vals.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                    if !installed.contains(id) && reported.insert(id.to_string()) {
                        out.push(Issue::new(
                            "file_associations",
                            format!("{} | {id}", list.display()),
                            format!("handler for {} is not installed", mime.trim()),
                            Fix::RemoveMimeHandler { file: list.clone(), desktop_id: id.to_string() },
                        ));
                    }
                }
            }
        }
        out
    }
}

check!(RecentFiles, "mru_history", "Recently used and history lists", "Recent-file entries that point to files which no longer exist.", ScanMode::Safe);
impl RecentFiles {
    fn scan_home(ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let file = home.join(".local/share/recently-used.xbel");
        let Ok(text) = fs::read_to_string(&file) else {
            return vec![];
        };
        let mut out = Vec::new();
        for href in xbel_hrefs(&text) {
            let Some(path) = href.strip_prefix("file://") else {
                continue;
            };
            let path = percent_decode(&xml_unescape(path));
            if !ctx.target_exists(&path) {
                out.push(Issue::new(
                    "mru_history",
                    format!("{} | {path}", file.display()),
                    "file no longer exists",
                    Fix::RemoveXbelBookmark { file: file.clone(), href: href.to_string() },
                ));
            }
        }
        out
    }
}

check!(DanglingSymlinks, "dangling_links", "Broken symbolic links", "Symbolic links in your configuration and local folders whose target is gone.", ScanMode::Moderate);
impl DanglingSymlinks {
    fn scan_home(ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let mut out = Vec::new();
        for (sub, depth) in [(".local/bin", 2), (".config", 4), (".local/share/applications", 3), (".local/share", 2)] {
            walk(&home.join(sub), depth, &mut |p, ft| {
                if !ft.is_symlink() {
                    return;
                }
                let Ok(target) = fs::read_link(p) else { return };
                let abs = if target.is_absolute() { target.clone() } else { p.parent().unwrap().join(&target) };
                if !ctx.target_exists(&abs.to_string_lossy()) {
                    out.push(Issue::new("dangling_links", p.display().to_string(), format!("target missing: {}", target.display()), Fix::DeleteFile { path: p.into() }));
                }
            });
        }
        out
    }
}

check!(EmptyDirs, "empty_keys", "Empty folders", "Empty folders left in configuration and data directories.", ScanMode::Moderate);
impl EmptyDirs {
    fn scan_home(_ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        let mut out = Vec::new();
        for sub in [".config", ".local/share"] {
            walk(&home.join(sub), 2, &mut |p, ft| {
                let standard = p.file_name().is_some_and(|n| SYSTEM_CONFIG_DIRS.contains(&n.to_string_lossy().as_ref()));
                if ft.is_dir() && !standard && fs::read_dir(p).is_ok_and(|mut r| r.next().is_none()) {
                    out.push(Issue::new("empty_keys", p.display().to_string(), "empty folder", Fix::DeleteEmptyDir { path: p.into() }));
                }
            });
        }
        out
    }
}

/// Folders that belong to the desktop / toolkits rather than to an installable application.
const SYSTEM_CONFIG_DIRS: &[&str] = &[
    "autostart",
    "dconf",
    "systemd",
    "environment.d",
    "fontconfig",
    "fonts",
    "gtk-2.0",
    "gtk-3.0",
    "gtk-4.0",
    "pulse",
    "pipewire",
    "wireplumber",
    "ibus",
    "user-dirs.dirs",
    "mimeapps.list",
    "menus",
    "applications",
    "icons",
    "themes",
    "mime",
    "keyrings",
    "recently-used.xbel",
    "trash",
    "Trash",
    "sounds",
    "desktop-directories",
    "flatpak",
    "gnome-shell",
    "gnome-session",
    "nautilus",
    "evolution",
    "goa-1.0",
    "gvfs-metadata",
    "tracker",
    "tracker3",
    "xorg",
    "kdedefaults",
    "plasma-workspace",
    "kwinrc",
    "kscreen",
    "kactivitymanagerd",
    "baloo",
    "akonadi",
    "kded5",
    "kded6",
    "session",
    "xdg-desktop-portal",
    "Kvantum",
    "qt5ct",
    "qt6ct",
    "go",
    "pki",
    "nvim",
    "vim",
    "git",
    "gh",
    "htop",
    "procps",
    "configstore",
    "enchant",
    "ubuntu-advantage",
    "update-notifier",
    "user-places.xbel",
    "user-places.xbel.bak",
    "backgrounds",
    "github-copilot",
    "ca-certificates",
    "containers",
    "docker",
    "dbus-1",
    "nano",
];

check!(OrphanedAppData, "uninstaller_data", "Leftover application data", "Configuration and data folders of applications that are no longer installed.", ScanMode::Advanced);
impl OrphanedAppData {
    fn scan_home(_ctx: &ScanContext, home: &Path) -> Vec<Issue> {
        // Everything that looks like an installed application name.
        let mut known: HashSet<String> = SYSTEM_CONFIG_DIRS.iter().map(|s| s.to_ascii_lowercase()).collect();
        if let Some(paths) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&paths) {
                if let Ok(rd) = fs::read_dir(d) {
                    known.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().to_ascii_lowercase()));
                }
            }
        }
        for id in installed_desktop_ids(home) {
            let stem = id.trim_end_matches(".desktop").to_ascii_lowercase();
            // `org.gnome.Nautilus` → also `nautilus`
            if let Some(last) = stem.rsplit('.').next() {
                known.insert(last.to_string());
            }
            known.insert(stem);
        }
        if let Ok(rd) = fs::read_dir(home.join(".var/app")) {
            known.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().to_ascii_lowercase()));
        }
        let is_known = |name: &str| {
            let n = name.trim_start_matches('.').to_ascii_lowercase();
            known.contains(&n)
                || known.contains(&n.replace(' ', "-"))
                || known.contains(&n.replace(' ', ""))
                || n.split(['-', '_', '.', ' ']).any(|part| part.len() >= 4 && known.contains(part))
        };

        let mut out = Vec::new();
        for sub in [".config", ".local/share"] {
            let Ok(rd) = fs::read_dir(home.join(sub)) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if !e.file_type().is_ok_and(|t| t.is_dir()) || is_known(&name) {
                    continue;
                }
                // Recently touched folders are probably in use (portable apps, AppImages...).
                let fresh = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d.as_secs() < 30 * 86400));
                if fresh {
                    continue;
                }
                out.push(Issue::new("uninstaller_data", p.display().to_string(), format!("no installed application named '{name}'"), Fix::DeleteDirTree { path: p }));
            }
        }
        out
    }
}

struct IncludeList;
impl Check for IncludeList {
    fn info(&self) -> CheckInfo {
        CheckInfo { id: "include_list", name: "Include list", description: "Contents of folders added to the include list.", min_mode: ScanMode::Advanced }
    }
    fn scan(&self, ctx: &ScanContext) -> Vec<Issue> {
        let mut out = Vec::new();
        for e in ctx.settings.include_list.iter().filter(|e| e.enabled) {
            let root = PathBuf::from(crate::pathutil::expand_env(&e.path));
            let Ok(rd) = fs::read_dir(&root) else {
                continue;
            };
            for c in rd.flatten() {
                let p = c.path();
                let is_dir = c.file_type().is_ok_and(|t| t.is_dir());
                if is_dir && !e.include_subkeys {
                    continue;
                }
                let fix = if is_dir { Fix::DeleteDirTree { path: p.clone() } } else { Fix::DeleteFile { path: p.clone() } };
                out.push(Issue::new("include_list", p.display().to_string(), format!("included by {}", e.path), fix));
            }
        }
        out
    }
}

pub fn checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(BrokenDesktopEntries),
        Box::new(BrokenAutostart),
        Box::new(StaleMimeHandlers),
        Box::new(RecentFiles),
        Box::new(DanglingSymlinks),
        Box::new(EmptyDirs),
        Box::new(OrphanedAppData),
        Box::new(IncludeList),
    ]
}

pub fn forced_scan(_ctx: &ScanContext, root: &str, needle: &str) -> Result<Vec<Issue>> {
    let root = PathBuf::from(crate::pathutil::expand_env(root));
    if !root.exists() {
        bail!("{} does not exist", root.display());
    }
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    walk(&root, 16, &mut |p, ft| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name.contains(&needle) {
            let fix = if ft.is_dir() { Fix::DeleteDirTree { path: p.into() } } else { Fix::DeleteFile { path: p.into() } };
            out.push(Issue::new("forced", p.display().to_string(), format!("name matches '{needle}'"), fix));
        }
    });
    Ok(out)
}

/// With "exclude external drives", a path on a removable mount that is not mounted right now
/// is treated as existing so its data is never reported.
pub fn target_exists(ctx: &ScanContext, path: &str) -> bool {
    let p = Path::new(path);
    if ctx.paths.exists(p) {
        return true;
    }
    if !ctx.settings.exclude_external_drives {
        return false;
    }
    let mount_parents = ["/media", "/run/media", "/mnt"];
    if !mount_parents.iter().any(|m| p.starts_with(m)) {
        return false;
    }
    // First existing ancestor: if it is a mount container (/media/<user>, /mnt...), the volume is absent.
    let first = p.ancestors().skip(1).find(|a| ctx.paths.exists(a));
    match first {
        Some(a) => mount_parents.iter().any(|m| a == Path::new(m)) || a.parent().is_some_and(|pp| mount_parents.iter().any(|m| pp == Path::new(m))),
        None => true,
    }
}

pub fn is_os_default(_location: &str) -> bool {
    false
}

// ---------------------------------------------------------------- backup / restore / fix

pub fn backup_fixes(dir: &Path, fixes: &[Fix]) -> Result<Vec<BackupEntry>> {
    let payload = dir.join("payload");
    ensure_dir(&payload)?;
    let mut done: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for fix in fixes {
        let path = match fix {
            Fix::DeleteFile { path } | Fix::DeleteEmptyDir { path } | Fix::DeleteDirTree { path } => path,
            Fix::RemoveXbelBookmark { file, .. } | Fix::RemoveMimeHandler { file, .. } | Fix::EditJson { file, .. } => file,
            Fix::WriteFile { path, .. } => path,
            Fix::WriteSetting { key, value, .. } => {
                let k = format!("{key}{value}");
                if done.insert(format!("dconf:{k}")) {
                    let o = std::process::Command::new("dconf").args(["read", &k]).output().context("running dconf read")?;
                    let value = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !value.is_empty() {
                        out.push(BackupEntry::DconfKey { key: k, value });
                    }
                }
                continue;
            }
            Fix::DconfResetDir { dir: d } => {
                if done.insert(format!("dconf:{d}")) {
                    let dump = std::process::Command::new("dconf").args(["dump", d]).output().context("running dconf dump")?;
                    let file = format!("payload/dconf-{:05}.ini", out.len());
                    fs::write(dir.join(&file), &dump.stdout)?;
                    out.push(BackupEntry::Dconf { dir: d.clone(), file });
                }
                continue;
            }
            Fix::DconfResetKey { key } => {
                if done.insert(format!("dconf:{key}")) {
                    let o = std::process::Command::new("dconf").args(["read", key]).output().context("running dconf read")?;
                    let value = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !value.is_empty() {
                        out.push(BackupEntry::DconfKey { key: key.clone(), value });
                    }
                }
                continue;
            }
            Fix::PurgePackage { manager, name } => {
                // System config is root-owned: record what is purged; readable conffiles are copied.
                let files = residual_conffiles(manager, name);
                for f in &files {
                    if fs::metadata(f).is_ok() && done.insert(f.display().to_string()) {
                        if let Ok(e) = super::files::backup_path(f, &payload, out.len()) {
                            out.push(e);
                        }
                    }
                }
                out.push(BackupEntry::Note {
                    label: format!("{manager} purge {name}"),
                    text: format!("Purged residual configuration of {name}. Reinstall the package to recreate it."),
                });
                continue;
            }
            Fix::DeleteRegKey { .. } | Fix::DeleteRegValue { .. } => bail!("registry fix on Linux"),
        };
        if !done.insert(path.display().to_string()) {
            continue;
        }
        out.push(super::files::backup_path(path, &payload, out.len())?);
    }
    Ok(out)
}

fn residual_conffiles(manager: &str, name: &str) -> Vec<PathBuf> {
    if manager != "dpkg" {
        return Vec::new();
    }
    let Ok(o) = std::process::Command::new("dpkg-query").args(["-W", "-f=${Conffiles}", name]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.split_whitespace().next().map(PathBuf::from)).collect()
}

/// Linux has no registry hive; a "full backup" snapshots the user's config stores.
pub fn backup_full(dir: &Path) -> Result<Vec<BackupEntry>> {
    let home = dirs::home_dir().context("no home directory")?;
    let payload = dir.join("payload");
    ensure_dir(&payload)?;
    let mut out = Vec::new();
    for sub in [".config", ".local/share/applications", ".local/share/recently-used.xbel"] {
        let p = home.join(sub);
        if p.exists() {
            out.push(super::files::backup_path(&p, &payload, out.len())?);
        }
    }
    Ok(out)
}

pub fn backup_key(dir: &Path, key: &str) -> Result<Vec<BackupEntry>> {
    let p = PathBuf::from(crate::pathutil::expand_env(key));
    let payload = dir.join("payload");
    ensure_dir(&payload)?;
    Ok(vec![super::files::backup_path(&p, &payload, 0)?])
}

pub fn restore_entries(dir: &Path, entries: &[&BackupEntry]) -> Result<()> {
    for e in entries {
        if super::files::restore_entry(dir, e)? {
            continue;
        }
        match e {
            BackupEntry::Dconf { dir: d, file } => {
                let data = fs::read(dir.join(file))?;
                let mut child = std::process::Command::new("dconf").args(["load", d]).stdin(std::process::Stdio::piped()).spawn().context("running dconf load")?;
                use std::io::Write;
                child.stdin.take().unwrap().write_all(&data)?;
                if !child.wait()?.success() {
                    bail!("dconf load {d} failed");
                }
            }
            BackupEntry::DconfKey { key, value } => {
                let st = std::process::Command::new("dconf").args(["write", key, value]).status().context("running dconf write")?;
                if !st.success() {
                    bail!("dconf write {key} failed");
                }
            }
            BackupEntry::Note { .. } => {}
            BackupEntry::RegFile { .. } | BackupEntry::Hive { .. } => {
                bail!("registry backup cannot be restored on Linux")
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn apply_fix(fix: &Fix, settings: &Settings) -> Result<()> {
    let guard = |p: &Path| -> Result<()> {
        // Never touch something the user explicitly excluded, even if a stale result asks for it.
        if settings.exclude_list.iter().filter(|e| e.enabled).any(|e| list_matches(e, &p.to_string_lossy())) {
            bail!("{} is on the exclude list", p.display());
        }
        Ok(())
    };
    match fix {
        Fix::DeleteFile { path } => {
            guard(path)?;
            super::files::delete_path(path, settings.delete_to_trash)
        }
        Fix::DeleteEmptyDir { path } => {
            guard(path)?;
            fs::remove_dir(path).with_context(|| format!("removing {}", path.display()))
        }
        Fix::DeleteDirTree { path } => {
            guard(path)?;
            super::files::delete_path(path, settings.delete_to_trash)
        }
        Fix::RemoveXbelBookmark { file, href } => {
            let text = fs::read_to_string(file)?;
            let new = remove_xbel_bookmark(&text, href);
            if new.len() == text.len() {
                bail!("entry not found");
            }
            write_atomic(file, &new)
        }
        Fix::RemoveMimeHandler { file, desktop_id } => {
            let text = fs::read_to_string(file)?;
            write_atomic(file, &remove_mime_handler(&text, desktop_id))
        }
        Fix::DconfResetDir { dir } => {
            let st = std::process::Command::new("dconf").args(["reset", "-f", dir]).status().context("running dconf reset")?;
            if !st.success() {
                bail!("dconf reset {dir} failed");
            }
            Ok(())
        }
        Fix::EditJson { file, remove } => super::files::edit_json(file, remove),
        Fix::WriteFile { path, data } => {
            guard(path)?;
            let bytes = crate::tracer::from_hex(data).context("damaged content in the log")?;
            let tmp = path.with_extension("cleancore-tmp");
            fs::write(&tmp, bytes)?;
            if let Ok(m) = fs::metadata(path) {
                let _ = fs::set_permissions(&tmp, m.permissions());
            }
            fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
        }
        Fix::WriteSetting { key, value, data } => {
            let text = String::from_utf8(crate::tracer::from_hex(data).context("damaged content in the log")?)?;
            let st = std::process::Command::new("dconf").args(["write", &format!("{key}{value}"), &text]).status().context("running dconf write")?;
            if !st.success() {
                bail!("dconf write {key}{value} failed");
            }
            Ok(())
        }
        Fix::DconfResetKey { key } => {
            let st = std::process::Command::new("dconf").args(["reset", key]).status().context("running dconf reset")?;
            if !st.success() {
                bail!("dconf reset {key} failed");
            }
            Ok(())
        }
        Fix::PurgePackage { manager, name } => super::linux_uninst::purge(manager, name),
        Fix::DeleteRegKey { .. } | Fix::DeleteRegValue { .. } => bail!("registry fix on Linux"),
    }
}

pub fn create_restore_point(description: &str) -> Result<()> {
    // Snapper / Timeshift integration: use whichever is installed.
    let cache = crate::pathutil::PathCache::default();
    if which("snapper", &cache).is_some() {
        let st = std::process::Command::new("snapper").args(["create", "-d", description]).status()?;
        if st.success() {
            return Ok(());
        }
    }
    if which("timeshift", &cache).is_some() {
        let st = std::process::Command::new("timeshift").args(["--create", "--comments", description]).status()?;
        if st.success() {
            return Ok(());
        }
    }
    bail!("no snapshot tool (snapper/timeshift) available or it failed")
}

/// Windows only: snapshots on Linux come from snapper or Timeshift, set up outside CleanCore.
pub fn enable_system_restore() -> Result<()> {
    bail!("System Restore is a Windows feature")
}

/// Show the item in the file manager.
pub fn open_in_editor(location: &str) -> Result<()> {
    let p = Path::new(location.split(" | ").next().unwrap_or(location));
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    std::process::Command::new("xdg-open").arg(dir).spawn()?;
    Ok(())
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("cleancore-tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------- small format helpers

fn xbel_hrefs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<bookmark ") {
        rest = &rest[i..];
        if let Some(h) = rest.find("href=\"") {
            let s = &rest[h + 6..];
            if let Some(end) = s.find('"') {
                out.push(&s[..end]);
            }
        }
        rest = &rest[10..];
    }
    out
}

fn remove_xbel_bookmark(text: &str, href: &str) -> String {
    let needle = format!("href=\"{href}\"");
    let Some(h) = text.find(&needle) else {
        return text.to_string();
    };
    let Some(start) = text[..h].rfind("<bookmark ") else {
        return text.to_string();
    };
    let after = &text[h..];
    let end = match (after.find("</bookmark>"), after.find("/>")) {
        // Self-closing bookmark that ends before any closing tag.
        (Some(c), Some(s)) if s < c && !after[..s].contains('>') => h + s + 2,
        (Some(c), _) => h + c + "</bookmark>".len(),
        (None, Some(s)) => h + s + 2,
        (None, None) => return text.to_string(),
    };
    // Also drop the indentation and trailing newline of the removed element.
    let line_start = text[..start].rfind('\n').map_or(start, |n| if text[n + 1..start].trim().is_empty() { n + 1 } else { start });
    let end = if text[end..].starts_with('\n') { end + 1 } else { end };
    format!("{}{}", &text[..line_start], &text[end..])
}

fn remove_mime_handler(text: &str, id: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match line.split_once('=') {
            Some((k, v)) if !line.trim_start().starts_with('[') && !line.trim_start().starts_with('#') => {
                let kept: Vec<&str> = v.split(';').map(str::trim).filter(|s| !s.is_empty() && *s != id).collect();
                if kept.is_empty() {
                    continue;
                }
                out.push_str(k);
                out.push('=');
                out.push_str(&kept.join(";"));
                if v.trim_end().ends_with(';') {
                    out.push(';');
                }
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------- tuner (gsettings)

use crate::tuner::{Prior, Setting, Tweak};
use std::collections::HashMap;
use std::sync::Mutex;

/// Reads and writes GNOME settings through the `gsettings` tool.
/// Each schema is read once with `list-recursively` and cached until it is written.
pub struct TuneBackend {
    schemas: Mutex<HashMap<String, Option<HashMap<String, String>>>>,
    have_gsettings: bool,
}

fn gsettings(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("gsettings").args(args).output().context("running gsettings")?;
    if !out.status.success() {
        bail!("gsettings {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// GVariant text compared loosely: collapse whitespace.
fn norm_variant(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl TuneBackend {
    pub fn new() -> Self {
        let have_gsettings = which("gsettings", &crate::pathutil::PathCache::default()).is_some();
        Self { schemas: Mutex::new(HashMap::new()), have_gsettings }
    }

    fn keys(&self, schema: &str) -> Option<HashMap<String, String>> {
        if let Some(v) = self.schemas.lock().unwrap().get(schema) {
            return v.clone();
        }
        let parsed = gsettings(&["list-recursively", schema]).ok().map(|out| {
            out.lines()
                .filter_map(|l| {
                    let (s, rest) = l.split_once(' ')?;
                    let (k, v) = rest.split_once(' ')?;
                    (s == schema).then(|| (k.to_string(), v.to_string()))
                })
                .collect::<HashMap<_, _>>()
        });
        self.schemas.lock().unwrap().insert(schema.to_string(), parsed.clone());
        parsed
    }

    fn get(&self, schema: &str, key: &str) -> Result<String> {
        self.keys(schema).and_then(|m| m.get(key).cloned()).with_context(|| format!("{schema} {key} is not installed"))
    }

    fn set(&self, schema: &str, key: &str, value: &str) -> Result<()> {
        gsettings(&["set", schema, key, value])?;
        self.schemas.lock().unwrap().remove(schema);
        Ok(())
    }

    pub fn available(&self, t: &Tweak) -> std::result::Result<(), String> {
        if !self.have_gsettings {
            return Err("gsettings not found (GNOME-based desktops only)".into());
        }
        for s in t.options.iter().flat_map(|o| &o.set) {
            let Setting::Gsettings { schema, key, .. } = s else {
                return Err("not a Linux tweak".into());
            };
            match self.keys(schema) {
                None => return Err(format!("schema {schema} is not installed")),
                Some(m) if !m.contains_key(key) => return Err(format!("{schema} has no key {key}")),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn matches(&self, s: &Setting) -> Result<bool> {
        let Setting::Gsettings { schema, key, value } = s else { bail!("not a Linux setting") };
        Ok(norm_variant(&self.get(schema, key)?) == norm_variant(value))
    }

    pub fn capture(&self, s: &Setting) -> Result<Prior> {
        let Setting::Gsettings { schema, key, .. } = s else { bail!("not a Linux setting") };
        Ok(Prior::Gsettings { schema: schema.clone(), key: key.clone(), value: self.get(schema, key)? })
    }

    pub fn apply(&self, s: &Setting) -> Result<()> {
        let Setting::Gsettings { schema, key, value } = s else { bail!("not a Linux setting") };
        self.set(schema, key, value)
    }

    pub fn restore(&self, p: &Prior) -> Result<()> {
        let Prior::Gsettings { schema, key, value } = p else { bail!("registry change cannot be reverted on Linux") };
        self.set(schema, key, value)
    }
}

impl Default for TuneBackend {
    fn default() -> Self {
        Self::new()
    }
}

pub fn restart_shell() -> Result<()> {
    bail!("not needed: GNOME applies settings immediately")
}

// ---------------------------------------------------------------- GUI helpers

pub fn browse_roots() -> Vec<super::BrowseNode> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        for (label, p) in [("Home", h.clone()), ("~/.config", h.join(".config")), ("~/.local/share", h.join(".local/share"))] {
            if p.is_dir() {
                v.push(super::BrowseNode { path: p.to_string_lossy().to_string(), name: label.into(), leaf: false });
            }
        }
    }
    v.push(super::BrowseNode { path: "/etc".into(), name: "/etc".into(), leaf: false });
    if which("dconf", &crate::pathutil::PathCache::default()).is_some() {
        v.push(super::BrowseNode { path: "dconf:/".into(), name: "dconf (GNOME settings)".into(), leaf: false });
    }
    v
}

pub fn browse_children(path: &str) -> Vec<super::BrowseNode> {
    let mut v = Vec::new();
    if let Some(dir) = path.strip_prefix("dconf:") {
        let out = std::process::Command::new("dconf").args(["list", dir]).output();
        for name in out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default().lines() {
            // Only folders can be watched as a group; single keys are shown as leaves.
            v.push(super::BrowseNode { path: format!("dconf:{dir}{name}"), name: name.trim_end_matches('/').into(), leaf: !name.ends_with('/') });
        }
    } else if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten().take(5000) {
            let leaf = !e.file_type().is_ok_and(|t| t.is_dir());
            v.push(super::BrowseNode { path: e.path().to_string_lossy().to_string(), name: e.file_name().to_string_lossy().to_string(), leaf });
        }
    }
    // Folders first, then by name.
    v.sort_by_key(|n| (n.leaf, n.name.to_lowercase()));
    v
}

pub fn open_folder(path: &Path) -> Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_removal() {
        let t = "[Default Applications]\ntext/plain=gone.desktop;gedit.desktop;\nimage/png=gone.desktop\n";
        assert_eq!(remove_mime_handler(t, "gone.desktop"), "[Default Applications]\ntext/plain=gedit.desktop;\n");
    }

    #[test]
    fn xbel_removal() {
        let t = "<xbel>\n  <bookmark href=\"file:///a\" added=\"x\">\n    <info/>\n  </bookmark>\n  <bookmark href=\"file:///b\"/>\n</xbel>\n";
        assert_eq!(xbel_hrefs(t), vec!["file:///a", "file:///b"]);
        let r = remove_xbel_bookmark(t, "file:///a");
        assert_eq!(r, "<xbel>\n  <bookmark href=\"file:///b\"/>\n</xbel>\n");
        assert_eq!(remove_xbel_bookmark(&r, "file:///b"), "<xbel>\n</xbel>\n");
    }

    #[test]
    fn percent() {
        assert_eq!(percent_decode("/a%20b/c%C3%A9"), "/a b/cé");
    }
}
