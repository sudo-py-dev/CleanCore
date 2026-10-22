//! Desktop GUI (egui/eframe): a sidebar with the tools, each tool a page with its own header
//! and actions, and toast notifications in the corner.

mod appicons;
mod autorun;
mod backups;
mod cleaner;
mod dupes;
mod extensions;
mod history;
mod hunter;
mod junk;
mod logsdb;
mod options;
mod schedule;
mod shred;
mod slop;
pub mod style;
mod systools;
mod tools;
mod traced;
mod tray;
mod tree;
mod tuner;
mod uninstaller;
mod updates;
mod watcher;

use crate::config::Settings;
use crate::i18n::tr;
use eframe::egui::{self, Ui, Vec2};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use style::{icon, Palette};

pub fn run() -> eframe::Result {
    // Windows: Direct3D 12 first (see Cargo.toml), OpenGL if it can't start. Elsewhere OpenGL.
    #[cfg(windows)]
    let renderers = [eframe::Renderer::Wgpu, eframe::Renderer::Glow];
    #[cfg(not(windows))]
    let renderers = [eframe::Renderer::Glow];
    let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut result = Ok(());
    for renderer in renderers {
        let options = eframe::NativeOptions {
            viewport: {
                let v = egui::ViewportBuilder::default().with_inner_size([1200.0, 780.0]).with_min_inner_size([960.0, 600.0]).with_title("CleanCore").with_app_id("cleancore");
                match appicons::window_icon() {
                    Some(icon) => v.with_icon(icon),
                    None => v,
                }
            },
            renderer,
            ..Default::default()
        };
        let s = started.clone();
        result = eframe::run_native(
            "CleanCore",
            options,
            Box::new(move |cc| {
                s.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(Box::new(App::new(cc)))
            }),
        );
        // Try the next renderer only when this one couldn't draw at all: never open the app twice.
        if result.is_ok() || started.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
    }
    result
}

// ---------------------------------------------------------------- background work

/// Result of work running on a background thread, polled once per frame.
pub struct Pending<T> {
    pub label: String,
    rx: Receiver<T>,
    started: Instant,
}

impl<T: Send + 'static> Pending<T> {
    pub fn spawn(ctx: &egui::Context, label: impl Into<String>, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        Self { label: label.into(), rx, started: Instant::now() }
    }

    pub fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// Seconds since the work started.
    pub fn elapsed(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    pub fn show_progress(&self, ui: &mut Ui) {
        let label = crate::i18n::tr(&self.label);
        let p = Palette::get(ui.ctx());
        crate::gui::style::column(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            crate::gui::style::row(ui, |ui| {
                ui.add(egui::Spinner::new().size(16.0).color(p.accent));
                ui.add(style::Label::new(label));
                ui.add(style::Label::new(style::rich(crate::tf!("{:.0}s", self.started.elapsed().as_secs_f32())).color(p.faint)));
            });
            style::busy_bar(ui, ui.available_width().clamp(120.0, 360.0));
        });
    }
}

pub fn take<T: Send + 'static>(slot: &mut Option<Pending<T>>) -> Option<T> {
    let v = slot.as_ref()?.poll()?;
    *slot = None;
    Some(v)
}

// ---------------------------------------------------------------- shared context

pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: chrono::DateTime<chrono::Local>,
    shown: Instant,
}

/// What every page gets: the settings and a place to report messages.
pub struct Cx<'a> {
    pub settings: &'a mut Settings,
    pub notices: &'a mut Vec<Notice>,
}

impl Cx<'_> {
    pub fn info(&mut self, t: impl Into<String>) {
        self.notices.push(Notice { text: t.into(), error: false, at: chrono::Local::now(), shown: Instant::now() });
    }
    pub fn error(&mut self, t: impl std::fmt::Display) {
        self.notices.push(Notice { text: t.to_string(), error: true, at: chrono::Local::now(), shown: Instant::now() });
    }
    pub fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.error(crate::tf!("Saving settings failed: {e:#}"));
        }
    }
}

pub use crate::backup::human_size as human;

/// The inside of an export menu: what can be written, a path field and the button. On click it
/// runs `export` (which returns the success message), reports the outcome and closes the menu.
pub fn export_fields(ui: &mut Ui, cx: &mut Cx, path: &mut String, formats: &str, export: impl FnOnce(&std::path::Path) -> anyhow::Result<String>) {
    ui.add(style::Label::new(style::muted(ui, formats)));
    style::path_input(ui, "export", path, "", 340.0, style::Pick::Save);
    if style::primary(ui, icon::EXPORT, "Export").clicked() {
        match export(std::path::Path::new(path.as_str())) {
            Ok(msg) => cx.info(msg),
            Err(e) => cx.error(crate::tf!("Export failed: {e:#}")),
        }
        ui.close();
    }
}

