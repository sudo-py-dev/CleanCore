//! Cleaner page: scan modes, custom/forced scans, results grouped by category, cleaning.

use super::schedule::{Kind, TaskPanel};
use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::config::ListEntry;
use crate::engine::{self, Selection};
use crate::i18n::tr;
use crate::model::{Issue, ScanMode, Scope};
use eframe::egui::{self, collapsing_header::CollapsingState, Ui, Vec2};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// Icon, title, description and depth (1-3) of a scan mode.
fn mode_meta(m: ScanMode) -> (&'static str, &'static str, &'static str, usize) {
    match m {
        ScanMode::Safe => (icon::SHIELD_CHECK, tr("Safe Scan"), tr("Only entries that are certainly safe to delete. The fastest scan."), 1),
        ScanMode::Moderate => (icon::GAUGE, tr("Moderate Scan"), tr("Looks deeper for invalid and orphaned entries."), 2),
        ScanMode::Advanced => (icon::ROCKET, tr("Advanced Scan"), tr("Exhaustive. Review the results before you clean."), 3),
    }
}

/// A running scan's plan and the checks that have finished, filled in from worker threads.
struct Progress {
    /// Fresh per scan, so the progress ring and bar start from empty.
    id: egui::Id,
    planned: Vec<&'static str>,
    done: Arc<Mutex<Vec<&'static str>>>,
    glyph: &'static str,
}

#[derive(PartialEq, Eq)]
enum Screen {
    Home,
    Managed,
    Forced,
    Results,
    Auto,
}

struct Confirm {
    restore_point: bool,
    full_backup: bool,
}

pub struct CleanerView {
    screen: Screen,
    all_users: bool,
    managed: BTreeSet<String>,
    forced_root: String,
    forced_text: String,
    scan: Option<Pending<Result<Vec<Issue>, String>>>,
    clean: Option<Pending<Result<crate::cleaner::CleanReport, String>>>,
    /// Quick Clean: clean everything as soon as the scan finishes.
    quick: bool,
    /// The result groups are open (the toolbar's Expand / Collapse all).
    expanded: bool,
    descs: HashMap<&'static str, &'static str>,
    /// The home screen's "Clean without reviewing" switch: the next scan is a Quick Clean.
    clean_now: bool,
    results: Vec<Issue>,
    checked: Vec<bool>,
    show_details: bool,
    set_open: Option<bool>,
    confirm: Option<Confirm>,
    export_path: String,
    auto: TaskPanel,
    names: HashMap<&'static str, &'static str>,
    order: Vec<&'static str>,
    scan_label: String,
    /// Summary of the last clean, shown when nothing is left.
    last_clean: Option<String>,
    /// The scan depth picked on the home screen.
    mode: ScanMode,
    progress: Option<Progress>,
}

impl CleanerView {
    pub fn new() -> Self {
        let checks = engine::all_checks();
        Self {
            screen: Screen::Home,
            all_users: false,
            managed: checks.iter().filter(|c| c.id != "include_list").map(|c| c.id.to_string()).collect(),
            forced_root: if cfg!(windows) { r"HKEY_CURRENT_USER\Software".into() } else { dirs::config_dir().map(|p| p.display().to_string()).unwrap_or_default() },
            forced_text: String::new(),
            scan: None,
            clean: None,
            quick: false,
            expanded: true,
            descs: checks.iter().map(|c| (c.id, c.description)).collect(),
            clean_now: false,
            results: Vec::new(),
            checked: Vec::new(),
            show_details: false,
            set_open: None,
            confirm: None,
            export_path: super::export_dir().join("cleancore-scan.csv").display().to_string(),
            auto: TaskPanel::new(Kind::Clean),
            names: checks.iter().map(|c| (c.id, c.name)).chain([("forced", "Forced scan")]).collect(),
            order: checks.iter().map(|c| c.id).chain(["forced"]).collect(),
            scan_label: String::new(),
            last_clean: None,
            mode: ScanMode::Safe,
            progress: None,
        }
    }

    fn busy(&self) -> bool {
        self.scan.is_some() || self.clean.is_some()
    }

