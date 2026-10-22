//! Autorun Manager: what starts with the system or at login,
//! with switches to enable/disable, Remove (backed up), Add, and clean-ups for broken entries.

use super::style::{self, icon, Palette};
use super::{Cx, Pending};
use crate::autorun::{self, AddMethod, AutorunEntry, Source};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};

struct AddForm {
    name: String,
    command: String,
    all_users: bool,
    method: AddMethod,
}

pub struct AutorunView {
    list: Vec<AutorunEntry>,
    /// The list being read, and whether it includes system entries.
    loading: Option<(Pending<Vec<AutorunEntry>>, bool)>,
    loaded: bool,
    search: String,
    source: Option<Source>,
    focus: Option<String>,
    confirm: Option<Vec<AutorunEntry>>,
    add: Option<AddForm>,
    export_path: String,
}

impl AutorunView {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            loading: None,
            loaded: false,
            search: String::new(),
            source: None,
            focus: None,
            confirm: None,
            add: None,
            export_path: super::export_dir().join("autorun.html").display().to_string(),
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn load(&mut self, ctx: &egui::Context, cx: &Cx) {
        // A read already running for the same setting is kept; one for the other setting is
        // replaced (its result is dropped), so the last toggle always wins.
        let sys = cx.settings.autorun_show_system;
        if self.loading.as_ref().is_none_or(|(_, s)| *s != sys) {
            let label = if sys { tr("Reading startup entries, including system ones") } else { tr("Reading startup entries") };
            self.loading = Some((Pending::spawn(ctx, label, move || autorun::list(sys)), sys));
        }
        self.loaded = true;
    }

