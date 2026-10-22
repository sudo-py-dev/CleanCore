//! Uninstaller › Browser extensions (Revo: Browser Extensions): every extension of every
//! profile of the installed browsers, grouped by browser and profile, removable in batches.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::cleaner::CleanReport;
use crate::extensions::{self as ext, Browser, Extension};
use crate::i18n::tr;
use eframe::egui::{self, Ui};
use std::collections::BTreeSet;

pub struct ExtensionsTab {
    list: Vec<Extension>,
    loading: Option<Pending<Vec<Extension>>>,
    loaded: bool,
    search: String,
    browser: Option<Browser>,
    show_builtin: bool,
    checked: BTreeSet<String>,
    confirm: bool,
    /// Browsers seen running at the last check (checked on load and before removing).
    running: Vec<Browser>,
    removing: Option<Pending<Result<CleanReport, String>>>,
}

fn key(e: &Extension) -> String {
    crate::tf!("{}|{}", e.profile_dir.display(), e.id)
}

impl ExtensionsTab {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            loading: None,
            loaded: false,
            search: String::new(),
            browser: None,
            show_builtin: false,
            checked: BTreeSet::new(),
            confirm: false,
            running: Vec::new(),
            removing: None,
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    pub fn count(&self) -> usize {
        self.list.iter().filter(|e| !e.builtin).count()
    }

    fn load(&mut self, ctx: &egui::Context) {
        if self.loading.is_none() {
            let builtin = self.show_builtin;
            self.loading = Some(Pending::spawn(ctx, "Reading browser extensions", move || ext::list(builtin)));
        }
        self.loaded = true;
    }