/// An export menu opened by `button`; see `export_fields`.
pub fn export_menu(button: &egui::Response, cx: &mut Cx, path: &mut String, formats: &str, export: impl FnOnce(&std::path::Path) -> anyhow::Result<String>) {
    egui::Popup::menu(button).width(360.0).show(|ui| export_fields(ui, cx, path, formats, export));
}

pub fn export_dir() -> std::path::PathBuf {
    dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir)
}

/// Apply the theme and accent stored in settings.
pub fn apply_theme(ctx: &egui::Context, s: &Settings) {
    style::apply(ctx, s.gui_theme.as_deref().unwrap_or("light"), s.gui_accent.as_deref().unwrap_or("blue"));
}

// ---------------------------------------------------------------- app

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Uninstaller,
    Autorun,
    Hunter,
    Cleaner,
    Tuner,
    Watcher,
    Backups,
    Tools,
    Settings,
}

impl Tool {
    /// Whether this build's platform can use the tool. Hunter Mode needs to pick other
    /// programs' windows, which Linux desktops (Wayland) don't allow, so it is Windows only.
    pub fn supported(self) -> bool {
        self != Tool::Hunter || cfg!(windows)
    }

    /// The sidebar, grouped.
    pub const SECTIONS: [(&'static str, &'static [Tool]); 3] = [
        ("Programs", &[Tool::Uninstaller, Tool::Autorun, Tool::Hunter]),
        ("Clean up", &[Tool::Cleaner, Tool::Tools, Tool::Tuner]),
        ("Monitor & restore", &[Tool::Watcher, Tool::Backups]),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Uninstaller => "Uninstaller",
            Tool::Autorun => "Autorun",
            Tool::Hunter => "Hunter Mode",
            Tool::Cleaner => "Cleaner",
            Tool::Tuner => "Tuner",
            Tool::Watcher => "Watcher",
            Tool::Backups => "Backups",
            Tool::Tools => "Tools",
            Tool::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Uninstaller => icon::TRASH,
            Tool::Autorun => icon::ROCKET,
            Tool::Hunter => icon::CROSSHAIR,
            Tool::Cleaner => icon::BROOM,
            Tool::Tuner => icon::SLIDERS,
            Tool::Watcher => icon::EYE,
            Tool::Backups => icon::ARCHIVE,
            Tool::Tools => icon::WRENCH,
            Tool::Settings => icon::GEAR,
        }
    }
}

