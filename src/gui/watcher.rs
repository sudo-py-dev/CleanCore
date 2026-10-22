//! Watcher page: choose places to watch, record, and read what changed in plain words.
//! Three steps across the top say what to do next; the detailed event table is one click away.

use super::style::{self, icon, Palette};
use super::{tree::Browser, Cx};
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::watcher::{self, Watch, WatchEvent};
use eframe::egui::{self, Color32, Ui};
use egui_extras::{Column, TableBuilder};
use std::time::{Duration, Instant};

pub struct WatcherView {
    watches: Vec<Watch>,
    last_reload: Option<Instant>,
    selected: Option<String>,
    browser: Browser,
    add_path: String,
    add_reads: bool,
    events: Vec<WatchEvent>,
    events_for: Option<String>,
    log_len: u64,
    filter: String,
    highlight: String,
    export_path: String,
    export_dir: String,
    in_app: bool,
    at_login: Option<String>,
    /// The full key / folder browser (quick picks cover the usual places).
    show_browser: bool,
    /// Detailed table instead of the readable list of changes.
    table_view: bool,
}

enum RowAction {
    Toggle(String),
    Clear(Watch),
    Delete(Watch),
}

impl WatcherView {
    pub fn new() -> Self {
        let dir = super::export_dir();
        Self {
            watches: Vec::new(),
            last_reload: None,
            selected: None,
            browser: Browser::default(),
            add_path: String::new(),
            add_reads: false,
            events: Vec::new(),
            events_for: None,
            log_len: 0,
            filter: String::new(),
            highlight: String::new(),
            export_path: dir.join("cleancore-watch.csv").display().to_string(),
            export_dir: dir.join("cleancore-watch-logs").display().to_string(),
            in_app: false,
            at_login: None,
            show_browser: false,
            table_view: false,
        }
    }

    pub fn refresh(&mut self) {
        self.last_reload = None;
        self.at_login = None;
    }

    fn selected_watch(&self) -> Option<&Watch> {
        self.selected.as_ref().and_then(|p| self.watches.iter().find(|w| &w.path == p))
    }

    fn reload(&mut self, cx: &mut Cx) {
        if self.last_reload.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.last_reload = Some(Instant::now());
        match watcher::load_watches() {
            Ok(w) => self.watches = w,
            Err(e) => cx.error(crate::tf!("{e:#}")),
        }
        if self.selected_watch().is_none() {
            self.selected = self.watches.first().map(|w| w.path.clone());
        }
        // Re-read the selected log only when it grew (or another watch got selected).
        if let Some(w) = self.selected_watch().cloned() {
            let len = std::fs::metadata(w.log_file()).map(|m| m.len()).unwrap_or(0);
            if self.events_for.as_deref() != Some(w.path.as_str()) || len != self.log_len {
                self.events = watcher::read_log(&w).unwrap_or_default();
                self.events_for = Some(w.path.clone());
                self.log_len = len;
            }
        } else {
            self.events.clear();
            self.events_for = None;
        }
    }

