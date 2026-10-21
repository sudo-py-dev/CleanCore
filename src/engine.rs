//! Platform-neutral scan engine: runs every selected check in parallel and applies filters.

use crate::config::Settings;
use crate::model::{CheckInfo, Issue, ScanMode, Scope};
use crate::pathutil::PathCache;
use rayon::prelude::*;
use std::collections::HashSet;

/// Shared state handed to every check.
pub struct ScanContext<'a> {
    pub scope: Scope,
    pub mode: ScanMode,
    pub settings: &'a Settings,
    pub paths: PathCache,
    /// Free-text filter used by the forced scan.
    pub needle: Option<String>,
}

impl ScanContext<'_> {
    /// Does a referenced file exist? Paths on a drive/mount that is not present right now count
    /// as existing when "exclude external drives" is on, so data of unplugged devices is kept.
    pub fn target_exists(&self, path: &str) -> bool {
        crate::platform::target_exists(self, path)
    }
}

pub trait Check: Sync {
    fn info(&self) -> CheckInfo;
    fn scan(&self, ctx: &ScanContext) -> Vec<Issue>;
}

/// What to run.
pub enum Selection {
    Mode(ScanMode),
    /// Managed scan: explicit check ids.
    Ids(Vec<String>),
}

pub fn run_scan(settings: &Settings, selection: &Selection, scope: Scope) -> Vec<Issue> {
    run_scan_with_progress(settings, selection, scope, &|_| {})
}

/// The check ids a selection runs, in the order `all_checks` lists them.
pub fn selected_ids(selection: &Selection) -> Vec<&'static str> {
    all_checks()
        .iter()
        .filter(|c| match selection {
            Selection::Mode(m) => c.min_mode <= *m && c.id != "include_list",
            Selection::Ids(ids) => ids.iter().any(|i| i == c.id),
        })
        .map(|c| c.id)
        .collect()
}

/// Like `run_scan`, calling `done(check id)` as each check finishes (from worker threads).
pub fn run_scan_with_progress(settings: &Settings, selection: &Selection, scope: Scope, done: &(dyn Fn(&'static str) + Sync)) -> Vec<Issue> {
    let checks = crate::platform::checks();
    let mode = match selection {
        Selection::Mode(m) => *m,
        Selection::Ids(_) => ScanMode::Advanced,
    };
    let ctx = ScanContext { scope, mode, settings, paths: PathCache::default(), needle: None };
    let selected: Vec<&dyn Check> = checks
        .iter()
        .map(|c| c.as_ref())
        .filter(|c| match selection {
            Selection::Mode(m) => c.info().min_mode <= *m && c.info().id != "include_list",
            Selection::Ids(ids) => ids.iter().any(|i| i == c.info().id),
        })
        .collect();

    let mut issues: Vec<Issue> = selected
        .par_iter()
        .flat_map_iter(|c| {
            let found = c.scan(&ctx);
            done(c.info().id);
            found
        })
        .collect();
    finish(settings, &mut issues);
    issues
}

/// Forced scan: every key/value/file under `root` whose path or data contains `needle`.
pub fn run_forced_scan(settings: &Settings, root: &str, needle: &str) -> anyhow::Result<Vec<Issue>> {
    let ctx = ScanContext { scope: Scope::CurrentUser, mode: ScanMode::Advanced, settings, paths: PathCache::default(), needle: Some(needle.to_string()) };
    let mut issues = crate::platform::forced_scan(&ctx, root, needle)?;
    finish(settings, &mut issues);
    Ok(issues)
}

fn finish(settings: &Settings, issues: &mut Vec<Issue>) {
    issues.retain(|i| !settings.is_excluded(&i.location));
    if settings.exclude_os_defaults {
        issues.retain(|i| !crate::platform::is_os_default(&i.location));
    }
    // Drop repeated fixes, keeping the first; compared by reference, nothing cloned.
    let keep: Vec<bool> = {
        let mut seen = HashSet::with_capacity(issues.len());
        issues.iter().map(|i| seen.insert(&i.fix)).collect()
    };
    let mut keep = keep.into_iter();
    issues.retain(|_| keep.next().unwrap_or(true));
    issues.sort_by(|a, b| a.category.cmp(b.category).then_with(|| a.location.cmp(&b.location)));
}

/// Every check's description, built once.
pub fn all_checks() -> &'static [CheckInfo] {
    static ALL: std::sync::OnceLock<Vec<CheckInfo>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| crate::platform::checks().iter().map(|c| c.info()).collect())
}
