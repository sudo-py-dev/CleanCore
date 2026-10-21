//! Backup Center: list, create, restore (whole or custom), delete, Auto Backup.

use super::schedule::{Kind, TaskPanel};
use super::style::{self, icon, Palette};
use super::{human, take, tree::Browser, Cx, Pending};
use crate::backup::{self, Backup, BackupKind, Manifest};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use eframe::egui::{self, RichText, Ui};
use egui_extras::{Column, TableBuilder};

struct CreateForm {
    name: String,
    note: String,
    whole: bool,
    key: String,
    browser: Browser,
}

struct CustomRestore {
    id: String,
    labels: Vec<String>,
    checked: Vec<bool>,
}

enum Confirm {
    Restore(String),
    Delete(String),
    DeleteAll,
}

pub struct BackupView {
    list: Vec<Manifest>,
    loaded: bool,
    selected: Option<String>,
    create: Option<CreateForm>,
    custom: Option<CustomRestore>,
    confirm: Option<Confirm>,
    job: Option<Pending<Result<String, String>>>,
    auto: Option<TaskPanel>,
    /// "Delete by time range" form: from, to (YYYY-MM-DD [HH:MM]).
    range: (String, String),
}

impl BackupView {
    pub fn new() -> Self {
        Self { list: Vec::new(), loaded: false, selected: None, create: None, custom: None, confirm: None, job: None, auto: None, range: (String::new(), String::new()) }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn load(&mut self, cx: &mut Cx) {
        match backup::list(cx.settings) {
            Ok(l) => self.list = l,
            Err(e) => cx.error(crate::tf!("Reading backups failed: {e:#}")),
        }
        if self.selected.as_ref().is_some_and(|s| !self.list.iter().any(|m| &m.id == s)) {
            self.selected = None;
        }
        self.loaded = true;
    }

    fn run(&mut self, ctx: &egui::Context, label: &str, work: impl FnOnce() -> anyhow::Result<String> + Send + 'static) {
        self.job = Some(Pending::spawn(ctx, label, move || work().map_err(|e| crate::tf!("{e:#}"))));
    }

    fn restore(&mut self, ctx: &egui::Context, cx: &Cx, id: String, only: Option<Vec<String>>) {
        let settings = cx.settings.clone();
        self.run(ctx, "Restoring…", move || {
            let b = Backup::open(&settings, &id)?;
            let n = b.restore(only.as_deref())?;
            Ok(crate::tf!("{n} item(s) restored from {id}"))
        });
    }

    fn open_custom(&mut self, id: String, cx: &mut Cx) {
        match Backup::open(cx.settings, &id) {
            Ok(b) => {
                let labels: Vec<String> = b.manifest.entries.iter().map(|e| e.label()).collect();
                let checked = vec![true; labels.len()];
                self.custom = Some(CustomRestore { id, labels, checked });
            }
            Err(e) => cx.error(crate::tf!("{e:#}")),
        }
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if let Some(r) = take(&mut self.job) {
            match r {
                Ok(m) => cx.info(m),
                Err(e) => cx.error(e),
            }
            self.loaded = false;
        }
        if let Some(panel) = &mut self.auto {
            let back = style::page_header_back(ui, "Auto Backup", "Create backups automatically on a schedule", |_| {});
            if back {
                self.auto = None;
                return;
            }
            panel.ui(ui, cx);
            return;
        }
        if !self.loaded {
            self.load(cx);
        }
        let p = Palette::get(ui.ctx());
        let busy = self.job.is_some();
        let total: u64 = self.list.iter().map(|m| m.size_bytes).sum();
        let subtitle = crate::tf!("{} backup(s) · {} · {}", self.list.len(), human(total), cx.settings.backup_dir().display());
        let (mut create, mut auto, mut folder) = (false, false, false);
        style::page_header(ui, "Backups", &subtitle, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                create = style::primary(ui, icon::PLUS, "Create backup").clicked();
                auto = style::secondary(ui, icon::CALENDAR, "Auto Backup").on_hover_text(tr("Scheduled backups")).clicked();
            });
            folder = style::icon_button(ui, icon::FOLDER_OPEN, "Open backup folder").clicked();
            let r = style::icon_button(ui, icon::CALENDAR, "Delete by time range");
            egui::Popup::menu(&r).width(320.0).show(|ui| {
                ui.label(style::strong(tr("Delete backups made between")));
                ui.label(style::muted(ui, tr("Dates like 2026-10-01, or 2026-10-01 14:30. An end date includes that whole day; leave it empty for \"now\".")).size(12.0));
                ui.add_space(4.0);
                style::text_input(ui, &mut self.range.0, "From", 300.0);
                style::text_input(ui, &mut self.range.1, "To (optional)", 300.0);
                let parsed = crate::backup::parse_local(&self.range.0, false).and_then(|f| {
                    let t = if self.range.1.trim().is_empty() { chrono::Local::now() } else { crate::backup::parse_local(&self.range.1, true)? };
                    Ok((f, t))
                });
                match &parsed {
                    Ok((f, t)) => {
                        let n = self.list.iter().filter(|m| m.created >= *f && m.created <= *t).count();
                        ui.add_enabled_ui(n > 0, |ui| {
                            if style::danger(ui, icon::TRASH, &crate::tf!("Delete {n} backup(s)")).clicked() {
                                match crate::backup::delete_range(cx.settings, *f, *t) {
                                    Ok(r) => cx.info(crate::tf!("Deleted {} backup(s)", r.len())),
                                    Err(e) => cx.error(crate::tf!("{e:#}")),
                                }
                                self.loaded = false;
                                ui.close();
                            }
                        });
                    }
                    Err(e) if !self.range.0.trim().is_empty() => {
                        ui.label(egui::RichText::new(crate::tf!("{e:#}")).size(12.0).color(Palette::get(ui.ctx()).danger));
                    }
                    Err(_) => {}
                }
            });
        });
        if create {
            self.create = Some(CreateForm { name: "Manual backup".into(), note: String::new(), whole: false, key: String::new(), browser: Browser::default() });
        }
        if auto {
            let mut panel = TaskPanel::new(Kind::Backup);
            panel.refresh(cx);
            self.auto = Some(panel);
        }
        if folder {
            let dir = cx.settings.backup_dir();
            let _ = crate::config::ensure_dir(&dir);
            if let Err(e) = crate::platform::open_folder(&dir) {
                cx.error(crate::tf!("{e:#}"));
            }
        }
        if let Some(pend) = &self.job {
            style::card(ui, |ui| pend.show_progress(ui));
            ui.add_space(10.0);
        }
        style::card(ui, |ui| {
            if self.list.is_empty() {
                style::empty_state(ui, icon::ARCHIVE, "No backups yet", "A backup is made automatically before every clean, or create one now.");
                return;
            }
            // Action bar for the selected backup.
            let sel = self.selected.clone();
            crate::gui::style::row(ui, |ui| {
                match self.list.iter().find(|m| Some(&m.id) == sel.as_ref()) {
                    Some(m) => {
                        ui.label(style::strong(&m.name));
                        ui.label(style::muted(ui, m.created.format("%d %b %Y, %H:%M").to_string()));
                    }
                    None => {
                        ui.label(style::muted(ui, tr("Select a backup to restore or delete it.")));
                    }
                }
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    let r = style::icon_button(ui, icon::DOTS, "More");
                    egui::Popup::menu(&r).show(|ui| {
                        if style::ghost(ui, icon::TRASH, "Delete all backups").clicked() {
                            self.confirm = Some(Confirm::DeleteAll);
                            ui.close();
                        }
                    });
                    ui.add_enabled_ui(!busy && sel.is_some(), |ui| {
                        if style::danger(ui, icon::TRASH, "Delete").clicked() {
                            self.confirm = Some(Confirm::Delete(sel.clone().unwrap()));
                        }
                        if style::secondary(ui, icon::LIST_CHECKS, "Custom restore").clicked() {
                            self.open_custom(sel.clone().unwrap(), cx);
                        }
                        if style::secondary(ui, icon::UNDO, "Restore").clicked() {
                            self.confirm = Some(Confirm::Restore(sel.clone().unwrap()));
                        }
                    });
                });
            });
            ui.add_space(6.0);
            self.table(ui, cx, p);
        });
        self.dialogs(ui.ctx(), cx);
    }

    fn table(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let mut clicked = None;
        let mut menu: Option<Confirm> = None;
        let mut custom = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .auto_shrink([false, false])
            .columns_in_order(vec![
                Column::exact(36.0),
                Column::initial(240.0).at_least(140.0).clip(true),
                Column::exact(150.0),
                Column::exact(150.0),
                Column::exact(80.0),
                Column::exact(90.0),
                Column::remainder(),
            ])
            .header(28.0, |mut h| {
                for t in crate::gui::style::ordered(vec!["", "Name", "Type", "Date", "Entries", "Size", "Note"]) {
                    h.col(|ui| {
                        ui.label(RichText::new(tr(t)).size(12.5).color(p.muted));
                    });
                }
            })
            .body(|body| {
                body.rows(40.0, self.list.len(), |mut row| {
                    let m = &self.list[row.index()];
                    row.set_selected(self.selected.as_deref() == Some(m.id.as_str()));
                    let (ic, col) = kind_icon(m.kind, p);
                    crate::gui::style::cells_in_order(
                        &mut row,
                        vec![
                            Box::new(|ui: &mut Ui| {
                                ui.label(RichText::new(ic).size(18.0).color(col));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(egui::Label::new(&m.name).truncate());
                            }),
                            Box::new(|ui: &mut Ui| {
                                style::badge(ui, kind_label(m.kind), col);
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(RichText::new(m.created.format("%d %b %Y, %H:%M").to_string()).color(p.muted));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(m.entries.len().to_string());
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.label(RichText::new(human(m.size_bytes)).color(p.muted));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(egui::Label::new(RichText::new(&m.note).color(p.muted)).truncate());
                            }),
                        ],
                    );
                    let r = row.response();
                    if r.clicked() {
                        clicked = Some(m.id.clone());
                    }
                    r.context_menu(|ui| {
                        if style::ghost(ui, icon::UNDO, "Restore").clicked() {
                            menu = Some(Confirm::Restore(m.id.clone()));
                            ui.close();
                        }
                        if style::ghost(ui, icon::LIST_CHECKS, "Custom restore").clicked() {
                            custom = Some(m.id.clone());
                            ui.close();
                        }
                        ui.separator();
                        if style::ghost(ui, icon::TRASH, "Delete").clicked() {
                            menu = Some(Confirm::Delete(m.id.clone()));
                            ui.close();
                        }
                        if style::ghost(ui, icon::TRASH, "Delete all").clicked() {
                            menu = Some(Confirm::DeleteAll);
                            ui.close();
                        }
                    });
                });
            });
        if let Some(c) = clicked {
            self.selected = Some(c);
        }
        if menu.is_some() {
            self.confirm = menu;
        }
        if let Some(id) = custom {
            self.open_custom(id, cx);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        // Confirmation
        if let Some(c) = &self.confirm {
            let text = match c {
                Confirm::Restore(id) => crate::tf!("Restore everything in backup {id}?"),
                Confirm::Delete(id) => crate::tf!("Are you sure you want to delete \"{id}\"?"),
                Confirm::DeleteAll => "Are you sure you want to delete all backups?".to_string(),
            };
            let mut answer = None;
            let r = egui::Modal::new(egui::Id::unique("backup-confirm")).show(ctx, |ui| {
                ui.set_width(420.0);
                let p = Palette::get(ui.ctx());
                let restore = text.starts_with("Restore");
                style::icon_tile(ui, if restore { icon::UNDO } else { icon::TRASH }, if restore { p.accent } else { p.danger }, 44.0);
                ui.add_space(8.0);
                ui.label(style::strong(text).size(17.0));
                ui.add_space(14.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, "", if restore { tr("Restore") } else { tr("Delete") }).clicked() {
                        answer = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            if r.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                let c = self.confirm.take().unwrap();
                if yes {
                    match c {
                        Confirm::Restore(id) => self.restore(ctx, cx, id, None),
                        Confirm::Delete(id) => match backup::delete(cx.settings, &id) {
                            Ok(()) => cx.info(crate::tf!("Deleted backup {id}")),
                            Err(e) => cx.error(crate::tf!("{e:#}")),
                        },
                        Confirm::DeleteAll => {
                            let mut n = 0;
                            for m in &self.list {
                                if backup::delete(cx.settings, &m.id).is_ok() {
                                    n += 1;
                                }
                            }
                            cx.info(crate::tf!("Deleted {n} backup(s)"));
                        }
                    }
                    self.loaded = false;
                }
            }
        }

        // Custom restore
        if let Some(c) = &mut self.custom {
            let mut done = None;
            egui::Modal::new(egui::Id::unique("custom-restore")).show(ctx, |ui| {
                ui.label(style::strong(tr("Custom restore")).size(19.0));
                ui.label(style::muted(ui, crate::tf!("Choose what to restore from {}", c.id)));
                crate::gui::style::row(ui, |ui| {
                    if style::ghost(ui, icon::CHECK, "Select all").clicked() {
                        c.checked.iter_mut().for_each(|x| *x = true);
                    }
                    if style::ghost(ui, icon::X, "Select none").clicked() {
                        c.checked.iter_mut().for_each(|x| *x = false);
                    }
                });
                egui::ScrollArea::vertical().max_height(360.0).min_scrolled_width(600.0).show(ui, |ui| {
                    for (l, ch) in c.labels.iter().zip(c.checked.iter_mut()) {
                        ui.checkbox(ch, RichText::new(l).monospace());
                    }
                });
                ui.add_space(6.0);
                let n = c.checked.iter().filter(|x| **x).count();
                crate::gui::style::row(ui, |ui| {
                    if ui.add_enabled_ui(n > 0, |ui| style::primary(ui, icon::UNDO, &crate::tf!("Restore {n} item(s)"))).inner.clicked() {
                        done = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(yes) = done {
                let c = self.custom.take().unwrap();
                if yes {
                    let only: Vec<String> = c.labels.into_iter().zip(c.checked).filter(|(_, k)| *k).map(|(l, _)| l).collect();
                    self.restore(ctx, cx, c.id, Some(only));
                }
            }
        }

        // Create backup
        if let Some(f) = &mut self.create {
            let mut done = None;
            egui::Modal::new(egui::Id::unique("create-backup")).show(ctx, |ui| {
                ui.set_width(620.0);
                let p = Palette::get(ui.ctx());
                ui.label(style::strong(tr("Create backup")).size(19.0));
                ui.add_space(8.0);
                egui::Grid::new("create-grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    ui.label(style::muted(ui, tr("Name")));
                    style::text_input(ui, &mut f.name, "", 460.0);
                    ui.end_row();
                    ui.label(style::muted(ui, tr("Note")));
                    style::text_input(ui, &mut f.note, "Optional", 460.0);
                    ui.end_row();
                    ui.label(style::muted(ui, tr("Back up")));
                    crate::gui::style::row(ui, |ui| {
                        if style::segment(ui, f.whole, if cfg!(windows) { tr("Whole registry") } else { tr("Whole config store") }).clicked() {
                            f.whole = true;
                        }
                        if style::segment(ui, !f.whole, "One key / folder").clicked() {
                            f.whole = false;
                        }
                    });
                    ui.end_row();
                });
                if !f.whole {
                    ui.add_space(6.0);
                    egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(8).show(ui, |ui| {
                        ui.set_height(260.0);
                        f.browser.show(ui, "create-backup-browser");
                    });
                    if let Some(s) = &f.browser.selected {
                        f.key = s.clone();
                    }
                    style::key_or_path_input(ui, "backup-key", &mut f.key, "or type a path", 600.0, style::Pick::Any);
                }
                ui.add_space(6.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::FLOPPY, "Create").clicked() {
                        done = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(yes) = done {
                if yes && f.name.trim().is_empty() {
                    cx.error(tr("Please enter a name."));
                } else if yes && !f.whole && f.key.trim().is_empty() {
                    cx.error(tr("Please select a key to backup."));
                } else {
                    let f = self.create.take().unwrap();
                    if yes {
                        let settings = cx.settings.clone();
                        let (name, note, key) = (f.name, f.note, (!f.whole).then(|| f.key.trim().to_string()));
                        self.run(ctx, "Creating backup…", move || {
                            let kind = if key.is_some() { BackupKind::Partial } else { BackupKind::Full };
                            let mut b = Backup::create(&settings, &name, &note, kind)?;
                            let entries = match &key {
                                Some(k) => crate::platform::backup_key(&b.dir, k),
                                None => crate::platform::backup_full(&b.dir),
                            };
                            match entries {
                                Ok(e) => b.manifest.entries = e,
                                Err(e) => {
                                    let _ = std::fs::remove_dir_all(&b.dir);
                                    return Err(e.context("Backup creation failed!"));
                                }
                            }
                            b.save()?;
                            Ok(crate::tf!("Created backup {} ({} entries)", b.manifest.id, b.manifest.entries.len()))
                        });
                    }
                }
            }
        }
    }
}

fn kind_icon(k: BackupKind, p: Palette) -> (&'static str, eframe::egui::Color32) {
    match k {
        BackupKind::Full | BackupKind::AutoFull | BackupKind::FullBeforeClean => (icon::DATABASE, p.accent),
        BackupKind::Partial | BackupKind::AutoPartial => (icon::FOLDER, p.accent),
        BackupKind::DeletedItems => (icon::BROOM, p.success),
    }
}

fn kind_label(k: BackupKind) -> &'static str {
    match k {
        BackupKind::Full => "Full",
        BackupKind::Partial => "Partial",
        BackupKind::AutoFull => "Auto Full",
        BackupKind::AutoPartial => "Auto Partial",
        BackupKind::FullBeforeClean => "Full before cleaning",
        BackupKind::DeletedItems => "Deleted items",
    }
}
