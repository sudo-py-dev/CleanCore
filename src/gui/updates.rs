//! Settings › Updates, and the automatic checks: when CleanCore starts, then once a day while
//! it runs. Installing goes through `crate::update`, which verifies the release signature and
//! the download's checksum first.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::i18n::tr;
use crate::update::{self, Release};
use chrono::{DateTime, Local};
use eframe::egui::{self, Ui};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Time between automatic checks that worked.
const DAY: Duration = Duration::from_secs(24 * 3600);
/// Waits before trying again after an automatic check failed (offline, GitHub not answering);
/// the last one repeats.
const RETRY: [Duration; 4] = [Duration::from_secs(5 * 60), Duration::from_secs(30 * 60), Duration::from_secs(2 * 3600), Duration::from_secs(6 * 3600)];

/// What an automatic check found, and when it runs again if it failed.
struct AutoCheck {
    result: Result<Option<Release>, String>,
    retry: DateTime<Local>,
}

pub struct Updates {
    check: Option<Pending<Result<Option<Release>, String>>>,
    install: Option<Pending<Result<update::Installed, String>>>,
    /// A newer release, once found.
    pub available: Option<Release>,
    /// The last check found nothing newer.
    up_to_date: bool,
    /// Finished installing: restart to use it.
    installed: bool,
    /// Results of the automatic checks, from their thread.
    auto: Option<Receiver<AutoCheck>>,
    /// The setting, read by that thread: switching it off pauses the checks.
    auto_on: Arc<AtomicBool>,
    /// The last automatic check failed: why, and when it is tried again.
    auto_failed: Option<(String, DateTime<Local>)>,
}

impl Updates {
    pub fn new() -> Self {
        Self { check: None, install: None, available: None, up_to_date: false, installed: false, auto: None, auto_on: Arc::new(AtomicBool::new(false)), auto_failed: None }
    }