    fn start_scan(&mut self, ctx: &egui::Context, cx: &Cx, sel: Selection, label: String) {
        let settings = cx.settings.clone();
        let scope = if self.all_users { Scope::AllUsers } else { Scope::CurrentUser };
        self.scan_label = label.clone();
        self.last_clean = None;
        let glyph = match &sel {
            Selection::Mode(m) => mode_meta(*m).0,
            Selection::Ids(_) => icon::SLIDERS,
        };
        let done = Arc::new(Mutex::new(Vec::new()));
        self.progress = Some(Progress { id: egui::Id::unique(("scan", ctx.input(|i| i.time).to_bits())), planned: engine::selected_ids(&sel), done: done.clone(), glyph });
        let repaint = ctx.clone();
        self.scan = Some(Pending::spawn(ctx, crate::tf!("{label}…"), move || {
            let tick = |id: &'static str| {
                done.lock().unwrap().push(id);
                repaint.request_repaint();
            };
            Ok(engine::run_scan_with_progress(&settings, &sel, scope, &tick))
        }));
        self.screen = Screen::Results;
    }

    fn start_forced(&mut self, ctx: &egui::Context, cx: &Cx) {
        let (settings, root, text) = (cx.settings.clone(), self.forced_root.clone(), self.forced_text.clone());
        self.scan_label = crate::tf!("Forced scan for \"{text}\"");
        self.last_clean = None;
        self.progress = Some(Progress { id: egui::Id::unique(("scan", ctx.input(|i| i.time).to_bits())), planned: Vec::new(), done: Arc::default(), glyph: icon::SEARCH });
        self.scan = Some(Pending::spawn(ctx, "Forced scan…", move || engine::run_forced_scan(&settings, &root, &text).map_err(|e| crate::tf!("{e:#}"))));
        self.screen = Screen::Results;
    }

    fn start_clean(&mut self, ctx: &egui::Context, cx: &Cx, restore_point: bool, full_backup: bool) {
        let mut settings = cx.settings.clone();
        settings.restore_point_before_clean = restore_point;
        settings.full_backup_before_clean = full_backup;
        let issues: Vec<Issue> = self.results.iter().zip(&self.checked).filter(|(_, c)| **c).map(|(i, _)| i.clone()).collect();
        let n = issues.len();
        self.clean = Some(Pending::spawn(ctx, crate::tf!("Cleaning {n} item(s)…"), move || crate::cleaner::clean(&settings, &issues).map_err(|e| crate::tf!("{e:#}"))));
    }

    fn selected_count(&self) -> usize {
        self.checked.iter().filter(|c| **c).count()
    }

    fn request_clean(&mut self, ctx: &egui::Context, cx: &Cx) {
        if cx.settings.skip_warnings {
            let (rp, fb) = (cx.settings.restore_point_before_clean, cx.settings.full_backup_before_clean);
            self.start_clean(ctx, cx, rp, fb);
        } else {
            self.confirm = Some(Confirm { restore_point: cx.settings.restore_point_before_clean, full_backup: cx.settings.full_backup_before_clean });
        }
    }

    fn poll(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some(r) = take(&mut self.scan) {
            match r {
                Ok(issues) => {
                    cx.info(crate::tf!("{}: found {} item(s)", self.scan_label, issues.len()));
                    self.checked = vec![true; issues.len()];
                    self.results = issues;
                    if self.quick {
                        self.quick = false;
                        if !self.results.is_empty() {
                            let (rp, fb) = (cx.settings.restore_point_before_clean, cx.settings.full_backup_before_clean);
                            self.start_clean(ctx, cx, rp, fb);
                        }
                    }
                }
                Err(e) => {
                    self.quick = false;
                    cx.error(crate::tf!("Scan failed: {e}"));
                    self.screen = Screen::Home;
                }
            }
        }
        if let Some(r) = take(&mut self.clean) {
            match r {
                Ok(rep) => {
                    if let Some(Err(e)) = &rep.restore_point {
                        cx.error(crate::tf!("Failed to create a restore point: {e}"));
                    }
                    let failed: BTreeSet<&str> = rep.failed.iter().map(|(l, _)| l.as_str()).collect();
                    let keep: Vec<(Issue, bool)> = self.results.drain(..).zip(self.checked.drain(..)).filter(|(i, c)| !*c || failed.contains(i.location.as_str())).collect();
                    (self.results, self.checked) = keep.into_iter().unzip();
                    let msg = crate::tf!("Deleted {} item(s). All deleted data has been backed up ({}).", rep.cleaned, rep.backup_id);
                    if rep.failed.is_empty() {
                        cx.info(msg.clone());
                    } else {
                        cx.error(crate::tf!("{msg} {} could not be deleted: {}", rep.failed.len(), rep.failed[0].1));
                    }
                    self.last_clean = Some(msg);
                }
                Err(e) => cx.error(crate::tf!("Cleaning failed: {e}")),
            }
        }
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        self.poll(ui.ctx(), cx);
        match self.screen {
            Screen::Home => self.home(ui, cx),
            Screen::Managed => self.managed_ui(ui, cx),
            Screen::Forced => self.forced_ui(ui, cx),
            Screen::Results => self.results_ui(ui, cx),
            Screen::Auto => {
                let back = style::page_header_back(ui, "Auto Clean", "Scan and clean automatically on a schedule", |_| {});
                if back {
                    self.screen = Screen::Home;
                }
                self.auto.ui(ui, cx);
            }
        }
        self.confirm_ui(ui.ctx(), cx);
    }

    // ------------------------------------------------------------ home

    fn home(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let busy = self.busy();
        let subtitle = if cfg!(windows) { tr("Find and remove invalid and leftover registry entries") } else { tr("Find and remove settings left behind by removed apps") };
        style::page_header(ui, "Cleaner", subtitle, |_| {});

        let p = Palette::get(ui.ctx());
        let checks = engine::all_checks();
        let count = |m: ScanMode| checks.iter().filter(|c| c.min_mode <= m && c.id != "include_list").count();
        let (glyph, title, _, _) = mode_meta(self.mode);
        let scope = if self.all_users { tr("every account") } else { tr("your account") };

        // Hero: what will be scanned, where, and the button that starts it.
        let mut start = false;
        style::hero(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                style::logo(ui, glyph, 60.0);
                ui.add_space(8.0);
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(2.0);
                    ui.add(style::Label::new(style::strong(tr("Ready to scan")).size(21.0)));
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("{title} · {} categories · {scope}", count(self.mode)))));
                    ui.add_space(2.0);
                    crate::gui::style::row(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if style::segment(ui, !self.all_users, &crate::tf!("{}  Current account", icon::USER)).clicked() {
                            self.all_users = false;
                        }
                        if style::segment(ui, self.all_users, &crate::tf!("{}  All accounts", icon::USERS)).clicked() {
                            self.all_users = true;
                        }
                    });
                });
                // The one main action, and under it whether to clean straight away
                // (Quick Clean) instead of reviewing what was found first.
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    let far = if crate::gui::style::rtl() { egui::Align::Min } else { egui::Align::Max };
                    ui.with_layout(egui::Layout::top_down(far), |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.spacing_mut().button_padding = Vec2::new(24.0, 12.0);
                            let label = if self.clean_now { "Scan and clean" } else { "Start scan" };
                            start = style::primary(ui, if self.clean_now { icon::LIGHTNING } else { icon::PLAY }, label).clicked();
                        });
                        ui.add_space(4.0);
                        crate::gui::style::row(ui, |ui| {
                            style::toggle(ui, &mut self.clean_now, "Clean without reviewing");
                            ui.add(style::Label::new(style::muted(ui, tr("Clean without reviewing")).size(12.5)).extend())
                                .on_hover_text(style::vis(tr("Remove everything the scan finds right away (a backup is made first)")));
                        });
                    });
                });
            });
        });
        if start {
            let m = self.mode;
            self.quick = self.clean_now;
            let label = if self.clean_now { "Quick Clean".to_string() } else { mode_meta(m).1.to_string() };
            self.start_scan(ui.ctx(), cx, Selection::Mode(m), label);
            return;
        }
        ui.add_space(18.0);

        // Depth: pick one; a double click starts it straight away.
        style::eyebrow(ui, "Scan depth");
        let gap = 14.0;
        let w = ((ui.available_width() - gap * 2.0) / 3.0).max(200.0);
        let mut go = None;
        crate::gui::style::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for m in [ScanMode::Safe, ScanMode::Moderate, ScanMode::Advanced] {
                let r = depth_card(ui, Vec2::new(w, if w < 260.0 { 190.0 } else { 168.0 }), m, self.mode == m, count(m), p);
                if r.clicked() {
                    self.mode = m;
                }
                if r.double_clicked() && !busy {
                    go = Some(m);
                }
            }
        });
        if let Some(m) = go {
            self.mode = m;
            self.start_scan(ui.ctx(), cx, Selection::Mode(m), mode_meta(m).1.to_string());
            return;
        }
        ui.add_space(20.0);

        // One card, a row each: fits any width.
        style::eyebrow(ui, "Other ways to scan");
        let mut open = None;
        style::card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let rows = [
                (icon::SLIDERS, "Custom Scan", "Choose exactly which categories to scan.", Screen::Managed),
                (icon::SEARCH, "Forced Scan", "Find every entry that mentions a program or vendor.", Screen::Forced),
                (icon::CALENDAR, "Auto Clean", "Scan and clean automatically on a schedule", Screen::Auto),
            ];
            let n = rows.len();
            for (i, (ic, title, desc, screen)) in rows.into_iter().enumerate() {
                if other_row(ui, ic, title, desc, p).clicked() && !busy {
                    open = Some(screen);
                }
                if i + 1 < n {
                    style::divider(ui);
                }
            }
        });
        if let Some(screen) = open {
            if matches!(screen, Screen::Auto) {
                self.auto.refresh(cx);
            }
            self.screen = screen;
        }
    }

    fn scanning_ui(&self, ui: &mut Ui, p: Palette) {
        let scope = if self.all_users { tr("All accounts") } else { tr("Current account") };
        style::page_header(ui, "Scanning", &crate::tf!("{} · {scope}", self.scan_label), |_| {});
        let (id, planned, done, glyph) = match &self.progress {
            Some(pr) => (pr.id, pr.planned.clone(), pr.done.lock().unwrap().clone(), pr.glyph),
            None => (egui::Id::unique("scan"), Vec::new(), Vec::new(), icon::SEARCH),
        };
        let total = planned.len();
        let n = done.len().min(total);
        let secs = self.scan.as_ref().map_or(0.0, |s| s.elapsed());
        style::hero(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                style::progress_ring(ui, id, 104.0, (total > 0).then(|| n as f32 / total as f32), glyph);
                ui.add_space(14.0);
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add_space(14.0);
                    let pct = (n * 100).checked_div(total).map(|x| crate::tf!("{x}%")).unwrap_or_default();
                    crate::gui::style::row(ui, |ui| {
                        ui.add(style::Label::new(style::strong(tr("Looking for leftovers…")).size(20.0)));
                        ui.add(style::Label::new(style::rich(pct).size(20.0).family(style::semibold()).color(p.accent)));
                    });
                    let status = if total > 0 { crate::tf!("{n} of {total} categories checked · {secs:.0}s") } else { crate::tf!("Searching · {secs:.0}s") };
                    ui.add(style::Label::new(style::muted(ui, status)));
                    let w = ui.available_width().min(520.0);
                    if total > 0 {
                        style::progress_bar(ui, id, w, n as f32 / total as f32);
                    } else {
                        style::busy_bar(ui, w);
                    }
                });
            });
        });
        if total == 0 {
            return;
        }
        ui.add_space(18.0);
        style::eyebrow(ui, "Categories");
        let time = ui.input(|i| i.time) as f32;
        let cols = ((ui.available_width() / 300.0).floor() as usize).clamp(1, 3);
        let w = (ui.available_width() - 10.0 * (cols - 1) as f32) / cols as f32;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for row in planned.chunks(cols) {
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (i, id) in row.iter().enumerate() {
                        let finished = done.contains(id);
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 44.0), egui::Sense::hover());
                        let fill = if finished { p.surface } else { p.surface.lerp_to_gamma(p.surface_alt, 0.6) };
                        ui.painter().rect(rect, 10.0, fill, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
                        let left = rect.left_center() + Vec2::new(16.0, 0.0);
                        if finished {
                            ui.painter().text(left, egui::Align2::CENTER_CENTER, style::vstr(icon::CHECK_CIRCLE), egui::FontId::new(18.0, style::icon_fill()), p.success);
                        } else {
                            // A dot that pulses, out of step with its neighbours.
                            let t = 0.5 + 0.5 * (time * 3.0 + i as f32 * 0.9).sin();
                            ui.painter().circle_filled(left, 3.0 + 2.0 * t, p.accent.gamma_multiply(0.35 + 0.5 * t));
                        }
                        let name = tr(self.names.get(id).copied().unwrap_or(id));
                        let color = if finished { p.text } else { p.muted };
                        ui.painter().text(left + Vec2::new(18.0, 0.0), egui::Align2::LEFT_CENTER, style::vstr(style::category_icon(id)), egui::FontId::proportional(15.0), color);
                        ui.painter().text(left + Vec2::new(40.0, 0.0), egui::Align2::LEFT_CENTER, style::vstr(name), egui::FontId::proportional(13.5), color);
                    }
                });
                ui.add_space(4.0);
            }
        });
    }

    fn managed_ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let n = self.managed.len();
        let mut scan = false;
        let back = style::page_header_back(ui, "Custom Scan", "Pick the categories to look at", |ui| {
            ui.add_enabled_ui(n > 0, |ui| scan = style::primary(ui, icon::SEARCH, &crate::tf!("Scan {n} categories")).clicked());
        });
        if back {
            self.screen = Screen::Home;
            return;
        }
        if scan {
            let ids = self.managed.iter().cloned().collect();
            self.start_scan(ui.ctx(), cx, Selection::Ids(ids), "Custom Scan".into());
            return;
        }
        crate::gui::style::row(ui, |ui| {
            if style::ghost(ui, icon::CHECK, "Select all").clicked() {
                self.managed = engine::all_checks().iter().map(|c| c.id.to_string()).collect();
            }
            if style::ghost(ui, icon::X, "Select none").clicked() {
                self.managed.clear();
            }
        });
        let p = Palette::get(ui.ctx());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            style::card(ui, |ui| {
                for (i, c) in engine::all_checks().iter().enumerate() {
                    if i > 0 {
                        style::divider(ui);
                    }
                    crate::gui::style::row(ui, |ui| {
                        style::icon_tile(ui, style::category_icon(c.id), p.accent, 32.0);
                        crate::gui::style::column(ui, |ui| {
                            ui.add(style::Label::new(style::strong(tr(c.name))));
                            ui.add(style::Label::new(style::muted(ui, tr(c.description)).size(12.5)));
                        });
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            let mut on = self.managed.contains(c.id);
                            if style::toggle(ui, &mut on, c.name).changed() {
                                if on {
                                    self.managed.insert(c.id.to_string());
                                } else {
                                    self.managed.remove(c.id);
                                }
                            }
                        });
                    });
                }
            });
        });
    }

    fn forced_ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let ok = !self.forced_text.trim().is_empty() && !self.forced_root.trim().is_empty();
        let mut scan = false;
        let back = style::page_header_back(ui, "Forced Scan", "Find every entry under a location that mentions a word", |ui| {
            ui.add_enabled_ui(ok, |ui| scan = style::primary(ui, icon::SEARCH, "Scan").clicked());
        });
        if back {
            self.screen = Screen::Home;
            return;
        }
        if scan {
            self.start_forced(ui.ctx(), cx);
            return;
        }
        style::card(ui, |ui| {
            egui::Grid::new("forced").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
                ui.add(style::Label::new(if cfg!(windows) { tr("Registry key") } else { tr("Folder") }));
                style::key_or_path_input(ui, "forced-root", &mut self.forced_root, "", 520.0, style::Pick::Folder);
                ui.end_row();
                ui.add(style::Label::new(tr("Search for")));
                style::text_input(ui, &mut self.forced_text, "e.g. a program or vendor name", 520.0);
                ui.end_row();
            });
        });
    }

    // ------------------------------------------------------------ results

    fn results_ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        if self.scan.is_some() {
            self.scanning_ui(ui, p);
            return;
        }

        let total = self.results.len();
        let sel = self.selected_count();
        let busy = self.busy();
        let cats = self.order.iter().filter(|c| self.results.iter().any(|i| i.category == **c)).count();
        let subtitle = if total == 0 { self.scan_label.clone() } else { crate::tf!("{total} item(s) in {cats} category(ies) · {sel} selected") };
        let (mut clean, mut new_scan) = (false, false);
        // One action up top; the rest sit in the toolbar above the list.
        style::page_header(ui, "Scan results", &subtitle, |ui| {
            ui.add_enabled_ui(!busy && sel > 0, |ui| clean = style::primary(ui, icon::BROOM, &crate::tf!("Clean ({sel})")).clicked());
            if total == 0 {
                ui.add_enabled_ui(!busy, |ui| new_scan = style::secondary(ui, icon::UNDO, "New scan").clicked());
            }
        });
        if clean {
            self.request_clean(ui.ctx(), cx);
        }
        if new_scan {
            self.screen = Screen::Home;
            self.results.clear();
            self.checked.clear();
            return;
        }
        if let Some(pend) = &self.clean {
            style::card(ui, |ui| pend.show_progress(ui));
            ui.add_space(10.0);
        }
        if total == 0 {
            style::card(ui, |ui| match &self.last_clean {
                Some(msg) => style::empty_state(ui, icon::CHECK_CIRCLE, "All clean", msg),
                None => style::empty_state(ui, icon::SHIELD_CHECK, "No problems found", "Everything we checked looks fine."),
            });
            return;
        }

        crate::gui::style::row(ui, |ui| {
            let mut all = sel == total;
            let cb = egui::Checkbox::new(&mut all, style::vis(tr("Select all"))).indeterminate(sel > 0 && sel < total);
            if ui.add(cb).changed() {
                self.checked.iter_mut().for_each(|c| *c = all);
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_enabled_ui(!busy, |ui| new_scan = style::ghost(ui, icon::UNDO, "New scan").clicked());
                let r = style::ghost(ui, icon::EXPORT, "Export");
                let results = &self.results;
                super::export_menu(&r, cx, &mut self.export_path, "File (.txt .csv .xlsx .html .xml .json)", |f| {
                    crate::export::export(results, f)?;
                    Ok(crate::tf!("Exported to {}", f.display()))
                });
                let (ic, label) = if self.expanded { (icon::X, "Collapse all") } else { (icon::LIST_CHECKS, "Expand all") };
                if style::ghost(ui, ic, label).clicked() {
                    self.expanded = !self.expanded;
                    self.set_open = Some(self.expanded);
                }
                ui.add_space(6.0);
                style::toggle(ui, &mut self.show_details, "Show details");
                ui.add_space(4.0);
                ui.add(style::Label::new(style::muted(ui, tr("Show details")).size(13.0)).extend());
            });
        });
        if new_scan {
            self.screen = Screen::Home;
            self.results.clear();
            self.checked.clear();
            return;
        }
        ui.add_space(6.0);

        let mut groups: Vec<(&'static str, Vec<usize>)> = Vec::new();
        for cat in &self.order {
            let idx: Vec<usize> = (0..total).filter(|i| self.results[*i].category == *cat).collect();
            if !idx.is_empty() {
                groups.push((cat, idx));
            }
        }
        let set_open = self.set_open.take();
        let mut action: Option<ItemAction> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (cat, idx) in &groups {
                let name = tr(self.names.get(cat).copied().unwrap_or(cat));
                let desc = self.descs.get(cat).map(|d| tr(d)).unwrap_or_default();
                style::card(ui, |ui| {
                    let id = ui.make_persistent_id(("cat", cat));
                    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
                    if let Some(o) = set_open {
                        state.set_open(o);
                    }
                    let checked = &mut self.checked;
                    state
                        .show_header(ui, |ui| {
                            let n_sel = idx.iter().filter(|i| checked[**i]).count();
                            let mut all = n_sel == idx.len();
                            let cb = egui::Checkbox::new(&mut all, style::vis("")).indeterminate(n_sel > 0 && n_sel < idx.len());
                            let r = ui.add(cb);
                            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), all, name));
                            if r.changed() {
                                idx.iter().for_each(|i| checked[*i] = all);
                            }
                            style::icon_tile(ui, style::category_icon(cat), p.accent, 28.0);
                            ui.add(style::Label::new(style::strong(name)).extend());
                            style::badge(ui, &idx.len().to_string(), p.accent);
                            // What the category means, cut to the room left.
                            if !desc.is_empty() {
                                ui.add(style::Label::new(style::muted(ui, desc).size(12.5)).truncate());
                            }
                        })
                        .body(|ui| {
                            ui.add_space(2.0);
                            for &i in idx {
                                let issue = &self.results[i];
                                // The hover background goes behind the row, painted once its size is known.
                                let bg = ui.painter().add(egui::Shape::Noop);
                                let row = crate::gui::style::row_top(ui, |ui| {
                                    ui.add_space(4.0);
                                    let r = ui.checkbox(&mut self.checked[i], style::vis(""));
                                    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), self.checked[i], friendly(&issue.location)));
                                    crate::gui::style::column(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        let r = ui.add(style::Label::new(style::rich(friendly(&issue.location)).monospace().size(12.5)).sense(egui::Sense::click()).truncate());
                                        let r = r.on_hover_text(style::vis(crate::tf!("{}\n{}", issue.location, issue.detail)));
                                        r.context_menu(|ui| item_menu(ui, i, self.checked[i], &issue.location, &mut action));
                                        // Why it was flagged: one line, or all of it with "Show details".
                                        if !issue.detail.is_empty() {
                                            let l = style::Label::new(style::rich(&issue.detail).size(12.0).color(p.muted));
                                            ui.add(if self.show_details { l.wrap() } else { l.truncate() });
                                        }
                                    });
                                });
                                let hover = ui.ctx().animate_bool_with_time(row.response.id.with("hover"), row.response.contains_pointer(), style::FADE);
                                if hover > 0.0 {
                                    ui.painter().set(bg, egui::Shape::rect_filled(row.response.rect.expand2(Vec2::new(4.0, 2.0)), 8.0, p.hover.gamma_multiply(hover * 0.7)));
                                }
                                ui.add_space(4.0);
                            }
                        });
                });
                ui.add_space(10.0);
            }
        });
        if let Some(a) = action {
            self.apply(a, cx);
        }
    }

    fn apply(&mut self, a: ItemAction, cx: &mut Cx) {
        match a {
            ItemAction::Toggle(i) => self.checked[i] = !self.checked[i],
            ItemAction::All(v) => self.checked.iter_mut().for_each(|c| *c = v),
            ItemAction::Open(v) => self.set_open = Some(v),
            ItemAction::Details => self.show_details = !self.show_details,
            ItemAction::Editor(i) => {
                if let Err(e) = crate::platform::open_in_editor(&self.results[i].location) {
                    cx.error(crate::tf!("Could not open: {e:#}"));
                }
            }
            ItemAction::Copy(i) => cx.info(crate::tf!("Copied {}", self.results[i].location)),
            ItemAction::Exclude(i) => {
                let path = self.results[i].location.split(" | ").next().unwrap_or_default().to_string();
                cx.settings.exclude_list.retain(|e| !e.path.eq_ignore_ascii_case(&path));
                cx.settings.exclude_list.push(ListEntry { path: path.clone(), include_subkeys: true, enabled: true });
                cx.save_settings();
                let settings = &*cx.settings;
                let keep: Vec<(Issue, bool)> = self.results.drain(..).zip(self.checked.drain(..)).filter(|(i, _)| !settings.is_excluded(&i.location)).collect();
                (self.results, self.checked) = keep.into_iter().unzip();
                cx.info(crate::tf!("Added to exclude list: {path}"));
            }
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some(c) = &mut self.confirm else { return };
        let n = self.checked.iter().filter(|c| **c).count();
        let mut go = None;
        let resp = crate::gui::style::modal("confirm-clean", 440.0, ctx, |ui| {
            let p = Palette::get(ui.ctx());
            style::icon_tile(ui, icon::BROOM, p.accent, 44.0);
            ui.add_space(8.0);
            ui.add(style::Label::new(style::strong(crate::tf!("Delete {n} item(s)?")).size(19.0)));
            ui.add(style::Label::new(style::muted(ui, tr("Everything deleted is backed up first and can be restored from Backups."))));
            ui.add_space(10.0);
            ui.checkbox(&mut c.restore_point, style::vis(if cfg!(windows) { tr("Create a System Restore Point") } else { tr("Create a system snapshot (snapper / timeshift)") }));
            ui.checkbox(&mut c.full_backup, style::vis(if cfg!(windows) { tr("Create a full registry backup") } else { tr("Create a full config backup") }));
            ui.add_space(14.0);
            crate::gui::style::row(ui, |ui| {
                if style::primary(ui, icon::TRASH, "Delete").clicked() {
                    go = Some(true);
                }
                if style::secondary(ui, "", "Cancel").clicked() {
                    go = Some(false);
                }
            });
        });
        if resp.should_close() && go.is_none() {
            go = Some(false);
        }
        if let Some(yes) = go {
            let (rp, fb) = (c.restore_point, c.full_backup);
            self.confirm = None;
            if yes {
                self.start_clean(ctx, cx, rp, fb);
            }
        }
    }
}

