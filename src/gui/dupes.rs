//! Tools › Duplicates: where to look and which files count, then the groups of identical
//! files with every copy but one ticked, and deleting the ticked copies.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::dupes::{self, DupeGroup, Keep, Progress};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::junk::DeleteReport;
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Minimum sizes offered (bytes).
const MIN_SIZES: [u64; 5] = [1, 64 * 1024, 1024 * 1024, 10 * 1024 * 1024, 100 * 1024 * 1024];

fn keep_label(k: Keep) -> &'static str {
    match k {
        Keep::Oldest => tr("Keep the oldest copy"),
        Keep::Newest => tr("Keep the newest copy"),
        Keep::ShortestPath => tr("Keep the shortest path"),
    }
}

fn min_size_label(b: u64) -> String {
    if b <= 1 {
        tr("Any size").to_string()
    } else {
        crate::tf!("At least {}", super::human(b))
    }
}

/// A line of the results table: a group's heading or one of its files.
#[derive(Clone, Copy)]
enum Line {
    Group(usize),
    File(usize, usize),
}

pub struct DupesView {
    groups: Option<Vec<DupeGroup>>,
    checked: Vec<Vec<bool>>,
    keep: Keep,
    scanning: Option<(Pending<Vec<DupeGroup>>, Arc<Progress>)>,
    deleting: Option<Pending<DeleteReport>>,
    new_location: String,
    new_exclude: String,
    confirm: bool,
    export_path: String,
}

impl DupesView {
    pub fn new() -> Self {
        Self {
            groups: None,
            checked: Vec::new(),
            keep: Keep::Oldest,
            scanning: None,
            deleting: None,
            new_location: String::new(),
            new_exclude: String::new(),
            confirm: false,
            export_path: super::export_dir().join("duplicates.txt").display().to_string(),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if let Some((pend, prog)) = &self.scanning {
            if let Some(groups) = pend.poll() {
                // A stopped scan has nothing trustworthy to show.
                if !prog.cancel.load(Ordering::Relaxed) {
                    self.checked = dupes::auto_select(&groups, self.keep);
                    self.groups = Some(groups);
                }
                self.scanning = None;
            }
        }
        if let Some(r) = take(&mut self.deleting) {
            if r.failed.is_empty() {
                cx.info(crate::tf!("Deleted {} duplicate file(s), {}", r.deleted, super::human(r.bytes)));
            } else {
                cx.error(crate::tf!("Deleted {} file(s); {} could not be deleted (first: {}: {})", r.deleted, r.failed.len(), r.failed[0].0, r.failed[0].1));
            }
            self.groups = None;
        }
        let p = Palette::get(ui.ctx());

        if let Some((pend, prog)) = &self.scanning {
            style::card(ui, |ui| {
                crate::gui::style::row(ui, |ui| {
                    ui.spinner();
                    let (done, total) = (prog.checked.load(Ordering::Relaxed), prog.to_check.load(Ordering::Relaxed));
                    let text = match prog.stage.load(Ordering::Relaxed) {
                        0 => crate::tf!("{} folders searched, {} files listed", prog.folders.load(Ordering::Relaxed), prog.files.load(Ordering::Relaxed)),
                        1 => crate::tf!("Comparing the start and end of {} of {} files", done, total),
                        _ => crate::tf!("Comparing contents: {} of {} files, {} read", done, total, super::human(prog.bytes_read.load(Ordering::Relaxed))),
                    };
                    ui.add(style::Label::new(text));
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::secondary(ui, icon::X, "Stop").clicked() {
                            prog.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                });
                pend.show_progress(ui);
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        if let Some(d) = &self.deleting {
            d.show_progress(ui);
            return;
        }
        if self.groups.is_some() {
            self.results(ui, cx, p);
        } else {
            self.setup(ui, cx, p);
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context, cx: &Cx) {
        let prog = Arc::new(Progress::default());
        let (s, pr) = (cx.settings.clone(), prog.clone());
        self.scanning = Some((Pending::spawn(ctx, "Looking for duplicate files", move || dupes::scan(&s, &pr)), prog));
    }

    fn setup(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let mut changed = false;
        crate::gui::style::row(ui, |ui| {
            let where_ = if cx.settings.dupes_locations.is_empty() { tr("the home folder").to_string() } else { crate::tf!("{} location(s)", cx.settings.dupes_locations.len()) };
            ui.add(style::Label::new(style::muted(ui, crate::tf!("Files with the same content in {where_}"))));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if style::primary(ui, icon::SEARCH, "Find duplicates").clicked() {
                    self.start_scan(ui.ctx(), cx);
                }
            });
        });
        ui.add_space(10.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            style::card(ui, |ui| {
                style::section_title(ui, "Where to look");
                if cx.settings.dupes_locations.is_empty() {
                    let defaults: Vec<String> = dupes::default_locations().iter().map(|d| d.display().to_string()).collect();
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("{} (default)", defaults.join(", "))).size(12.5)));
                }
                changed |= path_list(ui, p, &mut cx.settings.dupes_locations, icon::FOLDER, "Remove location");
                crate::gui::style::row_wrapped(ui, |ui| {
                    let w = (ui.available_width() - 200.0).clamp(180.0, 420.0);
                    style::text_input(ui, &mut self.new_location, if cfg!(windows) { r"D:\ or C:\Users\me\Pictures" } else { tr("/home/me/Pictures or /media/disk") }, w);
                    if let Some(p) = style::browse_button(ui, "dupes-location", style::Pick::Folder, &self.new_location) {
                        changed |= add_path(&mut cx.settings.dupes_locations, p);
                    }
                    if style::secondary(ui, icon::PLUS, "Add location").clicked() {
                        changed |= add_path(&mut cx.settings.dupes_locations, std::mem::take(&mut self.new_location));
                    }
                });
                ui.add_space(8.0);
                style::section_title(ui, "Never look in");
                changed |= path_list(ui, p, &mut cx.settings.dupes_exclude, icon::LOCK, "Remove");
                crate::gui::style::row(ui, |ui| {
                    let w = (ui.available_width() - 140.0).clamp(180.0, 420.0);
                    style::text_input(ui, &mut self.new_exclude, "A folder to skip", w);
                    if let Some(p) = style::browse_button(ui, "dupes-exclude", style::Pick::Folder, &self.new_exclude) {
                        changed |= add_path(&mut cx.settings.dupes_exclude, p);
                    }
                    if style::secondary(ui, icon::PLUS, "Add").clicked() {
                        changed |= add_path(&mut cx.settings.dupes_exclude, std::mem::take(&mut self.new_exclude));
                    }
                });
                ui.add(style::Label::new(
                    style::muted(
                        ui,
                        if cfg!(windows) {
                            tr("The Windows component store, Installer folder, Recycle Bin and CleanCore's backups are always skipped.")
                        } else {
                            tr("System folders (/usr, /proc, /var/lib, …), the Trash and CleanCore's backups are always skipped.")
                        },
                    )
                    .size(12.0),
                ));
            });
            ui.add_space(12.0);
            style::card(ui, |ui| {
                style::section_title(ui, "Which files");
                style::setting_row(ui, "Minimum file size", "Small files rarely free much space and are often meant to be alike.", |ui| {
                    style::dropdown(ui, "dupes-min-size", &min_size_label(cx.settings.dupes_min_size), 160.0, |ui| {
                        for b in MIN_SIZES {
                            if ui.selectable_label(cx.settings.dupes_min_size == b, style::vis(min_size_label(b))).clicked() {
                                cx.settings.dupes_min_size = b;
                                changed = true;
                            }
                        }
                    });
                });
                style::setting_row(ui, "Skip hidden files and folders", "Program data, settings and version-control folders often hold identical files on purpose.", |ui| {
                    changed |= style::toggle(ui, &mut cx.settings.dupes_skip_hidden, "Skip hidden files and folders").changed();
                });
                style::setting_row(ui, if cfg!(windows) { tr("Delete to the Recycle Bin") } else { tr("Delete to the Trash") }, "Otherwise files are deleted permanently.", |ui| {
                    changed |= style::toggle(ui, &mut cx.settings.dupes_to_trash, "Delete to the trash").changed();
                });
            });
        });
        if changed {
            cx.save_settings();
        }
    }

