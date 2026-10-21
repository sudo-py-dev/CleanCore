//! Browser Extensions (Revo: "Browser Extensions"): list and remove extensions of
//! Chromium-based browsers (Chrome, Chromium, Edge, Brave, Opera, Vivaldi) and Firefox, for
//! every profile. Removal deletes the extension and its stored data and drops its entries from
//! the browser's preference files; everything is backed up first. The browser must be closed.

use crate::model::{Fix, Issue};
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Browser {
    Chrome,
    Chromium,
    Edge,
    Brave,
    Opera,
    Vivaldi,
    Firefox,
}

impl Browser {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Google Chrome",
            Self::Chromium => "Chromium",
            Self::Edge => "Microsoft Edge",
            Self::Brave => "Brave",
            Self::Opera => "Opera",
            Self::Vivaldi => "Vivaldi",
            Self::Firefox => "Firefox",
        }
    }

    /// Command-line flag name (Revo /chrome /edge /firefox /opera).
    pub fn flag(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Brave => "brave",
            Self::Opera => "opera",
            Self::Vivaldi => "vivaldi",
            Self::Firefox => "firefox",
        }
    }

    pub fn from_flag(s: &str) -> Option<Self> {
        [Self::Chrome, Self::Chromium, Self::Edge, Self::Brave, Self::Opera, Self::Vivaldi, Self::Firefox]
            .into_iter()
            .find(|b| b.flag() == s.trim_start_matches(['-', '/']).to_lowercase())
    }

    /// Process names that mean the browser is running.
    fn processes(self) -> &'static [&'static str] {
        match self {
            Self::Chrome => &["chrome", "chrome.exe", "google-chrome"],
            Self::Chromium => &["chromium", "chromium-browse", "chromium.exe"],
            Self::Edge => &["msedge", "msedge.exe", "microsoft-edge"],
            Self::Brave => &["brave", "brave.exe", "brave-browser"],
            Self::Opera => &["opera", "opera.exe"],
            Self::Vivaldi => &["vivaldi", "vivaldi-bin", "vivaldi.exe"],
            Self::Firefox => &["firefox", "firefox.exe", "firefox-bin"],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Extension {
    pub browser: Browser,
    /// Profile folder and its display name.
    pub profile_dir: PathBuf,
    pub profile: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Extension / Theme / App / Dictionary …
    pub kind: String,
    pub enabled: bool,
    /// Shipped with the browser: listed but not removable.
    pub builtin: bool,
    /// Where the extension's files are.
    pub path: Option<PathBuf>,
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Chromium "User Data" folders per browser (and Opera's single-profile folder).
fn chromium_roots() -> Vec<(Browser, PathBuf)> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        if let Some(l) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            v.push((Browser::Chrome, l.join(r"Google\Chrome\User Data")));
            v.push((Browser::Chromium, l.join(r"Chromium\User Data")));
            v.push((Browser::Edge, l.join(r"Microsoft\Edge\User Data")));
            v.push((Browser::Brave, l.join(r"BraveSoftware\Brave-Browser\User Data")));
            v.push((Browser::Vivaldi, l.join(r"Vivaldi\User Data")));
        }
        if let Some(r) = std::env::var_os("APPDATA").map(PathBuf::from) {
            v.push((Browser::Opera, r.join(r"Opera Software\Opera Stable")));
            v.push((Browser::Opera, r.join(r"Opera Software\Opera GX Stable")));
        }
    }
    #[cfg(not(windows))]
    {
        let h = home();
        let c = h.join(".config");
        for (b, p) in [
            (Browser::Chrome, c.join("google-chrome")),
            (Browser::Chrome, h.join(".var/app/com.google.Chrome/config/google-chrome")),
            (Browser::Chromium, c.join("chromium")),
            (Browser::Chromium, h.join("snap/chromium/common/chromium")),
            (Browser::Chromium, h.join(".var/app/org.chromium.Chromium/config/chromium")),
            (Browser::Edge, c.join("microsoft-edge")),
            (Browser::Edge, h.join(".var/app/com.microsoft.Edge/config/microsoft-edge")),
            (Browser::Brave, c.join("BraveSoftware/Brave-Browser")),
            (Browser::Brave, h.join("snap/brave/current/.config/BraveSoftware/Brave-Browser")),
            (Browser::Brave, h.join(".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser")),
            (Browser::Opera, c.join("opera")),
            (Browser::Vivaldi, c.join("vivaldi")),
        ] {
            v.push((b, p));
        }
    }
    v.into_iter().filter(|(_, p)| p.is_dir()).collect()
}

fn firefox_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    if let Some(r) = std::env::var_os("APPDATA").map(PathBuf::from) {
        v.push(r.join(r"Mozilla\Firefox"));
    }
    #[cfg(not(windows))]
    {
        let h = home();
        v.push(h.join(".mozilla/firefox"));
        v.push(h.join("snap/firefox/common/.mozilla/firefox"));
        v.push(h.join(".var/app/org.mozilla.firefox/.mozilla/firefox"));
    }
    v.into_iter().filter(|p| p.join("profiles.ini").is_file()).collect()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Resolve `__MSG_name__` from the extension's `_locales`.
fn localize(text: &str, ext_dir: &Path, default_locale: Option<&str>) -> String {
    let Some(key) = text.strip_prefix("__MSG_").and_then(|t| t.strip_suffix("__")) else {
        return text.to_string();
    };
    let mut locales: Vec<String> = Vec::new();
    if let Some(d) = default_locale {
        locales.push(d.to_string());
    }
    locales.extend(["en".to_string(), "en_US".to_string()]);
    for loc in locales {
        if let Some(Value::Object(m)) = read_json(&ext_dir.join("_locales").join(&loc).join("messages.json")) {
            if let Some((_, v)) = m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
                if let Some(msg) = v.get("message").and_then(|x| x.as_str()) {
                    return msg.to_string();
                }
            }
        }
    }
    text.to_string()
}