    /// Start the automatic checks: one now, then one a day after each that worked, sooner
    /// after one that failed.
    pub fn startup(&mut self, ctx: &egui::Context, settings: &crate::config::Settings) {
        // Development builds (and the test runs) never reach out to GitHub on their own.
        if cfg!(debug_assertions) {
            return;
        }
        self.auto_on.store(settings.check_updates, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        self.auto = Some(rx);
        let (ctx, on) = (ctx.clone(), self.auto_on.clone());
        std::thread::spawn(move || auto_checks(&ctx, &on, &tx));
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        self.up_to_date = false;
        self.check = Some(Pending::spawn(ctx, "Checking for updates", || update::check().map_err(|e| crate::tf!("{e:#}"))));
    }

    /// A check worked: remember when, and tell about a release not told about yet.
    fn found(&mut self, cx: &mut Cx, release: Option<Release>) {
        cx.settings.last_update_check = Some(Local::now());
        cx.save_settings();
        self.auto_failed = None;
        match release {
            Some(rel) => {
                if self.available.as_ref().is_none_or(|r| r.version != rel.version) {
                    cx.info(crate::tf!("CleanCore {} is available. Install it from Settings › Updates.", rel.version));
                }
                self.available = Some(rel);
            }
            None => self.up_to_date = true,
        }
    }

    /// Collect finished background work; called every frame, whatever page is open.
    pub fn poll(&mut self, cx: &mut Cx) {
        self.auto_on.store(cx.settings.check_updates, Ordering::Relaxed);
        let results: Vec<AutoCheck> = self.auto.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for a in results {
            match a.result {
                Ok(release) => self.found(cx, release),
                // Not worth an error (offline, say): the card says when it is tried again.
                Err(e) => self.auto_failed = Some((e, a.retry)),
            }
        }
        if let Some(r) = take(&mut self.check) {
            match r {
                Ok(release) => self.found(cx, release),
                Err(e) => cx.error(crate::tf!("Checking for updates failed: {e}")),
            }
        }
        if let Some(r) = take(&mut self.install) {
            match r {
                // The new version is in place: start it and close this copy.
                Ok(update::Installed::InPlace) => match update::relaunch() {
                    Ok(()) => std::process::exit(0),
                    Err(e) => {
                        self.installed = true;
                        cx.error(crate::tf!("Updated, but CleanCore could not start again: {e:#}. Restart it to use the new version."));
                    }
                },
                // The setup replaces the files once CleanCore is closed, then starts it again.
                Ok(update::Installed::BySetup) => std::process::exit(0),
                Err(e) => cx.error(crate::tf!("The update was not installed: {e}")),
            }
        }
    }

    /// The Updates card on the Settings page.
    pub fn card(&mut self, ui: &mut Ui, cx: &mut Cx, check_updates: &mut bool) {
        let p = Palette::get(ui.ctx());
        style::card(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                crate::gui::style::column(ui, |ui| {
                    ui.add(style::Label::new(crate::tf!("CleanCore {}", update::CURRENT)));
                    let (text, col) = if self.installed {
                        (tr("Update installed. Restart CleanCore to use it.").to_string(), p.success)
                    } else if let Some(r) = &self.available {
                        (crate::tf!("Version {} is available.", r.version), p.accent)
                    } else if self.up_to_date {
                        (tr("You have the latest version.").to_string(), p.success)
                    } else if let Some((_, retry)) = &self.auto_failed {
                        (crate::tf!("Could not check for updates. Trying again at {}.", retry.format("%H:%M")), p.muted)
                    } else {
                        (tr("Updates come from the project's GitHub releases.").to_string(), p.muted)
                    };
                    let r = ui.add(style::Label::new(style::rich(text).size(12.5).color(col)));
                    if let (Some((why, _)), None, false) = (&self.auto_failed, &self.available, self.up_to_date) {
                        r.on_hover_text(style::vis(why.as_str()));
                    }
                });
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    let busy = self.check.is_some() || self.install.is_some();
                    if let (Some(r), false) = (&self.available, self.installed) {
                        if ui.add_enabled_ui(!busy, |ui| style::primary(ui, icon::DOWNLOAD, &crate::tf!("Install {}", r.version))).inner.clicked() {
                            let r = r.clone();
                            self.install = Some(Pending::spawn(ui.ctx(), "Downloading and verifying the update", move || {
                                let file = update::download(&r).map_err(|e| crate::tf!("{e:#}"))?;
                                update::install(&file).map_err(|e| crate::tf!("{e:#}"))
                            }));
                        }
                        if !r.url.is_empty() && style::ghost(ui, icon::LINK, "What's new").clicked() {
                            if let Err(e) = crate::platform::open_folder(std::path::Path::new(&r.url)) {
                                cx.error(crate::tf!("{e:#}"));
                            }
                        }
                    } else if ui.add_enabled_ui(!busy, |ui| style::secondary(ui, icon::REFRESH, "Check for updates")).inner.clicked() {
                        self.start_check(ui.ctx());
                    }
                });
            });
            if let Some(p) = self.check.as_ref() {
                p.show_progress(ui);
            }
            if let Some(p) = &self.install {
                p.show_progress(ui);
            }
            style::divider(ui);
            style::setting_row(
                ui,
                "Check for updates automatically",
                "When CleanCore starts, then once a day while it runs. Updates are installed only when you choose to.",
                |ui| {
                    style::toggle(ui, check_updates, "Check for updates automatically");
                },
            );
            ui.add(style::Label::new(
                style::rich(crate::tf!("{}  Every update is checked against the CleanCore release signature and its checksum before it is installed.", icon::SHIELD_CHECK))
                    .size(12.0)
                    .color(p.muted),
            ));
        });
    }
}

/// The automatic checks, on their own thread for as long as CleanCore runs (also from the tray,
/// when no frames are drawn). Waits are wall-clock time, so a computer that slept still checks
/// once it wakes.
fn auto_checks(ctx: &egui::Context, on: &AtomicBool, tx: &Sender<AutoCheck>) {
    let mut next = SystemTime::now();
    let mut failures = 0;
    let mut told: Option<String> = None;
    let mut first = true;
    loop {
        if on.load(Ordering::Relaxed) && SystemTime::now() >= next {
            let result = update::check().map_err(|e| crate::tf!("{e:#}"));
            let wait = match &result {
                Ok(found) => {
                    failures = 0;
                    // A desktop notification for a release found later on, when the window may
                    // be closed to the tray (at start-up the window says it).
                    if let Some(r) = found.as_ref().filter(|r| !first && told.as_deref() != Some(r.version.as_str())) {
                        let _ = crate::platform::notify("CleanCore", &crate::tf!("CleanCore {} is available. Install it from Settings › Updates.", r.version));
                        told = Some(r.version.clone());
                    }
                    DAY
                }
                Err(_) => {
                    failures += 1;
                    RETRY[(failures - 1).min(RETRY.len() - 1)]
                }
            };
            first = false;
            next = SystemTime::now() + wait;
            let retry = DateTime::<Local>::from(next);
            if tx.send(AutoCheck { result, retry }).is_err() {
                return;
            }
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_secs(60));
    }
}