    fn edit(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Vec<Watch>) -> Result<String, String>) {
        let mut all = watcher::load_watches().unwrap_or_default();
        match f(&mut all).and_then(|msg| watcher::save_watches(&all).map(|_| msg).map_err(|e| crate::tf!("{e:#}"))) {
            Ok(m) => cx.info(m),
            Err(e) => cx.error(e),
        }
        self.last_reload = None;
    }

    fn add(&mut self, cx: &mut Cx) {
        let path = watcher::normalize_target(&self.add_path);
        let reads = self.add_reads;
        if self.add_path.trim().is_empty() {
            return cx.error(tr("Pick or type a key/path to watch."));
        }
        self.edit(cx, |all| {
            if let Some(w) = all.iter_mut().find(|w| w.path.eq_ignore_ascii_case(&path)) {
                w.active = true;
                return Ok(crate::tf!("Resumed watching {path}"));
            }
            all.push(Watch { path: path.clone(), active: true, include_reads: reads, created: chrono::Local::now() });
            Ok(crate::tf!("Watching {path}"))
        });
        self.selected = Some(path);
        self.add_path.clear();
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        self.reload(cx);
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let p = Palette::get(ui.ctx());
        let running = watcher::recorder_running();
        let subtitle = if cfg!(windows) {
            tr("Find out exactly what a program changes in the registry")
        } else {
            tr("Find out exactly what a program changes in your settings and config files")
        };
        let (mut start, mut login_changed) = (false, false);
        let status = self.at_login.get_or_insert_with(crate::platform::recorder_status).clone();
        let mut at_login = status.starts_with("starts at login");
        style::page_header(ui, "Watcher", subtitle, |ui| {
            if running {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(p.success.r(), p.success.g(), p.success.b(), 30))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.add(style::Label::new(style::rich(crate::tf!("{}  Recording", icon::RECORD)).color(p.success).family(style::semibold())));
                    });
            } else {
                start = style::primary(ui, icon::PLAY, "Start recording").on_hover_text(style::vis(tr("Record changes while CleanCore is open"))).clicked();
            }
            ui.add_space(6.0);
            login_changed = style::toggle(ui, &mut at_login, "Start recorder at login")
                .on_hover_text(style::vis(tr("Keep recording in the background, even when CleanCore is closed")))
                .changed();
            ui.add(style::Label::new(tr("Record in the background")));
        });
        if start {
            let settings = cx.settings.clone();
            std::thread::spawn(move || {
                let _ = watcher::run_recorder(&settings, |_| {});
            });
            self.in_app = true;
            cx.info(tr("Recorder started"));
        }
        if login_changed {
            let r = if at_login {
                crate::scheduler::exe_args(&["watch", "run", "--quiet"]).and_then(|(exe, args)| crate::platform::recorder_enable(&exe, &args))
            } else {
                crate::platform::recorder_disable()
            };
            match r {
                Ok(()) => cx.info(if at_login { tr("The recorder now runs in the background from login") } else { tr("The recorder no longer runs in the background") }),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            }
            self.at_login = None;
        }

        // How it works: three steps that tick off as they're done.
        let watching = self.watches.iter().any(|w| w.active);
        let gap = 10.0;
        let sw = ((ui.available_width() - 2.0 * gap) / 3.0).max(150.0);
        crate::gui::style::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            // Where things are depends on the reading direction (the layout is mirrored).
            let rtl = crate::gui::style::rtl();
            step_tile(ui, sw, 1, "Choose what to watch", if rtl { "Use a quick pick on the right" } else { "Use a quick pick on the left" }, watching, p);
            step_tile(ui, sw, 2, "Start recording", if rtl { "The button at the top left" } else { "The button at the top right" }, running, p);
            step_tile(ui, sw, 3, "Use your program", "Its changes appear below", !self.events.is_empty(), p);
        });
        ui.add_space(12.0);

        let avail = ui.available_size();
        let left_w = (avail.x * 0.38).clamp(290.0, 420.0);
        let mut action = None;
        crate::gui::style::row_top(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.allocate_ui_with_layout(egui::vec2(left_w, avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                style::card(ui, |ui| {
                    ui.set_min_height(avail.y - 34.0);
                    action = self.watch_panel(ui, cx, p);
                });
            });
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), avail.y), egui::Layout::top_down(egui::Align::Min), |ui| {
                self.results(ui, cx, p, running);
            });
        });
        match action {
            Some(RowAction::Toggle(path)) => self.edit(cx, |all| {
                let w = all.iter_mut().find(|w| w.path == path).ok_or("not found")?;
                w.active = !w.active;
                Ok(crate::tf!("{} {path}", if w.active { tr("Resumed") } else { tr("Paused") }))
            }),
            Some(RowAction::Clear(w)) => {
                match watcher::clear_log(&w) {
                    Ok(()) => cx.info(tr("Changes cleared")),
                    Err(e) => cx.error(crate::tf!("{e:#}")),
                }
                self.events_for = None;
                self.last_reload = None;
            }
            Some(RowAction::Delete(w)) => {
                let _ = watcher::clear_log(&w);
                self.edit(cx, |all| {
                    all.retain(|x| x.path != w.path);
                    Ok(crate::tf!("Stopped watching {}", w.path))
                });
                self.selected = None;
            }
            None => {}
        }
    }

    /// Left side: quick picks, a path field, the full browser on demand, and what is watched.
    fn watch_panel(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) -> Option<RowAction> {
        style::section_title(ui, "What to watch");
        ui.add(style::Label::new(style::muted(ui, tr("Pick a place a program is likely to change, or choose your own.")).size(12.5)));
        ui.add_space(6.0);
        crate::gui::style::row_wrapped(ui, |ui| {
            for (ic, label, path) in quick_picks() {
                if style::secondary(ui, ic, label).on_hover_text(style::vis(&path)).clicked() {
                    self.add_path = path;
                    self.add(cx);
                }
            }
        });
        ui.add_space(6.0);
        // Narrow: the field gets the whole width, its buttons go under it.
        let picker = if cfg!(windows) { 0.0 } else { 2.0 * (style::CONTROL_H + 6.0) };
        let split = ui.available_width() - 96.0 - picker < 200.0;
        if split {
            style::text_input(ui, &mut self.add_path, if cfg!(windows) { tr("A registry key") } else { tr("A folder, file or dconf: path") }, ui.available_width());
        }
        crate::gui::style::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if !split {
                style::text_input(
                    ui,
                    &mut self.add_path,
                    if cfg!(windows) { tr("A registry key") } else { tr("A folder, file or dconf: path") },
                    ui.available_width() - 96.0 - picker,
                );
            }
            if !cfg!(windows) {
                if let Some(p) = style::browse_button(ui, "watch-add", style::Pick::Any, &self.add_path) {
                    self.add_path = p;
                }
            }
            if style::primary(ui, icon::PLUS, "Watch").clicked() {
                self.add(cx);
            }
        });
        if cfg!(windows) {
            ui.checkbox(&mut self.add_reads, style::vis(tr("Also record reads (very noisy)")));
        }
        let label = if self.show_browser { tr("Hide all locations") } else { tr("Browse all locations") };
        crate::gui::style::row(ui, |ui| {
            if style::ghost(ui, if self.show_browser { icon::X } else { icon::TREE }, label).clicked() {
                self.show_browser = !self.show_browser;
            }
            if self.show_browser && style::icon_button(ui, icon::REFRESH, "Reload").clicked() {
                self.browser.reload();
            }
        });
        if self.show_browser {
            egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(6).show(ui, |ui| {
                ui.set_height(170.0);
                ui.set_width(ui.available_width());
                self.browser.show(ui, "watch-browser");
            });
            if let Some(s) = self.browser.selected.take() {
                self.add_path = s;
            }
        }
        ui.add_space(8.0);
        style::divider(ui);
        ui.add_space(8.0);
        crate::gui::style::row(ui, |ui| {
            ui.add(style::Label::new(style::strong(tr("Watching"))));
            style::badge(ui, &self.watches.len().to_string(), p.accent);
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                let r = style::icon_button(ui, icon::EXPORT, "Export all");
                egui::Popup::menu(&r).width(360.0).show(|ui| {
                    ui.add(style::Label::new(style::muted(ui, tr("Folder (one CSV per watched place)"))));
                    style::path_input(ui, "watch-export-dir", &mut self.export_dir, "", 340.0, style::Pick::Folder);
                    if style::primary(ui, icon::EXPORT, "Export all").clicked() {
                        let dir = std::path::PathBuf::from(&self.export_dir);
                        let mut n = 0;
                        for w in &self.watches {
                            let f = dir.join(crate::tf!("{}.csv", w.log_file().file_stem().unwrap().to_string_lossy()));
                            if watcher::read_log(w).and_then(|ev| watcher::export_events(&ev, &f)).is_ok() {
                                n += 1;
                            }
                        }
                        cx.info(crate::tf!("Exported {n} log(s) to {}", dir.display()));
                        ui.close();
                    }
                });
            });
        });
        ui.add_space(4.0);
        let mut action = None;
        egui::ScrollArea::vertical().id_salt("watched").auto_shrink([false, false]).show(ui, |ui| {
            if self.watches.is_empty() {
                ui.add_space(10.0);
                ui.add(style::Label::new(style::muted(ui, tr("Nothing yet. Use a quick pick above to start."))));
            }
            for w in self.watches.clone() {
                let sel = self.selected.as_deref() == Some(w.path.as_str());
                if let Some(a) = self.watch_row(ui, &w, sel, p, cx) {
                    action = Some(a);
                }
            }
        });
        action
    }

    fn watch_row(&mut self, ui: &mut Ui, w: &Watch, sel: bool, p: Palette, cx: &mut Cx) -> Option<RowAction> {
        let mut out = None;
        let fill = if sel { p.accent_soft } else { Color32::TRANSPARENT };
        let resp = egui::Frame::new().fill(fill).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            crate::gui::style::row(ui, |ui| {
                let (dot, col, state) = if w.active { (icon::RECORD, p.success, "watching") } else { (icon::PAUSE, p.faint, "paused") };
                ui.add(style::Label::new(style::rich(dot).color(col))).on_hover_text(style::vis(state));
                crate::gui::style::column(ui, |ui| {
                    // Leave room for the action icons of the selected row.
                    ui.set_max_width((ui.available_width() - if sel { 150.0 } else { 0.0 }).max(60.0));
                    ui.add(style::Label::new(style::rich(watch_name(&w.path)).color(if sel { p.accent } else { p.text })).truncate());
                    ui.add(style::Label::new(style::rich(friendly_path(&w.path)).size(11.5).color(p.muted)).truncate());
                });
                if sel {
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::icon_button(ui, icon::TRASH, "Stop watching").clicked() {
                            out = Some(RowAction::Delete(w.clone()));
                        }
                        if style::icon_button(ui, icon::X, "Clear its changes").clicked() {
                            out = Some(RowAction::Clear(w.clone()));
                        }
                        let r = style::icon_button(ui, icon::EXPORT, "Export its changes");
                        super::export_menu(&r, cx, &mut self.export_path, "File (.txt .csv .json)", |f| {
                            watcher::export_events(&watcher::read_log(w)?, f)?;
                            Ok(crate::tf!("Exported to {}", f.display()))
                        });
                        let (ic, tip) = if w.active { (icon::PAUSE, "Pause watching") } else { (icon::PLAY, "Resume watching") };
                        if style::icon_button(ui, ic, tip).clicked() {
                            out = Some(RowAction::Toggle(w.path.clone()));
                        }
                    });
                }
            });
        });
        let r = resp.response.interact(egui::Sense::click());
        if r.clicked() && !sel {
            self.selected = Some(w.path.clone());
            self.last_reload = None;
        }
        out
    }

    fn results(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette, running: bool) {
        let fill_h = ui.available_height();
        style::card(ui, |ui| {
            ui.set_min_height(fill_h - 34.0);
            // Narrow: the filter goes on its own line under the title.
            let narrow = ui.available_width() < 470.0;
            crate::gui::style::row(ui, |ui| {
                ui.add(style::Label::new(style::strong(tr("Changes"))));
                style::badge(ui, &self.events.len().to_string(), p.accent);
                if let Some(w) = self.selected_watch() {
                    ui.add(style::Label::new(style::rich(crate::tf!("in {}", watch_name(&w.path))).size(12.5).color(p.muted)).truncate());
                }
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    if style::segment(ui, self.table_view, "Details").on_hover_text(style::vis(tr("Every field, as a table"))).clicked() {
                        self.table_view = true;
                    }
                    if style::segment(ui, !self.table_view, "Simple").clicked() {
                        self.table_view = false;
                    }
                    if !narrow {
                        ui.add_space(6.0);
                        style::search_field(ui, &mut self.filter, "Filter", 120.0);
                    }
                });
            });
            if narrow {
                style::search_field(ui, &mut self.filter, "Filter changes", ui.available_width() - 48.0);
            }
            ui.add_space(6.0);
            if self.events.is_empty() {
                let (title, text) = if self.watches.is_empty() {
                    (
                        "Nothing to show yet",
                        if crate::gui::style::rtl() {
                            "Choose what to watch on the right, then start recording."
                        } else {
                            "Choose what to watch on the left, then start recording."
                        },
                    )
                } else if !running {
                    (
                        "Not recording",
                        if crate::gui::style::rtl() {
                            "Press Start recording at the top left, then use your program."
                        } else {
                            "Press Start recording at the top right, then use your program."
                        },
                    )
                } else {
                    ("Waiting for changes", "Use your program now: install it, change a setting, close it. Changes appear here.")
                };
                style::empty_state(ui, if running { icon::RECORD } else { icon::HISTORY }, title, text);
                return;
            }
            let f = self.filter.to_lowercase();
            // Newest first, so live changes appear at the top.
            let rows: Vec<&WatchEvent> = self.events.iter().rev().filter(|e| f.is_empty() || e.haystack().contains(&f)).collect();
            if !self.table_view {
                let mut open: Option<String> = None;
                egui::ScrollArea::vertical().id_salt("changes").auto_shrink([false, false]).show(ui, |ui| {
                    for (i, e) in rows.iter().enumerate() {
                        if i > 0 {
                            style::divider(ui);
                        }
                        if change_row(ui, e, p) {
                            open = Some(e.key.clone());
                        }
                    }
                });
                if let Some(k) = open {
                    if let Err(e) = crate::platform::open_in_editor(k.trim_start_matches("dconf:")) {
                        cx.error(crate::tf!("{e:#}"));
                    }
                }
                return;
            }
            crate::gui::style::row(ui, |ui| {
                style::search_field(ui, &mut self.highlight, "Highlight rows", 160.0);
            });
            let h = self.highlight.to_lowercase();
            let hl = Color32::from_rgba_unmultiplied(p.warning.r(), p.warning.g(), p.warning.b(), 40);
            let mut open: Option<String> = None;
            let mut clear = false;
            egui::ScrollArea::horizontal().id_salt("events-h").show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .auto_shrink([false, false])
                    .sense(egui::Sense::click())
                    .columns_in_order(vec![
                        Column::exact(95.0),
                        Column::initial(110.0).clip(true),
                        Column::exact(55.0),
                        Column::exact(110.0),
                        Column::initial(230.0).clip(true),
                        Column::initial(120.0).clip(true),
                        Column::initial(110.0).clip(true),
                        Column::initial(110.0).clip(true),
                        Column::remainder().at_least(80.0),
                    ])
                    .header(26.0, |mut hd| {
                        for t in crate::gui::style::ordered(vec!["Time", "Process", "PID", "Operation", "Key", "Value", "New data", "Old data", "Type"]) {
                            hd.col(|ui| {
                                ui.add(style::Label::new(style::rich(tr(t)).size(12.5).color(p.muted)));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(26.0, rows.len(), |mut row| {
                            let e = rows[row.index()];
                            let lit = !h.is_empty() && e.haystack().contains(&h);
                            let op_color = match e.op {
                                watcher::Op::DeletedKey | watcher::Op::DeletedValue => p.danger,
                                watcher::Op::CreatedKey => p.success,
                                _ => p.text,
                            };
                            let cells = [
                                (e.time.format("%H:%M:%S%.3f").to_string(), p.muted),
                                (e.process.clone(), p.text),
                                (e.pid.map(|p| p.to_string()).unwrap_or_default(), p.muted),
                                (e.op.label().to_string(), op_color),
                                (e.key.clone(), p.text),
                                (e.value.clone().map(|v| if v.is_empty() { "(Default)".into() } else { v }).unwrap_or_default(), p.text),
                                (e.new_data.clone().unwrap_or_default(), p.text),
                                (e.old_data.clone().unwrap_or_default(), p.muted),
                                (e.data_type.clone().unwrap_or_default(), p.muted),
                            ];
                            for (c, col) in cells {
                                crate::gui::style::cells_in_order(
                                    &mut row,
                                    vec![Box::new(|ui: &mut Ui| {
                                        if lit {
                                            ui.painter().rect_filled(ui.max_rect(), 0.0, hl);
                                        }
                                        ui.add(style::Label::new(style::rich(c).color(col)).truncate()).on_hover_text(style::vis(&e.key));
                                    })],
                                );
                            }
                            row.response().context_menu(|ui| {
                                if style::ghost(ui, icon::FOLDER_OPEN, if cfg!(windows) { tr("Open in Registry Editor") } else { tr("Open containing folder") }).clicked() {
                                    open = Some(e.key.clone());
                                    ui.close();
                                }
                                if style::ghost(ui, icon::X, "Clear log").clicked() {
                                    clear = true;
                                    ui.close();
                                }
                            });
                        });
                    });
            });
            if let Some(k) = open {
                if let Err(e) = crate::platform::open_in_editor(k.trim_start_matches("dconf:")) {
                    cx.error(crate::tf!("{e:#}"));
                }
            }
            if clear {
                if let Some(w) = self.selected_watch().cloned() {
                    let _ = watcher::clear_log(&w);
                    self.events_for = None;
                    self.last_reload = None;
                }
            }
        });
    }
}

