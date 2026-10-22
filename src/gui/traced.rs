//! Traced Programs: the "Install & trace" screen, the logs tab and the log editor.

use super::style::{self, icon, Palette};
use super::{Cx, Pending};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::tracer::{self, ItemKind, ItemStatus, Session, TraceItem, TraceLog};
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeSet;
use std::time::Duration;

/// What the logs tab asks the uninstaller page to do.
pub enum TracedAction {
    Uninstall(TraceLog),
    Custom(TraceLog),
}

// ---------------------------------------------------------------- install & trace

enum TraceStep {
    Snapshot(Pending<Session>),
    Tracing(Option<Session>),
    Analyzing(Pending<(Vec<TraceItem>, usize)>),
    Review(Vec<TraceItem>),
}

pub struct TraceScreen {
    step: TraceStep,
    installer: String,
    child: Option<std::process::Child>,
    child_status: Option<String>,
    name: String,
    comment: String,
    /// File items left out because no installer process wrote them.
    left_out: usize,
}

impl TraceScreen {
    pub fn new(ctx: &egui::Context, cx: &Cx) -> Self {
        let s = cx.settings.clone();
        Self {
            step: TraceStep::Snapshot(Pending::spawn(ctx, "Recording the current state", move || Session::start(&s, None))),
            installer: String::new(),
            child: None,
            child_status: None,
            name: String::new(),
            comment: String::new(),
            left_out: 0,
        }
    }

    /// Start tracing with the installer already filled in ("Install with CleanCore").
    pub fn with_installer(ctx: &egui::Context, cx: &Cx, installer: &str) -> Self {
        let mut s = Self::new(ctx, cx);
        s.installer = installer.to_string();
        s
    }