    fn shown(&self) -> Vec<&AutorunEntry> {
        let q = self.search.to_lowercase();
        self.list
            .iter()
            .filter(|e| self.source.is_none_or(|s| e.source == s))
            .filter(|e| {
                q.is_empty() || e.name.to_lowercase().contains(&q) || e.command.to_lowercase().contains(&q) || e.publisher.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !self.loaded {
            self.load(ui.ctx(), cx);
        }
        if let Some(l) = self.loading.as_ref().and_then(|(l, _)| l.poll()) {
            self.loading = None;
            self.list = l;
        }
        let p = Palette::get(ui.ctx());
        let enabled = self.list.iter().filter(|e| e.enabled).count();
        let subtitle = if self.loading.is_some() { "Reading startup entries…".to_string() } else { crate::tf!("{} entries · {enabled} start automatically", self.list.len()) };
        // Counted every frame; the entries are only copied when the action is picked.
        let is_invalid = |e: &&AutorunEntry| !e.valid && !e.system;
        let is_disabled = |e: &&AutorunEntry| !e.enabled && !e.system;
        let n_invalid = self.list.iter().filter(is_invalid).count();
        let n_disabled = self.list.iter().filter(is_disabled).count();

        style::page_header(ui, "Autorun", &subtitle, |ui| {
            if style::primary(ui, icon::PLUS, "Add").clicked() {
                self.add = Some(AddForm { name: String::new(), command: String::new(), all_users: false, method: AddMethod::Run });
            }
            let r = style::secondary(ui, icon::DOTS, "More");
            egui::Popup::menu(&r).width(360.0).show(|ui| {
                ui.add_enabled_ui(n_invalid > 0, |ui| {
                    if style::ghost(ui, icon::WARNING, &crate::tf!("Remove invalid entries ({n_invalid})"))
                        .on_hover_text(style::vis(tr("Entries whose program no longer exists")))
                        .clicked()
                    {
                        self.confirm = Some(self.list.iter().filter(is_invalid).cloned().collect());
                        ui.close();
                    }
                });
                ui.add_enabled_ui(n_disabled > 0, |ui| {
                    if style::ghost(ui, icon::PAUSE, &crate::tf!("Remove disabled entries ({n_disabled})")).clicked() {
                        self.confirm = Some(self.list.iter().filter(is_disabled).cloned().collect());
                        ui.close();
                    }
                });
                ui.separator();
                let list: Vec<AutorunEntry> = self.shown().into_iter().cloned().collect();
                super::export_fields(ui, cx, &mut self.export_path, "Export (.txt .html .csv .json)", |f| {
                    autorun::export(&list, f)?;
                    Ok(crate::tf!("Exported {} entries to {}", list.len(), f.display()))
                });
            });
            if style::icon_button(ui, icon::REFRESH, "Refresh").clicked() {
                self.loaded = false;
            }
        });

        // Filters
        let mut sources: Vec<Source> = Vec::new();
        for e in &self.list {
            if !sources.contains(&e.source) {
                sources.push(e.source);
            }
        }
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, "Search name, command or publisher", 250.0);
            ui.add_space(6.0);
            if style::segment(ui, self.source.is_none(), "All").clicked() {
                self.source = None;
            }
            for s in sources {
                let n = self.list.iter().filter(|e| e.source == s).count();
                if style::segment(ui, self.source == Some(s), &crate::tf!("{}  {n}", tr(s.label()))).clicked() {
                    self.source = Some(s);
                }
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if style::toggle(ui, &mut cx.settings.autorun_show_system, "Show system entries").changed() {
                    cx.save_settings();
                    if !cx.settings.autorun_show_system {
                        // Hide them at once; the reload below only confirms it.
                        self.list.retain(|e| !e.system);
                    }
                    self.loaded = false;
                }
                ui.add(style::Label::new(style::muted(ui, if cfg!(windows) { tr("Windows entries") } else { tr("System entries") })));
            });
        });
        ui.add_space(8.0);

        if let Some((l, _)) = &self.loading {
            l.show_progress(ui);
            ui.add_space(6.0);
        }
        let shown: Vec<AutorunEntry> = self.shown().into_iter().cloned().collect();
        if shown.is_empty() && self.loading.is_none() {
            style::empty_state(
                ui,
                icon::ROCKET,
                "Nothing starts automatically",
                if self.list.is_empty() { tr("No startup entries were found.") } else { tr("Nothing matches the filter.") },
            );
        } else {
            self.table(ui, cx, &shown, p);
        }
        self.dialogs(ui.ctx(), cx);
    }

    fn table(&mut self, ui: &mut Ui, cx: &mut Cx, rows: &[AutorunEntry], p: Palette) {
        let mut toggled: Option<(AutorunEntry, bool)> = None;
        let mut refresh = false;
        let mut remove = None;
        let mut focus = None;
        TableBuilder::new(ui)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .columns_in_order(vec![
                Column::exact(56.0),
                Column::remainder().at_least(260.0).clip(true),
                Column::exact(130.0),
                Column::exact(180.0).clip(true),
                Column::exact(120.0),
            ])
            .header(26.0, |mut h| {
                for t in crate::gui::style::ordered(vec!["On", "Program", "Type", "Publisher", ""]) {
                    h.col(|ui| {
                        ui.add(style::Label::new(style::muted(ui, tr(t)).size(12.5)));
                    });
                }
            })
            .body(|body| {
                body.rows(52.0, rows.len(), |mut row| {
                    let e = &rows[row.index()];
                    row.set_selected(self.focus.as_deref() == Some(&e.id));
                    crate::gui::style::cells_in_order(
                        &mut row,
                        vec![
                            Box::new(|ui: &mut Ui| {
                                let mut on = e.enabled;
                                let can = e.source != Source::RunOnce;
                                ui.add_enabled_ui(can, |ui| {
                                    if style::toggle(ui, &mut on, &crate::tf!("Start {} automatically", e.name)).changed() {
                                        toggled = Some((e.clone(), on));
                                    }
                                });
                            }),
                            Box::new(|ui: &mut Ui| {
                                crate::gui::style::column(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    crate::gui::style::row(ui, |ui| {
                                        ui.add(style::Label::new(style::strong(&e.name).color(if e.enabled { p.text } else { p.faint })));
                                        if e.running {
                                            style::badge(ui, "Running", p.success);
                                        }
                                        if !e.valid {
                                            style::badge(ui, "Program missing", p.danger);
                                        }
                                        if e.all_users {
                                            ui.add(style::Label::new(style::rich(icon::USERS).color(p.faint).size(13.0))).on_hover_text(style::vis(tr("For all users")));
                                        }
                                    });
                                    ui.add(style::Label::new(style::rich(&e.command).monospace().size(11.5).color(p.muted)).truncate()).on_hover_text(style::vis(&e.location));
                                });
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(style::muted(ui, tr(e.source.label()))));
                            }),
                            Box::new(|ui: &mut Ui| {
                                ui.add(style::Label::new(style::muted(ui, e.publisher.as_deref().unwrap_or(""))).truncate());
                            }),
                            Box::new(|ui: &mut Ui| {
                                crate::gui::style::row(ui, |ui| {
                                    let tip = if e.removable { tr("Remove (backed up)") } else { tr("Can't be deleted: disable it instead") };
                                    ui.add_enabled_ui(e.removable, |ui| {
                                        if style::icon_button(ui, icon::TRASH, tip).clicked() {
                                            remove = Some(e.clone());
                                        }
                                    });
                                    let r = style::icon_button(ui, icon::DOTS, "More");
                                    egui::Popup::menu(&r).width(220.0).show(|ui| {
                                        if style::ghost(ui, icon::PLAY, "Run now").clicked() {
                                            match autorun::run_now(e) {
                                                Ok(()) => cx.info(crate::tf!("Started {}", e.name)),
                                                Err(err) => cx.error(crate::tf!("{err:#}")),
                                            }
                                            ui.close();
                                        }
                                        ui.add_enabled_ui(e.running, |ui| {
                                            if style::ghost(ui, icon::X_CIRCLE, "Stop it").clicked() {
                                                match autorun::kill(e) {
                                                    Ok(n) => cx.info(crate::tf!("Stopped {} ({n} process(es))", e.name)),
                                                    Err(err) => cx.error(crate::tf!("{err:#}")),
                                                }
                                                refresh = true;
                                                ui.close();
                                            }
                                        });
                                    });
                                    if let Some(exe) = &e.exe {
                                        if style::icon_button(ui, icon::FOLDER_OPEN, "Open file location").clicked() {
                                            let path = std::path::Path::new(exe);
                                            if let Err(err) = crate::platform::open_folder(path.parent().unwrap_or(path)) {
                                                cx.error(crate::tf!("{err:#}"));
                                            }
                                        }
                                    }
                                })
                                .inner
                            }),
                        ],
                    );
                    if row.response().clicked() {
                        focus = Some(e.id.clone());
                    }
                });
            });
        if let Some(f) = focus {
            self.focus = Some(f);
        }
        if let Some((e, on)) = toggled {
            match autorun::set_enabled(&e, on) {
                Ok(()) => {
                    if let Some(x) = self.list.iter_mut().find(|x| x.id == e.id) {
                        x.enabled = on;
                    }
                    cx.info(crate::tf!("{} {}", e.name, if on { tr("will start automatically") } else { tr("won't start automatically") }));
                }
                Err(err) => cx.error(crate::tf!("{err:#}")),
            }
        }
        if let Some(e) = remove {
            self.confirm = Some(vec![e]);
        }
        if refresh {
            self.loaded = false;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some(list) = self.confirm.clone() {
            let mut close = false;
            crate::gui::style::modal("autorun-remove", 460.0, ctx, |ui| {
                let title = if list.len() == 1 { crate::tf!("Remove \"{}\"?", list[0].name) } else { crate::tf!("Remove {} entries?", list.len()) };
                ui.add(style::Label::new(style::strong(title).size(18.0)));
                ui.add_space(6.0);
                for e in list.iter().take(8) {
                    ui.add(style::Label::new(style::rich(crate::tf!("• {}: {}", e.name, e.command)).size(12.5)).truncate());
                }
                if list.len() > 8 {
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("and {} more", list.len() - 8))));
                }
                ui.add_space(6.0);
                ui.add(style::Label::new(style::muted(ui, tr("The programs themselves stay installed; they just won't start automatically. Removed entries are backed up."))));
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::primary(ui, icon::TRASH, "Remove").clicked() {
                        match autorun::remove(cx.settings, &list) {
                            Ok((r, d)) => cx.info(if d > 0 { crate::tf!("Removed {r}, disabled {d}") } else { crate::tf!("Removed {r} entry(ies)") }),
                            Err(e) => cx.error(crate::tf!("{e:#}")),
                        }
                        self.loaded = false;
                        close = true;
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = None;
            }
        }

        if let Some(f) = &mut self.add {
            let (mut close, mut done) = (false, false);
            crate::gui::style::modal("autorun-add", 480.0, ctx, |ui| {
                ui.add(style::Label::new(style::strong(tr("Add a startup entry")).size(18.0)));
                ui.add_space(8.0);
                ui.add(style::Label::new(style::muted(ui, tr("Name"))));
                style::text_input(ui, &mut f.name, "My program", 460.0);
                ui.add_space(4.0);
                ui.add(style::Label::new(style::muted(ui, tr("Command"))));
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    style::text_input(
                        ui,
                        &mut f.command,
                        if cfg!(windows) { r#""C:\Program Files\App\app.exe" --minimized"# } else { tr("/usr/bin/app --minimized") },
                        460.0 - style::CONTROL_H - 6.0,
                    );
                    if let Some(p) = style::browse_button(ui, "autorun-command", style::Pick::File, &f.command) {
                        f.command = style::command_from_path(&p);
                    }
                });
                ui.add_space(8.0);
                if cfg!(windows) {
                    crate::gui::style::row(ui, |ui| {
                        for (m, l) in [(AddMethod::Run, "Registry Run"), (AddMethod::RunOnce, "Run once"), (AddMethod::StartupFolder, "Startup folder")] {
                            if style::segment(ui, f.method == m, l).clicked() {
                                f.method = m;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    crate::gui::style::row(ui, |ui| {
                        style::toggle(ui, &mut f.all_users, "For all users");
                        ui.add(style::Label::new(tr("For all users (needs administrator rights)")));
                    });
                } else {
                    ui.add(style::Label::new(style::muted(ui, tr("Added to your autostart folder (~/.config/autostart).")).size(12.5)));
                    f.method = AddMethod::StartupFolder;
                }
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    ui.add_enabled_ui(!f.name.trim().is_empty() && !f.command.trim().is_empty(), |ui| {
                        if style::primary(ui, icon::PLUS, "Add").clicked() {
                            match autorun::add(&f.name, &f.command, f.all_users, f.method) {
                                Ok(()) => {
                                    cx.info(crate::tf!("Added \"{}\"", f.name.trim()));
                                    done = true;
                                }
                                Err(e) => cx.error(crate::tf!("{e:#}")),
                            }
                        }
                    });
                    if style::secondary(ui, "", "Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close || done {
                self.add = None;
            }
            if done {
                self.loaded = false;
            }
        }
    }
}
