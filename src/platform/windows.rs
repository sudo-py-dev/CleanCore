//! Windows backend: registry scanning, `.reg` backups, hive snapshots, restore points.

use crate::backup::BackupEntry;
use crate::config::Settings;
use crate::engine::{Check, ScanContext};
use crate::model::{CheckInfo, Fix, Issue, ScanMode, Scope};
use crate::pathutil::{expand_env, extract_windows_path, windows_drive_root};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use winreg::enums::*;
use winreg::{RegKey, RegValue, HKEY};

// ---------------------------------------------------------------- registry helpers

const HKLM: &str = "HKEY_LOCAL_MACHINE";
const CV: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion";

pub(super) fn split_root(full: &str) -> Option<(HKEY, &str)> {
    let (root, rest) = full.split_once('\\').unwrap_or((full, ""));
    let h = match root.to_ascii_uppercase().as_str() {
        "HKEY_LOCAL_MACHINE" | "HKLM" => HKEY_LOCAL_MACHINE,
        "HKEY_CURRENT_USER" | "HKCU" => HKEY_CURRENT_USER,
        "HKEY_USERS" | "HKU" => HKEY_USERS,
        "HKEY_CLASSES_ROOT" | "HKCR" => HKEY_CLASSES_ROOT,
        "HKEY_CURRENT_CONFIG" | "HKCC" => HKEY_CURRENT_CONFIG,
        _ => return None,
    };
    Some((h, rest))
}

pub(super) fn open(full: &str) -> Option<RegKey> {
    let (h, rest) = split_root(full)?;
    RegKey::predef(h).open_subkey_with_flags(rest, KEY_READ).ok()
}

pub(super) fn join(a: &str, b: &str) -> String {
    format!(r"{a}\{b}")
}

pub(super) fn subkeys(full: &str) -> Vec<String> {
    open(full).map(|k| k.enum_keys().flatten().collect()).unwrap_or_default()
}

pub(super) fn values(full: &str) -> Vec<(String, RegValue)> {
    open(full).map(|k| k.enum_values().flatten().collect()).unwrap_or_default()
}

pub(super) fn reg_string(v: &RegValue) -> Option<String> {
    match v.vtype {
        REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => {
            let b: &[u8] = &v.bytes;
            let words: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let s = String::from_utf16_lossy(&words);
            // MULTI_SZ: first string only.
            Some(s.split('\0').next().unwrap_or("").to_string())
        }
        _ => None,
    }
}

pub(super) fn reg_u32(v: &RegValue) -> Option<u32> {
    (v.vtype == REG_DWORD && v.bytes.len() >= 4).then(|| u32::from_le_bytes([v.bytes[0], v.bytes[1], v.bytes[2], v.bytes[3]]))
}

pub(super) fn get_str(full: &str, name: &str) -> Option<String> {
    let k = open(full)?;
    let v = k.get_raw_value(name).ok()?;
    reg_string(&v).filter(|s| !s.is_empty())
}

pub(super) fn get_u32(full: &str, name: &str) -> Option<u32> {
    reg_u32(&open(full)?.get_raw_value(name).ok()?)
}

fn key_exists(full: &str) -> bool {
    open(full).is_some()
}

fn is_empty_key(full: &str) -> bool {
    open(full).and_then(|k| k.query_info().ok()).is_some_and(|i| i.sub_keys == 0 && i.values == 0)
}

/// User hives to scan: HKCU, or every loaded `HKEY_USERS\S-1-5-21-*` profile.
fn user_hives(ctx: &ScanContext) -> Vec<String> {
    if ctx.scope == Scope::CurrentUser {
        return vec!["HKEY_CURRENT_USER".into()];
    }
    let sids: Vec<String> = subkeys("HKEY_USERS").into_iter().filter(|s| s.starts_with("S-1-5-21-") && !s.ends_with("_Classes")).map(|s| join("HKEY_USERS", &s)).collect();
    if sids.is_empty() {
        vec!["HKEY_CURRENT_USER".into()]
    } else {
        sids
    }
}

/// Machine-wide plus per-user `Software\Classes` roots.
fn class_roots(ctx: &ScanContext) -> Vec<String> {
    let mut v = vec![join(HKLM, r"SOFTWARE\Classes")];
    v.extend(user_hives(ctx).iter().map(|h| join(h, r"Software\Classes")));
    v
}

/// Does `path` (as found in registry data) point to something missing?
fn missing(ctx: &ScanContext, data: &str) -> Option<String> {
    let p = extract_windows_path(data)?;
    (!ctx.target_exists(&p)).then_some(p)
}

pub(super) fn value_loc(key: &str, value: &str) -> String {
    format!("{key} | {}", if value.is_empty() { "(Default)" } else { value })
}

fn del_value(cat: &'static str, key: &str, value: &str, detail: impl Into<String>) -> Issue {
    Issue::new(cat, value_loc(key, value), detail, Fix::DeleteRegValue { key: key.into(), value: value.into() })
}