    /// Returns false when the screen should close.
    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) -> bool {
        let p = Palette::get(ui.ctx());
        let mut close = false;
        style::page_header(ui, "Install & trace", "Every change the installation makes is recorded, so it can be undone exactly", |ui| {
            if style::secondary(ui, icon::X, "Cancel").clicked() {
                close = true;
            }
        });
        if close {
            if let Some(c) = &mut self.child {
                let _ = c.kill();
            }
            return false;
        }
        let step_idx = match self.step {
            TraceStep::Snapshot(_) => 0,
            TraceStep::Tracing(_) => 1,
            TraceStep::Analyzing(_) => 2,
            TraceStep::Review(_) => 3,
        };
        steps(ui, step_idx, &["Snapshot", "Install", "Analyze", "Save"], p);
        ui.add_space(12.0);

        // Advance background steps
        if let TraceStep::Snapshot(pend) = &mut self.step {
            if let Some(session) = pend.poll() {
                self.step = TraceStep::Tracing(Some(session));
            }
        }
        if let TraceStep::Analyzing(pend) = &mut self.step {
            if let Some((items, left_out)) = pend.poll() {
                self.left_out = left_out;
                self.name = tracer::suggest_name(&items, Some(&self.installer).filter(|s| !s.is_empty()).map(|s| s.as_str()));
                self.step = TraceStep::Review(items);
            }
        }

        match &mut self.step {
            TraceStep::Snapshot(pend) => busy(ui, p, "Recording the current state…", &pend.label),
            TraceStep::Analyzing(_) => busy(ui, p, "Comparing with the snapshot…", "Finding everything the installation created"),
            TraceStep::Tracing(session) => {
                if let Some(c) = &mut self.child {
                    if let Ok(Some(st)) = c.try_wait() {
                        self.child_status = Some(crate::tf!("The installer finished ({st}). Run and configure the program if you like, close it, then stop the trace."));
                        self.child = None;
                    }
                }
                let mut stop = false;
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 6.0, p.danger);
                        let since = session.as_ref().map(|s| (chrono::Local::now() - s.started).num_seconds()).unwrap_or(0);
                        ui.add(style::Label::new(style::strong(crate::tf!("Tracing  {:02}:{:02}", since / 60, since % 60)).size(16.0)));
                    });
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::muted(
                        ui,
                        tr("Install the program now: run its installer below, or start it any way you like. Install one program at a time and close other programs."),
                    )));
                    ui.add_space(10.0);
                    crate::gui::style::row(ui, |ui| {
                        style::text_input(
                            ui,
                            &mut self.installer,
                            if cfg!(windows) { r"C:\Downloads\setup.exe" } else { tr("/home/you/Downloads/install.sh or a .deb") },
                            ui.available_width() - 160.0 - style::CONTROL_H - 8.0,
                        );
                        if let Some(p) = style::browse_button(ui, "trace-installer", style::Pick::File, &self.installer) {
                            self.installer = p;
                        }
                        let can_run = !self.installer.trim().is_empty() && self.child.is_none();
                        if ui.add_enabled_ui(can_run, |ui| style::secondary(ui, icon::PLAY, "Run installer")).inner.clicked() {
                            let launched = match session.as_ref() {
                                Some(s) => s.launch(cx.settings, self.installer.trim()),
                                None => crate::platform::launch(self.installer.trim()),
                            };
                            match launched {
                                Ok(c) => {
                                    self.child = Some(c);
                                    self.child_status = Some("The installer is running…".into());
                                }
                                Err(e) => cx.error(crate::tf!("Could not start the installer: {e:#}")),
                            }
                        }
                    });
                    if let Some(s) = &self.child_status {
                        ui.add_space(4.0);
                        ui.add(style::Label::new(style::rich(s).color(p.accent)));
                    }
                    ui.add_space(12.0);
                    stop = style::primary(ui, icon::CHECK, "Stop & analyze").clicked();
                });
                if stop {
                    if let Some(sess) = session.take() {
                        let s = cx.settings.clone();
                        self.step = TraceStep::Analyzing(Pending::spawn(ui.ctx(), "Analyzing", move || sess.finish_report(&s)));
                    }
                }
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
            TraceStep::Review(items) => {
                let created = items.iter().filter(|i| i.status == ItemStatus::Created).count();
                let modified = items.len() - created;
                if created == 0 {
                    style::card(ui, |ui| style::empty_state(ui, icon::INFO, "Nothing was installed", "No new files, settings or packages appeared during the trace."));
                    return true;
                }
                let mut save = false;
                if self.left_out > 0 {
                    ui.add(style::Label::new(
                        style::muted(
                            ui,
                            crate::tf!("{} change(s) made by other programs during the trace were left out (only the installer's own processes count).", self.left_out),
                        )
                        .size(12.5),
                    ));
                    ui.add_space(6.0);
                }
                style::card(ui, |ui| {
                    egui::Grid::new("trace-save").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                        ui.add(style::Label::new(style::muted(ui, tr("Name"))));
                        style::text_input(ui, &mut self.name, "", 420.0);
                        ui.end_row();
                        ui.add(style::Label::new(style::muted(ui, tr("Comment"))));
                        style::text_input(ui, &mut self.comment, "Optional", 420.0);
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::muted(
                        ui,
                        crate::tf!("{created} item(s) created, removable with this log · {modified} existing item(s) changed (kept on uninstall)"),
                    )));
                    ui.add_space(8.0);
                    save = ui.add_enabled_ui(!self.name.trim().is_empty(), |ui| style::primary(ui, icon::FLOPPY, "Save log")).inner.clicked();
                });
                ui.add_space(10.0);
                item_list(ui, items, p, None);
                if save {
                    let mut log = tracer::new_log(self.name.trim(), Some(self.installer.clone()).filter(|s| !s.trim().is_empty()), items.clone());
                    log.comment = self.comment.clone();
                    match tracer::save(&log) {
                        Ok(()) => {
                            cx.info(crate::tf!("Saved trace log \"{}\"", log.name));
                            return false;
                        }
                        Err(e) => cx.error(crate::tf!("{e:#}")),
                    }
                }
            }
        }
        true
    }
}

fn busy(ui: &mut Ui, p: Palette, title: &str, sub: &str) {
    style::card(ui, |ui| {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(36.0).color(p.accent));
            ui.add_space(6.0);
            ui.add(style::Label::new(style::strong(title)));
            ui.add(style::Label::new(style::muted(ui, sub)));
        });
        ui.add_space(20.0);
    });
    ui.ctx().request_repaint_after(Duration::from_millis(200));
}