/// A selectable scan-depth card: icon, radio mark, title, description, a depth meter and the
/// number of categories it covers.
fn depth_card(ui: &mut Ui, size: Vec2, m: ScanMode, selected: bool, categories: usize, p: Palette) -> egui::Response {
    let (ic, title, desc, level) = mode_meta(m);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), selected, title));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    style::tile_frame(ui, rect, &resp, selected);
    let s = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, style::FADE * 1.5);
    let painter = ui.painter();
    let pad = 18.0;
    let mr = style::Mirror::new(rect);
    let tile = mr.rect(pad, rect.top() + pad, Vec2::splat(40.0));
    painter.rect_filled(tile, 11.0, p.accent_soft.lerp_to_gamma(p.accent, s));
    painter.text(tile.center(), egui::Align2::CENTER_CENTER, style::vstr(ic), egui::FontId::new(20.0, style::icon_fill()), p.accent.lerp_to_gamma(p.on_accent, s));
    // Radio mark, at the far end.
    let rc = mr.pos(rect.width() - pad - 10.0, tile.center().y);
    painter.circle_stroke(rc, 10.0, egui::Stroke::new(1.5, p.border.lerp_to_gamma(p.accent, s)));
    if s > 0.0 {
        painter.circle_filled(rc, 10.0 * s, p.accent);
        painter.text(rc, egui::Align2::CENTER_CENTER, style::vstr(icon::CHECK), egui::FontId::proportional(12.0 * s.max(0.01)), p.on_accent);
    }
    let ty = tile.bottom() + 14.0;
    let tg = style::text_galley(ui, title, egui::FontId::new(16.0, style::semibold()), p.text, f32::INFINITY);
    let tw = tg.size().x;
    painter.galley(egui::pos2(mr.x(pad), ty), tg, p.text);
    if m == ScanMode::Safe {
        let g = painter.layout_no_wrap(style::vstr(tr("Recommended")), egui::FontId::new(11.0, style::semibold()), p.success);
        let r = mr.rect(pad + tw + 8.0, ty + 1.0, g.size() + Vec2::new(14.0, 4.0));
        painter.rect_filled(r, 9.0, egui::Color32::from_rgba_unmultiplied(p.success.r(), p.success.g(), p.success.b(), 30));
        painter.galley(r.min + Vec2::new(7.0, 2.0), g, p.success);
    }
    let dg = style::text_galley(ui, desc, egui::FontId::proportional(13.0), p.muted, rect.width() - pad * 2.0);
    painter.galley(egui::pos2(mr.x(pad), ty + 24.0), dg, p.muted);
    // Depth meter and category count along the bottom.
    let by = rect.bottom() - pad - 4.0;
    for i in 0..3 {
        let bar = mr.rect(pad + i as f32 * 22.0, by - 3.0, Vec2::new(18.0, 6.0));
        painter.rect_filled(bar, 3.0, if i < level { p.accent } else { p.surface_alt.lerp_to_gamma(p.border, 0.6) });
    }
    if rect.width() >= 260.0 {
        painter.text(mr.pos(pad + 72.0, by), mr.start(egui::Align::Center), style::vstr(tr("Depth")), egui::FontId::proportional(12.0), p.faint);
    }
    painter.text(mr.pos(rect.width() - pad, by), mr.end(egui::Align::Center), style::vstr(crate::tf!("{categories} categories")), egui::FontId::proportional(12.0), p.muted);
    resp
}