fn chromium_profiles(browser: Browser, root: &Path) -> Vec<(PathBuf, String)> {
    // Opera keeps a single profile directly in its folder.
    if root.join("Preferences").is_file() && !root.join("Local State").is_file() || browser == Browser::Opera && root.join("Preferences").is_file() {
        return vec![(root.to_path_buf(), "Default".into())];
    }
    let names = read_json(&root.join("Local State"));
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let dir = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            if dir.join("Preferences").is_file() && (n == "Default" || n.starts_with("Profile ")) {
                let display = names.as_ref().and_then(|v| v.pointer(&format!("/profile/info_cache/{n}/name"))).and_then(|x| x.as_str()).map(str::to_string).unwrap_or(n);
                out.push((dir, display));
            }
        }
    }
    out.sort();
    out
}

fn chromium_extensions(browser: Browser, profile_dir: &Path, profile: &str) -> Vec<Extension> {
    let prefs = read_json(&profile_dir.join("Preferences"));
    let secure = read_json(&profile_dir.join("Secure Preferences"));
    let setting = |id: &str| -> Option<Value> {
        for p in [&secure, &prefs].into_iter().flatten() {
            if let Some(s) = p.pointer(&format!("/extensions/settings/{id}")) {
                return Some(s.clone());
            }
        }
        None
    };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(profile_dir.join("Extensions")) else {
        return out;
    };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().to_string();
        if id == "Temp" {
            continue;
        }
        // Newest version folder
        let Some(ver_dir) = std::fs::read_dir(e.path()).ok().and_then(|r| r.flatten().map(|v| v.path()).filter(|p| p.join("manifest.json").is_file()).max()) else {
            continue;
        };
        let Some(m) = read_json(&ver_dir.join("manifest.json")) else {
            continue;
        };
        let dl = m.get("default_locale").and_then(|x| x.as_str());
        let s = setting(&id);
        let location = s.as_ref().and_then(|s| s.get("location")).and_then(|x| x.as_i64()).unwrap_or(1);
        let disabled = s.as_ref().is_some_and(|s| {
            s.get("state").and_then(|x| x.as_i64()) == Some(0)
                || s.get("disable_reasons").is_some_and(|d| d.as_i64().is_some_and(|n| n != 0) || d.as_array().is_some_and(|a| !a.is_empty()))
        });
        let kind = if m.get("theme").is_some() {
            "Theme"
        } else if m.get("app").is_some() {
            "App"
        } else {
            "Extension"
        };
        out.push(Extension {
            browser,
            profile_dir: profile_dir.to_path_buf(),
            profile: profile.to_string(),
            name: localize(m.get("name").and_then(|x| x.as_str()).unwrap_or(&id), &ver_dir, dl),
            description: localize(m.get("description").and_then(|x| x.as_str()).unwrap_or(""), &ver_dir, dl),
            version: m.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            kind: kind.into(),
            enabled: !disabled,
            // 5 = component, 10 = external component: part of the browser
            builtin: matches!(location, 5 | 10),
            path: Some(e.path()),
            id,
        });
    }
    out
}

