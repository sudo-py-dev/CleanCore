//! Uninstaller › Logs database (Revo: Logs Database): trace logs shared through a folder (and,
//! optionally, downloaded from a URL). Uninstalling with one imports it into Traced Programs.

use super::style::{self, icon, Palette};
use super::traced::TracedAction;
use super::{take, Cx, Pending};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::tracer::{self, DbLog};
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};

pub struct LogsDbTab {
    list: Vec<DbLog>,
    loaded: bool,
    search: String,
    compatible_only: bool,
    focus: Option<usize>,
    updating: Option<Pending<Result<usize, String>>>,
}

impl LogsDbTab {
    pub fn new() -> Self {
        Self { list: Vec::new(), loaded: false, search: String::new(), compatible_only: true, focus: None, updating: None }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    pub fn count(&mut self, cx: &Cx) -> usize {
        if !self.loaded {
            self.load(cx);
        }
        self.list.iter().filter(|d| d.compatible()).count()
    }

    fn load(&mut self, cx: &Cx) {
        self.list = tracer::db_list(cx.settings);
        self.list.sort_by_key(|d| d.log.name.to_lowercase());
        self.focus = None;
        self.loaded = true;
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) -> Option<TracedAction> {
        if !self.loaded {
            self.load(cx);
        }
        if let Some(r) = take(&mut self.updating) {
            match r {
                Ok(n) => cx.info(crate::tf!("Logs database updated: {n} new log(s)")),
                Err(e) => cx.error(e),
            }
            self.loaded = false;
        }
        let p = Palette::get(ui.ctx());
        let folder = tracer::db_folder(cx.settings);
        let mut action = None;

        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, "Search logs", 220.0);
            ui.add_space(6.0);
            style::toggle(ui, &mut self.compatible_only, "Only logs for this system");
            ui.add(style::Label::new(style::muted(ui, crate::tf!("Only {} {}", std::env::consts::OS, std::env::consts::ARCH))));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                let has_url = cx.settings.logs_db_url.as_deref().is_some_and(|u| !u.trim().is_empty());
                ui.add_enabled_ui(self.updating.is_none(), |ui| {
                    let r = style::secondary(ui, icon::DOWNLOAD, "Update").on_hover_text(style::vis(if has_url {
                        tr("Download new logs from the database URL")
                    } else {
                        tr("Set a database URL in Settings › Uninstaller first")
                    }));
                    if r.clicked() {
                        if has_url {
                            let s = cx.settings.clone();
                            self.updating = Some(Pending::spawn(ui.ctx(), "Downloading logs", move || tracer::db_update(&s).map_err(|e| crate::tf!("{e:#}"))));
                        } else {
                            cx.error(tr("No logs database URL is set. Add one in Settings › Uninstaller, or copy .cptrace files into the database folder."));
                        }
                    }
                });
                if style::icon_button(ui, icon::FOLDER_OPEN, "Open the database folder").clicked() {
                    let _ = crate::config::ensure_dir(&folder);
                    if let Err(e) = crate::platform::open_folder(&folder) {
                        cx.error(crate::tf!("{e:#}"));
                    }
                }
                if style::icon_button(ui, icon::REFRESH, "Refresh").clicked() {
                    self.loaded = false;
                }
            });
        });
        ui.add_space(8.0);
        if let Some(u) = &self.updating {
            u.show_progress(ui);
        }

        let q = self.search.to_lowercase();
        let rows: Vec<usize> =
            (0..self.list.len()).filter(|&i| (!self.compatible_only || self.list[i].compatible()) && (q.is_empty() || self.list[i].log.name.to_lowercase().contains(&q))).collect();
        if rows.is_empty() {
            let text = if self.list.is_empty() {
                crate::tf!("Put shared .cptrace logs in {} or download them with Update. Add your own from Traced programs.", folder.display())
            } else {
                "No log matches. Turn off \"Only logs for this system\" to see the others.".to_string()
            };
            style::empty_state(ui, icon::DATABASE, "The logs database is empty", &text);
            return None;
        }

        // Action bar for the selected log
        style::card(ui, |ui| {
            crate::gui::style::row(ui, |ui| match self.focus.and_then(|i| self.list.get(i)) {
                Some(d) => {
                    style::icon_tile(ui, icon::DATABASE, p.accent, 34.0);
                    crate::gui::style::column(ui, |ui| {
                        ui.add(style::Label::new(style::strong(&d.log.name)));
                        ui.add(style::Label::new(
                            style::muted(
                                ui,
                                crate::tf!("{} {} · {} settings · {} files · {}", d.log.os, d.log.arch, d.log.count(true), d.log.count(false), d.log.created.format("%d %b %Y")),
                            )
                            .size(12.5),
                        ));
                    });
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(d.compatible(), |ui| {
                            let import = |cx: &mut Cx| match tracer::db_import(d) {
                                Ok(l) => Some(l),
                                Err(e) => {
                                    cx.error(crate::tf!("{e:#}"));
                                    None
                                }
                            };
                            if style::primary(ui, icon::TRASH, "Uninstall")
                                .on_hover_text(style::vis(tr("Imports the log into Traced programs and removes what it recorded")))
                                .clicked()
                            {
                                action = import(cx).map(TracedAction::Uninstall);
                            }
                            if style::secondary(ui, icon::LIST_CHECKS, "Custom uninstall").clicked() {
                                action = import(cx).map(TracedAction::Custom);
                            }
                        });
                        if !d.compatible() {
                            style::badge(ui, "Recorded on another system", p.warning);
                        }
                    });
                }
                None => {
                    ui.add(style::Label::new(style::muted(ui, tr("Select a log. Uninstalling with it imports it into Traced programs first."))));
                }
            });
        });
        ui.add_space(8.0);

        let mut clicked = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .auto_shrink([false, false])
            .columns_in_order(vec![Column::remainder().at_least(220.0), Column::exact(120.0), Column::exact(90.0), Column::exact(90.0), Column::exact(110.0)])
            .header(26.0, |mut h| {
                for t in crate::gui::style::ordered(vec!["Name", "System", "Settings", "Files", "Recorded"]) {
                    h.col(|ui| {
                        ui.add(style::Label::new(style::muted(ui, tr(t)).size(12.5)));
                    });
                }
            })
            .body(|body| {
                body.rows(34.0, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let d = &self.list[i];
                    let sel = self.focus == Some(i);
                    row.set_selected(sel);
                    let soft = |ui: &Ui, t: String| style::muted(ui, t);
                    crate::gui::style::cells_in_order(
                        &mut row,
                        vec![
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(style::rich(icon::DATABASE).color(if d.compatible() { p.accent } else { p.faint })));
                                ui.add(style::Label::new(style::strong(&d.log.name)).truncate());
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(soft(ui, crate::tf!("{} {}", d.log.os, d.log.arch))));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(d.log.count(true).to_string()));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(d.log.count(false).to_string()));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(soft(ui, d.log.created.format("%d %b %Y").to_string())));
                            }),
                        ],
                    );
                    if row.response().clicked() {
                        clicked = Some(i);
                    }
                });
            });
        if let Some(i) = clicked {
            self.focus = Some(i);
        }
        action
    }
}