fn del_key(cat: &'static str, key: &str, detail: impl Into<String>) -> Issue {
    Issue::new(cat, key, detail, Fix::DeleteRegKey { key: key.into() })
}

// ---------------------------------------------------------------- checks

macro_rules! check {
    ($ty:ident, $id:literal, $name:literal, $desc:literal, $mode:expr, |$ctx:ident| $body:block) => {
        struct $ty;
        impl Check for $ty {
            fn info(&self) -> CheckInfo {
                CheckInfo { id: $id, name: $name, description: $desc, min_mode: $mode }
            }
            fn scan(&self, $ctx: &ScanContext) -> Vec<Issue> $body
        }
    };
}

const SHARED_DLLS: [&str; 2] = [r"SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\SharedDlls"];

check!(MissingSharedDlls, "shared_dlls_missing", "Missing Shared DLLs", "Shared dynamic-link libraries that do not exist.", ScanMode::Safe, |ctx| {
    let mut out = Vec::new();
    for sub in SHARED_DLLS {
        let key = join(HKLM, sub);
        for (name, _) in values(&key) {
            if let Some(p) = missing(ctx, &name) {
                out.push(del_value("shared_dlls_missing", &key, &name, format!("missing: {p}")));
            }
        }
    }
    out
});

check!(UnusedSharedDlls, "shared_dlls_unused", "Unused Shared DLLs", "Shared DLLs whose usage counter is 0.", ScanMode::Advanced, |ctx| {
    let mut out = Vec::new();
    for sub in SHARED_DLLS {
        let key = join(HKLM, sub);
        for (name, v) in values(&key) {
            if reg_u32(&v) == Some(0) && missing(ctx, &name).is_none() {
                out.push(del_value("shared_dlls_unused", &key, &name, "usage counter is 0"));
            }
        }
    }
    out
});

const RUN_KEYS: [&str; 6] = [
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunServices",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunServicesOnce",
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run",
    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
];

check!(Startup, "startup", "Missing Windows start-up items", "Programs set to start with Windows that no longer exist.", ScanMode::Safe, |ctx| {
    let mut out = Vec::new();
    let mut hives = vec![HKLM.to_string()];
    hives.extend(user_hives(ctx));
    for h in &hives {
        for sub in RUN_KEYS {
            let key = join(h, sub);
            for (name, v) in values(&key) {
                if let Some(p) = reg_string(&v).and_then(|d| missing(ctx, &d)) {
                    out.push(del_value("startup", &key, &name, format!("missing: {p}")));
                }
            }
        }
    }
    out
});