fn firefox_profiles(root: &Path) -> Vec<(PathBuf, String)> {
    let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let (mut name, mut path, mut relative) = (None::<String>, None::<String>, true);
    let mut flush = |name: &mut Option<String>, path: &mut Option<String>, relative: bool| {
        if let Some(p) = path.take() {
            let dir = if relative { root.join(&p) } else { PathBuf::from(&p) };
            if dir.is_dir() {
                out.push((dir, name.take().unwrap_or(p)));
            }
        }
        *name = None;
    };
    for line in ini.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            flush(&mut name, &mut path, relative);
            relative = true;
        } else if let Some(v) = l.strip_prefix("Name=") {
            name = Some(v.to_string());
        } else if let Some(v) = l.strip_prefix("Path=") {
            path = Some(v.to_string());
        } else if let Some(v) = l.strip_prefix("IsRelative=") {
            relative = v == "1";
        }
    }
    flush(&mut name, &mut path, relative);
    out
}

fn firefox_extensions(profile_dir: &Path, profile: &str) -> Vec<Extension> {
    let Some(v) = read_json(&profile_dir.join("extensions.json")) else {
        return Vec::new();
    };
    let Some(addons) = v.get("addons").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    let s = |a: &Value, p: &str| a.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    addons
        .iter()
        .filter(|a| s(a, "/type") != "locale")
        .map(|a| {
            let location = s(a, "/location");
            let kind = match s(a, "/type").as_str() {
                "theme" => "Theme",
                "dictionary" => "Dictionary",
                _ => "Extension",
            };
            Extension {
                browser: Browser::Firefox,
                profile_dir: profile_dir.to_path_buf(),
                profile: profile.to_string(),
                id: s(a, "/id"),
                name: Some(s(a, "/defaultLocale/name")).filter(|n| !n.is_empty()).unwrap_or_else(|| s(a, "/id")),
                version: s(a, "/version"),
                description: s(a, "/defaultLocale/description"),
                kind: kind.into(),
                enabled: a.get("active").and_then(|x| x.as_bool()).unwrap_or(false),
                builtin: location.starts_with("app-system") || location == "app-builtin" || location == "app-global" || location == "temporary-addon",
                path: Some(s(a, "/path")).filter(|p| !p.is_empty()).map(PathBuf::from),
            }
        })
        .collect()
}

/// Every extension of every profile of every installed browser.
pub fn list(include_builtin: bool) -> Vec<Extension> {
    let mut v = Vec::new();
    for (b, root) in chromium_roots() {
        for (dir, name) in chromium_profiles(b, &root) {
            v.extend(chromium_extensions(b, &dir, &name));
        }
    }
    for root in firefox_roots() {
        for (dir, name) in firefox_profiles(&root) {
            v.extend(firefox_extensions(&dir, &name));
        }
    }
    if !include_builtin {
        v.retain(|e| !e.builtin);
    }
    v.sort_by(|a, b| (a.browser.label(), &a.profile, a.name.to_lowercase()).cmp(&(b.browser.label(), &b.profile, b.name.to_lowercase())));
    v
}