fn steps(ui: &mut Ui, idx: usize, labels: &[&str], p: Palette) {
    crate::gui::style::row(ui, |ui| {
        for (i, label) in labels.iter().enumerate() {
            let (bg, fg) = if i < idx {
                (p.success, egui::Color32::WHITE)
            } else if i == idx {
                (p.accent, egui::Color32::WHITE)
            } else {
                (p.surface_alt, p.muted)
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 12.0, bg);
            let mark = if i < idx { icon::CHECK.to_string() } else { (i + 1).to_string() };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(mark), egui::FontId::new(12.5, style::semibold()), fg);
            ui.add(style::Label::new(style::rich(tr(label)).color(if i <= idx { p.text } else { p.muted })));
            if i + 1 < labels.len() {
                let (r, _) = ui.allocate_exact_size(egui::vec2(36.0, 2.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, if i < idx { p.success } else { p.border });
            }
        }
    });
}

/// Items grouped by kind; `remove` lets the editor delete entries.
fn item_list(ui: &mut Ui, items: &[TraceItem], p: Palette, mut remove: Option<&mut Option<usize>>) {
    egui::ScrollArea::vertical().id_salt("trace-items").auto_shrink([false, false]).show(ui, |ui| {
        let reg = if cfg!(windows) { tr("Registry") } else { tr("Settings (dconf)") };
        for (kinds, title, ic) in [
            (&[ItemKind::Package][..], "Packages", icon::PACKAGE),
            (&[ItemKind::SettingsKey, ItemKind::SettingsValue][..], reg, icon::TREE),
            (&[ItemKind::Folder, ItemKind::File][..], "Files and folders", icon::FOLDER),
        ] {
            let idx: Vec<usize> = (0..items.len()).filter(|i| kinds.contains(&items[*i].kind)).collect();
            if idx.is_empty() {
                continue;
            }
            style::card(ui, |ui| {
                crate::gui::style::row(ui, |ui| {
                    style::icon_tile(ui, ic, p.accent, 28.0);
                    ui.add(style::Label::new(style::strong(title)));
                    style::badge(ui, &idx.len().to_string(), p.accent);
                });
                ui.add_space(4.0);
                for i in idx.iter().take(500) {
                    let it = &items[*i];
                    crate::gui::style::row(ui, |ui| {
                        let (mark, col) = if it.status == ItemStatus::Created { (icon::PLUS, p.success) } else { (icon::DOTS, p.muted) };
                        ui.add(style::Label::new(style::rich(mark).color(col))).on_hover_text(style::vis(if it.status == ItemStatus::Created {
                            tr("Created: removed on uninstall")
                        } else {
                            tr("Existed before: kept on uninstall")
                        }));
                        ui.add(style::Label::new(style::rich(it.label()).monospace().size(12.5).color(if it.status == ItemStatus::Created { p.text } else { p.muted })).truncate());
                        if let Some(r) = remove.as_deref_mut() {
                            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                                if style::icon_button(ui, icon::TRASH, "Remove from log").clicked() {
                                    *r = Some(*i);
                                }
                            });
                        }
                    });
                }
                if idx.len() > 500 {
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("… and {} more (export the log to see all)", idx.len() - 500))));
                }
            });
            ui.add_space(10.0);
        }
    });
}

// ---------------------------------------------------------------- logs tab

struct Editor {
    log: TraceLog,
    new_kind: ItemKind,
    new_path: String,
    new_value: String,
}

pub struct TracedTab {
    pub logs: Vec<TraceLog>,
    loaded: bool,
    focus: Option<String>,
    checked: BTreeSet<String>,
    search: String,
    export_path: String,
    import_path: String,
    merge_name: String,
    rename: Option<(String, String)>,
    editor: Option<Editor>,
    confirm_delete: Option<String>,
    /// Group filter (None: all logs).
    group: Option<String>,
    new_group: String,
}

