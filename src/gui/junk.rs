//! Tools › Junk files: where to look and what counts as junk, then the files found, ticked,
//! and deleted to the Recycle Bin / Trash (or permanently).

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::config::Toggle;
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::junk::{self, DeleteReport, JunkFile, Progress};
use eframe::egui::{self, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub struct JunkView {
    files: Option<Vec<JunkFile>>,
    checked: Vec<bool>,
    scanning: Option<(Pending<Vec<JunkFile>>, Arc<Progress>)>,
    deleting: Option<Pending<DeleteReport>>,
    new_location: String,
    new_pattern: String,
    new_exclude: String,
    confirm: bool,
    export_path: String,
    /// Drives offered as quick "add location" buttons (read once).
    drives: Option<Vec<crate::platform::Drive>>,
}

impl JunkView {
    pub fn new() -> Self {
        Self {
            files: None,
            checked: Vec::new(),
            scanning: None,
            deleting: None,
            new_location: String::new(),
            new_pattern: String::new(),
            new_exclude: String::new(),
            confirm: false,
            export_path: super::export_dir().join("junk-files.txt").display().to_string(),
            drives: None,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if let Some(files) = self.scanning.as_ref().and_then(|(p, _)| p.poll()) {
            self.checked = vec![true; files.len()];
            self.files = Some(files);
            self.scanning = None;
        }
        if let Some(r) = take(&mut self.deleting) {
            if r.failed.is_empty() {
                cx.info(crate::tf!("Deleted {} junk file(s), {}", r.deleted, super::human(r.bytes)));
            } else {
                cx.error(crate::tf!("Deleted {} file(s); {} could not be deleted (first: {})", r.deleted, r.failed.len(), r.failed[0].0));
            }
            self.files = None;
        }
        let p = Palette::get(ui.ctx());

        if let Some((pend, prog)) = &self.scanning {
            style::card(ui, |ui| {
                crate::gui::style::row(ui, |ui| {
                    ui.spinner();
                    ui.label(crate::tf!("{} folders searched, {} junk files found", prog.folders.load(Ordering::Relaxed), prog.found.load(Ordering::Relaxed)));
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::secondary(ui, icon::X, "Stop").clicked() {
                            prog.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                });
                pend.show_progress(ui);
            });
            return;
        }
        if let Some(d) = &self.deleting {
            d.show_progress(ui);
            return;
        }
        if self.files.is_some() {
            self.results(ui, cx, p);
        } else {
            self.setup(ui, cx, p);
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context, cx: &Cx) {
        let prog = Arc::new(Progress::default());
        let (s, pr) = (cx.settings.clone(), prog.clone());
        self.scanning = Some((Pending::spawn(ctx, "Looking for junk files", move || junk::scan(&s, &pr)), prog));
    }

    fn setup(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let mut changed = false;
        // Toolbar: what will be scanned, and the scan button.
        let any = cx.settings.junk_patterns.iter().any(|t| t.enabled);
        crate::gui::style::row(ui, |ui| {
            let n_pat = cx.settings.junk_patterns.iter().filter(|t| t.enabled).count();
            let where_ =
                if cx.settings.junk_locations.is_empty() { tr("the default locations").to_string() } else { crate::tf!("{} location(s)", cx.settings.junk_locations.len()) };
            ui.label(style::muted(ui, crate::tf!("{n_pat} patterns in {where_}")));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(any, |ui| {
                    if style::primary(ui, icon::SEARCH, "Scan for junk files").clicked() {
                        self.start_scan(ui.ctx(), cx);
                    }
                });
            });
        });
        ui.add_space(10.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            style::card(ui, |ui| {
                style::section_title(ui, "Where to look");
                let defaults: Vec<String> = junk::default_locations().iter().map(|d| d.display().to_string()).collect();
                if cx.settings.junk_locations.is_empty() {
                    ui.label(style::muted(ui, crate::tf!("{} (default)", defaults.join(", "))).size(12.5));
                }
                let mut remove = None;
                for (i, l) in cx.settings.junk_locations.iter().enumerate() {
                    crate::gui::style::row(ui, |ui| {
                        ui.label(RichText::new(icon::FOLDER).color(p.accent));
                        ui.label(RichText::new(l).monospace());
                        if style::icon_button(ui, icon::X, "Remove location").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    cx.settings.junk_locations.remove(i);
                    changed = true;
                }
                crate::gui::style::row_wrapped(ui, |ui| {
                    let w = (ui.available_width() - 200.0).clamp(180.0, 420.0);
                    style::text_input(ui, &mut self.new_location, if cfg!(windows) { r"D:\ or C:\Users\me\Downloads" } else { tr("/home/me/Downloads or /media/disk") }, w);
                    if let Some(p) = style::browse_button(ui, "junk-location", style::Pick::Folder, &self.new_location) {
                        if !cx.settings.junk_locations.contains(&p) {
                            cx.settings.junk_locations.push(p);
                            changed = true;
                        }
                    }
                    if style::secondary(ui, icon::PLUS, "Add location").clicked() && !self.new_location.trim().is_empty() {
                        cx.settings.junk_locations.push(self.new_location.trim().to_string());
                        self.new_location.clear();
                        changed = true;
                    }
                    for d in self.drives.get_or_insert_with(crate::platform::drives).iter() {
                        let path = d.path.display().to_string();
                        if !cx.settings.junk_locations.contains(&path) && style::ghost(ui, icon::DRIVES, &d.label).on_hover_text(crate::tf!("Add {path}")).clicked() {
                            cx.settings.junk_locations.push(path);
                            changed = true;
                        }
                    }
                });
                ui.add_space(8.0);
                style::section_title(ui, "Never look in");
                let mut remove = None;
                for (i, l) in cx.settings.junk_exclude.iter().enumerate() {
                    crate::gui::style::row(ui, |ui| {
                        ui.label(RichText::new(icon::LOCK).color(p.muted));
                        ui.label(RichText::new(l).monospace());
                        if style::icon_button(ui, icon::X, "Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    cx.settings.junk_exclude.remove(i);
                    changed = true;
                }
                crate::gui::style::row(ui, |ui| {
                    let w = (ui.available_width() - 140.0).clamp(180.0, 420.0);
                    style::text_input(ui, &mut self.new_exclude, "A folder to skip", w);
                    if let Some(p) = style::browse_button(ui, "junk-exclude", style::Pick::Folder, &self.new_exclude) {
                        if !cx.settings.junk_exclude.contains(&p) {
                            cx.settings.junk_exclude.push(p);
                            changed = true;
                        }
                    }
                    if style::secondary(ui, icon::PLUS, "Add").clicked() && !self.new_exclude.trim().is_empty() {
                        cx.settings.junk_exclude.push(self.new_exclude.trim().to_string());
                        self.new_exclude.clear();
                        changed = true;
                    }
                });
                ui.label(
                    style::muted(
                        ui,
                        if cfg!(windows) {
                            tr("The Windows component store, Installer folder, Recycle Bin and CleanCore's backups are always skipped.")
                        } else {
                            tr("System folders (/usr, /proc, /var/lib, …), the Trash and CleanCore's backups are always skipped.")
                        },
                    )
                    .size(12.0),
                );
            });
            ui.add_space(12.0);
            style::card(ui, |ui| {
                crate::gui::style::row(ui, |ui| {
                    style::section_title(ui, "What counts as junk");
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::ghost(ui, icon::UNDO, "Reset to defaults").clicked() {
                            cx.settings.junk_patterns = junk::DEFAULT_PATTERNS.iter().map(|p| Toggle { value: p.to_string(), enabled: true }).collect();
                            changed = true;
                        }
                    });
                });
                ui.label(style::muted(ui, tr("File name patterns: * matches anything, ? one character. Click a pattern to switch it off.")).size(12.5));
                ui.add_space(4.0);
                let mut remove = None;
                crate::gui::style::row_wrapped(ui, |ui| {
                    for (i, t) in cx.settings.junk_patterns.iter_mut().enumerate() {
                        let r = style::segment(ui, t.enabled, &t.value).on_hover_text(tr("Click to switch on/off · right-click to remove"));
                        if r.clicked() {
                            t.enabled = !t.enabled;
                            changed = true;
                        }
                        if r.secondary_clicked() {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    cx.settings.junk_patterns.remove(i);
                    changed = true;
                }
                crate::gui::style::row(ui, |ui| {
                    style::text_input(ui, &mut self.new_pattern, "*.ext", 160.0);
                    if style::secondary(ui, icon::PLUS, "Add pattern").clicked() && !self.new_pattern.trim().is_empty() {
                        cx.settings.junk_patterns.push(Toggle { value: self.new_pattern.trim().to_string(), enabled: true });
                        self.new_pattern.clear();
                        changed = true;
                    }
                });
                ui.add_space(8.0);
                style::setting_row(ui, "Skip files changed in the last 24 hours", "They may still be in use.", |ui| {
                    changed |= style::toggle(ui, &mut cx.settings.junk_skip_recent, "Skip files changed in the last 24 hours").changed();
                });
                style::setting_row(ui, if cfg!(windows) { tr("Delete to the Recycle Bin") } else { tr("Delete to the Trash") }, "Otherwise files are deleted permanently.", |ui| {
                    changed |= style::toggle(ui, &mut cx.settings.junk_to_trash, "Delete to the trash").changed();
                });
            });
        });
        if changed {
            cx.save_settings();
        }
    }

    fn results(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let files = self.files.clone().unwrap_or_default();
        let sel: Vec<JunkFile> = files.iter().zip(&self.checked).filter(|(_, c)| **c).map(|(f, _)| f.clone()).collect();
        let sel_size: u64 = sel.iter().map(|f| f.size).sum();
        let total: u64 = files.iter().map(|f| f.size).sum();
        crate::gui::style::row(ui, |ui| {
            if style::secondary(ui, crate::gui::style::icon_back(), "New scan").clicked() {
                self.files = None;
            }
            ui.label(style::strong(crate::tf!("{} junk files · {}", files.len(), super::human(total))));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!sel.is_empty(), |ui| {
                    if style::primary(ui, icon::TRASH, &crate::tf!("Delete ({}, {})", sel.len(), super::human(sel_size))).clicked() {
                        if cx.settings.skip_warnings {
                            self.delete(ui.ctx(), cx, sel.clone());
                        } else {
                            self.confirm = true;
                        }
                    }
                });
                let r = style::icon_button(ui, icon::EXPORT, "Export list");
                super::export_menu(&r, cx, &mut self.export_path, "File (.txt or .csv)", |f| {
                    junk::export(&files, f)?;
                    Ok(crate::tf!("Exported to {}", f.display()))
                });
                if style::ghost(ui, "", "Select none").clicked() {
                    self.checked.iter_mut().for_each(|c| *c = false);
                }
                if style::ghost(ui, "", "Select all").clicked() {
                    self.checked.iter_mut().for_each(|c| *c = true);
                }
            });
        });
        ui.add_space(8.0);
        if files.is_empty() {
            style::empty_state(ui, icon::CHECK_CIRCLE, "No junk files", "Nothing matched the junk patterns in the chosen locations.");
            return;
        }
        let mut toggle = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .auto_shrink([false, false])
            .columns_in_order(vec![Column::exact(28.0), Column::remainder().at_least(300.0).clip(true), Column::exact(90.0), Column::exact(120.0), Column::exact(90.0)])
            .header(26.0, |mut h| {
                for t in crate::gui::style::ordered(vec!["", "File", "Size", "Changed", "Pattern"]) {
                    h.col(|ui| {
                        ui.label(style::muted(ui, tr(t)).size(12.5));
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, files.len(), |mut row| {
                    let i = row.index();
                    let f = &files[i];
                    crate::gui::style::cells_in_order(
                        &mut row,
                        vec![
                            Box::new(|ui: &mut Ui| {
                                let mut c = self.checked[i];
                                if ui.checkbox(&mut c, "").changed() {
                                    toggle = Some(i);
                                }
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(egui::Label::new(RichText::new(f.path.display().to_string()).size(12.5)).truncate());
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(super::human(f.size));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(style::muted(ui, f.modified.map(|m| m.format("%d %b %Y").to_string()).unwrap_or_default()));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(RichText::new(&f.pattern).monospace().size(12.0).color(p.muted));
                            }),
                        ],
                    );
                    if row.response().clicked() {
                        toggle = Some(i);
                    }
                });
            });
        if let Some(i) = toggle {
            self.checked[i] = !self.checked[i];
        }

        if self.confirm {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("junk-confirm")).show(ui.ctx(), |ui| {
                ui.set_width(420.0);
                ui.label(style::strong(crate::tf!("Delete {} junk file(s)?", sel.len())).size(18.0));
                ui.add_space(6.0);
                ui.label(style::muted(
                    ui,
                    if cx.settings.junk_to_trash { tr("They go to the Trash, so you can still get them back.") } else { tr("They are deleted permanently.") },
                ));
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::TRASH, "Delete").clicked() {
                        self.delete(ui.ctx(), cx, sel.clone());
                        close = true;
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = false;
            }
        }
    }

    fn delete(&mut self, ctx: &egui::Context, cx: &Cx, files: Vec<JunkFile>) {
        let s = cx.settings.clone();
        self.deleting = Some(Pending::spawn(ctx, "Deleting junk files", move || junk::delete(&s, &files)));
    }
}
