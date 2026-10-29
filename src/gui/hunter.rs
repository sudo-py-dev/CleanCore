//! Hunter Mode: drag the crosshair onto any window (or pick from the
//! running programs) to see what it is and uninstall it, stop it from auto-starting, kill it,
//! kill and delete it, or open its folder.

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::hunter::{self, Target};
use crate::i18n::tr;
use crate::platform::Proc;
use eframe::egui::{self, Ui, Vec2};
use std::time::{Duration, Instant};

/// What the App does for the Hunter page (switches to the Uninstaller).
pub enum HunterAction {
    Uninstall(Box<crate::uninstaller::Program>),
    /// Not a listed program: Forced Uninstall with its name and folder.
    Forced {
        name: String,
        folder: String,
    },
}

pub struct HunterView {
    procs: Vec<Proc>,
    loaded: bool,
    search: String,
    /// List every process of the desktop session, not just applications.
    all: bool,
    target: Option<Target>,
    finding: Option<Pending<Result<Option<Target>, String>>>,
    /// "Pick in 3 s" countdown end.
    countdown: Option<Instant>,
    confirm_delete: bool,
    /// The always-on-top target window is open (Hunter Mode with the main window hidden).
    floating: bool,
    tray: Option<super::tray::Tray>,
}

impl HunterView {
    pub fn new() -> Self {
        Self {
            procs: Vec::new(),
            loaded: false,
            search: String::new(),
            all: false,
            target: None,
            finding: None,
            countdown: None,
            confirm_delete: false,
            floating: false,
            tray: None,
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn identify_pid(&mut self, ctx: &egui::Context, cx: &Cx, pid: u32) {
        let s = cx.settings.clone();
        self.finding = Some(Pending::spawn(ctx, "Identifying", move || hunter::identify(pid, &s).map(Some).map_err(|e| crate::tf!("{e:#}"))));
    }

    pub fn active(&self) -> bool {
        self.floating || self.tray.is_some()
    }

    /// Enter Hunter Mode proper: minimize CleanCore, show the target window and the tray icon.
    pub fn start_floating(&mut self, ctx: &egui::Context) {
        self.floating = true;
        if self.tray.is_none() && std::env::var_os("CLEANCORE_NO_TRAY").is_none() {
            self.tray = super::tray::Tray::start(ctx);
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(true));
    }

    fn stop_floating(&mut self, ctx: &egui::Context) {
        self.floating = false;
        self.tray = None;
        show_main(ctx);
    }

    /// The floating target and the tray icon. Returns true when the main window should switch
    /// to Hunter Mode (a window was hunted, or "Open CleanCore" was chosen).
    pub fn floating_ui(&mut self, ctx: &egui::Context, cx: &mut Cx) -> bool {
        let mut to_main = false;
        while let Some(a) = self.tray.as_ref().and_then(|t| t.poll()) {
            match a {
                super::tray::TrayAction::ShowTarget => self.floating = true,
                super::tray::TrayAction::OpenMain => {
                    show_main(ctx);
                    to_main = true;
                }
                super::tray::TrayAction::Exit => {
                    self.stop_floating(ctx);
                    return true;
                }
            }
        }
        if !self.floating {
            return to_main;
        }
        let p = Palette::get(ctx);
        let (mut hunted, mut close, mut open_main, mut hide) = (false, false, false, false);
        let builder = egui::ViewportBuilder::default()
            .with_title("CleanCore Hunter")
            .with_inner_size([132.0, 150.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("cleancore-hunter-target"), builder, |ui, _class| {
            if ui.input(|i| i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.surface).stroke(egui::Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(6, 4))).show(ui, |ui| {
                // Title bar: all of it moves the window, except its two small buttons at the far end.
                let (bar, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::drag());
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if r.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                r.on_hover_text(style::vis(tr("Move")));
                let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(style::far_layout(egui::Align::Center)));
                buttons.spacing_mut().item_spacing.x = 2.0;
                close = bar_button(&mut buttons, icon::X, "Exit Hunter Mode", p).clicked() || close;
                hide = bar_button(&mut buttons, icon::APP_WINDOW, "Open CleanCore", p).clicked();

                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    // The target: a round button with a ring that answers hover, red while hunting.
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::drag());
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Floating hunter target")));
                    let hover = ui.ctx().animate_bool(r.id, r.hovered() || r.dragged());
                    let col = if r.dragged() { p.danger } else { p.accent };
                    let painter = ui.painter();
                    let radius = 30.0 + 2.0 * hover;
                    painter.circle_filled(rect.center(), radius, col.gamma_multiply(if p.dark { 0.22 } else { 0.12 } + 0.08 * hover));
                    painter.circle_stroke(rect.center(), radius, egui::Stroke::new(1.5, col.gamma_multiply(0.35 + 0.65 * hover)));
                    painter.text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::CROSSHAIR), egui::FontId::proportional(34.0 + 2.0 * hover), col);
                    if r.hovered() || r.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                    if r.drag_stopped() {
                        hunted = true;
                    }
                    ui.add_space(2.0);
                    ui.add(style::Label::new(style::rich(tr("Drag me")).size(12.0).color(if r.dragged() { p.danger } else { p.muted })));
                });
            });
        });
        if close {
            self.stop_floating(ctx);
            return true;
        }
        if hide {
            open_main = true;
        }
        if hunted {
            self.identify_pointer(ctx, cx);
            open_main = true;
        }
        if open_main {
            show_main(ctx);
            to_main = true;
        }
        to_main
    }

    fn identify_pointer(&mut self, ctx: &egui::Context, cx: &Cx) {
        let s = cx.settings.clone();
        self.finding = Some(Pending::spawn(ctx, "Identifying the window under the pointer", move || hunter::identify_at_pointer(&s).map_err(|e| crate::tf!("{e:#}"))));
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) -> Option<HunterAction> {
        if !self.loaded {
            self.procs = hunter::gui_processes(self.all);
            self.procs.sort_by_key(|p| p.name.to_lowercase());
            self.loaded = true;
        }
        if let Some(r) = take(&mut self.finding) {
            match r {
                Ok(Some(t)) => self.target = Some(t),
                Ok(None) => cx.error(tr("No other program's window is under the pointer.")),
                Err(e) => cx.error(e),
            }
        }
        if let Some(end) = self.countdown {
            if Instant::now() >= end {
                self.countdown = None;
                self.identify_pointer(ui.ctx(), cx);
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        let p = Palette::get(ui.ctx());
        let mut action = None;

        let mut float = false;
        style::page_header(ui, "Hunter Mode", "Point at any window to find out which program it belongs to", |ui| {
            float = style::primary(ui, icon::CROSSHAIR, "Floating target").on_hover_text(style::vis(tr("Hide CleanCore and keep a small target on top of every window"))).clicked();
            if style::icon_button(ui, icon::REFRESH, "Refresh running programs").clicked() {
                self.loaded = false;
            }
        });
        if float {
            self.start_floating(ui.ctx());
        }

        // Wide: the finder and the running programs in reading order first, what was found
        // beside them. Narrower: one column, with what was found right under the finder.
        let wide = ui.available_width() >= 760.0;
        if wide {
            let gap = 14.0;
            let left_w = ((ui.available_width() - gap) * 0.46).max(320.0);
            let h = ui.available_height();
            let near = if crate::gui::style::rtl() { egui::Align::Max } else { egui::Align::Min };
            crate::gui::style::row_top(ui, |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(Vec2::new(left_w, h), egui::Layout::top_down(near), |ui| {
                    ui.set_width(left_w);
                    self.finder_card(ui, cx, p);
                    ui.add_space(10.0);
                    self.programs_card(ui, cx, p, true);
                });
                ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), h), egui::Layout::top_down(near), |ui| {
                    egui::ScrollArea::vertical().id_salt("hunter-result").auto_shrink([false, false]).show(ui, |ui| {
                        action = self.result_panel(ui, cx, p);
                    });
                });
            });
        } else {
            // The result only once there is one (or it is being found), so the list stays in view.
            egui::ScrollArea::vertical().id_salt("hunter-page").auto_shrink([false, false]).show(ui, |ui| {
                self.finder_card(ui, cx, p);
                ui.add_space(10.0);
                if self.finding.is_some() || self.target.is_some() {
                    action = self.result_panel(ui, cx, p);
                    ui.add_space(10.0);
                }
                self.programs_card(ui, cx, p, false);
            });
        }

        if self.confirm_delete {
            if let Some(t) = self.target.clone() {
                let mut close = false;
                crate::gui::style::modal("hunter-kill-delete", 440.0, ui.ctx(), |ui| {
                    ui.add(style::Label::new(style::strong(crate::tf!("Close {} and delete its program file?", t.name)).size(18.0)));
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::rich(t.exe.clone().unwrap_or_default()).monospace().size(12.0)));
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::muted(ui, tr("Unsaved work in it is lost. The file is backed up and can be restored from Backups."))));
                    ui.add_space(12.0);
                    crate::gui::style::row(ui, |ui| {
                        if style::danger(ui, icon::TRASH, "Close and delete").clicked() {
                            match hunter::kill_and_delete(&t, cx.settings) {
                                Ok(id) => cx.info(crate::tf!("Deleted {}. Backup: {id}", t.name)),
                                Err(e) => cx.error(crate::tf!("{e:#}")),
                            }
                            self.target = None;
                            self.loaded = false;
                            close = true;
                        }
                        if style::secondary(ui, "", "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if close {
                    self.confirm_delete = false;
                }
            } else {
                self.confirm_delete = false;
            }
        }
        action
    }

    /// The crosshair to drag onto a window, and the timed pick for windows that can't take a drag.
    fn finder_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        style::card(ui, |ui| {
            crate::gui::style::row_top(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let size = 72.0;
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::drag());
                let dragging = resp.dragged();
                let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() || dragging, style::FADE);
                let col = if dragging { p.danger } else { p.accent };
                ui.painter().rect_filled(rect, 18.0, col.gamma_multiply(if p.dark { 0.25 } else { 0.14 } + 0.08 * hover));
                if dragging {
                    ui.painter().rect_stroke(rect, 18.0, egui::Stroke::new(2.0, col), egui::StrokeKind::Inside);
                }
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::CROSSHAIR), egui::FontId::proportional(40.0 + 2.0 * hover), col);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Hunter target: drag onto a window")));
                if resp.hovered() || dragging {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                if resp.drag_stopped() {
                    self.identify_pointer(ui.ctx(), cx);
                }
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let title = if dragging { tr("Now let go over the window") } else { tr("Drag the target onto a window") };
                    ui.add(style::Label::new(style::strong(title).size(15.0)).wrap());
                    ui.add(style::Label::new(style::muted(ui, tr("Hold the mouse button on the crosshair, move it over another program's window and let go.")).size(12.5)).wrap());
                    ui.add_space(2.0);
                    match self.countdown {
                        Some(end) => {
                            let left = end.saturating_duration_since(Instant::now()).as_secs_f32().ceil();
                            ui.add(style::Label::new(style::rich(crate::tf!("{}  Move the pointer over the window… {left:.0}", icon::CLOCK)).color(p.accent)));
                        }
                        None => {
                            if style::secondary(ui, icon::CLOCK, "Pick in 3 seconds").on_hover_text(style::vis(tr("For windows you can't drag onto"))).clicked() {
                                self.countdown = Some(Instant::now() + Duration::from_secs(3));
                            }
                        }
                    }
                    if cfg!(not(windows)) && std::env::var_os("WAYLAND_DISPLAY").is_some() {
                        ui.add(
                            style::Label::new(
                                style::rich(crate::tf!("{}  Wayland doesn't allow reading other windows; pick from the list below.", icon::INFO)).size(12.0).color(p.warning),
                            )
                            .wrap(),
                        );
                    }
                });
            });
        });
    }

    /// The running programs to pick from, with a filter. `scroll`: the list scrolls within
    /// the rest of the column (else it is part of the page's scrolling).
    fn programs_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette, scroll: bool) {
        let q = self.search.to_lowercase();
        let shown: Vec<Proc> =
            self.procs.iter().filter(|x| q.is_empty() || x.name.to_lowercase().contains(&q) || x.title.as_deref().unwrap_or("").to_lowercase().contains(&q)).cloned().collect();
        let mut pick = None;
        style::card(ui, |ui| {
            if scroll {
                ui.set_min_height(ui.available_height() - 34.0);
            }
            crate::gui::style::row(ui, |ui| {
                ui.add(style::Label::new(style::strong(tr("Running programs"))));
                style::badge(ui, &shown.len().to_string(), p.accent);
            });
            ui.add_space(4.0);
            // The switch keeps its room at the far end; the filter takes the rest.
            crate::gui::style::row(ui, |ui| {
                let key = egui::Id::unique("hunter-all-switch");
                let switch_w = ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(150.0);
                // (`search_field` adds 48 for its icon and clear button.)
                style::search_field(ui, &mut self.search, "Filter", (ui.available_width() - switch_w - 12.0 - 48.0).max(100.0));
                let r = ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    ui.add(style::Label::new(style::muted(ui, tr("All processes"))).extend());
                    if style::toggle(ui, &mut self.all, "Show all processes").changed() {
                        self.loaded = false;
                    }
                    ui.min_rect().width()
                });
                if (r.inner - switch_w).abs() > 0.5 {
                    ui.ctx().data_mut(|d| d.insert_temp(key, r.inner));
                }
            });
            ui.add_space(6.0);
            let mut rows = |ui: &mut Ui| {
                if shown.is_empty() {
                    ui.add(style::Label::new(style::muted(ui, tr("No running programs match."))));
                }
                for pr in &shown {
                    let selected = self.target.as_ref().is_some_and(|t| t.pids.contains(&pr.pid));
                    if program_row(ui, pr, selected, p).clicked() {
                        pick = Some(pr.pid);
                    }
                }
            };
            if scroll {
                egui::ScrollArea::vertical().id_salt("hunter-procs").auto_shrink([false, false]).show(ui, rows);
            } else {
                rows(ui);
            }
        });
        if let Some(pid) = pick {
            self.identify_pid(ui.ctx(), cx, pid);
        }
    }

    /// What was found: identifying, the target and its actions, or how to find one.
    fn result_panel(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) -> Option<HunterAction> {
        if let Some(f) = &self.finding {
            style::card(ui, |ui| {
                ui.add_space(8.0);
                f.show_progress(ui);
                ui.add_space(8.0);
            });
            return None;
        }
        match self.target.clone() {
            Some(t) => self.target_panel(ui, cx, &t, p),
            None => {
                style::card(ui, |ui| {
                    style::empty_state(ui, icon::CROSSHAIR, "No target yet", "Drag the crosshair onto a window or pick a running program.");
                });
                None
            }
        }
    }

    fn target_panel(&mut self, ui: &mut Ui, cx: &mut Cx, t: &Target, p: Palette) -> Option<HunterAction> {
        let mut action = None;
        style::card(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                style::icon_tile(ui, icon::APP_WINDOW, p.accent, 44.0);
                crate::gui::style::column(ui, |ui| {
                    ui.add(style::Label::new(style::strong(t.program.as_ref().map(|x| x.name.clone()).unwrap_or(t.name.clone())).size(18.0)));
                    let n = t.pids.len();
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("{} · {n} process(es)", t.name)).size(12.5)));
                });
            });
            ui.add_space(10.0);
            let row = |ui: &mut Ui, k: &str, v: &str| {
                crate::gui::style::row(ui, |ui| {
                    ui.add_sized([90.0, 18.0], style::Label::new(style::muted(ui, k).size(12.5)));
                    ui.add(style::Label::new(style::rich(v).size(12.5)).wrap());
                });
            };
            if let Some(w) = &t.title {
                row(ui, "Window", w);
            }
            if let Some(e) = &t.exe {
                row(ui, "File", e);
            }
            if let Some(pb) = &t.publisher {
                row(ui, "Publisher", pb);
            }
            match &t.program {
                Some(pr) => row(ui, "Installed as", &crate::tf!("{} {} ({})", pr.name, pr.version, tr(pr.kind.label()))),
                None => row(ui, "Installed as", "Not in the list of installed programs"),
            }
            if t.autoruns.is_empty() {
                row(ui, "Autorun", "Doesn't start automatically");
            }
            for a in &t.autoruns {
                row(ui, "Autorun", &crate::tf!("{} · {}{}", a.name, tr(a.source.label()), if a.enabled { "" } else { tr(" (disabled)") }));
            }
        });
        ui.add_space(10.0);
        style::card(ui, |ui| {
            style::section_title(ui, "Actions");
            ui.add_space(4.0);
            crate::gui::style::row(ui, |ui| {
                if style::primary(ui, icon::TRASH, "Uninstall").on_hover_text(style::vis(tr("Uninstall the program and scan for leftovers"))).clicked() {
                    action = Some(match &t.program {
                        Some(pr) => HunterAction::Uninstall(Box::new(pr.clone())),
                        None => HunterAction::Forced {
                            name: t.name.clone(),
                            folder: t.exe.as_deref().and_then(|e| std::path::Path::new(e).parent()).map(|d| d.display().to_string()).unwrap_or_default(),
                        },
                    });
                }
                let can_stop = t.autoruns.iter().any(|a| a.enabled);
                ui.add_enabled_ui(can_stop, |ui| {
                    if style::secondary(ui, icon::PAUSE, "Stop autorun").on_hover_text(style::vis(tr("Don't start this program automatically"))).clicked() {
                        match hunter::stop_autostart(t) {
                            Ok(n) => cx.info(crate::tf!("Disabled {n} startup entry(ies)")),
                            Err(e) => cx.error(crate::tf!("{e:#}")),
                        }
                        self.identify_pid(ui.ctx(), cx, t.pid);
                    }
                });
                if style::secondary(ui, icon::X_CIRCLE, "Kill process").on_hover_text(style::vis(tr("Force-close it; unsaved work is lost"))).clicked() {
                    match hunter::kill(t) {
                        Ok(n) => cx.info(crate::tf!("Stopped {n} process(es)")),
                        Err(e) => cx.error(crate::tf!("{e:#}")),
                    }
                    self.target = None;
                    self.loaded = false;
                }
            });
            ui.add_space(4.0);
            crate::gui::style::row(ui, |ui| {
                ui.add_enabled_ui(t.exe.is_some(), |ui| {
                    if style::danger(ui, icon::TRASH, "Kill and delete").clicked() {
                        self.confirm_delete = true;
                    }
                    if style::secondary(ui, icon::FOLDER_OPEN, "Open folder").clicked() {
                        if let Err(e) = hunter::open_folder(t) {
                            cx.error(crate::tf!("{e:#}"));
                        }
                    }
                });
                if style::secondary(ui, icon::GLOBE, "Search online").on_hover_text(style::vis(tr("Look the program up on the web"))).clicked() {
                    if let Err(e) = hunter::search_online(t) {
                        cx.error(crate::tf!("{e:#}"));
                    }
                }
            });
        });
        action
    }
}