/// One of the three "how it works" steps; ticked when done.
fn step_tile(ui: &mut Ui, width: f32, n: usize, title: &str, text: &str, done: bool, p: Palette) {
    let (title, text) = (tr(title), tr(text));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 58.0), egui::Sense::hover());
    let fill = if done { p.surface.lerp_to_gamma(p.success, if p.dark { 0.10 } else { 0.06 }) } else { p.surface };
    ui.painter().rect(rect, 12.0, fill, egui::Stroke::new(1.0, if done { p.success.gamma_multiply(0.5) } else { p.border }), egui::StrokeKind::Inside);
    let mr = crate::gui::style::Mirror::new(rect);
    let c = mr.pos(26.0, rect.center().y);
    if done {
        ui.painter().circle_filled(c, 12.0, p.success);
        ui.painter().text(c, egui::Align2::CENTER_CENTER, style::vstr(icon::CHECK), egui::FontId::proportional(13.0), Color32::WHITE);
    } else {
        ui.painter().circle_stroke(c, 11.0, egui::Stroke::new(1.5, p.accent));
        ui.painter().text(c, egui::Align2::CENTER_CENTER, style::vstr(n.to_string()), egui::FontId::new(12.5, style::semibold()), p.accent);
    }
    let painter = ui.painter().with_clip_rect(rect.shrink(4.0));
    painter.text(mr.pos(48.0, rect.center().y - 9.0), mr.start(egui::Align::Center), style::vstr(title), egui::FontId::new(14.0, style::semibold()), p.text);
    painter.text(mr.pos(48.0, rect.center().y + 10.0), mr.start(egui::Align::Center), style::vstr(text), egui::FontId::proportional(12.0), p.muted);
}

