//! Watcher page: pick keys/paths from a browser, manage the watch list, see events live.

use super::style::{self, icon, Palette};
use super::{tree::Browser, Cx};
use crate::watcher::{self, Watch, WatchEvent};
use eframe::egui::{self, Color32, RichText, Ui};
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
            Err(e) => cx.error(format!("{e:#}")),
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
        match f(&mut all).and_then(|msg| watcher::save_watches(&all).map(|_| msg).map_err(|e| format!("{e:#}"))) {
            Ok(m) => cx.info(m),
            Err(e) => cx.error(e),
        }
        self.last_reload = None;
    }

    fn add(&mut self, cx: &mut Cx) {
        let path = watcher::normalize_target(&self.add_path);
        let reads = self.add_reads;
        if self.add_path.trim().is_empty() {
            return cx.error("Pick or type a key/path to watch.");
        }
        self.edit(cx, |all| {
            if let Some(w) = all.iter_mut().find(|w| w.path.eq_ignore_ascii_case(&path)) {
                w.active = true;
                return Ok(format!("Resumed watching {path}"));
            }
            all.push(Watch { path: path.clone(), active: true, include_reads: reads, created: chrono::Local::now() });
            Ok(format!("Watching {path}"))
        });
        self.selected = Some(path);
        self.add_path.clear();
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        self.reload(cx);
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let p = Palette::get(ui.ctx());
        let running = watcher::recorder_running();
        let subtitle =
            if cfg!(windows) { "See which program changes which registry key, as it happens" } else { "See every change to config files and GNOME settings, as it happens" };
        let (mut start, mut login_changed) = (false, false);
        let status = self.at_login.get_or_insert_with(crate::platform::recorder_status).clone();
        let mut at_login = status.starts_with("starts at login");
        style::page_header(ui, "Watcher", subtitle, |ui| {
            if running {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(p.success.r(), p.success.g(), p.success.b(), 30))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("{}  Recording", icon::RECORD)).color(p.success));
                    });
            } else {
                start = style::primary(ui, icon::PLAY, "Start recording").on_hover_text("Record while CleanCore is open").clicked();
            }
            ui.add_space(6.0);
            ui.label("Start at login");
            login_changed = style::toggle(ui, &mut at_login, "Start recorder at login").changed();
        });
        if start {
            let settings = cx.settings.clone();
            std::thread::spawn(move || {
                let _ = watcher::run_recorder(&settings, |_| {});
            });
            self.in_app = true;
            cx.info("Recorder started");
        }
        if login_changed {
            let r = if at_login {
                crate::scheduler::exe_args(&["watch", "run", "--quiet"]).and_then(|(exe, args)| crate::platform::recorder_enable(&exe, &args))
            } else {
                crate::platform::recorder_disable()
            };
            match r {
                Ok(()) => cx.info(if at_login { "The recorder now starts at login" } else { "The recorder no longer starts at login" }),
                Err(e) => cx.error(format!("{e:#}")),
            }
            self.at_login = None;
        }

        let top_h = (ui.available_height() * 0.42).clamp(230.0, 360.0);
        let mut action = None;
        ui.columns(2, |cols| {
            // Add a watch
            let ui = &mut cols[0];
            style::card(ui, |ui| {
                ui.set_height(top_h);
                ui.horizontal(|ui| {
                    ui.label(style::strong("Add a watch"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if style::icon_button(ui, icon::REFRESH, "Reload").clicked() {
                            self.browser.reload();
                        }
                    });
                });
                egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(6).show(ui, |ui| {
                    ui.set_height(top_h - 100.0);
                    ui.set_width(ui.available_width());
                    self.browser.show(ui, "watch-browser");
                });
                if let Some(s) = self.browser.selected.take() {
                    self.add_path = s;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let picker = if cfg!(windows) { 0.0 } else { 2.0 * (style::CONTROL_H + 8.0) };
                    style::text_input(ui, &mut self.add_path, "Key or path to watch", ui.available_width() - 100.0 - picker);
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
                    ui.checkbox(&mut self.add_reads, "Also record reads (very noisy)");
                }
            });

            // Watched list
            let ui = &mut cols[1];
            style::card(ui, |ui| {
                ui.set_height(top_h);
                ui.horizontal(|ui| {
                    ui.label(style::strong("Watching"));
                    style::badge(ui, &self.watches.len().to_string(), p.accent);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = style::icon_button(ui, icon::EXPORT, "Export all");
                        egui::Popup::menu(&r).width(360.0).show(|ui| {
                            ui.label(style::muted(ui, "Folder (one CSV per watch)"));
                            style::path_input(ui, "watch-export-dir", &mut self.export_dir, "", 340.0, style::Pick::Folder);
                            if style::primary(ui, icon::EXPORT, "Export all").clicked() {
                                let dir = std::path::PathBuf::from(&self.export_dir);
                                let mut n = 0;
                                for w in &self.watches {
                                    let f = dir.join(format!("{}.csv", w.log_file().file_stem().unwrap().to_string_lossy()));
                                    if watcher::read_log(w).and_then(|ev| watcher::export_events(&ev, &f)).is_ok() {
                                        n += 1;
                                    }
                                }
                                cx.info(format!("Exported {n} log(s) to {}", dir.display()));
                                ui.close();
                            }
                        });
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("watched").auto_shrink([false, false]).show(ui, |ui| {
                    if self.watches.is_empty() {
                        style::empty_state(ui, icon::EYE, "Nothing watched yet", "Pick a key or folder on the left.");
                    }
                    for w in self.watches.clone() {
                        let sel = self.selected.as_deref() == Some(w.path.as_str());
                        if let Some(a) = self.watch_row(ui, &w, sel, p, cx) {
                            action = Some(a);
                        }
                    }
                });
            });
        });
        match action {
            Some(RowAction::Toggle(path)) => self.edit(cx, |all| {
                let w = all.iter_mut().find(|w| w.path == path).ok_or("not found")?;
                w.active = !w.active;
                Ok(format!("{} {path}", if w.active { "Resumed" } else { "Paused" }))
            }),
            Some(RowAction::Clear(w)) => {
                match watcher::clear_log(&w) {
                    Ok(()) => cx.info("Log cleared"),
                    Err(e) => cx.error(format!("{e:#}")),
                }
                self.events_for = None;
                self.last_reload = None;
            }
            Some(RowAction::Delete(w)) => {
                let _ = watcher::clear_log(&w);
                self.edit(cx, |all| {
                    all.retain(|x| x.path != w.path);
                    Ok(format!("Stopped watching {}", w.path))
                });
                self.selected = None;
            }
            None => {}
        }
        ui.add_space(12.0);
        self.results(ui, cx, p, running);
    }

    fn watch_row(&mut self, ui: &mut Ui, w: &Watch, sel: bool, p: Palette, cx: &mut Cx) -> Option<RowAction> {
        let mut out = None;
        let fill = if sel { p.accent_soft } else { Color32::TRANSPARENT };
        let resp = egui::Frame::new().fill(fill).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, col) = if w.active { (icon::RECORD, p.success) } else { (icon::PAUSE, p.faint) };
                ui.label(RichText::new(dot).color(col));
                let name = w.path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(&w.path).to_string();
                ui.vertical(|ui| {
                    // Leave room for the action icons of the selected row.
                    ui.set_max_width(ui.available_width() - if sel { 150.0 } else { 0.0 });
                    ui.label(RichText::new(name).color(if sel { p.accent } else { p.text }));
                    ui.add(egui::Label::new(RichText::new(&w.path).size(11.5).color(p.muted)).truncate());
                });
                if sel {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if style::icon_button(ui, icon::TRASH, "Stop watching").clicked() {
                            out = Some(RowAction::Delete(w.clone()));
                        }
                        if style::icon_button(ui, icon::X, "Clear log").clicked() {
                            out = Some(RowAction::Clear(w.clone()));
                        }
                        let r = style::icon_button(ui, icon::EXPORT, "Export log");
                        super::export_menu(&r, cx, &mut self.export_path, "File (.txt .csv .json)", |f| {
                            watcher::export_events(&watcher::read_log(w)?, f)?;
                            Ok(format!("Exported to {}", f.display()))
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
        style::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(style::strong("Events"));
                style::badge(ui, &self.events.len().to_string(), p.accent);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    style::search_field(ui, &mut self.highlight, "Highlight", 140.0);
                    style::search_field(ui, &mut self.filter, "Filter", 160.0);
                });
            });
            ui.add_space(6.0);
            if self.events.is_empty() {
                let text = if running { "Change something in a watched location and it shows up here." } else { "Start recording to capture changes." };
                style::empty_state(ui, icon::HISTORY, "No events yet", text);
                ui.add_space(ui.available_height().max(0.0));
                return;
            }
            let f = self.filter.to_lowercase();
            let h = self.highlight.to_lowercase();
            // Newest first, so live changes appear at the top.
            let rows: Vec<&WatchEvent> = self.events.iter().rev().filter(|e| f.is_empty() || e.haystack().contains(&f)).collect();
            let hl = Color32::from_rgba_unmultiplied(p.warning.r(), p.warning.g(), p.warning.b(), 40);
            let mut open: Option<String> = None;
            let mut clear = false;
            egui::ScrollArea::horizontal().id_salt("events-h").show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .auto_shrink([false, false])
                    .sense(egui::Sense::click())
                    .column(Column::exact(95.0))
                    .column(Column::initial(110.0).clip(true))
                    .column(Column::exact(55.0))
                    .column(Column::exact(110.0))
                    .column(Column::initial(230.0).clip(true))
                    .column(Column::initial(120.0).clip(true))
                    .column(Column::initial(110.0).clip(true))
                    .column(Column::initial(110.0).clip(true))
                    .column(Column::remainder().at_least(80.0))
                    .header(26.0, |mut hd| {
                        for t in ["Time", "Process", "PID", "Operation", "Key", "Value", "New data", "Old data", "Type"] {
                            hd.col(|ui| {
                                ui.label(RichText::new(t).size(12.5).color(p.muted));
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
                                row.col(|ui| {
                                    if lit {
                                        ui.painter().rect_filled(ui.max_rect(), 0.0, hl);
                                    }
                                    ui.add(egui::Label::new(RichText::new(c).color(col)).truncate()).on_hover_text(&e.key);
                                });
                            }
                            row.response().context_menu(|ui| {
                                if style::ghost(ui, icon::FOLDER_OPEN, if cfg!(windows) { "Open in Registry Editor" } else { "Open containing folder" }).clicked() {
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
                    cx.error(format!("{e:#}"));
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