pub struct App {
    settings: Settings,
    tool: Tool,
    notices: Vec<Notice>,
    seen_notices: usize,
    uninstaller: uninstaller::UninstallerView,
    autorun: autorun::AutorunView,
    hunter: hunter::HunterView,
    cleaner: cleaner::CleanerView,
    tuner: tuner::TunerView,
    watcher: watcher::WatcherView,
    backups: backups::BackupView,
    tools: tools::ToolsView,
    /// What the command line asked for (`--trace FILE`, `--uninstall PATH`), done on the first frame.
    startup: Option<(String, String)>,
    /// Today's automatic full backup, made in the background at start.
    daily: Option<Pending<anyhow::Result<Option<String>>>>,
    updates: updates::Updates,
    options: options::SettingsView,
    /// The page drawn last frame and when it appeared, for the page fade-in.
    shown: Option<Tool>,
    page_since: f64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut notices = Vec::new();
        let settings = Settings::load().unwrap_or_else(|e| {
            notices.push(Notice { text: crate::tf!("Settings could not be read, using defaults: {e:#}"), error: true, at: chrono::Local::now(), shown: Instant::now() });
            Settings::default()
        });
        crate::i18n::set_language(&settings.language);
        style::install_fonts(&cc.egui_ctx);
        let s = settings.clone();
        let daily = settings.daily_full_backup.then(|| Pending::spawn(&cc.egui_ctx, "Daily backup", move || crate::backup::daily_full_if_due(&s)));
        apply_theme(&cc.egui_ctx, &settings);
        let mut updates = updates::Updates::new();
        updates.startup(&cc.egui_ctx, &settings);
        Self {
            options: options::SettingsView::new(&settings),
            settings,
            // `cleancore --hunter` (or /hunter) opens Hunter Mode.
            tool: if Tool::Hunter.supported() && std::env::args().skip(1).any(|a| a == "--hunter" || a.eq_ignore_ascii_case("/hunter")) { Tool::Hunter } else { Tool::Uninstaller },
            notices,
            seen_notices: 0,
            uninstaller: uninstaller::UninstallerView::new(),
            autorun: autorun::AutorunView::new(),
            hunter: hunter::HunterView::new(),
            cleaner: cleaner::CleanerView::new(),
            tuner: tuner::TunerView::new(),
            watcher: watcher::WatcherView::new(),
            backups: backups::BackupView::new(),
            tools: tools::ToolsView::new(),
            startup: {
                let a: Vec<String> = std::env::args().skip(1).collect();
                a.iter().position(|x| x == "--trace" || x == "--uninstall").and_then(|i| Some((a[i].clone(), a.get(i + 1)?.clone())))
            },
            daily,
            updates,
            shown: None,
            page_since: 0.0,
        }
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn switch(&mut self, t: Tool) {
        if self.tool == t || !t.supported() {
            return;
        }
        self.tool = t;
        match t {
            Tool::Backups => self.backups.refresh(),
            Tool::Tools => self.tools.refresh(),
            Tool::Tuner => self.tuner.refresh(),
            Tool::Watcher => self.watcher.refresh(),
            Tool::Settings => self.options = options::SettingsView::new(&self.settings),
            Tool::Uninstaller => self.uninstaller.refresh(),
            Tool::Autorun => self.autorun.refresh(),
            Tool::Hunter => self.hunter.refresh(),
            Tool::Cleaner => {}
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let p = Palette::get(ui.ctx());
        // The selected entry's pill is painted here, behind the entries, once we know where it is.
        let indicator = ui.painter().add(egui::Shape::Noop);
        let mut selected_rect = None;
        ui.add_space(18.0);
        crate::gui::style::row(ui, |ui| {
            ui.add_space(6.0);
            style::brand(ui, 36.0);
            ui.add_space(2.0);
            crate::gui::style::column(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space(1.0);
                ui.add(style::Label::new(style::strong(tr("CleanCore")).size(17.0)));
                ui.add(style::Label::new(style::muted(ui, tr("Uninstaller & cleaner")).size(12.0)));
            });
        });
        ui.add_space(12.0);

        // Bottom block first, as its own panel: the tool list above it scrolls when the window
        // is short instead of running underneath it.
        let mut selected_clip = ui.clip_rect();
        egui::Panel::bottom("sidebar-bottom").frame(egui::Frame::NONE).show_separator_line(false).resizable(false).show(ui, |ui| {
            ui.add_space(8.0);
            let rec = crate::watcher::recorder_running();
            // The recorder's state, with the theme switch and notifications at its far end.
            egui::Frame::new().fill(p.surface_alt).corner_radius(10).inner_margin(egui::Margin { left: 10, right: 2, top: 1, bottom: 1 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                    if rec {
                        // A slow pulse while recording.
                        let phase = (ui.input(|i| i.time) / 1.6).fract() as f32;
                        let a = ((1.0 - phase) * 90.0) as u8;
                        ui.painter().circle_filled(r.center(), 4.0 + 5.0 * phase, egui::Color32::from_rgba_unmultiplied(p.success.r(), p.success.g(), p.success.b(), a));
                        ui.ctx().request_repaint_after(Duration::from_millis(33));
                    }
                    ui.painter().circle_filled(r.center(), 4.0, if rec { p.success } else { p.faint });
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        self.notifications_button(ui);
                        let dark = p.dark;
                        if style::icon_button(ui, if dark { icon::SUN } else { icon::MOON }, if dark { tr("Switch to light") } else { tr("Switch to dark") }).clicked() {
                            self.settings.gui_theme = Some(if dark { "light" } else { "dark" }.into());
                            apply_theme(ui.ctx(), &self.settings);
                            let _ = self.settings.save();
                        }
                        ui.with_layout(crate::gui::style::near_layout(egui::Align::Center), |ui| {
                            ui.add(style::Label::new(style::rich(if rec { tr("Recorder running") } else { tr("Recorder stopped") }).size(12.0).color(p.muted)).truncate());
                        });
                    });
                });
            });
            ui.add_space(4.0);
            // Settings: a full-width entry like the ones above, so it lines up with them.
            let r = style::nav_item(ui, Tool::Settings.icon(), Tool::Settings.label(), self.tool == Tool::Settings);
            if self.tool == Tool::Settings {
                selected_rect = Some(r.rect);
                selected_clip = ui.clip_rect();
            }
            if r.clicked() {
                self.switch(Tool::Settings);
            }
            ui.add_space(8.0);
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (section, tools) in Tool::SECTIONS {
                let tools = tools.iter().copied().filter(|t| t.supported());
                ui.add_space(10.0);
                crate::gui::style::row(ui, |ui| {
                    ui.add_space(14.0);
                    ui.add(style::Label::new(style::rich(crate::i18n::tr(section).to_uppercase()).size(11.0).family(style::semibold()).color(p.faint)));
                });
                ui.add_space(4.0);
                for t in tools {
                    let r = style::nav_item(ui, t.icon(), t.label(), self.tool == t);
                    if self.tool == t {
                        selected_rect = Some(r.rect);
                        selected_clip = ui.clip_rect();
                    }
                    if r.clicked() {
                        self.switch(t);
                    }
                }
            }
        });
        if let Some(r) = selected_rect {
            style::nav_indicator(ui, indicator, r, selected_clip);
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }

    fn notifications_button(&mut self, ui: &mut Ui) {
        let unread = self.notices.len().saturating_sub(self.seen_notices);
        let tip = if unread > 0 { crate::tf!("Notifications ({unread} new)") } else { "Notifications".to_string() };
        let r = style::icon_button(ui, icon::BELL, &tip);
        if unread > 0 {
            let p = Palette::get(ui.ctx());
            // Inside the button, with a ring in the background color so it reads as a badge.
            let c = r.rect.right_top() + Vec2::new(-9.0, 9.0);
            ui.painter().circle_filled(c, 4.5, p.surface_alt);
            ui.painter().circle_filled(c, 3.5, p.danger);
        }
        egui::Popup::menu(&r).width(380.0).show(|ui| {
            self.seen_notices = self.notices.len();
            ui.add(style::Label::new(style::strong(tr("Notifications"))));
            ui.add_space(4.0);
            if self.notices.is_empty() {
                ui.add(style::Label::new(style::muted(ui, tr("Nothing yet."))));
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for n in self.notices.iter().rev() {
                    notice_row(ui, n);
                    ui.add_space(4.0);
                }
            });
            if !self.notices.is_empty() && style::ghost(ui, icon::TRASH, "Clear all").clicked() {
                self.notices.clear();
                self.seen_notices = 0;
            }
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let live: Vec<&Notice> = self.notices.iter().filter(|n| n.shown.elapsed() < Duration::from_secs(4)).collect();
        if live.is_empty() {
            return;
        }
        let p = Palette::get(ctx);
        let shadow = egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: egui::Color32::from_black_alpha(if p.dark { 100 } else { 32 }) };
        // The newest toast slides in from the right; each fades out at the end of its time.
        let newest = live.last().map_or(1.0, |n| egui::emath::easing::cubic_out((n.shown.elapsed().as_secs_f32() / 0.28).min(1.0)));
        let id = egui::Id::unique("toasts");
        ctx.set_transform_layer(egui::LayerId::new(egui::Order::Foreground, id), egui::emath::TSTransform::from_translation(Vec2::new((1.0 - newest) * 60.0, 0.0)));
        egui::Area::new(id).anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-20.0, -20.0)).order(egui::Order::Foreground).interactable(false).show(ctx, |ui| {
            for n in live.iter().rev().take(2).rev() {
                let age = n.shown.elapsed().as_secs_f32();
                let fade = ((age / 0.2).min(1.0)).min(((4.0 - age) / 0.35).clamp(0.0, 1.0));
                ui.set_opacity(fade);
                let accent = if n.error { p.danger } else { p.success };
                let r = egui::Frame::new()
                    .fill(p.surface)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .shadow(shadow)
                    .show(ui, |ui| {
                        // One width for every toast, so stacked toasts line up.
                        ui.set_width(360.0);
                        notice_row(ui, n);
                    });
                // A coloured edge on the left says success or error at a glance.
                let edge = egui::Rect::from_min_size(r.response.rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, r.response.rect.height() - 20.0));
                ui.painter().rect_filled(edge, 1.5, accent);
                ui.add_space(8.0);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn notice_row(ui: &mut Ui, n: &Notice) {
    let p = Palette::get(ui.ctx());
    crate::gui::style::row_top(ui, |ui| {
        let (ic, col) = if n.error { (icon::WARNING, p.danger) } else { (icon::CHECK_CIRCLE, p.success) };
        ui.add(style::Label::new(style::rich(ic).size(17.0).color(col)));
        crate::gui::style::column(ui, |ui| {
            ui.add(style::Label::new(&n.text).wrap());
            ui.add(style::Label::new(style::rich(n.at.format("%H:%M:%S").to_string()).size(11.5).color(p.faint)));
        });
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Palette::get(&ctx);
        // The sidebar sits at the start of the reading direction: right for right-to-left.
        let side = if style::rtl() { egui::Panel::right("sidebar") } else { egui::Panel::left("sidebar") };
        side.resizable(false)
            .exact_size(236.0)
            .frame(egui::Frame::new().fill(p.surface).stroke(egui::Stroke::new(1.0, p.border)).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 28, right: 28, top: 22, bottom: 18 })).show(ui, |ui| {
            // Pages are laid out from the start edge of the reading direction.
            let mut page =
                ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()).layout(egui::Layout::top_down(if style::rtl() { egui::Align::Max } else { egui::Align::Min })));
            let ui = &mut page;
            // A new page fades in and settles from slightly below.
            let now = ui.input(|i| i.time);
            if self.shown != Some(self.tool) {
                self.shown = Some(self.tool);
                self.page_since = now;
            }
            let t = egui::emath::easing::cubic_out((((now - self.page_since) as f32) / 0.24).clamp(0.0, 1.0));
            if t < 1.0 {
                ui.multiply_opacity(t);
                ui.add_space((1.0 - t) * 10.0);
                ui.ctx().request_repaint();
            }
            let mut cx = Cx { settings: &mut self.settings, notices: &mut self.notices };
            if let Some((flag, path)) = self.startup.take() {
                self.tool = Tool::Uninstaller;
                if flag == "--trace" {
                    self.uninstaller.trace_installer(ui.ctx(), &cx, &path);
                } else {
                    self.uninstaller.uninstall_path(ui.ctx(), &cx, &path);
                }
            }
            match self.tool {
                Tool::Uninstaller => self.uninstaller.main(ui, &mut cx),
                Tool::Autorun => self.autorun.main(ui, &mut cx),
                Tool::Hunter => match self.hunter.main(ui, &mut cx) {
                    Some(hunter::HunterAction::Uninstall(p)) => {
                        self.tool = Tool::Uninstaller;
                        self.uninstaller.uninstall_from_hunter(ui.ctx(), &cx, *p);
                    }
                    Some(hunter::HunterAction::Forced { name, folder }) => {
                        self.tool = Tool::Uninstaller;
                        self.uninstaller.forced_from_hunter(name, folder);
                    }
                    None => {}
                },
                Tool::Cleaner => self.cleaner.main(ui, &mut cx),
                Tool::Tuner => self.tuner.main(ui, &mut cx),
                Tool::Watcher => self.watcher.main(ui, &mut cx),
                Tool::Backups => self.backups.main(ui, &mut cx),
                Tool::Tools => self.tools.main(ui, &mut cx),
                Tool::Settings => self.options.main(ui, &mut cx, &mut self.updates),
            }
            self.updates.poll(&mut cx);
        });
        match take(&mut self.daily) {
            Some(Ok(Some(id))) => self.notices.push(Notice { text: crate::tf!("Daily full backup created ({id})"), error: false, at: chrono::Local::now(), shown: Instant::now() }),
            Some(Err(e)) => self.notices.push(Notice { text: crate::tf!("Daily backup failed: {e:#}"), error: true, at: chrono::Local::now(), shown: Instant::now() }),
            _ => {}
        }
        if self.hunter.active() {
            let mut cx = Cx { settings: &mut self.settings, notices: &mut self.notices };
            if self.hunter.floating_ui(&ctx, &mut cx) {
                self.tool = Tool::Hunter;
            }
        }
        self.toasts(&ctx);
        if self.settings.gui_theme.is_none() {
            first_run(&ctx, &mut self.settings);
        }
    }
}

fn first_run(ctx: &egui::Context, settings: &mut Settings) {
    let mut chosen = None;
    crate::gui::style::modal("first-run-theme", 420.0, ctx, |ui| {
        style::brand(ui, 48.0);
        ui.add_space(10.0);
        ui.add(style::Label::new(style::strong(tr("Welcome to CleanCore")).size(20.0)));
        ui.add(style::Label::new(style::muted(ui, tr("Pick a look. You can change it any time in Settings."))));
        ui.add_space(14.0);
        crate::gui::style::row(ui, |ui| {
            if style::secondary(ui, icon::SUN, "Light").clicked() {
                chosen = Some("light");
            }
            if style::secondary(ui, icon::MOON, "Dark").clicked() {
                chosen = Some("dark");
            }
            if style::secondary(ui, icon::MONITOR, "System").clicked() {
                chosen = Some("system");
            }
        });
    });
    if let Some(t) = chosen {
        settings.gui_theme = Some(t.into());
        apply_theme(ctx, settings);
        let _ = settings.save();
    }
}