/// Which of these browsers (repeats allowed) are running now, each listed once.
pub fn running_browsers(browsers: impl IntoIterator<Item = Browser>) -> Vec<Browser> {
    let mut seen: Vec<Browser> = Vec::new();
    for b in browsers {
        if !seen.contains(&b) {
            seen.push(b);
        }
    }
    if seen.is_empty() {
        return seen;
    }
    // One process listing for all of them.
    let procs = crate::platform::list_processes();
    seen.retain(|b| {
        let names = b.processes();
        procs.iter().any(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
    });
    seen
}

pub fn browser_running(b: Browser) -> bool {
    let names = b.processes();
    crate::platform::list_processes().iter().any(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
}

/// What removing `e` deletes and edits, as backed-up issues.
pub fn removal(e: &Extension) -> Result<Vec<Issue>> {
    if e.builtin {
        bail!("\"{}\" is part of {} and can't be removed", e.name, e.browser.label());
    }
    let mut v = Vec::new();
    let label = format!("{} › {} › {}", e.browser.label(), e.profile, e.name);
    let mut push_path = |p: PathBuf, what: &str| {
        if let Ok(m) = std::fs::symlink_metadata(&p) {
            let fix = if m.is_dir() { Fix::DeleteDirTree { path: p.clone() } } else { Fix::DeleteFile { path: p.clone() } };
            v.push(Issue::new("leftover_files", p.display().to_string(), format!("{what} of {label}"), fix));
        }
    };
    if e.browser == Browser::Firefox {
        if let Some(p) = &e.path {
            push_path(p.clone(), "extension package");
        }
        push_path(e.profile_dir.join("browser-extension-data").join(&e.id), "stored data");
        let file = e.profile_dir.join("extensions.json");
        v.push(Issue::new(
            "leftover_registry",
            format!("{} | {}", file.display(), e.id),
            format!("registration of {label}"),
            Fix::EditJson { file, remove: vec![format!("/addons[id={}]", e.id)] },
        ));
    } else {
        let p = &e.profile_dir;
        push_path(p.join("Extensions").join(&e.id), "extension files");
        for sub in ["Local Extension Settings", "Sync Extension Settings", "Managed Extension Settings"] {
            push_path(p.join(sub).join(&e.id), "stored data");
        }
        push_path(p.join("IndexedDB").join(format!("chrome-extension_{}_0.indexeddb.leveldb", e.id)), "database");
        push_path(p.join("IndexedDB").join(format!("chrome-extension_{}_0.indexeddb.blob", e.id)), "database");
        let paths = vec![
            format!("/extensions/settings/{}", e.id),
            format!("/protection/macs/extensions/settings/{}", e.id),
            format!("/protection/macs/extensions/settings_encrypted_hash/{}", e.id),
        ];
        for f in ["Preferences", "Secure Preferences"] {
            let file = p.join(f);
            if file.is_file() {
                v.push(Issue::new(
                    "leftover_registry",
                    format!("{} | {}", file.display(), e.id),
                    format!("registration of {label}"),
                    Fix::EditJson { file, remove: paths.clone() },
                ));
            }
        }
    }
    Ok(v)
}

/// Remove extensions (backed up). Refuses while their browser runs.
pub fn remove(settings: &crate::config::Settings, exts: &[Extension]) -> Result<crate::cleaner::CleanReport> {
    let mut issues = Vec::new();
    for b in exts.iter().map(|e| e.browser).collect::<std::collections::HashSet<_>>() {
        if browser_running(b) {
            bail!("{} is running. Close it and try again.", b.label());
        }
    }
    for e in exts {
        issues.extend(removal(e)?);
    }
    let mut s = settings.clone();
    s.restore_point_before_clean = false;
    s.full_backup_before_clean = false;
    // Browser data goes straight to the backup, not to the Trash.
    s.delete_to_trash = false;
    crate::cleaner::clean(&s, &issues)
}

/// Revo `/be "Ext1;Ext2" /chrome "Profile"`: find extensions by name, browser and profile.
pub fn find(names: &[String], browser: Option<Browser>, profile: Option<&str>) -> Result<Vec<Extension>> {
    let all = list(false);
    let mut out = Vec::new();
    for n in names {
        let hits: Vec<&Extension> = all
            .iter()
            .filter(|e| e.name.eq_ignore_ascii_case(n) || e.id == *n)
            .filter(|e| browser.is_none_or(|b| e.browser == b))
            .filter(|e| profile.is_none_or(|p| e.profile.eq_ignore_ascii_case(p) || e.profile_dir.file_name().is_some_and(|d| d.to_string_lossy().eq_ignore_ascii_case(p))))
            .collect();
        if hits.is_empty() {
            bail!("no extension named \"{n}\"{}", browser.map(|b| format!(" in {}", b.label())).unwrap_or_default());
        }
        out.extend(hits.into_iter().cloned());
    }
    Ok(out)
}

/// Every browser profile: (browser, profile folder, display name).
pub fn profiles() -> Vec<(Browser, PathBuf, String)> {
    let mut v: Vec<(Browser, PathBuf, String)> = Vec::new();
    for (b, root) in chromium_roots() {
        v.extend(chromium_profiles(b, &root).into_iter().map(|(d, n)| (b, d, n)));
    }
    for root in firefox_roots() {
        v.extend(firefox_profiles(&root).into_iter().map(|(d, n)| (Browser::Firefox, d, n)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localization() {
        let dir = std::env::temp_dir().join(format!("cp-ext-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("_locales/en")).unwrap();
        std::fs::write(dir.join("_locales/en/messages.json"), r#"{"appName":{"message":"My Ext"}}"#).unwrap();
        assert_eq!(localize("__MSG_appName__", &dir, Some("en")), "My Ext");
        assert_eq!(localize("Plain", &dir, None), "Plain");
        let _ = std::fs::remove_dir_all(dir);
    }
}