    fn check_running(&mut self) {
        self.running = ext::running_browsers(self.list.iter().map(|e| e.browser));
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !self.loaded {
            self.load(ui.ctx());
        }
        if let Some(l) = take(&mut self.loading) {
            self.list = l;
            self.check_running();
            let keys: BTreeSet<String> = self.list.iter().map(key).collect();
            self.checked.retain(|k| keys.contains(k));
        }
        if let Some(r) = take(&mut self.removing) {
            match r {
                Ok(rep) if rep.failed.is_empty() => cx.info(crate::tf!("Removed {} item(s). Backup: {}", rep.cleaned, rep.backup_id)),
                Ok(rep) => cx.error(crate::tf!("{} item(s) could not be removed: {}", rep.failed.len(), rep.failed[0].1)),
                Err(e) => cx.error(e),
            }
            self.checked.clear();
            self.loaded = false;
        }
        let p = Palette::get(ui.ctx());

        // Toolbar
        let browsers: Vec<Browser> = {
            let mut b: Vec<Browser> = Vec::new();
            for e in &self.list {
                if !b.contains(&e.browser) {
                    b.push(e.browser);
                }
            }
            b
        };
        let selected: Vec<Extension> = self.list.iter().filter(|e| self.checked.contains(&key(e))).cloned().collect();
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, "Search extensions", 200.0);
            ui.add_space(6.0);
            if style::segment(ui, self.browser.is_none(), "All browsers").clicked() {
                self.browser = None;
            }
            for b in &browsers {
                let n = self.list.iter().filter(|e| e.browser == *b).count();
                if style::segment(ui, self.browser == Some(*b), &crate::tf!("{}  {n}", b.label())).clicked() {
                    self.browser = Some(*b);
                }
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!selected.is_empty() && self.removing.is_none(), |ui| {
                    if style::primary(ui, icon::TRASH, &crate::tf!("Remove ({})", selected.len())).clicked() {
                        self.confirm = true;
                        self.check_running();
                    }
                });
                if style::icon_button(ui, icon::REFRESH, "Refresh").clicked() {
                    self.loaded = false;
                }
                if style::toggle(ui, &mut self.show_builtin, "Show built-in extensions").changed() {
                    self.loaded = false;
                }
                ui.add(style::Label::new(style::muted(ui, tr("Built-in"))));
            });
        });
        ui.add_space(8.0);
        if let Some(l) = &self.loading {
            l.show_progress(ui);
        }
        if let Some(r) = &self.removing {
            r.show_progress(ui);
        }

        let q = self.search.to_lowercase();
        let shown: Vec<&Extension> = self
            .list
            .iter()
            .filter(|e| self.browser.is_none_or(|b| e.browser == b))
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.id.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q))
            .collect();
        if shown.is_empty() && self.loading.is_none() {
            style::empty_state(
                ui,
                icon::GLOBE,
                "No extensions",
                if self.list.is_empty() { tr("No extensions were found in Chrome, Edge, Brave, Opera, Vivaldi, Chromium or Firefox.") } else { tr("Nothing matches the search.") },
            );
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last: Option<(Browser, String)> = None;
            for e in shown {
                let group = (e.browser, e.profile.clone());
                if last.as_ref() != Some(&group) {
                    if last.is_some() {
                        ui.add_space(10.0);
                    }
                    crate::gui::style::row(ui, |ui| {
                        ui.add(style::Label::new(style::rich(icon::GLOBE).size(16.0).color(p.accent)));
                        ui.add(style::Label::new(style::strong(crate::tf!("{}  ›  {}", e.browser.label(), e.profile))));
                        if self.running.contains(&e.browser) {
                            style::badge(ui, "Running: close it before removing", p.warning);
                        }
                    });
                    ui.add_space(4.0);
                    last = Some(group);
                }
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        let k = key(e);
                        let mut on = self.checked.contains(&k);
                        ui.add_enabled_ui(!e.builtin, |ui| {
                            if ui.checkbox(&mut on, style::vis("")).changed() {
                                if on {
                                    self.checked.insert(k.clone());
                                } else {
                                    self.checked.remove(&k);
                                }
                            }
                        });
                        style::icon_tile(ui, if e.kind == "Theme" { icon::PALETTE } else { icon::CUBE }, if e.enabled { p.accent } else { p.faint }, 32.0);
                        crate::gui::style::column(ui, |ui| {
                            crate::gui::style::row(ui, |ui| {
                                ui.add(style::Label::new(style::strong(&e.name)));
                                ui.add(style::Label::new(style::muted(ui, &e.version).size(12.5)));
                                if e.kind != "Extension" {
                                    style::badge(ui, &e.kind, p.accent);
                                }
                                if !e.enabled {
                                    style::badge(ui, "Disabled", p.muted);
                                }
                                if e.builtin {
                                    style::badge(ui, "Built-in", p.muted);
                                }
                            });
                            let sub = if e.description.is_empty() { e.id.clone() } else { e.description.clone() };
                            ui.add(style::Label::new(style::muted(ui, sub).size(12.5)).truncate());
                        });
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            if let Some(path) = &e.path {
                                if style::icon_button(ui, icon::FOLDER_OPEN, "Open folder").clicked() {
                                    let dir = if path.is_dir() { path.clone() } else { path.parent().map(|p| p.to_path_buf()).unwrap_or(path.clone()) };
                                    if let Err(err) = crate::platform::open_folder(&dir) {
                                        cx.error(crate::tf!("{err:#}"));
                                    }
                                }
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });

        if self.confirm {
            let running: Vec<&str> = self.running.iter().filter(|b| selected.iter().any(|e| e.browser == **b)).map(|b| b.label()).collect();
            let (mut close, mut recheck) = (false, false);
            egui::Modal::new(egui::Id::unique("ext-confirm")).show(ui.ctx(), |ui| {
                ui.set_width(440.0);
                ui.add(style::Label::new(style::strong(crate::tf!("Remove {} extension(s)?", selected.len())).size(18.0)));
                ui.add_space(6.0);
                for e in selected.iter().take(8) {
                    ui.add(style::Label::new(crate::tf!("• {} ({} › {})", e.name, e.browser.label(), e.profile)));
                }
                if selected.len() > 8 {
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("and {} more", selected.len() - 8))));
                }
                ui.add_space(6.0);
                ui.add(style::Label::new(style::muted(ui, tr("Their files, stored data and browser settings entries are removed. Everything is backed up first."))));
                if !running.is_empty() {
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::rich(crate::tf!("{}  Close {} first.", icon::WARNING, running.join(", "))).color(p.warning)));
                }
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if !running.is_empty() && style::secondary(ui, icon::REFRESH, "Check again").clicked() {
                        recheck = true;
                    }
                    ui.add_enabled_ui(running.is_empty(), |ui| {
                        if style::primary(ui, icon::TRASH, "Remove").clicked() {
                            let (s, list) = (cx.settings.clone(), selected.clone());
                            self.removing = Some(Pending::spawn(ui.ctx(), "Removing extensions", move || ext::remove(&s, &list).map_err(|e| crate::tf!("{e:#}"))));
                            close = true;
                        }
                    });
                    if style::secondary(ui, "", "Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = false;
            }
            if recheck {
                self.check_running();
            }
        }
    }
}