impl TracedTab {
    pub fn new() -> Self {
        let dir = super::export_dir();
        Self {
            logs: Vec::new(),
            loaded: false,
            focus: None,
            checked: BTreeSet::new(),
            search: String::new(),
            export_path: dir.join("trace.cptrace").display().to_string(),
            import_path: String::new(),
            merge_name: String::new(),
            rename: None,
            editor: None,
            confirm_delete: None,
            group: None,
            new_group: String::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    pub fn count(&mut self) -> usize {
        if !self.loaded {
            self.logs = tracer::list();
            self.loaded = true;
        }
        self.logs.len()
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) -> Option<TracedAction> {
        self.count();
        let p = Palette::get(ui.ctx());
        let mut action = None;
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, "Search logs", 240.0);
            let mut groups: Vec<String> = self.logs.iter().filter_map(|l| l.group.clone()).collect();
            groups.sort_by_key(|g| g.to_lowercase());
            groups.dedup();
            if !groups.is_empty() {
                ui.add_space(6.0);
                if style::segment(ui, self.group.is_none(), "All").clicked() {
                    self.group = None;
                }
                for g in groups {
                    let n = self.logs.iter().filter(|l| l.group.as_deref() == Some(g.as_str())).count();
                    if style::segment(ui, self.group.as_deref() == Some(g.as_str()), &crate::tf!("{g}  {n}")).clicked() {
                        self.group = Some(g);
                    }
                }
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                let r = style::secondary(ui, icon::DOTS, "More");
                egui::Popup::menu(&r).width(380.0).show(|ui| {
                    ui.add(style::Label::new(style::muted(ui, tr("Import a .cptrace log"))));
                    crate::gui::style::row(ui, |ui| {
                        style::text_input(ui, &mut self.import_path, "path/to/log.cptrace", 260.0 - style::CONTROL_H - 8.0);
                        if let Some(p) = style::browse_button(ui, "trace-import", style::Pick::File, &self.import_path) {
                            self.import_path = p;
                        }
                        if style::secondary(ui, icon::DOWNLOAD, "Import").clicked() {
                            match tracer::import(std::path::Path::new(self.import_path.trim())) {
                                Ok(l) => cx.info(crate::tf!("Imported \"{}\"", l.name)),
                                Err(e) => cx.error(crate::tf!("{e:#}")),
                            }
                            self.loaded = false;
                            ui.close();
                        }
                    });
                    ui.separator();
                    ui.add(style::Label::new(style::muted(ui, tr("Back up or restore every log (one file)"))));
                    style::path_input(ui, "trace-backup", &mut self.export_path, "", 360.0, style::Pick::Save);
                    crate::gui::style::row(ui, |ui| {
                        if style::secondary(ui, icon::FLOPPY, "Back up all").clicked() {
                            let path = std::path::Path::new(&self.export_path).with_extension("cptraces");
                            match tracer::backup_all(&path) {
                                Ok(n) => cx.info(crate::tf!("Saved {n} log(s) to {}", path.display())),
                                Err(e) => cx.error(crate::tf!("{e:#}")),
                            }
                            ui.close();
                        }
                        if style::secondary(ui, icon::UNDO, "Restore").clicked() {
                            match tracer::restore_all(std::path::Path::new(&self.export_path)) {
                                Ok(n) => cx.info(crate::tf!("Restored {n} log(s)")),
                                Err(e) => cx.error(crate::tf!("{e:#}")),
                            }
                            self.loaded = false;
                            ui.close();
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);

        let q = self.search.to_lowercase();
        let shown: Vec<TraceLog> = self
            .logs
            .iter()
            .filter(|l| self.group.is_none() || l.group == self.group)
            .filter(|l| q.is_empty() || crate::tf!("{} {}", l.name, l.comment).to_lowercase().contains(&q))
            .cloned()
            .collect();
        let focus = self.focus.as_ref().and_then(|id| self.logs.iter().find(|l| &l.id == id)).cloned();

        // Action bar, only while a log is selected or several are ticked.
        if focus.is_some() || self.checked.len() > 1 {
            style::card(ui, |ui| {
                crate::gui::style::row(ui, |ui| {
                    let n = self.checked.len();
                    if n > 1 {
                        ui.add(style::Label::new(style::strong(crate::tf!("{n} logs selected"))));
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            let ready = !self.merge_name.trim().is_empty();
                            if ui.add_enabled_ui(ready, |ui| style::primary(ui, icon::LIST_CHECKS, "Merge")).inner.clicked() {
                                let ids: Vec<String> = self.checked.iter().cloned().collect();
                                match tracer::merge(&ids, self.merge_name.trim()) {
                                    Ok(m) => cx.info(crate::tf!("Merged into \"{}\"", m.name)),
                                    Err(e) => cx.error(crate::tf!("{e:#}")),
                                }
                                self.checked.clear();
                                self.merge_name.clear();
                                self.loaded = false;
                            }
                            style::text_input(ui, &mut self.merge_name, "Name of the merged log", 220.0);
                        });
                    } else if let Some(l) = &focus {
                        style::icon_tile(ui, l.icon.as_deref().and_then(style::icon_for_key).unwrap_or(icon::RECORD), p.accent, 34.0);
                        crate::gui::style::column(ui, |ui| {
                            ui.add(style::Label::new(style::strong(&l.name)));
                            ui.add(style::Label::new(
                                style::muted(
                                    ui,
                                    crate::tf!(
                                        "{} · {} settings · {} files · {}",
                                        l.created.format("%d %b %Y"),
                                        l.count(true),
                                        l.count(false),
                                        crate::backup::human_size(l.size())
                                    ),
                                )
                                .size(12.5),
                            ));
                        });
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            let r = style::icon_button(ui, icon::DOTS, "More");
                            egui::Popup::menu(&r).width(360.0).show(|ui| {
                                if style::ghost(ui, icon::CHAT, "Rename").clicked() {
                                    self.rename = Some((l.id.clone(), l.name.clone()));
                                    ui.close();
                                }
                                ui.menu_button(style::vis(crate::tf!("{}  Group", icon::TAG)), |ui| {
                                    let mut pick: Option<Option<String>> = None;
                                    let mut groups: Vec<String> = self.logs.iter().filter_map(|x| x.group.clone()).collect();
                                    groups.sort_by_key(|g| g.to_lowercase());
                                    groups.dedup();
                                    for g in groups {
                                        if ui.button(style::vis(&g)).clicked() {
                                            pick = Some(Some(g));
                                        }
                                    }
                                    crate::gui::style::row(ui, |ui| {
                                        style::text_input(ui, &mut self.new_group, "New group", 140.0);
                                        if ui.button(style::vis(tr("Add"))).clicked() && !self.new_group.trim().is_empty() {
                                            pick = Some(Some(self.new_group.trim().to_string()));
                                            self.new_group.clear();
                                        }
                                    });
                                    if l.group.is_some() && style::ghost(ui, icon::X, "Remove from group").clicked() {
                                        pick = Some(None);
                                    }
                                    if let Some(g) = pick {
                                        if let Err(e) = tracer::set_group(&l.id, g.as_deref()) {
                                            cx.error(crate::tf!("{e:#}"));
                                        }
                                        self.loaded = false;
                                        ui.close();
                                    }
                                });
                                ui.menu_button(style::vis(crate::tf!("{}  Change icon", icon::PALETTE)), |ui| {
                                    if let Some(choice) = style::icon_picker(ui, l.icon.as_deref()) {
                                        if let Err(e) = tracer::set_icon(&l.id, choice.as_deref()) {
                                            cx.error(crate::tf!("{e:#}"));
                                        }
                                        self.loaded = false;
                                        ui.close();
                                    }
                                });
                                if style::ghost(ui, icon::TRASH, "Delete log").clicked() {
                                    self.confirm_delete = Some(l.id.clone());
                                    ui.close();
                                }
                                if style::ghost(ui, icon::DATABASE, "Add to logs database")
                                    .on_hover_text(style::vis(tr("Share this log through the logs database folder")))
                                    .clicked()
                                {
                                    match tracer::db_add(cx.settings, l) {
                                        Ok(p) => cx.info(crate::tf!("Added to the logs database: {}", p.display())),
                                        Err(e) => cx.error(crate::tf!("{e:#}")),
                                    }
                                    ui.close();
                                }
                                ui.separator();
                                super::export_fields(ui, cx, &mut self.export_path, "Export (.cptrace to share, .html / .txt to read)", |f| {
                                    tracer::export(l, f)?;
                                    Ok(crate::tf!("Exported to {}", f.display()))
                                });
                            });
                            if style::secondary(ui, icon::WRENCH, "Edit").clicked() {
                                self.editor = Some(Editor { log: l.clone(), new_kind: ItemKind::File, new_path: String::new(), new_value: String::new() });
                            }
                            if style::secondary(ui, icon::LIST_CHECKS, "Custom uninstall").clicked() {
                                action = Some(TracedAction::Custom(l.clone()));
                            }
                            if style::primary(ui, icon::TRASH, "Uninstall").clicked() {
                                action = Some(TracedAction::Uninstall(l.clone()));
                            }
                        });
                    }
                });
            });
            ui.add_space(8.0);
        }

        style::card(ui, |ui| {
            if shown.is_empty() {
                style::empty_state(ui, icon::RECORD, "No traced programs", "Use Install & trace to record an installation; it can then be undone exactly.");
                return;
            }
            let mut clicked = None;
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
                .auto_shrink([false, false])
                .columns_in_order(vec![
                    Column::exact(28.0),
                    Column::remainder().at_least(200.0).clip(true),
                    Column::exact(130.0),
                    Column::exact(90.0),
                    Column::exact(90.0),
                    Column::exact(90.0),
                ])
                .header(26.0, |mut h| {
                    for t in crate::gui::style::ordered(vec!["", "Name", "Created", if cfg!(windows) { tr("Registry") } else { tr("Settings") }, "Files", "Size"]) {
                        h.col(|ui| {
                            ui.add(style::Label::new(style::rich(tr(t)).size(12.5).color(p.muted)));
                        });
                    }
                })
                .body(|body| {
                    body.rows(44.0, shown.len(), |mut row| {
                        let l = &shown[row.index()];
                        row.set_selected(self.focus.as_deref() == Some(l.id.as_str()));
                        crate::gui::style::cells_in_order(
                            &mut row,
                            vec![
                                Box::new(|ui: &mut Ui| {
                                    let mut c = self.checked.contains(&l.id);
                                    if ui.checkbox(&mut c, style::vis("")).changed() {
                                        if c {
                                            self.checked.insert(l.id.clone());
                                        } else {
                                            self.checked.remove(&l.id);
                                        }
                                    }
                                }),
                                Box::new(|ui: &mut Ui| {
                                    crate::gui::style::column(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.spacing_mut().interact_size.y = 18.0;
                                        crate::gui::style::row(ui, |ui| {
                                            ui.add(style::Label::new(style::rich(l.icon.as_deref().and_then(style::icon_for_key).unwrap_or(icon::RECORD)).color(p.accent)));
                                            ui.add(style::Label::new(style::strong(&l.name)).truncate());
                                            if let Some(g) = &l.group {
                                                style::badge(ui, g, p.accent);
                                            }
                                            if l.imported {
                                                style::badge(ui, "Imported", p.muted);
                                            }
                                        });
                                        let sub = if l.comment.is_empty() { l.installer.clone().unwrap_or_default() } else { l.comment.clone() };
                                        ui.add(style::Label::new(style::rich(sub).size(12.0).color(p.muted)).truncate());
                                    });
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(l.created.format("%d %b %Y %H:%M").to_string()).color(p.muted)));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(l.count(true).to_string()));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(l.count(false).to_string()));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(crate::backup::human_size(l.size())).color(p.muted)));
                                }),
                            ],
                        );
                        let r = row.response();
                        if r.clicked() {
                            clicked = Some(l.id.clone());
                        }
                        if r.double_clicked() {
                            action = Some(TracedAction::Uninstall(l.clone()));
                        }
                    });
                });
            if let Some(c) = clicked {
                self.focus = Some(c);
            }
        });
        self.dialogs(ui.ctx(), cx);
        action
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let p = Palette::get(ctx);
        if let Some((id, name)) = &mut self.rename {
            let mut done = None;
            egui::Modal::new(egui::Id::unique("rename-log")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.add(style::Label::new(style::strong(tr("Rename log")).size(18.0)));
                style::text_input(ui, name, "", 380.0);
                ui.add_space(10.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, "", "Rename").clicked() {
                        done = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                if ok {
                    match tracer::rename(id, name.trim()) {
                        Ok(()) => cx.info(tr("Log renamed")),
                        Err(e) => cx.error(crate::tf!("{e:#}")),
                    }
                    self.loaded = false;
                }
                self.rename = None;
            }
        }
        if let Some(id) = self.confirm_delete.clone() {
            let mut done = None;
            egui::Modal::new(egui::Id::unique("delete-log")).show(ctx, |ui| {
                ui.set_width(420.0);
                style::icon_tile(ui, icon::TRASH, p.danger, 44.0);
                ui.add_space(8.0);
                ui.add(style::Label::new(style::strong(tr("Delete this log?")).size(18.0)));
                ui.add(style::Label::new(style::muted(ui, tr("The program stays installed; only the record of its installation is deleted."))));
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, "", "Delete").clicked() {
                        done = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(ok) = done {
                if ok {
                    match tracer::delete(&id) {
                        Ok(()) => cx.info(tr("Log deleted")),
                        Err(e) => cx.error(crate::tf!("{e:#}")),
                    }
                    self.focus = None;
                    self.loaded = false;
                }
                self.confirm_delete = None;
            }
        }
        if let Some(ed) = &mut self.editor {
            let mut done = None;
            let mut remove = None;
            egui::Modal::new(egui::Id::unique("edit-log")).show(ctx, |ui| {
                ui.set_width(760.0);
                ui.set_max_height(620.0);
                ui.add(style::Label::new(style::strong(tr("Edit log")).size(19.0)));
                egui::Grid::new("edit-props").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.add(style::Label::new(style::muted(ui, tr("Name"))));
                    style::text_input(ui, &mut ed.log.name, "", 420.0);
                    ui.end_row();
                    ui.add(style::Label::new(style::muted(ui, tr("Author"))));
                    style::text_input(ui, &mut ed.log.author, "", 420.0);
                    ui.end_row();
                    ui.add(style::Label::new(style::muted(ui, tr("Comment"))));
                    style::text_input(ui, &mut ed.log.comment, "", 420.0);
                    ui.end_row();
                    ui.add(style::Label::new(style::muted(ui, tr("Add item"))));
                    crate::gui::style::row(ui, |ui| {
                        let kinds = [
                            (ItemKind::File, "File"),
                            (ItemKind::Folder, "Folder"),
                            (ItemKind::SettingsKey, if cfg!(windows) { tr("Registry key") } else { tr("dconf folder") }),
                            (ItemKind::SettingsValue, if cfg!(windows) { tr("Registry value") } else { tr("dconf key") }),
                        ];
                        egui::ComboBox::from_id_salt("new-kind")
                            .width(130.0)
                            .selected_text(style::vis(kinds.iter().find(|k| k.0 == ed.new_kind).map(|k| k.1).unwrap_or("")))
                            .show_ui(ui, |ui| {
                                for (k, l) in kinds {
                                    ui.selectable_value(&mut ed.new_kind, k, style::vis(l));
                                }
                            });
                        style::text_input(ui, &mut ed.new_path, "Path / key", 300.0);
                        if matches!(ed.new_kind, ItemKind::File | ItemKind::Folder) {
                            let pick = if ed.new_kind == ItemKind::Folder { style::Pick::Folder } else { style::Pick::File };
                            if let Some(p) = style::browse_button(ui, "trace-edit-path", pick, &ed.new_path) {
                                ed.new_path = p;
                            }
                        }
                        if ed.new_kind == ItemKind::SettingsValue {
                            style::text_input(ui, &mut ed.new_value, "Value name", 120.0);
                        }
                        if style::secondary(ui, icon::PLUS, "Add").clicked() && !ed.new_path.trim().is_empty() {
                            ed.log.items.push(TraceItem {
                                kind: ed.new_kind,
                                status: ItemStatus::Created,
                                path: ed.new_path.trim().to_string(),
                                value: (ed.new_kind == ItemKind::SettingsValue).then(|| ed.new_value.trim().to_string()),
                                size: None,
                                original: None,
                            });
                            ed.new_path.clear();
                            ed.new_value.clear();
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), 360.0), |ui| item_list(ui, &ed.log.items, p, Some(&mut remove)));
                ui.add_space(8.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::FLOPPY, "Save").clicked() {
                        done = Some(true);
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if let Some(i) = remove {
                ed.log.items.remove(i);
            }
            if let Some(ok) = done {
                if ok {
                    ed.log.modified = chrono::Local::now();
                    match tracer::save(&ed.log) {
                        Ok(()) => cx.info(tr("Log saved")),
                        Err(e) => cx.error(crate::tf!("{e:#}")),
                    }
                    self.loaded = false;
                }
                self.editor = None;
            }
        }
    }
}