/// Clickable card with an icon tile, a title, a description and an optional tag.
/// A location as people know it: the home folder as `~`.
fn friendly(location: &str) -> String {
    match dirs::home_dir().map(|h| h.display().to_string()).filter(|h| h.len() > 1) {
        Some(h) if location.starts_with(&h) => format!("~{}", &location[h.len()..]),
        _ => location.to_string(),
    }
}

/// A row of "Other ways to scan": icon, title and one line of description, an arrow on hover.
fn other_row(ui: &mut Ui, ic: &str, title: &str, desc: &str, p: Palette) -> egui::Response {
    let (title, desc) = (tr(title), tr(desc));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), title));
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), style::FADE);
    if t > 0.0 {
        ui.painter().rect_filled(rect.expand2(Vec2::new(8.0, -2.0)), 10.0, p.hover.gamma_multiply(t));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mr = style::Mirror::new(rect);
    let cy = rect.center().y;
    let tile = mr.rect(0.0, cy - 18.0, Vec2::splat(36.0));
    ui.painter().rect_filled(tile, 10.0, p.accent_soft.lerp_to_gamma(p.accent, t * 0.15));
    ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, style::vstr(ic), egui::FontId::new(18.0, style::icon_fill()), p.accent);
    let d = 36.0 + 14.0;
    let room = (rect.width() - d - 30.0).max(20.0);
    let (tg, _) = style::one_line(ui, title, egui::FontId::new(15.0, style::semibold()), p.text, room);
    let (dg, cut) = style::one_line(ui, desc, egui::FontId::proportional(13.0), p.muted, room);
    ui.painter().galley(egui::pos2(mr.galley_x(d, tg.size().x), cy - 1.0 - tg.size().y), tg, p.text);
    ui.painter().galley(egui::pos2(mr.galley_x(d, dg.size().x), cy + 1.0), dg, p.muted);
    let c = mr.pos(rect.width() - 10.0 - 4.0 * (1.0 - t), cy);
    ui.painter().text(c, egui::Align2::CENTER_CENTER, style::vstr(crate::gui::style::icon_forward()), egui::FontId::proportional(16.0), p.faint.lerp_to_gamma(p.accent, t));
    if cut {
        resp.on_hover_text(style::vis(desc))
    } else {
        resp
    }
}

