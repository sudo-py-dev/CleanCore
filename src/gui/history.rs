//! Tools › History: everything the History Cleaner can clean, grouped by browser profile,
//! system and Office, with sizes. The selection is remembered.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::history::{self, CleanReport, HistoryItem};
use crate::i18n::tr;
use eframe::egui::{self, RichText, Ui};
use std::collections::BTreeSet;

pub struct HistoryView {
    items: Vec<HistoryItem>,
    loading: Option<Pending<Vec<HistoryItem>>>,
    loaded: bool,
    checked: BTreeSet<String>,
    cleaning: Option<Pending<CleanReport>>,
    confirm: bool,
    /// Browsers open at the last check.
    running: Vec<crate::extensions::Browser>,
}

impl HistoryView {
    pub fn new() -> Self {
        Self { items: Vec::new(), loading: None, loaded: false, checked: BTreeSet::new(), cleaning: None, confirm: false, running: Vec::new() }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn check_running(&mut self) {
        self.running = crate::extensions::running_browsers(self.items.iter().filter_map(|i| i.browser));
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !self.loaded && self.loading.is_none() {
            self.loading = Some(Pending::spawn(ui.ctx(), "Measuring history and caches", history::items));
            self.loaded = true;
        }
        if let Some(items) = take(&mut self.loading) {
            self.checked = history::selected_by_default(cx.settings, &items).into_iter().collect();
            self.items = items;
            self.check_running();
        }
        if let Some(r) = take(&mut self.cleaning) {
            let mut msg = crate::tf!("Cleaned {} item(s), {}", r.cleaned, super::human(r.bytes));
            if !r.skipped.is_empty() {
                msg += &crate::tf!(". Skipped {} item(s) of open browsers", r.skipped.len());
            }
            if r.failed.is_empty() {
                cx.info(msg);
            } else {
                cx.error(crate::tf!("{msg}. Failed: {}", r.failed.iter().map(|(i, e)| crate::tf!("{i} ({e})")).collect::<Vec<_>>().join("; ")));
            }
            self.loaded = false;
        }
        let p = Palette::get(ui.ctx());
        if let Some(l) = &self.loading {
            l.show_progress(ui);
            return;
        }
        if let Some(c) = &self.cleaning {
            c.show_progress(ui);
            return;
        }

        let picked: Vec<HistoryItem> = self.items.iter().filter(|i| self.checked.contains(&i.id)).cloned().collect();
        let size: u64 = picked.iter().map(|i| i.size).sum();
        crate::gui::style::row(ui, |ui| {
            ui.label(style::strong(crate::tf!("{} selected · {}", picked.len(), super::human(size))));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!picked.is_empty(), |ui| {
                    if style::primary(ui, icon::BROOM, "Clean").clicked() {
                        self.check_running();
                        self.confirm = true;
                    }
                });
                if style::icon_button(ui, icon::REFRESH, "Refresh").clicked() {
                    self.loaded = false;
                }
            });
        });
        ui.add_space(8.0);
        if self.items.is_empty() {
            style::empty_state(ui, icon::CHECK_CIRCLE, "Nothing to clean", "No browser profiles, recent-file lists or caches were found.");
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut groups: Vec<&str> = Vec::new();
            for i in &self.items {
                if !groups.contains(&i.group.as_str()) {
                    groups.push(&i.group);
                }
            }
            let groups: Vec<String> = groups.into_iter().map(String::from).collect();
            for g in groups {
                let in_group: Vec<HistoryItem> = self.items.iter().filter(|i| i.group == g).cloned().collect();
                let browser = in_group.iter().find_map(|i| i.browser);
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        let ic = if browser.is_some() {
                            icon::GLOBE
                        } else if g == "Office" {
                            icon::FILE_TEXT
                        } else {
                            icon::MONITOR
                        };
                        ui.label(RichText::new(ic).size(17.0).color(p.accent));
                        ui.label(style::strong(&g));
                        if browser.is_some_and(|b| self.running.contains(&b)) {
                            style::badge(ui, "Open — close it to clean", p.warning);
                        }
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            let all = in_group.iter().all(|i| self.checked.contains(&i.id));
                            if style::ghost(ui, "", if all { tr("None") } else { tr("All") }).clicked() {
                                for i in &in_group {
                                    if all {
                                        self.checked.remove(&i.id);
                                    } else {
                                        self.checked.insert(i.id.clone());
                                    }
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                    for i in &in_group {
                        crate::gui::style::row(ui, |ui| {
                            let mut on = self.checked.contains(&i.id);
                            if ui.checkbox(&mut on, "").on_hover_text(&i.description).changed() {
                                if on {
                                    self.checked.insert(i.id.clone());
                                } else {
                                    self.checked.remove(&i.id);
                                }
                            }
                            crate::gui::style::column(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(tr(&i.name));
                                ui.label(style::muted(ui, &i.description).size(12.0));
                            });
                            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                                if i.size > 0 {
                                    ui.label(style::muted(ui, super::human(i.size)));
                                }
                            });
                        });
                    }
                });
                ui.add_space(8.0);
            }
        });

        if self.confirm {
            let open: Vec<&str> = self.running.iter().filter(|b| picked.iter().any(|i| i.browser == Some(**b))).map(|b| b.label()).collect();
            let (mut close, mut recheck) = (false, false);
            egui::Modal::new(egui::Id::unique("history-confirm")).show(ui.ctx(), |ui| {
                ui.set_width(440.0);
                ui.label(style::strong(crate::tf!("Clean {} item(s)?", picked.len())).size(18.0));
                ui.add_space(6.0);
                ui.label(style::muted(ui, tr("History, caches and recent lists are deleted for good; there is no backup of them.")));
                if !open.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(crate::tf!("{}  {} is open; its items will be skipped. Close it first to clean them.", icon::WARNING, open.join(", "))).color(p.warning),
                    );
                }
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::BROOM, "Clean").clicked() {
                        cx.settings.history_selected = self.checked.iter().cloned().collect();
                        cx.save_settings();
                        let (s, items) = (cx.settings.clone(), picked.clone());
                        self.cleaning = Some(Pending::spawn(ui.ctx(), "Cleaning", move || history::clean(&s, &items)));
                        close = true;
                    }
                    if !open.is_empty() && style::secondary(ui, icon::REFRESH, "Check again").clicked() {
                        recheck = true;
                    }
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