    fn results(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let groups = self.groups.clone().unwrap_or_default();
        let (sel_n, sel_size) = groups.iter().zip(&self.checked).fold((0, 0), |(n, b), (g, c)| {
            let k = c.iter().filter(|c| **c).count();
            (n + k, b + g.size * k as u64)
        });
        let wasted: u64 = groups.iter().map(|g| g.wasted()).sum();
        crate::gui::style::row(ui, |ui| {
            if style::secondary(ui, crate::gui::style::icon_back(), "New scan").clicked() {
                self.groups = None;
            }
            ui.add(style::Label::new(style::strong(crate::tf!("{} groups · {} can be freed", groups.len(), super::human(wasted)))));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(sel_n > 0, |ui| {
                    if style::primary(ui, icon::TRASH, &crate::tf!("Delete ({}, {})", sel_n, super::human(sel_size))).clicked() {
                        if cx.settings.skip_warnings {
                            self.delete(ui.ctx(), cx, groups.clone());
                        } else {
                            self.confirm = true;
                        }
                    }
                });
                let r = style::icon_button(ui, icon::EXPORT, "Export list");
                super::export_menu(&r, cx, &mut self.export_path, "File (.txt or .csv)", |f| {
                    dupes::export(&groups, f)?;
                    Ok(crate::tf!("Exported to {}", f.display()))
                });
                if style::ghost(ui, "", "Select none").clicked() {
                    self.checked.iter_mut().flatten().for_each(|c| *c = false);
                }
                style::dropdown(ui, "dupes-keep", keep_label(self.keep), 200.0, |ui| {
                    for k in Keep::ALL {
                        if ui.selectable_label(self.keep == k, style::vis(keep_label(k))).clicked() {
                            self.keep = k;
                            self.checked = dupes::auto_select(&groups, k);
                        }
                    }
                });
            });
        });
        ui.add_space(8.0);
        if groups.is_empty() {
            style::empty_state(ui, icon::CHECK_CIRCLE, "No duplicates", "No two files in the chosen locations have the same content.");
            return;
        }
        let lines: Vec<Line> = groups.iter().enumerate().flat_map(|(g, d)| std::iter::once(Line::Group(g)).chain((0..d.files.len()).map(move |f| Line::File(g, f)))).collect();
        let mut toggle = None;
        TableBuilder::new(ui)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .auto_shrink([false, false])
            .columns_in_order(vec![Column::exact(28.0), Column::remainder().at_least(300.0).clip(true), Column::exact(120.0)])
            .header(26.0, |mut h| {
                for t in crate::gui::style::ordered(vec!["", "File", "Changed"]) {
                    h.col(|ui| {
                        ui.add(style::Label::new(style::muted(ui, tr(t)).size(12.5)));
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, lines.len(), |mut row| match lines[row.index()] {
                    Line::Group(g) => {
                        let d = &groups[g];
                        crate::gui::style::cells_in_order(
                            &mut row,
                            vec![
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(icon::COPY).color(p.accent)));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    let text = crate::tf!("{} copies of {} · {} can be freed", d.files.len(), super::human(d.size), super::human(d.wasted()));
                                    ui.add(style::Label::new(style::strong(text).size(12.5)).truncate());
                                }),
                                Box::new(|_: &mut Ui| {}),
                            ],
                        );
                    }
                    Line::File(g, i) => {
                        let f = &groups[g].files[i];
                        let keep = !self.checked[g][i];
                        crate::gui::style::cells_in_order(
                            &mut row,
                            vec![
                                Box::new(|ui: &mut Ui| {
                                    let mut c = !keep;
                                    if ui.checkbox(&mut c, style::vis("")).changed() {
                                        toggle = Some((g, i));
                                    }
                                }),
                                Box::new(|ui: &mut Ui| {
                                    let path = style::rich(f.path.display().to_string()).size(12.5);
                                    ui.add(style::Label::new(if keep { path } else { path.color(p.muted) }).truncate());
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::muted(ui, f.modified.map(|m| m.format("%d %b %Y").to_string()).unwrap_or_default())));
                                }),
                            ],
                        );
                        if row.response().clicked() {
                            toggle = Some((g, i));
                        }
                    }
                });
            });
        if let Some((g, i)) = toggle {
            self.checked[g][i] = !self.checked[g][i];
            if self.checked[g].iter().all(|c| *c) {
                // Every copy ticked would lose the file: untick the one just clicked again.
                self.checked[g][i] = false;
                cx.error(tr("At least one copy of each file is kept."));
            }
        }

        if self.confirm {
            let mut close = false;
            crate::gui::style::modal("dupes-confirm", 420.0, ui.ctx(), |ui| {
                ui.add(style::Label::new(style::strong(crate::tf!("Delete {} duplicate file(s)?", sel_n)).size(18.0)));
                ui.add_space(6.0);
                ui.add(style::Label::new(style::muted(
                    ui,
                    if cx.settings.dupes_to_trash { tr("They go to the Trash, so you can still get them back.") } else { tr("They are deleted permanently.") },
                )));
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::TRASH, "Delete").clicked() {
                        self.delete(ui.ctx(), cx, groups.clone());
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

    fn delete(&mut self, ctx: &egui::Context, cx: &Cx, groups: Vec<DupeGroup>) {
        let (s, checked) = (cx.settings.clone(), self.checked.clone());
        self.deleting = Some(Pending::spawn(ctx, "Deleting duplicate files", move || dupes::delete(&s, &groups, &checked)));
    }
}

/// The folders in `list`, each with a remove button. Returns whether one was removed.
fn path_list(ui: &mut Ui, p: Palette, list: &mut Vec<String>, glyph: &str, remove_tip: &str) -> bool {
    let mut remove = None;
    for (i, l) in list.iter().enumerate() {
        crate::gui::style::row(ui, |ui| {
            ui.add(style::Label::new(style::rich(glyph).color(if glyph == icon::LOCK { p.muted } else { p.accent })));
            ui.add(style::Label::new(style::rich(l).monospace()));
            if style::icon_button(ui, icon::X, remove_tip).clicked() {
                remove = Some(i);
            }
        });
    }
    remove.map(|i| list.remove(i)).is_some()
}

fn add_path(list: &mut Vec<String>, path: String) -> bool {
    let path = path.trim().to_string();
    if path.is_empty() || list.contains(&path) {
        return false;
    }
    list.push(path);
    true
}