check!(AppPaths, "app_paths", "Wrong applications/paths", "App Paths and Applications entries whose program is missing.", ScanMode::Safe, |ctx| {
    let mut out = Vec::new();
    let mut roots = vec![join(HKLM, &format!(r"{CV}\App Paths")), join(HKLM, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths")];
    roots.extend(user_hives(ctx).iter().map(|h| join(h, r"Software\Microsoft\Windows\CurrentVersion\App Paths")));
    for root in roots {
        for sub in subkeys(&root) {
            let key = join(&root, &sub);
            if let Some(p) = get_str(&key, "").and_then(|d| missing(ctx, &d)) {
                out.push(del_key("app_paths", &key, format!("missing: {p}")));
            }
        }
    }
    out
});

check!(Uninstall, "uninstaller_data", "Incorrect uninstaller data", "Uninstall entries for programs that are gone, or empty entries.", ScanMode::Safe, |ctx| {
    let mut out = Vec::new();
    let mut roots = vec![join(HKLM, &format!(r"{CV}\Uninstall")), join(HKLM, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall")];
    roots.extend(user_hives(ctx).iter().map(|h| join(h, r"Software\Microsoft\Windows\CurrentVersion\Uninstall")));
    for root in roots {
        for sub in subkeys(&root) {
            let key = join(&root, &sub);
            if is_empty_key(&key) {
                out.push(del_key("uninstaller_data", &key, "empty uninstall entry"));
                continue;
            }
            let Some(cmd) = get_str(&key, "UninstallString") else {
                continue;
            };
            let Some(p) = missing(ctx, &cmd) else {
                continue;
            };
            // Keep it if the install folder still exists.
            let loc_ok = get_str(&key, "InstallLocation").is_some_and(|l| ctx.target_exists(&expand_env(&l)));
            if !loc_ok {
                out.push(del_key("uninstaller_data", &key, format!("uninstaller missing: {p}")));
            }
        }
    }
    out
});

check!(FirewallRules, "firewall_rules", "Invalid firewall rules", "Firewall rules for programs that no longer exist.", ScanMode::Safe, |ctx| {
    let key = join(HKLM, r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules");
    let mut out = Vec::new();
    for (name, v) in values(&key) {
        let Some(rule) = reg_string(&v) else { continue };
        let Some(app) = rule.split('|').find_map(|f| f.strip_prefix("App=")) else {
            continue;
        };
        if let Some(p) = missing(ctx, app) {
            out.push(del_value("firewall_rules", &key, &name, format!("missing: {p}")));
        }
    }
    out
});

check!(CacheEntries, "cache_entries", "Invalid cache entries", "MUI cache and compatibility-assistant entries for missing files.", ScanMode::Safe, |ctx| {
    let mut out = Vec::new();
    for h in user_hives(ctx) {
        let mui = join(&h, r"Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache");
        for (name, _) in values(&mui) {
            // "C:\x\app.exe.FriendlyAppName" / ".ApplicationCompany"
            let path = name.rsplit_once('.').map_or(name.as_str(), |(p, _)| p);
            if let Some(p) = missing(ctx, path) {
                out.push(del_value("cache_entries", &mui, &name, format!("missing: {p}")));
            }
        }
        for sub in [r"Compatibility Assistant\Store", r"Compatibility Assistant\Persisted", "Layers"] {
            let key = join(&h, &format!(r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\{sub}"));
            for (name, _) in values(&key) {
                if let Some(p) = missing(ctx, &name) {
                    out.push(del_value("cache_entries", &key, &name, format!("missing: {p}")));
                }
            }
        }
    }
    out
});

check!(Services, "services", "Unavailable Windows services", "Services whose executable or driver file is missing.", ScanMode::Moderate, |ctx| {
    let root = join(HKLM, r"SYSTEM\CurrentControlSet\Services");
    let mut out = Vec::new();
    for sub in subkeys(&root) {
        let key = join(&root, &sub);
        // Never touch boot/system-start drivers.
        if get_u32(&key, "Start").is_some_and(|s| s <= 1) {
            continue;
        }
        if let Some(p) = get_str(&key, "ImagePath").and_then(|d| missing(ctx, &d)) {
            out.push(del_key("services", &key, format!("missing: {p}")));
        }
    }
    out
});

check!(MruLists, "mru_history", "Most recently used and history lists", "Recent files, typed paths, run history and dialog history.", ScanMode::Moderate, |ctx| {
    const MRU: [&str; 10] = [
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\OpenSavePidlMRU",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRU",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\OpenSaveMRU",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedMRU",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\RecentDocs",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\RunMRU",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\TypedPaths",
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\WordWheelQuery",
        r"Software\Microsoft\Windows\CurrentVersion\Applets\Wordpad\Recent File List",
        r"Software\Microsoft\Windows\CurrentVersion\Applets\Paint\Recent File List",
    ];
    let mut out = Vec::new();
    for h in user_hives(ctx) {
        for sub in MRU {
            let key = join(&h, sub);
            if let Some(info) = open(&key).and_then(|k| k.query_info().ok()) {
                if info.values + info.sub_keys > 0 {
                    out.push(del_key("mru_history", &key, format!("{} entries", info.values + info.sub_keys)));
                }
            }
        }
    }
    out
});

check!(FileExtensions, "file_extensions", "Unused file extensions", "Extensions not associated with any program.", ScanMode::Moderate, |ctx| {
    let mut out = Vec::new();
    for root in class_roots(ctx) {
        for ext in subkeys(&root).into_iter().filter(|s| s.starts_with('.')) {
            let key = join(&root, &ext);
            let Some(k) = open(&key) else { continue };
            let Ok(info) = k.query_info() else { continue };
            let progid = get_str(&key, "");
            let only_default = info.sub_keys == 0 && info.values <= 1;
            let dead_progid = progid.as_ref().is_some_and(|p| !class_roots(ctx).iter().any(|r| key_exists(&join(r, p))));
            if only_default && (progid.is_none() || dead_progid) {
                out.push(del_key(
                    "file_extensions",
                    &key,
                    match progid {
                        Some(p) => format!("handler '{p}' does not exist"),
                        None => "no associated program".into(),
                    },
                ));
            }
        }
    }
    out
});

const SHELL_BASES: [&str; 7] = ["*", "Directory", r"Directory\Background", "Folder", "Drive", "AllFilesystemObjects", "DesktopBackground"];

check!(ContextMenu, "context_menu", "Corrupted context menu commands", "Context menu commands and handlers that point to missing files or classes.", ScanMode::Moderate, |ctx| {
    let mut out = Vec::new();
    let roots = class_roots(ctx);
    for root in &roots {
        let mut bases: Vec<String> = SHELL_BASES.iter().map(|b| join(root, b)).collect();
        bases.extend(subkeys(&join(root, "Applications")).iter().map(|a| join(root, &format!(r"Applications\{a}"))));
        for base in bases {
            let shell = join(&base, "shell");
            for verb in subkeys(&shell) {
                let vkey = join(&shell, &verb);
                if let Some(p) = get_str(&join(&vkey, "command"), "").and_then(|c| missing(ctx, &c)) {
                    out.push(del_key("context_menu", &vkey, format!("missing: {p}")));
                }
            }
            let handlers = join(&base, r"shellex\ContextMenuHandlers");
            for h in subkeys(&handlers) {
                let hkey = join(&handlers, &h);
                let clsid = get_str(&hkey, "").unwrap_or_else(|| h.clone());
                if clsid.starts_with('{') && !roots.iter().any(|r| key_exists(&join(r, &format!(r"CLSID\{clsid}")))) {
                    out.push(del_key("context_menu", &hkey, format!("handler class {clsid} is not registered")));
                }
            }
        }
    }
    out
});

check!(ComActiveX, "com_activex", "COM and ActiveX missing components", "COM class and type-library registrations whose files are missing.", ScanMode::Moderate, |ctx| {
    let mut out = Vec::new();
    let mut clsid_roots = vec![join(HKLM, r"SOFTWARE\Classes\CLSID"), join(HKLM, r"SOFTWARE\Classes\WOW6432Node\CLSID")];
    clsid_roots.extend(user_hives(ctx).iter().map(|h| join(h, r"Software\Classes\CLSID")));
    for root in clsid_roots {
        for guid in subkeys(&root) {
            let key = join(&root, &guid);
            for server in ["InprocServer32", "LocalServer32"] {
                if let Some(p) = get_str(&join(&key, server), "").and_then(|d| missing(ctx, &d)) {
                    out.push(del_key("com_activex", &key, format!("{server} missing: {p}")));
                    break;
                }
            }
        }
    }
    let tl = join(HKLM, r"SOFTWARE\Classes\TypeLib");
    for guid in subkeys(&tl) {
        for ver in subkeys(&join(&tl, &guid)) {
            let vkey = join(&tl, &format!(r"{guid}\{ver}"));
            for lcid in subkeys(&vkey).into_iter().filter(|s| s.chars().all(|c| c.is_ascii_hexdigit())) {
                for arch in ["win32", "win64"] {
                    if let Some(p) = get_str(&join(&vkey, &format!(r"{lcid}\{arch}")), "").and_then(|d| missing(ctx, &d)) {
                        out.push(del_key("com_activex", &vkey, format!("type library missing: {p}")));
                    }
                }
            }
        }
    }
    out
});

check!(MsiPaths, "msi_paths", "Invalid MSI paths", "Windows Installer folder references to folders that no longer exist.", ScanMode::Moderate, |ctx| {
    let key = join(HKLM, &format!(r"{CV}\Installer\Folders"));
    values(&key).into_iter().filter_map(|(name, _)| missing(ctx, &name).map(|p| del_value("msi_paths", &key, &name, format!("missing: {p}")))).collect()
});

check!(SystemSettings, "system_settings", "Invalid system settings items", "Fonts, sound events and shell icons that point to missing files.", ScanMode::Moderate, |ctx| {
    let mut out = Vec::new();
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let fonts = join(HKLM, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts");
    for (name, v) in values(&fonts) {
        let Some(file) = reg_string(&v) else { continue };
        let path = if crate::pathutil::is_abs_windows(&file) { file } else { format!(r"{windir}\Fonts\{file}") };
        if !ctx.target_exists(&path) {
            out.push(del_value("system_settings", &fonts, &name, format!("missing: {path}")));
        }
    }
    let icons = join(HKLM, &format!(r"{CV}\Explorer\Shell Icons"));
    for (name, v) in values(&icons) {
        if let Some(p) = reg_string(&v).and_then(|d| missing(ctx, &d)) {
            out.push(del_value("system_settings", &icons, &name, format!("missing: {p}")));
        }
    }
    for h in user_hives(ctx) {
        let apps = join(&h, r"AppEvents\Schemes\Apps");
        for app in subkeys(&apps) {
            for event in subkeys(&join(&apps, &app)) {
                let cur = join(&apps, &format!(r"{app}\{event}\.Current"));
                if let Some(p) = get_str(&cur, "").and_then(|d| missing(ctx, &d)) {
                    out.push(del_value("system_settings", &cur, "", format!("missing sound: {p}")));
                }
            }
        }
    }
    out
});

check!(ClassKeys, "class_keys", "Invalid Class keys items", "File-type icons and class data that reference missing files.", ScanMode::Advanced, |ctx| {
    let mut out = Vec::new();
    for root in class_roots(ctx) {
        for class in subkeys(&root).into_iter().filter(|s| !s.starts_with('.') && !s.starts_with('{') && s != "CLSID") {
            let icon = join(&root, &format!(r"{class}\DefaultIcon"));
            if let Some(p) = get_str(&icon, "").and_then(|d| missing(ctx, &d)) {
                out.push(del_key("class_keys", &icon, format!("missing icon: {p}")));
            }
        }
    }
    out
});

const EMPTY_SKIP: [&str; 8] = ["classes", "microsoft", "policies", "wow6432node", "clients", "registeredapplications", "intel", "nvidia corporation"];

check!(EmptyKeys, "empty_keys", "Empty keys", "Software-related keys that do not contain any data.", ScanMode::Advanced, |ctx| {
    let mut out = Vec::new();
    let mut roots = vec![join(HKLM, "SOFTWARE"), join(HKLM, r"SOFTWARE\WOW6432Node")];
    roots.extend(user_hives(ctx).iter().map(|h| join(h, "Software")));
    for root in roots {
        for vendor in subkeys(&root).into_iter().filter(|v| !EMPTY_SKIP.contains(&v.to_ascii_lowercase().as_str())) {
            let vkey = join(&root, &vendor);
            if is_empty_key(&vkey) {
                out.push(del_key("empty_keys", &vkey, "empty key"));
                continue;
            }
            for product in subkeys(&vkey) {
                let pkey = join(&vkey, &product);
                if is_empty_key(&pkey) {
                    out.push(del_key("empty_keys", &pkey, "empty key"));
                }
            }
        }
    }
    out
});

struct IncludeList;
impl Check for IncludeList {
    fn info(&self) -> CheckInfo {
        CheckInfo { id: "include_list", name: "Include list", description: "Contents of keys added to the include list.", min_mode: ScanMode::Advanced }
    }
    fn scan(&self, ctx: &ScanContext) -> Vec<Issue> {
        let mut out = Vec::new();
        for e in ctx.settings.include_list.iter().filter(|e| e.enabled) {
            for (name, _) in values(&e.path) {
                out.push(del_value("include_list", &e.path, &name, format!("included by {}", e.path)));
            }
            if e.include_subkeys {
                for sub in subkeys(&e.path) {
                    out.push(del_key("include_list", &join(&e.path, &sub), format!("included by {}", e.path)));
                }
            }
        }
        out
    }
}

pub fn checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(MissingSharedDlls),
        Box::new(UnusedSharedDlls),
        Box::new(MsiPaths),
        Box::new(ComActiveX),
        Box::new(MruLists),
        Box::new(Startup),
        Box::new(CacheEntries),
        Box::new(Services),
        Box::new(Uninstall),
        Box::new(FileExtensions),
        Box::new(FirewallRules),
        Box::new(EmptyKeys),
        Box::new(AppPaths),
        Box::new(ClassKeys),
        Box::new(ContextMenu),
        Box::new(SystemSettings),
        Box::new(IncludeList),
    ]
}

pub fn forced_scan(_ctx: &ScanContext, root: &str, needle: &str) -> Result<Vec<Issue>> {
    if open(root).is_none() {
        bail!("cannot open {root}");
    }
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    fn walk(key: &str, needle: &str, out: &mut Vec<Issue>, depth: usize) {
        for (name, v) in values(key) {
            let data = reg_string(&v).unwrap_or_default();
            if name.to_lowercase().contains(needle) || data.to_lowercase().contains(needle) {
                out.push(del_value("forced", key, &name, format!("matches: {data}")));
            }
        }
        if depth == 0 {
            return;
        }
        for sub in subkeys(key) {
            let full = join(key, &sub);
            if sub.to_lowercase().contains(needle) {
                out.push(del_key("forced", &full, "key name matches"));
            } else {
                walk(&full, needle, out, depth - 1);
            }
        }
    }
    walk(root, &needle, &mut out, 32);
    Ok(out)
}

/// With "exclude external drives", a path on a drive that is not mounted counts as existing.
pub fn target_exists(ctx: &ScanContext, path: &str) -> bool {
    if ctx.paths.exists(Path::new(path)) {
        return true;
    }
    if ctx.settings.exclude_external_drives {
        if let Some(root) = windows_drive_root(path) {
            return !ctx.paths.exists(Path::new(&root));
        }
        if path.starts_with(r"\\") {
            return true; // network share: never judge
        }
    }
    false
}

/// Locations Windows recreates or keeps for compatibility; reporting them is noise.
const OS_DEFAULTS: [&str; 6] = [
    r"\software\microsoft\windows\currentversion\explorer\fileexts\",
    r"\software\classes\local settings\software\microsoft\windows\shell\muicache | @",
    r"\currentversion\uninstall\connection manager",
    r"\currentversion\uninstall\wic",
    r"\currentversion\uninstall\mobileoptionpack",
    r"\services\sharedaccess\parameters\firewallpolicy\firewallrules | ms-",
];

pub fn is_os_default(location: &str) -> bool {
    let l = location.to_ascii_lowercase();
    OS_DEFAULTS.iter().any(|d| l.contains(d))
}

// ---------------------------------------------------------------- backup / restore / fix

/// regedit-compatible export of a key (recursive) or of one value.
fn export_reg(key: &str, only_value: Option<&str>, out: &mut String) {
    let Some(k) = open(key) else { return };
    out.push_str(&format!("[{key}]\r\n"));
    match only_value {
        Some(name) => {
            if let Ok(v) = k.get_raw_value(name) {
                out.push_str(&reg_line(name, &v));
            }
        }
        None => {
            for (name, v) in k.enum_values().flatten() {
                out.push_str(&reg_line(&name, &v));
            }
        }
    }
    out.push_str("\r\n");
    if only_value.is_none() {
        for sub in k.enum_keys().flatten() {
            export_reg(&join(key, &sub), None, out);
        }
    }
}

fn reg_line(name: &str, v: &RegValue) -> String {
    let esc = |s: &str| s.replace('\\', r"\\").replace('"', "\\\"");
    let lhs = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", esc(name)) };
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(",");
    let rhs = match v.vtype {
        REG_SZ => format!("\"{}\"", esc(&reg_string(v).unwrap_or_default())),
        REG_DWORD => format!("dword:{:08x}", reg_u32(v).unwrap_or(0)),
        REG_EXPAND_SZ => format!("hex(2):{}", hex(&v.bytes)),
        REG_MULTI_SZ => format!("hex(7):{}", hex(&v.bytes)),
        REG_QWORD => format!("hex(b):{}", hex(&v.bytes)),
        REG_BINARY => format!("hex:{}", hex(&v.bytes)),
        ref other => format!("hex({:x}):{}", other.clone() as u32, hex(&v.bytes)),
    };
    format!("{lhs}={rhs}\r\n")
}

fn write_reg_file(path: &Path, body: &str) -> Result<()> {
    let text = format!("Windows Registry Editor Version 5.00\r\n\r\n{body}");
    let mut bytes = vec![0xFF, 0xFE];
    for w in text.encode_utf16() {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

pub fn backup_fixes(dir: &Path, fixes: &[Fix]) -> Result<Vec<BackupEntry>> {
    let mut out = Vec::new();
    for (n, fix) in fixes.iter().enumerate() {
        let (key, value) = match fix {
            Fix::DeleteRegKey { key } => (key, None),
            Fix::DeleteRegValue { key, value } | Fix::WriteSetting { key, value, .. } => (key, Some(value.as_str())),
            Fix::DeleteFile { path } | Fix::DeleteEmptyDir { path } | Fix::DeleteDirTree { path } | Fix::EditJson { file: path, .. } | Fix::WriteFile { path, .. } => {
                out.push(super::files::backup_path(path, &dir.join("payload"), n)?);
                continue;
            }
            Fix::PurgePackage { manager, name } => {
                out.push(BackupEntry::Note { label: format!("{manager} remove {name}"), text: format!("Removed {name}. Reinstall it from the Store to get it back.") });
                continue;
            }
            other => bail!("unsupported fix on Windows: {other:?}"),
        };
        let mut body = String::new();
        export_reg(key, value, &mut body);
        let file = format!("{n:05}.reg");
        write_reg_file(&dir.join(&file), &body)?;
        out.push(BackupEntry::RegFile { label: value.map_or_else(|| key.clone(), |v| value_loc(key, v)), file });
    }
    Ok(out)
}

pub fn backup_key(dir: &Path, key: &str) -> Result<Vec<BackupEntry>> {
    if open(key).is_none() {
        bail!("cannot open {key}");
    }
    let mut body = String::new();
    export_reg(key, None, &mut body);
    write_reg_file(&dir.join("key.reg"), &body)?;
    Ok(vec![BackupEntry::RegFile { label: key.into(), file: "key.reg".into() }])
}

/// Full backup: binary hive copies (needs administrator) plus a `Restore.bat`
/// for the Windows Recovery Environment, where the hives are not in use.
pub fn backup_full(dir: &Path) -> Result<Vec<BackupEntry>> {
    let hives =
        [(r"HKLM\SOFTWARE", "SOFTWARE"), (r"HKLM\SYSTEM", "SYSTEM"), (r"HKLM\SAM", "SAM"), (r"HKLM\SECURITY", "SECURITY"), (r"HKU\.DEFAULT", "DEFAULT"), (r"HKCU", "NTUSER.DAT")];
    let mut out = Vec::new();
    let mut bat = String::from("@echo off\r\nrem Run from the Windows Recovery Environment command prompt.\r\nset CFG=%SystemDrive%\\Windows\\System32\\config\r\n");
    for (key, file) in hives {
        let target = dir.join(file);
        let st = Command::new("reg").args(["save", key, &target.to_string_lossy(), "/y"]).output()?;
        if !st.status.success() {
            bail!("reg save {key} failed (run as administrator): {}", String::from_utf8_lossy(&st.stderr).trim());
        }
        out.push(BackupEntry::Hive { label: key.into(), file: file.into() });
        if file == "NTUSER.DAT" {
            bat.push_str(&format!("copy /y \"%~dp0{file}\" \"%USERPROFILE%\\NTUSER.DAT\"\r\n"));
        } else {
            bat.push_str(&format!("copy /y \"%~dp0{file}\" \"%CFG%\\{file}\"\r\n"));
        }
    }
    std::fs::write(dir.join("Restore.bat"), bat)?;
    Ok(out)
}

pub fn restore_entries(dir: &Path, entries: &[&BackupEntry]) -> Result<()> {
    for e in entries {
        if super::files::restore_entry(dir, e)? {
            continue;
        }
        match e {
            BackupEntry::RegFile { file, label } => {
                let st = Command::new("reg").args(["import", &dir.join(file).to_string_lossy()]).output()?;
                if !st.status.success() {
                    bail!("reg import failed for {label}: {}", String::from_utf8_lossy(&st.stderr).trim());
                }
            }
            BackupEntry::Hive { .. } => bail!("full hive backups are restored offline: boot into Windows Recovery Environment and run {}", dir.join("Restore.bat").display()),
            BackupEntry::Note { .. } => {}
            other => bail!("unsupported backup entry on Windows: {other:?}"),
        }
    }
    Ok(())
}

pub fn apply_fix(fix: &Fix, settings: &Settings) -> Result<()> {
    match fix {
        Fix::DeleteFile { path } | Fix::DeleteDirTree { path } => {
            if settings.is_excluded(&path.display().to_string()) {
                bail!("{} is on the exclude list", path.display());
            }
            super::files::delete_path(path, settings.delete_to_trash)
        }
        Fix::DeleteEmptyDir { path } => std::fs::remove_dir(path).with_context(|| format!("removing {}", path.display())),
        Fix::PurgePackage { manager, name } => super::windows_uninst::purge(manager, name),
        Fix::EditJson { file, remove } => super::files::edit_json(file, remove),
        Fix::WriteFile { path, data } => {
            if settings.is_excluded(&path.display().to_string()) {
                bail!("{} is on the exclude list", path.display());
            }
            let bytes = crate::tracer::from_hex(data).context("damaged content in the log")?;
            std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
        }
        Fix::WriteSetting { key, value, data } => {
            let b = crate::tracer::from_hex(data).context("damaged content in the log")?;
            anyhow::ensure!(b.len() >= 4, "damaged content in the log");
            let raw = crate::tuner::RawValue { vtype: u32::from_le_bytes([b[0], b[1], b[2], b[3]]), bytes: b[4..].to_vec() };
            write_raw(key, value, &raw)
        }
        Fix::DeleteRegKey { key } => {
            let (h, rest) = split_root(key).context("bad key")?;
            let (parent, leaf) = rest.rsplit_once('\\').unwrap_or(("", rest));
            if leaf.is_empty() {
                bail!("refusing to delete a root key");
            }
            let p = RegKey::predef(h).open_subkey_with_flags(parent, KEY_ALL_ACCESS).with_context(|| format!("opening {parent}"))?;
            p.delete_subkey_all(leaf).with_context(|| format!("deleting {key}"))
        }
        Fix::DeleteRegValue { key, value } => {
            let (h, rest) = split_root(key).context("bad key")?;
            let k = RegKey::predef(h).open_subkey_with_flags(rest, KEY_SET_VALUE).with_context(|| format!("opening {key}"))?;
            k.delete_value(value).with_context(|| format!("deleting {}", value_loc(key, value)))
        }
        other => bail!("unsupported fix on Windows: {other:?}"),
    }
}

pub fn create_restore_point(description: &str) -> Result<()> {
    let cmd = format!("Checkpoint-Computer -Description '{}' -RestorePointType MODIFY_SETTINGS", description.replace('\'', "''"));
    let st = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &cmd]).output()?;
    if !st.status.success() {
        bail!("{}", String::from_utf8_lossy(&st.stderr).trim());
    }
    Ok(())
}

pub fn open_in_editor(key: &str) -> Result<()> {
    let hk = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit")?.0;
    hk.set_value("LastKey", &format!(r"Computer\{}", key.split(" | ").next().unwrap_or(key)))?;
    Command::new("regedit.exe").arg("/m").spawn()?;
    Ok(())
}

// ---------------------------------------------------------------- tuner (registry)

use crate::tuner::{Prior, RawValue, Setting, Tweak};

fn to_reg_type(t: u32) -> RegType {
    match t {
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        8 => REG_RESOURCE_LIST,
        9 => REG_FULL_RESOURCE_DESCRIPTOR,
        10 => REG_RESOURCE_REQUIREMENTS_LIST,
        11 => REG_QWORD,
        _ => REG_NONE,
    }
}

pub(super) fn read_raw(key: &str, name: &str) -> Option<RawValue> {
    let v = open(key)?.get_raw_value(name).ok()?;
    Some(RawValue { vtype: v.vtype.clone() as u32, bytes: v.bytes.to_vec() })
}

pub(super) fn write_raw(key: &str, name: &str, raw: &RawValue) -> Result<()> {
    let (h, rest) = split_root(key).context("bad key")?;
    let (k, _) = RegKey::predef(h).create_subkey(rest).with_context(|| format!("creating {key}{}", admin_hint(h)))?;
    let v = RegValue { bytes: raw.bytes.clone().into(), vtype: to_reg_type(raw.vtype) };
    k.set_raw_value(name, &v).with_context(|| format!("writing {}{}", value_loc(key, name), admin_hint(h)))
}

fn delete_value_if_present(key: &str, name: &str) -> Result<()> {
    let (h, rest) = split_root(key).context("bad key")?;
    let Ok(k) = RegKey::predef(h).open_subkey_with_flags(rest, KEY_SET_VALUE) else {
        return if open(key).is_none() { Ok(()) } else { bail!("cannot open {key} for writing{}", admin_hint(h)) };
    };
    match k.delete_value(name) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("deleting {}", value_loc(key, name))),
        _ => Ok(()),
    }
}

fn admin_hint(h: HKEY) -> &'static str {
    if h == HKEY_LOCAL_MACHINE {
        " (run as administrator)"
    } else {
        ""
    }
}

/// The highest key in `key`'s path that does not exist yet (what a create would add).
fn first_missing_ancestor(key: &str) -> Option<String> {
    let mut path = String::new();
    for (i, part) in key.split('\\').enumerate() {
        if i > 0 {
            path.push('\\');
        }
        path.push_str(part);
        if i > 0 && open(&path).is_none() {
            return Some(path);
        }
    }
    None
}

pub struct TuneBackend {
    build: u32,
}

impl TuneBackend {
    pub fn new() -> Self {
        let build = get_str(&join(HKLM, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), "CurrentBuildNumber").and_then(|s| s.parse().ok()).unwrap_or(0);
        Self { build }
    }

    pub fn available(&self, t: &Tweak) -> std::result::Result<(), String> {
        let lo = t.min_build.unwrap_or(0);
        let hi = t.max_build.unwrap_or(u32::MAX);
        if self.build != 0 && !(lo..=hi).contains(&self.build) {
            return Err(match (t.min_build, t.max_build) {
                (Some(a), Some(b)) => {
                    format!("needs Windows build {a}–{b}, this is {}", self.build)
                }
                (Some(a), None) => {
                    format!("needs Windows build {a} or newer, this is {}", self.build)
                }
                (None, Some(b)) => format!("not supported after build {b}, this is {}", self.build),
                (None, None) => unreachable!(),
            });
        }
        if t.options.iter().flat_map(|o| &o.set).any(|s| matches!(s, Setting::Gsettings { .. })) {
            return Err("not a Windows tweak".into());
        }
        Ok(())
    }

    pub fn matches(&self, s: &Setting) -> Result<bool> {
        Ok(match s {
            Setting::Reg { key, name, data } => read_raw(key, name).is_some_and(|v| v.same_as(&data.to_raw())),
            Setting::RegDelete { key, name } => read_raw(key, name).is_none(),
            Setting::Gsettings { .. } => bail!("not a Windows setting"),
        })
    }

    pub fn capture(&self, s: &Setting) -> Result<Prior> {
        let (key, name, creates) = match s {
            Setting::Reg { key, name, .. } => (key, name, true),
            Setting::RegDelete { key, name } => (key, name, false),
            Setting::Gsettings { .. } => bail!("not a Windows setting"),
        };
        Ok(Prior::Reg { key: key.clone(), name: name.clone(), value: read_raw(key, name), created_key: if creates { first_missing_ancestor(key) } else { None } })
    }

    pub fn apply(&self, s: &Setting) -> Result<()> {
        match s {
            Setting::Reg { key, name, data } => write_raw(key, name, &data.to_raw()),
            Setting::RegDelete { key, name } => delete_value_if_present(key, name),
            Setting::Gsettings { .. } => bail!("not a Windows setting"),
        }
    }

    pub fn restore(&self, p: &Prior) -> Result<()> {
        let Prior::Reg { key, name, value, created_key } = p else { bail!("gsettings change cannot be reverted on Windows") };
        if let Some(created) = created_key {
            if open(created).is_some() {
                return apply_fix(&Fix::DeleteRegKey { key: created.clone() }, &Settings::default());
            }
            return Ok(());
        }
        match value {
            Some(v) => write_raw(key, name, v),
            None => delete_value_if_present(key, name),
        }
    }
}

/// Restart File Explorer so shell tweaks take effect.
pub fn restart_shell() -> Result<()> {
    Command::new("taskkill").args(["/f", "/im", "explorer.exe"]).output()?;
    Command::new("explorer.exe").spawn()?;
    Ok(())
}

// ---------------------------------------------------------------- GUI helpers

pub fn browse_roots() -> Vec<super::BrowseNode> {
    ["HKEY_CLASSES_ROOT", "HKEY_CURRENT_USER", "HKEY_LOCAL_MACHINE", "HKEY_USERS", "HKEY_CURRENT_CONFIG"]
        .iter()
        .map(|r| super::BrowseNode { path: r.to_string(), name: r.to_string(), leaf: false })
        .collect()
}

pub fn browse_children(path: &str) -> Vec<super::BrowseNode> {
    let mut v: Vec<super::BrowseNode> = subkeys(path).into_iter().map(|s| super::BrowseNode { path: join(path, &s), name: s, leaf: false }).collect();
    v.sort_by_key(|n| n.name.to_lowercase());
    v
}

pub fn open_folder(path: &Path) -> Result<()> {
    Command::new("explorer.exe").arg(path).spawn()?;
    Ok(())
}