/// The usual places programs keep their settings, for one-click watching.
fn quick_picks() -> Vec<(&'static str, &'static str, String)> {
    if cfg!(windows) {
        vec![
            (icon::USER, "Your programs", r"HKEY_CURRENT_USER\Software".to_string()),
            (icon::USERS, "All programs", r"HKEY_LOCAL_MACHINE\SOFTWARE".to_string()),
            (icon::ROCKET, "Startup", r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run".to_string()),
        ]
    } else {
        let mut v = Vec::new();
        if let Some(c) = dirs::config_dir() {
            v.push((icon::GEAR, "App settings", c.display().to_string()));
        }
        if let Some(d) = dirs::data_dir() {
            v.push((icon::FOLDER, "App data", d.display().to_string()));
        }
        v.push((icon::SLIDERS, "Desktop settings", "dconf:/".to_string()));
        v
    }
}

/// A short name for a watched place: its last part, or a description of a root.
fn watch_name(path: &str) -> String {
    if path == "dconf:/" {
        return "Desktop settings (dconf)".into();
    }
    path.trim_start_matches("dconf:").rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(path).to_string()
}

/// A path with the home folder shown as `~`, and dconf paths labelled.
fn friendly_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("dconf:") {
        return crate::tf!("Desktop setting {rest}");
    }
    match dirs::home_dir().map(|h| h.display().to_string()) {
        Some(h) if !h.is_empty() && path.starts_with(&h) => crate::tf!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

/// What happened, in plain words: icon, colour and verb.
fn describe(e: &WatchEvent, p: Palette) -> (&'static str, Color32, &'static str) {
    use watcher::Op;
    match e.op {
        Op::CreatedKey => (icon::PLUS, p.success, "was created"),
        Op::DeletedKey => (icon::TRASH, p.danger, "was deleted"),
        Op::RenamedKey => (icon::TAG, p.accent, "was renamed"),
        Op::ValueSet if e.old_data.is_none() => (icon::PLUS, p.success, "was added"),
        Op::ValueSet => (icon::REFRESH, p.accent, "was changed"),
        Op::DeletedValue => (icon::TRASH, p.danger, "was removed"),
        Op::SetInformationKey => (icon::REFRESH, p.accent, "was updated"),
        _ => (icon::EYE, p.muted, "was read"),
    }
}

/// One change as a readable row: what changed, from what to what, where, when and by whom.
/// Returns true when "Open" was picked.
fn change_row(ui: &mut Ui, e: &WatchEvent, p: Palette) -> bool {
    let (ic, col, verb) = describe(e, p);
    let what = match e.value.as_deref() {
        Some("") => "(Default)".to_string(),
        Some(v) => v.to_string(),
        None => watch_name(&e.key),
    };
    let mut open = false;
    let r = ui
        .horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 9.0, col.gamma_multiply(0.14));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(ic), egui::FontId::proportional(15.0), col);
            crate::gui::style::column(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.set_width((ui.available_width() - 110.0).max(140.0));
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.add(style::Label::new(style::strong(&what)).truncate());
                    ui.add(style::Label::new(style::rich(tr(verb)).color(p.muted)));
                });
                // Old and new values, when the change has them.
                let short = |s: &str| if s.chars().count() > 60 { s.chars().take(59).collect::<String>() + "…" } else { s.to_string() };
                match (e.old_data.as_deref(), e.new_data.as_deref()) {
                    (Some(old), Some(new)) => {
                        crate::gui::style::row(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(style::Label::new(style::rich(short(old)).monospace().size(12.0).color(p.danger).strikethrough()));
                            ui.add(style::Label::new(style::rich(crate::gui::style::icon_forward()).size(12.0).color(p.faint)));
                            ui.add(style::Label::new(style::rich(short(new)).monospace().size(12.0).color(p.success)));
                        });
                    }
                    (None, Some(new)) if !new.is_empty() => {
                        ui.add(style::Label::new(style::rich(short(new)).monospace().size(12.0).color(p.success)));
                    }
                    (Some(old), None) if !old.is_empty() => {
                        ui.add(style::Label::new(style::rich(short(old)).monospace().size(12.0).color(p.danger).strikethrough()));
                    }
                    _ => {}
                }
                ui.add(style::Label::new(style::rich(friendly_path(&e.key)).size(11.5).color(p.faint)).truncate()).on_hover_text(style::vis(&e.key));
            });
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Min), |ui| {
                crate::gui::style::column(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                        ui.add(style::Label::new(style::rich(e.time.format("%H:%M:%S").to_string()).size(12.0).color(p.muted)));
                        if !e.process.is_empty() {
                            style::badge(ui, &e.process, p.accent);
                        }
                    });
                });
            });
        })
        .response;
    r.interact(egui::Sense::click()).context_menu(|ui| {
        if style::ghost(ui, icon::FOLDER_OPEN, if cfg!(windows) { tr("Open in Registry Editor") } else { tr("Open containing folder") }).clicked() {
            open = true;
            ui.close();
        }
    });
    ui.add_space(2.0);
    open
}
