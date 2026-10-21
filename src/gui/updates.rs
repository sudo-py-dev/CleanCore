//! Settings › Updates, and the once-a-day check at start-up. Installing goes through
//! `crate::update`, which verifies the release signature and the download's checksum first.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::update::{self, Release};
use eframe::egui::{self, RichText, Ui};

pub struct Updates {
    check: Option<Pending<Result<Option<Release>, String>>>,
    install: Option<Pending<Result<String, String>>>,
    /// A newer release, once found.
    pub available: Option<Release>,
    /// The last check found nothing newer.
    up_to_date: bool,
    /// Finished installing: restart to use it.
    installed: bool,
    /// The check was started by the user (so "up to date" is worth saying).
    manual: bool,
}

impl Updates {
    pub fn new() -> Self {
        Self { check: None, install: None, available: None, up_to_date: false, installed: false, manual: false }
    }

    /// Look for an update in the background if the setting is on and a day has passed.
    pub fn startup(&mut self, ctx: &egui::Context, settings: &mut crate::config::Settings) {
        // Development builds (and the test runs) never reach out to GitHub on their own.
        if cfg!(debug_assertions) {
            return;
        }
        let due = settings.last_update_check.is_none_or(|t| chrono::Local::now() - t > chrono::Duration::hours(24));
        if settings.check_updates && due {
            settings.last_update_check = Some(chrono::Local::now());
            let _ = settings.save();
            self.start_check(ctx, false);
        }
    }

    fn start_check(&mut self, ctx: &egui::Context, manual: bool) {
        self.manual = manual;
        self.up_to_date = false;
        self.check = Some(Pending::spawn(ctx, "Checking for updates", || update::check().map_err(|e| crate::tf!("{e:#}"))));
    }

    /// Collect finished background work; called every frame, whatever page is open.
    pub fn poll(&mut self, cx: &mut Cx) {
        if let Some(r) = take(&mut self.check) {
            match r {
                Ok(Some(rel)) => {
                    cx.info(crate::tf!("CleanCore {} is available. Install it from Settings › Updates.", rel.version));
                    self.available = Some(rel);
                }
                Ok(None) => self.up_to_date = true,
                // A failed automatic check (offline, say) is not worth an error.
                Err(e) if self.manual => cx.error(crate::tf!("Checking for updates failed: {e}")),
                Err(_) => {}
            }
        }
        if let Some(r) = take(&mut self.install) {
            match r {
                Ok(msg) => {
                    self.installed = true;
                    cx.info(msg);
                }
                Err(e) => cx.error(crate::tf!("The update was not installed: {e}")),
            }
        }
    }

    /// The Updates card on the Settings page.
    pub fn card(&mut self, ui: &mut Ui, cx: &mut Cx, check_updates: &mut bool) {
        let p = Palette::get(ui.ctx());
        style::card(ui, |ui| {
            style::section_title(ui, "Updates");
            crate::gui::style::row(ui, |ui| {
                crate::gui::style::column(ui, |ui| {
                    ui.label(crate::tf!("CleanCore {}", update::CURRENT));
                    let (text, col) = if self.installed {
                        ("Update installed. Restart CleanCore to use it.".to_string(), p.success)
                    } else if let Some(r) = &self.available {
                        (crate::tf!("Version {} is available.", r.version), p.accent)
                    } else if self.up_to_date {
                        ("You have the latest version.".to_string(), p.success)
                    } else {
                        ("Updates come from the project's GitHub releases.".to_string(), p.muted)
                    };
                    ui.label(RichText::new(text).size(12.5).color(col));
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
                        self.start_check(ui.ctx(), true);
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
            style::setting_row(ui, "Check for updates automatically", "Once a day, when CleanCore starts. Updates are installed only when you choose to.", |ui| {
                style::toggle(ui, check_updates, "Check for updates automatically");
            });
            ui.label(
                RichText::new(crate::tf!("{}  Every update is checked against the CleanCore release signature and its checksum before it is installed.", icon::SHIELD_CHECK))
                    .size(12.0)
                    .color(p.muted),
            );
        });
    }
}
