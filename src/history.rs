//! History Cleaner: browser history, caches, cookies and
//! sessions; the system's recently-used lists, temporary files, thumbnails, crash dumps,
//! Trash and clipboard; Office / LibreOffice recent documents.

use crate::config::Settings;
use crate::extensions::{self, Browser};
use crate::model::Fix;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What cleaning an item does.
#[derive(Debug, Clone, Serialize)]
pub enum Target {
    /// Delete a file or folder tree.
    Path(PathBuf),
    /// Delete everything inside a folder, keeping the folder.
    Contents(PathBuf),
    /// Run SQL on an SQLite database (Firefox keeps history and bookmarks in one file).
    Sql {
        db: PathBuf,
        statements: Vec<String>,
    },
    /// Registry keys/values (Windows), backed up first.
    Fix(Fix),
    /// Empty the Recycle Bin / Trash.
    EmptyTrash,
    Clipboard,
    /// Replace the file with this content (e.g. an empty `recently-used.xbel`).
    Rewrite {
        file: PathBuf,
        content: String,
    },
    /// Remove the lines containing this text (LibreOffice recent documents).
    DropLines {
        file: PathBuf,
        containing: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    /// Stable id, e.g. `browser:chrome:Default:cache` or `system:thumbnails`.
    pub id: String,
    /// Section heading: "Google Chrome › Work", "System", "Office".
    pub group: String,
    pub name: String,
    pub description: String,
    pub size: u64,
    /// Ticked when nothing was chosen before (cookies and sessions are opt-in).
    pub default_on: bool,
    /// The browser that must be closed first.
    pub browser: Option<Browser>,
    pub targets: Vec<Target>,
}

use crate::backup::dir_size;

fn target_size(t: &Target) -> u64 {
    match t {
        Target::Path(p) | Target::Contents(p) => dir_size(p),
        Target::Sql { db, .. } => dir_size(db),
        Target::Rewrite { file, .. } | Target::DropLines { file, .. } => dir_size(file),
        _ => 0,
    }
}

impl HistoryItem {
    pub fn new(id: impl Into<String>, group: impl Into<String>, name: impl Into<String>, description: impl Into<String>, default_on: bool, targets: Vec<Target>) -> Self {
        Self { id: id.into(), group: group.into(), name: name.into(), description: description.into(), size: 0, default_on, browser: None, targets }
    }