/// One running program in the list: its name and window title (or file), cut to fit.
fn program_row(ui: &mut Ui, pr: &Proc, selected: bool, p: Palette) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &pr.name));
    let hover = ui.ctx().animate_bool_with_time(r.id.with("hover"), r.hovered(), style::FADE);
    if selected {
        ui.painter().rect_filled(rect, 9.0, p.accent_soft);
    } else if hover > 0.0 {
        ui.painter().rect_filled(rect, 9.0, p.hover.gamma_multiply(hover));
    }
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mr = crate::gui::style::Mirror::new(rect);
    let cy = rect.center().y;
    let tile = mr.rect(8.0, cy - 15.0, Vec2::splat(30.0));
    ui.painter().rect_filled(tile, 8.0, if selected { p.surface } else { p.surface_alt });
    ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::APP_WINDOW), egui::FontId::proportional(16.0), if selected { p.accent } else { p.muted });
    let room = (rect.width() - 48.0 - 10.0).max(20.0);
    let (name, _) = style::one_line(ui, &pr.name, egui::FontId::new(13.5, style::semibold()), if selected { p.accent } else { p.text }, room);
    let sub = pr.title.clone().filter(|t| !t.trim().is_empty()).or(pr.exe.clone()).unwrap_or_default();
    let (sub, _) = style::one_line(ui, &sub, egui::FontId::proportional(11.5), p.muted, room);
    ui.painter().galley(egui::pos2(mr.galley_x(48.0, name.size().x), cy - 1.0 - name.size().y), name, p.text);
    ui.painter().galley(egui::pos2(mr.galley_x(48.0, sub.size().x), cy + 1.0), sub, p.muted);
    r
}

/// Bring the main window back from the taskbar.
/// A small icon button for the floating target's title bar (the usual ones would crowd it).
fn bar_button(ui: &mut Ui, glyph: &str, tip: &str, p: Palette) -> egui::Response {
    let tip = tr(tip);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let t = ui.ctx().animate_bool(r.id, r.hovered());
    if t > 0.0 {
        ui.painter().rect_filled(rect, 6.0, p.surface_alt.lerp_to_gamma(p.border, 0.5).gamma_multiply(t));
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(glyph), egui::FontId::proportional(14.0), p.faint.lerp_to_gamma(p.text, t));
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip.clone()));
    r.on_hover_text(style::vis(tip))
}

fn show_main(ctx: &egui::Context) {
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
}