enum ItemAction {
    Toggle(usize),
    All(bool),
    Open(bool),
    Details,
    Editor(usize),
    Copy(usize),
    Exclude(usize),
}

fn item_menu(ui: &mut Ui, i: usize, checked: bool, location: &str, action: &mut Option<ItemAction>) {
    let mut pick = |ui: &mut Ui, ic: &str, label: &str, a: ItemAction| {
        if style::ghost(ui, ic, label).clicked() {
            *action = Some(a);
            ui.close();
        }
    };
    pick(ui, if checked { icon::X } else { icon::CHECK }, if checked { tr("Unselect this") } else { tr("Select this") }, ItemAction::Toggle(i));
    pick(ui, icon::LIST_CHECKS, "Select all", ItemAction::All(true));
    pick(ui, icon::X, "Select none", ItemAction::All(false));
    ui.separator();
    pick(ui, icon::LIST_CHECKS, "Expand all", ItemAction::Open(true));
    pick(ui, icon::X, "Collapse all", ItemAction::Open(false));
    pick(ui, icon::INFO, "Show details", ItemAction::Details);
    ui.separator();
    pick(ui, icon::FOLDER_OPEN, if cfg!(windows) { tr("Open in Registry Editor") } else { tr("Open containing folder") }, ItemAction::Editor(i));
    pick(ui, icon::SHIELD_CHECK, "Add to exclude list", ItemAction::Exclude(i));
    if style::ghost(ui, icon::COPY, "Copy").clicked() {
        ui.ctx().copy_text(location.to_string());
        *action = Some(ItemAction::Copy(i));
        ui.close();
    }
}
