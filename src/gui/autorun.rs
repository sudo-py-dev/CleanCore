//! Autorun Manager: what starts with the system or at login. The simple view lists programs,
//! heaviest first, with an on/off switch and how much each slows startup down; Details shows
//! where each entry comes from, its command and the advanced actions.

use super::style::{self, icon, Palette};
use super::{Cx, Pending};
use crate::autorun::{self, AddMethod, AutorunEntry, Impact, Need, Source};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use eframe::egui::{self, Color32, Ui, Vec2};
use egui_extras::{Column, TableBuilder};

struct AddForm {
    name: String,
    command: String,
    all_users: bool,
    method: AddMethod,
}

/// What a click in the list asks for, applied once the list is drawn.
enum Act {
    Toggle(AutorunEntry, bool),
    Remove(AutorunEntry),
    Focus(String),
    Refresh,
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
    icons: super::appicons::IconCache,
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
            icons: Default::default(),
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

    /// Entries matching the search (and in Details, the source filter). The simple view puts
    /// what starts now first, the heaviest at the top.
    fn shown(&self, details: bool) -> Vec<&AutorunEntry> {
        let q = self.search.to_lowercase();
        let mut v: Vec<&AutorunEntry> = self
            .list
            .iter()
            .filter(|e| !details || self.source.is_none_or(|s| e.source == s))
            .filter(|e| {
                q.is_empty()
                    || e.display_name().to_lowercase().contains(&q)
                    || e.name.to_lowercase().contains(&q)
                    || e.command.to_lowercase().contains(&q)
                    || e.publisher.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .collect();
        if !details {
            v.sort_by_cached_key(|e| (!e.enabled, std::cmp::Reverse(e.startup.impact), e.display_name().to_lowercase()));
        }
        v
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !self.loaded {
            self.load(ui.ctx(), cx);
        }
        if let Some(l) = self.loading.as_ref().and_then(|(l, _)| l.poll()) {
            self.loading = None;
            self.list = l;
            self.icons.load_autorun(ui.ctx(), &self.list);
        }
        self.icons.poll(ui.ctx());
        let p = Palette::get(ui.ctx());
        let details = cx.settings.autorun_details;
        let enabled = self.list.iter().filter(|e| e.enabled).count();
        let heavy = self.list.iter().filter(|e| e.enabled && e.startup.impact == Impact::High).count();
        let subtitle = if self.loading.is_some() {
            tr("Reading startup entries…").to_string()
        } else if details {
            crate::tf!("{} entries · {enabled} start automatically", self.list.len())
        } else if heavy > 0 {
            crate::tf!("{enabled} start with your computer · {heavy} slow it down")
        } else {
            crate::tf!("{enabled} start with your computer")
        };
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
                let list: Vec<AutorunEntry> = self.shown(details).into_iter().cloned().collect();
                super::export_fields(ui, cx, &mut self.export_path, "Export (.txt .html .csv .xlsx .json)", |f| {
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
            style::search_field(ui, &mut self.search, if details { "Search name, command or publisher" } else { "Search name or publisher" }, 250.0);
            if details {
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
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                let r = style::toggle(ui, &mut cx.settings.autorun_details, "Details");
                if r.on_hover_text(style::vis(tr("Show where each entry comes from, its command and more actions"))).changed() {
                    cx.save_settings();
                }
                ui.add(style::Label::new(style::muted(ui, tr("Details"))));
                if details {
                    ui.add_space(12.0);
                    if style::toggle(ui, &mut cx.settings.autorun_show_system, "Show system entries").changed() {
                        cx.save_settings();
                        if !cx.settings.autorun_show_system {
                            // Hide them at once; the reload below only confirms it.
                            self.list.retain(|e| !e.system);
                        }
                        self.loaded = false;
                    }
                    ui.add(style::Label::new(style::muted(ui, if cfg!(windows) { tr("Windows entries") } else { tr("System entries") })));
                }
            });
        });
        ui.add_space(8.0);

        if let Some((l, _)) = &self.loading {
            l.show_progress(ui);
            ui.add_space(6.0);
        }
        let shown: Vec<AutorunEntry> = self.shown(details).into_iter().cloned().collect();
        if shown.is_empty() && self.loading.is_none() {
            style::empty_state(
                ui,
                icon::ROCKET,
                "Nothing starts automatically",
                if self.list.is_empty() { tr("No startup entries were found.") } else { tr("Nothing matches the filter.") },
            );
        } else {
            if shown.iter().any(|e| !e.startup.measured && e.startup.impact != Impact::Unknown) {
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::rich(icon::INFO).color(p.muted).size(14.0)));
                    let note = if cfg!(windows) {
                        tr("≈ marks an estimate from the program's size. Open CleanCore as administrator to use the startup times Windows records.")
                    } else {
                        tr("≈ marks an estimate from the program's size. Services show the startup times systemd records.")
                    };
                    ui.add(style::Label::new(style::muted(ui, note).size(12.5)).wrap());
                });
                ui.add_space(6.0);
            }
            let act = if details { self.table(ui, cx, &shown, p) } else { self.simple_list(ui, cx, &shown, p) };
            self.apply(cx, act);
        }
        self.dialogs(ui.ctx(), cx);
    }

    fn apply(&mut self, cx: &mut Cx, act: Option<Act>) {
        match act {
            Some(Act::Toggle(e, on)) => match autorun::set_enabled(&e, on) {
                Ok(()) => {
                    if let Some(x) = self.list.iter_mut().find(|x| x.id == e.id) {
                        x.enabled = on;
                    }
                    cx.info(crate::tf!("{} {}", e.display_name(), if on { tr("will start automatically") } else { tr("won't start automatically") }));
                }
                Err(err) => cx.error(crate::tf!("{err:#}")),
            },
            Some(Act::Remove(e)) => self.confirm = Some(vec![e]),
            Some(Act::Focus(id)) => self.focus = Some(id),
            Some(Act::Refresh) => self.loaded = false,
            None => {}
        }
    }

    /// The program's icon, or a glyph for its kind.
    fn icon(&self, ui: &mut Ui, e: &AutorunEntry, size: f32, p: Palette) {
        match self.icons.get(&e.id) {
            Some(t) => {
                ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::splat(size)));
            }
            None => {
                let glyph = match e.source {
                    Source::Service | Source::UserService => icon::GEAR,
                    Source::ScheduledTask | Source::Cron => icon::CLOCK,
                    _ => icon::APP_WINDOW,
                };
                ui.add(style::Label::new(style::rich(glyph).size(size * 0.9).color(p.muted)));
            }
        }
    }

    /// The simple view: icon, name, publisher and a plain hint, the impact and a switch.
    fn simple_list(&mut self, ui: &mut Ui, cx: &mut Cx, rows: &[AutorunEntry], p: Palette) -> Option<Act> {
        let mut act = None;
        let impact_w =
            [Impact::High, Impact::Medium, Impact::Low, Impact::Unknown].iter().map(|i| style::badge_width(ui, &crate::tf!("≈ {}", tr(i.label())))).fold(110.0, f32::max);
        style::card(ui, |ui| {
            TableBuilder::new(ui)
                .sense(egui::Sense::click())
                .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
                .auto_shrink([false, false])
                .columns_in_order(vec![Column::exact(34.0), Column::remainder().at_least(200.0).clip(true), Column::exact(impact_w), Column::exact(48.0)])
                .header(26.0, |mut h| {
                    for t in crate::gui::style::ordered(vec!["", "Program", "Startup impact", "On"]) {
                        h.col(|ui| {
                            ui.add(style::Label::new(style::muted(ui, tr(t)).size(12.5)));
                        });
                    }
                })
                .body(|body| {
                    body.rows(56.0, rows.len(), |mut row| {
                        let e = &rows[row.index()];
                        row.set_selected(self.focus.as_deref() == Some(&e.id));
                        crate::gui::style::cells_in_order(
                            &mut row,
                            vec![
                                Box::new(|ui: &mut Ui| self.icon(ui, e, 26.0, p)),
                                Box::new(|ui: &mut Ui| {
                                    crate::gui::style::column(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.spacing_mut().interact_size.y = 18.0;
                                        crate::gui::style::row(ui, |ui| {
                                            ui.add(style::Label::new(style::strong(e.display_name()).color(if e.enabled { p.text } else { p.faint })).truncate());
                                            if !e.valid {
                                                style::badge(ui, "Program missing", p.danger);
                                            }
                                        });
                                        crate::gui::style::row(ui, |ui| {
                                            let publisher = e.publisher.as_deref().filter(|s| !s.is_empty());
                                            if let Some(publisher) = publisher {
                                                ui.add(style::Label::new(style::muted(ui, publisher).size(12.0)));
                                            }
                                            if let Some((text, color)) = hint(e, p) {
                                                if publisher.is_some() {
                                                    ui.add(style::Label::new(style::muted(ui, "·").size(12.0)));
                                                }
                                                ui.add(style::Label::new(style::rich(tr(text)).size(12.0).color(color)).truncate());
                                            }
                                        });
                                    });
                                }),
                                Box::new(|ui: &mut Ui| impact_badge(ui, e, p)),
                                Box::new(|ui: &mut Ui| {
                                    let mut on = e.enabled;
                                    ui.add_enabled_ui(e.source != Source::RunOnce, |ui| {
                                        if style::toggle(ui, &mut on, &crate::tf!("Start {} automatically", e.display_name())).changed() {
                                            act = Some(Act::Toggle(e.clone(), on));
                                        }
                                    });
                                }),
                            ],
                        );
                        let r = row.response();
                        if r.clicked() {
                            act = Some(Act::Focus(e.id.clone()));
                        }
                        r.context_menu(|ui| {
                            if let Some(a) = entry_menu(ui, cx, e) {
                                act = Some(a);
                            }
                        });
                    });
                });
        });
        act
    }

    /// The Details view: source, command and location, with every action.
    fn table(&mut self, ui: &mut Ui, cx: &mut Cx, rows: &[AutorunEntry], p: Palette) -> Option<Act> {
        let mut act = None;
        // Each cell closure needs its own variable.
        let mut toggled = None;
        // Publisher goes first when the window is narrow, so no column runs into the next.
        const PUBLISHER: usize = 4;
        let wide = ui.available_width() >= 960.0;
        let mut columns = vec![
            Column::exact(56.0),
            Column::remainder().at_least(240.0).clip(true),
            Column::exact(140.0).clip(true),
            Column::exact(150.0).clip(true),
            Column::exact(160.0).clip(true),
            Column::exact(120.0),
        ];
        let mut headers = vec!["On", "Program", "Type", "Startup impact", "Publisher", ""];
        if !wide {
            columns.remove(PUBLISHER);
            headers.remove(PUBLISHER);
        }
        TableBuilder::new(ui)
            .sense(egui::Sense::click())
            .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
            .columns_in_order(columns)
            .header(26.0, |mut h| {
                for t in crate::gui::style::ordered(headers) {
                    h.col(|ui| {
                        ui.add(style::Label::new(style::muted(ui, tr(t)).size(12.5)));
                    });
                }
            })
            .body(|body| {
                body.rows(66.0, rows.len(), |mut row| {
                    let e = &rows[row.index()];
                    row.set_selected(self.focus.as_deref() == Some(&e.id));
                    let mut cells: Vec<Box<dyn FnOnce(&mut Ui) + '_>> = vec![
                        Box::new(|ui: &mut Ui| {
                            let mut on = e.enabled;
                            ui.add_enabled_ui(e.source != Source::RunOnce, |ui| {
                                if style::toggle(ui, &mut on, &crate::tf!("Start {} automatically", e.display_name())).changed() {
                                    toggled = Some(Act::Toggle(e.clone(), on));
                                }
                            });
                        }),
                        Box::new(|ui: &mut Ui| {
                            crate::gui::style::column(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.spacing_mut().interact_size.y = 18.0;
                                crate::gui::style::row(ui, |ui| {
                                    ui.add(style::Label::new(style::strong(e.display_name()).color(if e.enabled { p.text } else { p.faint })));
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
                                ui.add(style::Label::new(style::rich(&e.command).monospace().size(11.5).color(p.muted)).truncate()).on_hover_text(style::vis(&e.command));
                                ui.add(style::Label::new(style::rich(&e.location).size(11.5).color(p.faint)).truncate()).on_hover_text(style::vis(&e.location));
                            });
                        }),
                        Box::new(|ui: &mut Ui| {
                            ui.add(style::Label::new(style::muted(ui, tr(e.source.label()))).truncate()).on_hover_text(style::vis(tr(e.source.label())));
                        }),
                        Box::new(|ui: &mut Ui| {
                            crate::gui::style::column(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.spacing_mut().interact_size.y = 18.0;
                                impact_badge(ui, e, p);
                                ui.add(style::Label::new(style::muted(ui, impact_detail(e)).size(11.5)).truncate());
                            });
                        }),
                        Box::new(|ui: &mut Ui| {
                            ui.add(style::Label::new(style::muted(ui, e.publisher.as_deref().unwrap_or(""))).truncate());
                        }),
                        Box::new(|ui: &mut Ui| {
                            crate::gui::style::row(ui, |ui| {
                                let tip = if e.removable { tr("Remove (backed up)") } else { tr("Can't be deleted: disable it instead") };
                                ui.add_enabled_ui(e.removable, |ui| {
                                    if style::icon_button(ui, icon::TRASH, tip).clicked() {
                                        act = Some(Act::Remove(e.clone()));
                                    }
                                });
                                let r = style::icon_button(ui, icon::DOTS, "More");
                                egui::Popup::menu(&r).width(220.0).show(|ui| {
                                    if let Some(a) = entry_menu(ui, cx, e) {
                                        act = Some(a);
                                    }
                                });
                                if let Some(exe) = &e.exe {
                                    if style::icon_button(ui, icon::FOLDER_OPEN, "Open file location").clicked() {
                                        open_location(cx, exe);
                                    }
                                }
                            })
                            .inner
                        }),
                    ];
                    if !wide {
                        drop(cells.remove(PUBLISHER));
                    }
                    crate::gui::style::cells_in_order(&mut row, cells);
                    let r = row.response();
                    if r.clicked() {
                        act = Some(Act::Focus(e.id.clone()));
                    }
                    r.context_menu(|ui| {
                        if let Some(a) = entry_menu(ui, cx, e) {
                            act = Some(a);
                        }
                    });
                });
            });
        toggled.or(act)
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some(list) = self.confirm.clone() {
            let mut close = false;
            crate::gui::style::modal("autorun-remove", 460.0, ctx, |ui| {
                let title = if list.len() == 1 { crate::tf!("Remove \"{}\"?", list[0].display_name()) } else { crate::tf!("Remove {} entries?", list.len()) };
                ui.add(style::Label::new(style::strong(title).size(18.0)));
                ui.add_space(6.0);
                for e in list.iter().take(8) {
                    ui.add(style::Label::new(style::rich(crate::tf!("• {}: {}", e.display_name(), e.command)).size(12.5)).truncate());
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

/// A plain-language hint under the name: why to keep it, or what turning it off gains.
fn hint(e: &AutorunEntry, p: Palette) -> Option<(&'static str, Color32)> {
    if !e.valid {
        return Some(("Its program is gone: you can remove this", p.danger));
    }
    match e.startup.need {
        Need::System if cfg!(windows) => return Some(("Windows needs this: leave it on", p.muted)),
        Need::System => return Some(("Your system needs this: leave it on", p.muted)),
        Need::Hardware => return Some(("Helps your hardware work: best left on", p.muted)),
        Need::No => {}
    }
    match (e.enabled, e.startup.impact) {
        (true, Impact::High) => Some(("Turning this off makes startup faster", p.warning)),
        (true, Impact::Medium) => Some(("Turning this off makes startup a bit faster", p.muted)),
        _ => None,
    }
}

fn impact_color(i: Impact, p: Palette) -> Color32 {
    match i {
        Impact::High => p.danger,
        Impact::Medium => p.warning,
        Impact::Low => p.success,
        Impact::Unknown => p.muted,
    }
}

/// The impact as a badge: "≈" marks an estimate; an entry that is off is drawn faded.
fn impact_badge(ui: &mut Ui, e: &AutorunEntry, p: Palette) {
    let s = &e.startup;
    let label = tr(s.impact.label());
    let text = if s.measured || s.impact == Impact::Unknown { label.to_string() } else { crate::tf!("≈ {label}") };
    let color = if e.enabled { impact_color(s.impact, p) } else { p.faint };
    ui.scope(|ui| style::badge(ui, &text, color)).response.on_hover_text(style::vis(impact_detail(e)));
}

/// What the impact is based on, in a few words.
fn impact_detail(e: &AutorunEntry) -> String {
    let s = &e.startup;
    let mut parts = Vec::new();
    if let Some(ms) = s.delay_ms {
        parts.push(crate::tf!("Delayed startup by {:.1} s", ms as f64 / 1000.0));
    }
    if let Some(cpu) = s.cpu_ms {
        parts.push(crate::tf!("CPU {cpu} ms at startup"));
    }
    if let Some(disk) = s.disk_bytes.filter(|d| *d > 0) {
        parts.push(crate::tf!("{} read from disk", crate::backup::human_size(disk)));
    }
    if !parts.is_empty() {
        return parts.join(" · ");
    }
    match s.size {
        Some(b) if s.impact != Impact::Unknown => crate::tf!("Estimated from the program's size ({})", crate::backup::human_size(b)),
        _ => tr("No startup measurements for this entry").to_string(),
    }
}

/// Run now, Stop it, Open file location and Remove: the row's menu in both views.
fn entry_menu(ui: &mut Ui, cx: &mut Cx, e: &AutorunEntry) -> Option<Act> {
    let mut act = None;
    if style::ghost(ui, icon::PLAY, "Run now").clicked() {
        match autorun::run_now(e) {
            Ok(()) => cx.info(crate::tf!("Started {}", e.display_name())),
            Err(err) => cx.error(crate::tf!("{err:#}")),
        }
        ui.close();
    }
    ui.add_enabled_ui(e.running, |ui| {
        if style::ghost(ui, icon::X_CIRCLE, "Stop it").clicked() {
            match autorun::kill(e) {
                Ok(n) => cx.info(crate::tf!("Stopped {} ({n} process(es))", e.display_name())),
                Err(err) => cx.error(crate::tf!("{err:#}")),
            }
            act = Some(Act::Refresh);
            ui.close();
        }
    });
    if let Some(exe) = &e.exe {
        if style::ghost(ui, icon::FOLDER_OPEN, "Open file location").clicked() {
            open_location(cx, exe);
            ui.close();
        }
    }
    ui.add_enabled_ui(e.removable, |ui| {
        if style::ghost(ui, icon::TRASH, "Remove (backed up)").clicked() {
            act = Some(Act::Remove(e.clone()));
            ui.close();
        }
    });
    act
}

fn open_location(cx: &mut Cx, exe: &str) {
    let path = std::path::Path::new(exe);
    if let Err(err) = crate::platform::open_folder(path.parent().unwrap_or(path)) {
        cx.error(crate::tf!("{err:#}"));
    }
}