    fn present(&self) -> bool {
        self.targets.iter().any(|t| match t {
            Target::Path(p) | Target::Contents(p) => p.exists(),
            Target::Sql { db, .. } => db.is_file(),
            Target::Rewrite { file, .. } | Target::DropLines { file, .. } => file.is_file(),
            _ => true,
        })
    }
}

// ---------------------------------------------------------------- browsers

/// Where a Chromium profile's caches live: next to it on Windows, under ~/.cache on Linux.
fn chromium_cache_dir(profile: &Path) -> PathBuf {
    if cfg!(windows) {
        return profile.to_path_buf();
    }
    let home = dirs::home_dir().unwrap_or_default();
    match profile.strip_prefix(home.join(".config")) {
        Ok(rel) => home.join(".cache").join(rel),
        Err(_) => profile.to_path_buf(),
    }
}

fn firefox_cache_dir(profile: &Path) -> PathBuf {
    if cfg!(windows) {
        // %APPDATA%\Mozilla\Firefox\Profiles\x → %LOCALAPPDATA%\Mozilla\Firefox\Profiles\x
        if let (Some(roaming), Some(local)) = (dirs::config_dir(), dirs::cache_dir()) {
            if let Ok(rel) = profile.strip_prefix(&roaming) {
                return local.join(rel);
            }
        }
        return profile.to_path_buf();
    }
    let home = dirs::home_dir().unwrap_or_default();
    match profile.strip_prefix(home.join(".mozilla")) {
        Ok(rel) => home.join(".cache/mozilla").join(rel),
        Err(_) => profile.to_path_buf(),
    }
}

fn browser_items(b: Browser, profile: &Path, name: &str) -> Vec<HistoryItem> {
    let group = format!("{} › {name}", b.label());
    let id = |k: &str| format!("browser:{}:{}:{k}", b.flag(), profile.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
    let mut v = Vec::new();
    if b == Browser::Firefox {
        let cache = firefox_cache_dir(profile);
        v.push(HistoryItem::new(
            id("cache"),
            &group,
            "Cache",
            "Downloaded copies of web pages and images",
            true,
            vec![Target::Path(cache.join("cache2")), Target::Path(cache.join("startupCache")), Target::Path(cache.join("thumbnails"))],
        ));
        v.push(HistoryItem::new(
            id("history"),
            &group,
            "Browsing and download history",
            "Visited pages and downloads; bookmarks are kept",
            true,
            vec![Target::Sql {
                db: profile.join("places.sqlite"),
                statements: [
                    "DELETE FROM moz_historyvisits",
                    "DELETE FROM moz_inputhistory",
                    "DELETE FROM moz_annos",
                    "DELETE FROM moz_places WHERE id NOT IN (SELECT fk FROM moz_bookmarks WHERE fk IS NOT NULL)",
                    "UPDATE moz_places SET visit_count = 0, last_visit_date = NULL, frecency = 0",
                ]
                .map(String::from)
                .to_vec(),
            }],
        ));
        v.push(HistoryItem::new(
            id("forms"),
            &group,
            "Form and search history",
            "Text typed into forms and the search bar",
            true,
            vec![Target::Path(profile.join("formhistory.sqlite"))],
        ));
        v.push(HistoryItem::new(
            id("cookies"),
            &group,
            "Cookies",
            "Signs you out of websites",
            false,
            vec![Target::Path(profile.join("cookies.sqlite")), Target::Path(profile.join("cookies.sqlite-wal"))],
        ));
        v.push(HistoryItem::new(
            id("session"),
            &group,
            "Session",
            "Open tabs restored at the next start",
            false,
            vec![Target::Path(profile.join("sessionstore.jsonlz4")), Target::Path(profile.join("sessionstore-backups"))],
        ));
    } else {
        let cache = chromium_cache_dir(profile);
        v.push(HistoryItem::new(
            id("cache"),
            &group,
            "Cache",
            "Downloaded copies of web pages and images",
            true,
            vec![
                Target::Path(cache.join("Cache")),
                Target::Path(cache.join("Code Cache")),
                Target::Path(profile.join("GPUCache")),
                Target::Path(profile.join("Service Worker/CacheStorage")),
            ],
        ));
        v.push(HistoryItem::new(
            id("history"),
            &group,
            "Browsing and download history",
            "Visited pages, downloads, top sites and typed addresses",
            true,
            [
                "History",
                "History-journal",
                "Visited Links",
                "Top Sites",
                "Top Sites-journal",
                "Shortcuts",
                "Shortcuts-journal",
                "Network Action Predictor",
                "Network Action Predictor-journal",
            ]
            .iter()
            .map(|f| Target::Path(profile.join(f)))
            .collect(),
        ));
        v.push(HistoryItem::new(
            id("forms"),
            &group,
            "Form data",
            "Text typed into forms (autofill)",
            true,
            vec![Target::Path(profile.join("Web Data")), Target::Path(profile.join("Web Data-journal"))],
        ));
        v.push(HistoryItem::new(
            id("cookies"),
            &group,
            "Cookies",
            "Signs you out of websites",
            false,
            vec![
                Target::Path(profile.join("Cookies")),
                Target::Path(profile.join("Cookies-journal")),
                Target::Path(profile.join("Network/Cookies")),
                Target::Path(profile.join("Network/Cookies-journal")),
            ],
        ));
        v.push(HistoryItem::new(
            id("session"),
            &group,
            "Session",
            "Open tabs restored at the next start",
            false,
            vec![
                Target::Path(profile.join("Sessions")),
                Target::Path(profile.join("Current Session")),
                Target::Path(profile.join("Current Tabs")),
                Target::Path(profile.join("Last Session")),
                Target::Path(profile.join("Last Tabs")),
            ],
        ));
    }
    for i in &mut v {
        i.browser = Some(b);
    }
    v
}

/// Every item that exists on this system, with its size.
pub fn items() -> Vec<HistoryItem> {
    let mut v = Vec::new();
    for (b, dir, name) in extensions::profiles() {
        v.extend(browser_items(b, &dir, &name));
    }
    v.extend(crate::platform::system_history_items());
    v.retain(|i| i.present());
    use rayon::prelude::*;
    v.par_iter_mut().for_each(|i| i.size = i.targets.iter().map(target_size).sum());
    v
}

/// Ids ticked by default: the user's last choice, or each item's default.
pub fn selected_by_default(settings: &Settings, items: &[HistoryItem]) -> Vec<String> {
    if settings.history_selected.is_empty() {
        items.iter().filter(|i| i.default_on).map(|i| i.id.clone()).collect()
    } else {
        items.iter().filter(|i| settings.history_selected.contains(&i.id)).map(|i| i.id.clone()).collect()
    }
}

#[derive(Debug, Default, Serialize)]
pub struct CleanReport {
    pub cleaned: usize,
    pub bytes: u64,
    /// Items left alone because their browser is open.
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
}

fn delete_contents(dir: &Path) -> Result<()> {
    let mut first_err = None;
    for e in std::fs::read_dir(dir)?.flatten() {
        // Files in use (Windows temp) are skipped, not fatal.
        if let Err(err) = crate::platform::files::delete_path(&e.path(), false) {
            first_err.get_or_insert(err);
        }
    }
    match first_err {
        Some(e) if cfg!(not(windows)) => Err(e),
        _ => Ok(()),
    }
}

fn run_sql(db: &Path, statements: &[String]) -> Result<()> {
    let conn = rusqlite::Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    for s in statements {
        conn.execute(s, []).with_context(|| format!("{s} on {}", db.display()))?;
    }
    conn.execute_batch("VACUUM")?;
    Ok(())
}

fn apply(t: &Target, settings: &Settings) -> Result<()> {
    match t {
        Target::Path(p) => {
            if std::fs::symlink_metadata(p).is_ok() {
                crate::platform::files::delete_path(p, false)?;
            }
            Ok(())
        }
        Target::Contents(p) if p.is_dir() => delete_contents(p),
        Target::Contents(_) => Ok(()),
        Target::Sql { db, statements } => run_sql(db, statements),
        Target::Fix(f) => {
            let issue = crate::model::Issue::new("history", "history", "history", f.clone());
            let mut s = settings.clone();
            s.restore_point_before_clean = false;
            s.full_backup_before_clean = false;
            let rep = crate::cleaner::clean(&s, &[issue])?;
            match rep.failed.first() {
                Some((_, e)) => anyhow::bail!("{e}"),
                None => Ok(()),
            }
        }
        Target::EmptyTrash => crate::platform::empty_trash(),
        Target::Clipboard => crate::platform::clear_clipboard(),
        Target::Rewrite { file, content } => Ok(std::fs::write(file, content)?),
        Target::DropLines { file, containing } => {
            let text = std::fs::read_to_string(file)?;
            let kept: String = text.lines().filter(|l| !l.contains(containing.as_str())).map(|l| format!("{l}\n")).collect();
            Ok(std::fs::write(file, kept)?)
        }
    }
}

/// Clean the chosen items. Items of a running browser are skipped (its files are locked or
/// would be written back on exit).
pub fn clean(settings: &Settings, items: &[HistoryItem]) -> CleanReport {
    let mut r = CleanReport::default();
    let mut running: Vec<(Browser, bool)> = Vec::new();
    for i in items {
        if let Some(b) = i.browser {
            let open = match running.iter().find(|(x, _)| *x == b) {
                Some((_, o)) => *o,
                None => {
                    let o = extensions::browser_running(b);
                    running.push((b, o));
                    o
                }
            };
            if open {
                r.skipped.push(format!("{} › {}", i.group, i.name));
                continue;
            }
        }
        let errs: Vec<String> = i.targets.iter().filter_map(|t| apply(t, settings).err()).map(|e| format!("{e:#}")).collect();
        if errs.is_empty() {
            r.cleaned += 1;
            r.bytes += i.size;
        } else {
            r.failed.push((format!("{} › {}", i.group, i.name), errs.join("; ")));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_history_keeps_bookmarks() {
        let dir = std::env::temp_dir().join(format!("cleancore-places-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("places.sqlite");
        let _ = std::fs::remove_file(&db);
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, visit_count INT, last_visit_date INT, frecency INT);
             CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, place_id INT);
             CREATE TABLE moz_inputhistory(place_id INT, input TEXT);
             CREATE TABLE moz_annos(id INTEGER PRIMARY KEY);
             CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, fk INT);
             INSERT INTO moz_places VALUES(1,'https://kept.example',3,1,100),(2,'https://gone.example',5,1,100);
             INSERT INTO moz_historyvisits VALUES(1,1),(2,2);
             INSERT INTO moz_bookmarks VALUES(1,1),(2,NULL);",
        )
        .unwrap();
        drop(c);
        let item = browser_items(Browser::Firefox, &dir, "t").into_iter().find(|i| i.name.starts_with("Browsing")).unwrap();
        for t in &item.targets {
            apply(t, &Settings::default()).unwrap();
        }
        let c = rusqlite::Connection::open(&db).unwrap();
        let urls: Vec<String> = c.prepare("SELECT url FROM moz_places").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(urls, ["https://kept.example"]);
        let visits: i64 = c.query_row("SELECT count(*) FROM moz_historyvisits", [], |r| r.get(0)).unwrap();
        assert_eq!(visits, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
