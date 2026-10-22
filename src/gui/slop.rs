//! Tools › Microsoft slop (Windows): preinstalled and promoted apps to remove, and the AI,
//! ads and telemetry settings to turn off (Tuner tweaks, so they can be turned back on).

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::i18n::tr;
use crate::keepaway::{self, App};
use crate::scheduler::{self, Action, Schedule, Task};
use crate::slop::{self, Found, RemoveReport, Scan};
use crate::tuner::{self, Requires, State, Status, Tweak};
use eframe::egui::{self, Ui};
use std::collections::{BTreeSet, HashMap};

pub struct SlopView {
    scan: Option<Result<Scan, String>>,
    scanning: Option<Pending<Result<Scan, String>>>,
    checked: BTreeSet<&'static str>,
    all_users: bool,
    deprovision: bool,
    confirm: bool,
    removing: Option<Pending<RemoveReport>>,
    tweaks: Vec<Tweak>,
    status: HashMap<String, Status>,
    state: State,
    tweaks_loaded: bool,
    pending: BTreeSet<Requires>,
    /// Automatic removal: the daily task that removes these apps again when Windows brings
    /// them back. `None` until read from the task list.
    auto: Option<Auto>,
    /// Edge and OneDrive: what is kept away (read once), the one awaiting confirmation, and
    /// the removal or reinstall running.
    kept: Option<keepaway::Kept>,
    keep_confirm: Option<App>,
    keep_busy: Option<Pending<Result<String, String>>>,
}

/// Automatic removal as set up: on (the task exists) and which catalog apps it removes.
struct Auto {
    on: bool,
    apps: BTreeSet<&'static str>,
    last_run: Option<scheduler::LastRun>,
}

/// The scheduled task behind automatic removal.
const AUTO_TASK: &str = "remove-microsoft-slop";

impl SlopView {
    pub fn new() -> Self {
        Self {
            scan: None,
            scanning: None,
            checked: BTreeSet::new(),
            all_users: true,
            deprovision: true,
            confirm: false,
            removing: None,
            tweaks: Vec::new(),
            status: HashMap::new(),
            state: State::default(),
            tweaks_loaded: false,
            pending: BTreeSet::new(),
            auto: None,
            kept: None,
            keep_confirm: None,
            keep_busy: None,
        }
    }

    /// Automatic removal as the task list has it; recommended apps when it is off.
    fn load_auto() -> Auto {
        let task = scheduler::load_tasks().unwrap_or_default().into_iter().find(|t| t.id == AUTO_TASK);
        match task {
            Some(Task { action: Action::RemoveSlop { apps }, last_run, .. }) => {
                Auto { on: true, apps: slop::CATALOG.iter().map(|a| a.name).filter(|n| apps.iter().any(|x| x == n)).collect(), last_run }
            }
            _ => Auto { on: false, apps: slop::CATALOG.iter().filter(|a| a.recommended).map(|a| a.name).collect(), last_run: None },
        }
    }

    /// Create, update or remove the daily task to match `auto`.
    fn save_auto(auto: &Auto) -> anyhow::Result<()> {
        let mut all = scheduler::load_tasks()?;
        let pos = all.iter().position(|t| t.id == AUTO_TASK);
        let action = Action::RemoveSlop { apps: auto.apps.iter().map(|s| s.to_string()).collect() };
        match (auto.on, pos) {
            (true, Some(i)) => all[i].action = action,
            (true, None) => {
                let task = Task {
                    id: AUTO_TASK.into(),
                    name: "Remove Microsoft slop".into(),
                    note: String::new(),
                    schedule: Schedule::Daily { time: chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap() },
                    action,
                    created: chrono::Local::now(),
                    last_run: None,
                };
                scheduler::register(&task)?;
                all.push(task);
            }
            (false, Some(i)) => {
                scheduler::unregister(AUTO_TASK)?;
                all.remove(i);
            }
            (false, None) => return Ok(()),
        }
        scheduler::save_tasks(&all)
    }

    pub fn refresh(&mut self) {
        self.tweaks_loaded = false;
    }

    fn set_scan(&mut self, s: Result<Scan, String>) {
        if let Ok(scan) = &s {
            self.checked = scan.found.iter().filter(|f| f.app.recommended).map(|f| f.app.name).collect();
            if !scan.admin {
                self.all_users = false;
                self.deprovision = false;
            }
        }
        self.scan = Some(s);
    }

    fn load_tweaks(&mut self, cx: &mut Cx) {
        self.tweaks = tuner::tweaks().into_iter().filter(|t| slop::TWEAKS.contains(&t.id.as_str())).collect();
        self.tweaks.sort_by_key(|t| slop::TWEAKS.iter().position(|i| *i == t.id));
        match State::load() {
            Ok(s) => self.state = s,
            Err(e) => cx.error(crate::tf!("{e:#}")),
        }
        self.status = tuner::status_all(&self.tweaks, &self.state).into_iter().map(|s| (s.id.clone(), s)).collect();
        self.tweaks_loaded = true;
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !cfg!(windows) && self.scan.is_none() {
            style::empty_state(
                ui,
                icon::MONITOR,
                "Windows only",
                "The Microsoft slop cleaner removes Windows' preinstalled apps and turns off Copilot, ads and telemetry. There's nothing like it to clean on Linux.",
            );
            return;
        }
        if self.scan.is_none() && self.scanning.is_none() {
            self.scanning = Some(Pending::spawn(ui.ctx(), "Looking for preinstalled apps", || slop::scan().map_err(|e| crate::tf!("{e:#}"))));
        }
        if let Some(s) = take(&mut self.scanning) {
            self.set_scan(s);
        }
        if let Some(r) = take(&mut self.removing) {
            if r.failed.is_empty() {
                cx.info(crate::tf!("Removed {} app(s)", r.removed.len()));
            } else {
                cx.error(crate::tf!(
                    "Removed {}; {} failed: {}",
                    r.removed.len(),
                    r.failed.len(),
                    r.failed.iter().map(|(n, e)| crate::tf!("{n} ({e})")).collect::<Vec<_>>().join("; ")
                ));
            }
            self.scan = None;
        }
        if !self.tweaks_loaded && cfg!(windows) {
            self.load_tweaks(cx);
        }
        let p = Palette::get(ui.ctx());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.apps_card(ui, cx, p);
            ui.add_space(12.0);
            self.keep_away_card(ui, cx, p);
            ui.add_space(12.0);
            self.auto_card(ui, cx, p);
            ui.add_space(12.0);
            self.settings_card(ui, cx, p);
        });
        self.confirm_dialog(ui.ctx(), cx);
        self.keep_away_dialog(ui.ctx());
    }

    /// Edge and OneDrive: removed and kept away, or allowed again.
    fn keep_away_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        if let Some(r) = take(&mut self.keep_busy) {
            match r {
                Ok(m) => cx.info(m),
                Err(e) => cx.error(e),
            }
            self.kept = None;
        }
        let kept = self.kept.get_or_insert_with(keepaway::load).clone();
        let admin = crate::slop::is_admin();
        style::card(ui, |ui| {
            style::section_title(ui, "Edge and OneDrive");
            ui.add(style::Label::new(
                style::muted(ui, tr("Windows reinstalls these on its own. CleanCore removes them, sets Windows' own blocks against reinstalling them, and checks every day that they stay away.")).size(12.5),
            ));
            if !admin {
                ui.add(style::Label::new(style::rich(crate::tf!("{}  Run CleanCore as administrator to remove them.", icon::INFO)).size(12.5).color(p.warning)));
            }
            if let Some(b) = &self.keep_busy {
                b.show_progress(ui);
                return;
            }
            for app in App::ALL {
                ui.add_space(8.0);
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::strong(app.name())));
                    let (state, col) = match (kept.get(app).is_some(), keepaway::installed(app)) {
                        (true, false) => (tr("Removed and kept away"), p.success),
                        (true, true) => (tr("Came back: removed again at the next daily check"), p.warning),
                        (false, true) => (tr("Installed"), p.muted),
                        (false, false) => (tr("Not installed"), p.muted),
                    };
                    style::badge(ui, state, col);
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(admin, |ui| {
                            if kept.get(app).is_some() {
                                if style::secondary(ui, icon::UNDO, "Allow again").on_hover_text(style::vis(tr("Undo the blocks and reinstall it"))).clicked() {
                                    self.keep_busy =
                                        Some(Pending::spawn(ui.ctx(), crate::tf!("Reinstalling {}", app.name()), move || keepaway::allow(app).map_err(|e| crate::tf!("{e:#}"))));
                                }
                            } else if style::danger(ui, icon::TRASH, if keepaway::installed(app) { "Remove and keep away" } else { "Keep away" }).clicked() {
                                self.keep_confirm = Some(app);
                            }
                        });
                    });
                });
                let note = match app {
                    App::Edge => tr("Links and Windows features that open Edge use your default browser instead. WebView2, which other apps need, stays."),
                    App::OneDrive => tr("Files in your OneDrive folder stay on this PC; they just stop syncing."),
                };
                ui.add(style::Label::new(style::muted(ui, note).size(12.0)));
            }
        });
    }

    fn keep_away_dialog(&mut self, ctx: &egui::Context) {
        let Some(app) = self.keep_confirm else { return };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("keep-away-confirm")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.add(style::Label::new(style::strong(crate::tf!("Remove {} and keep it away?", app.name())).size(18.0)));
            ui.add_space(6.0);
            let what = match app {
                App::Edge => tr("Edge is uninstalled for every user, and Edge Update is told not to install it again. Links and Windows features that open Edge use your default browser instead. WebView2 stays."),
                App::OneDrive => tr("OneDrive is uninstalled for every user and stopped from running or installing for new accounts. Files in your OneDrive folder stay on this PC; they just stop syncing."),
            };
            ui.add(style::Label::new(what).wrap());
            ui.add_space(4.0);
            ui.add(style::Label::new(style::muted(ui, tr("You can undo this any time with Allow again."))));
            ui.add_space(12.0);
            crate::gui::style::row(ui, |ui| {
                if style::danger(ui, icon::TRASH, "Remove").clicked() {
                    self.keep_busy = Some(Pending::spawn(ui.ctx(), crate::tf!("Removing {}", app.name()), move || keepaway::remove(app).map_err(|e| crate::tf!("{e:#}"))));
                    close = true;
                }
                if style::secondary(ui, "", "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.keep_confirm = None;
        }
    }

    fn apps_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        style::card(ui, |ui| {
            let found: Vec<Found> = match &self.scan {
                Some(Ok(s)) => s.found.clone(),
                _ => Vec::new(),
            };
            let admin = matches!(&self.scan, Some(Ok(s)) if s.admin);
            let picked = found.iter().filter(|f| self.checked.contains(f.app.name)).count();
            crate::gui::style::row(ui, |ui| {
                style::section_title(ui, "Preinstalled and promoted apps");
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(picked > 0 && self.removing.is_none(), |ui| {
                        if style::primary(ui, icon::TRASH, &crate::tf!("Remove ({picked})")).clicked() {
                            self.confirm = true;
                        }
                    });
                    if style::ghost(ui, "", "None").clicked() {
                        self.checked.clear();
                    }
                    if style::ghost(ui, icon::CHECK, "Recommended").clicked() {
                        self.checked = found.iter().filter(|f| f.app.recommended).map(|f| f.app.name).collect();
                    }
                    if style::icon_button(ui, icon::REFRESH, "Scan again").clicked() {
                        self.scan = None;
                    }
                });
            });
            ui.add(style::Label::new(style::muted(ui, tr("Apps Windows installs or pushes on its own. Removed apps can be reinstalled from the Microsoft Store.")).size(12.5)));
            ui.add_space(6.0);
            if let Some(s) = &self.scanning {
                s.show_progress(ui);
                return;
            }
            if let Some(r) = &self.removing {
                r.show_progress(ui);
                return;
            }
            if let Some(Err(e)) = &self.scan {
                ui.add(style::Label::new(style::rich(crate::tf!("{}  {e}", icon::WARNING)).color(p.danger)));
                return;
            }
            if !admin {
                ui.add(style::Label::new(
                    style::rich(crate::tf!("{}  Run CleanCore as administrator to remove apps for every user and stop them from coming back.", icon::INFO))
                        .size(12.5)
                        .color(p.warning),
                ));
                ui.add_space(4.0);
            }
            crate::gui::style::row(ui, |ui| {
                ui.add_enabled_ui(admin, |ui| {
                    style::toggle(ui, &mut self.all_users, "For every user account");
                    ui.add(style::Label::new(tr("For every user account")));
                    ui.add_space(16.0);
                    style::toggle(ui, &mut self.deprovision, "Also remove the preinstalled copy");
                    ui.add(style::Label::new(tr("Also remove the preinstalled copy"))).on_hover_text(style::vis(tr("New accounts and feature updates won't bring the app back")));
                });
            });
            ui.add_space(6.0);
            if found.is_empty() {
                ui.add(style::Label::new(style::rich(crate::tf!("{}  None of the known preinstalled apps are installed.", icon::CHECK_CIRCLE)).color(p.success)));
                return;
            }
            let mut last = "";
            for f in &found {
                if f.app.group != last {
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::strong(f.app.group).size(13.5)));
                    last = f.app.group;
                }
                crate::gui::style::row(ui, |ui| {
                    let mut on = self.checked.contains(f.app.name);
                    if ui.checkbox(&mut on, style::vis(f.app.name)).changed() {
                        if on {
                            self.checked.insert(f.app.name);
                        } else {
                            self.checked.remove(f.app.name);
                        }
                    }
                    if f.app.recommended {
                        style::badge(ui, "Recommended", p.success);
                    }
                    if !f.provisioned.is_empty() {
                        style::badge(ui, "Preinstalled for new users", p.muted);
                    }
                    if f.packages.is_empty() {
                        style::badge(ui, "Only the preinstalled copy", p.muted);
                    }
                    if !f.app.note.is_empty() {
                        ui.add(style::Label::new(style::muted(ui, f.app.note).size(12.0)));
                    }
                });
            }
            let _ = cx;
        });
    }

    /// Automatic removal: pick from every known app, installed now or not, and CleanCore removes
    /// them again each day if Windows brought them back.
    fn auto_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let auto = self.auto.get_or_insert_with(Self::load_auto);
        let (mut changed, mut switched) = (false, false);
        style::card(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                style::section_title(ui, "Automatic removal");
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    if style::ghost(ui, "", "None").clicked() {
                        auto.apps.clear();
                        changed = true;
                    }
                    if style::ghost(ui, icon::CHECK, "Recommended").clicked() {
                        auto.apps = slop::CATALOG.iter().filter(|a| a.recommended).map(|a| a.name).collect();
                        changed = true;
                    }
                });
            });
            ui.add(style::Label::new(
                style::muted(ui, tr("Windows often brings these apps back with updates. Tick the ones you never want, and CleanCore checks every day and removes them again."))
                    .size(12.5),
            ));
            ui.add_space(6.0);
            crate::gui::style::row(ui, |ui| {
                if style::toggle(ui, &mut auto.on, "Remove the ticked apps automatically").changed() {
                    changed = true;
                    switched = true;
                }
                ui.add(style::Label::new(tr("Remove the ticked apps automatically")));
            });
            if let (true, Some(r)) = (auto.on, &auto.last_run) {
                let col = if r.ok { p.muted } else { p.danger };
                ui.add(style::Label::new(style::rich(crate::tf!("Last check {}: {}", r.time.format("%d %b %H:%M"), r.summary)).size(12.0).color(col)).truncate());
            }
            let mut last = "";
            for a in slop::CATALOG {
                if a.group != last {
                    ui.add_space(6.0);
                    ui.add(style::Label::new(style::strong(a.group).size(13.5)));
                    last = a.group;
                }
                crate::gui::style::row(ui, |ui| {
                    let mut on = auto.apps.contains(a.name);
                    if ui.checkbox(&mut on, style::vis(a.name)).changed() {
                        if on {
                            auto.apps.insert(a.name);
                        } else {
                            auto.apps.remove(a.name);
                        }
                        changed = true;
                    }
                    if !a.note.is_empty() {
                        ui.add(style::Label::new(style::muted(ui, a.note).size(12.0)));
                    }
                });
            }
        });
        if changed {
            let now_on = auto.on;
            match Self::save_auto(auto) {
                Ok(()) => {
                    if switched && now_on {
                        cx.info(tr("Automatic removal is on: CleanCore checks every day."));
                    }
                }
                Err(e) => {
                    cx.error(crate::tf!("Setting up automatic removal failed: {e:#}"));
                    self.auto = None;
                }
            }
        }
    }

    fn settings_card(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        style::card(ui, |ui| {
            let off: Vec<Tweak> = self.tweaks.iter().filter(|t| self.status.get(&t.id).is_some_and(|s| s.available.is_ok() && s.current.is_none())).cloned().collect();
            let mut apply_all = false;
            crate::gui::style::row(ui, |ui| {
                style::section_title(ui, "AI, ads and suggestions");
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!off.is_empty(), |ui| {
                        apply_all = style::primary(ui, icon::SHIELD_CHECK, &crate::tf!("Turn all off ({})", off.len())).clicked();
                    });
                });
            });
            ui.add(style::Label::new(style::muted(ui, tr("Each switch can be turned back on here or in the Tuner. Some need administrator rights.")).size(12.5)));
            ui.add_space(6.0);
            if !self.pending.is_empty() {
                let msgs: Vec<&str> = self.pending.iter().filter_map(|r| r.message()).collect();
                ui.add(style::Label::new(style::rich(crate::tf!("{}  {}", icon::WARNING, msgs.join(" "))).size(12.5).color(p.warning)));
                ui.add_space(4.0);
            }
            let mut change: Option<(Tweak, bool)> = None;
            for t in &self.tweaks {
                let st = self.status.get(&t.id);
                crate::gui::style::row(ui, |ui| {
                    let mut applied = st.is_some_and(|s| s.current.is_some());
                    let available = st.is_none_or(|s| s.available.is_ok());
                    ui.add_enabled_ui(available, |ui| {
                        if style::toggle(ui, &mut applied, &t.name).changed() {
                            change = Some((t.clone(), applied));
                        }
                    });
                    crate::gui::style::column(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        crate::gui::style::row(ui, |ui| {
                            ui.add(style::Label::new(tr(&t.name)));
                            if st.is_some_and(|s| s.ours.is_some()) {
                                style::badge(ui, "by CleanCore", p.accent);
                            }
                        });
                        let note = match st.map(|s| &s.available) {
                            Some(Err(why)) => why.clone(),
                            _ => tr(&t.description).to_string(),
                        };
                        ui.add(style::Label::new(style::muted(ui, note).size(12.0)));
                    });
                });
                ui.add_space(2.0);
            }
            if apply_all {
                for t in off {
                    self.change(cx, &t, true);
                }
            }
            if let Some((t, on)) = change {
                self.change(cx, &t, on);
            }
        });
    }

    fn change(&mut self, cx: &mut Cx, t: &Tweak, on: bool) {
        let r = if on {
            tuner::apply(t, &t.options[0].id, &mut self.state)
        } else if self.state.applied.contains_key(&t.id) {
            tuner::revert(t, &mut self.state)
        } else {
            return cx.error(crate::tf!("\"{}\" was set outside CleanCore, so there is nothing to restore. Change it in Windows Settings.", t.name));
        };
        match r {
            Ok(req) => {
                if req != Requires::None {
                    self.pending.insert(req);
                }
                cx.info(crate::tf!("{}: {}", if on { tr("Done") } else { tr("Restored") }, t.name));
            }
            Err(e) => cx.error(crate::tf!("{}: {e:#}", t.name)),
        }
        if let Some(s) = tuner::status_all(std::slice::from_ref(t), &self.state).pop() {
            self.status.insert(t.id.clone(), s);
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if !self.confirm {
            return;
        }
        let picked: Vec<Found> = match &self.scan {
            Some(Ok(s)) => s.found.iter().filter(|f| self.checked.contains(f.app.name)).cloned().collect(),
            _ => Vec::new(),
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("slop-confirm")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.add(style::Label::new(style::strong(crate::tf!("Remove {} app(s)?", picked.len())).size(18.0)));
            ui.add_space(6.0);
            for f in picked.iter().take(10) {
                ui.add(style::Label::new(crate::tf!("• {}", f.app.name)));
            }
            if picked.len() > 10 {
                ui.add(style::Label::new(style::muted(ui, crate::tf!("and {} more", picked.len() - 10))));
            }
            ui.add_space(6.0);
            let scope = match (self.all_users, self.deprovision) {
                (true, true) => tr("For every user, and they won't come back for new accounts."),
                (true, false) => tr("For every user account."),
                (false, true) => tr("For you, and they won't come back for new accounts."),
                (false, false) => tr("For you only."),
            };
            ui.add(style::Label::new(style::muted(ui, crate::tf!("{scope} They can be reinstalled from the Microsoft Store."))));
            ui.add_space(12.0);
            crate::gui::style::row(ui, |ui| {
                if style::primary(ui, icon::TRASH, "Remove").clicked() {
                    let (s, list, all, dep) = (cx.settings.clone(), picked.clone(), self.all_users, self.deprovision);
                    self.removing = Some(Pending::spawn(ui.ctx(), "Removing apps", move || slop::remove(&s, &list, all, dep)));
                    close = true;
                }
                if style::secondary(ui, "", "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.confirm = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    /// Renders the tab with Windows-like data on any OS and saves a screenshot.
    #[test]
    fn slop_tab_renders() {
        let mut view = SlopView::new();
        let installed: Vec<String> =
            ["king.com.CandyCrushSaga", "Microsoft.BingNews", "Microsoft.GamingApp", "Microsoft.Copilot", "Clipchamp.Clipchamp"].map(String::from).to_vec();
        let prov = vec![("Clipchamp.Clipchamp".to_string(), "Clipchamp.Clipchamp_1_neutral_~_x".to_string())];
        view.set_scan(Ok(Scan { found: slop::match_catalog(&installed, &prov), admin: true }));
        view.tweaks = tuner::parse_table(tuner::WINDOWS_TABLE).unwrap().into_iter().filter(|t| slop::TWEAKS.contains(&t.id.as_str())).collect();
        view.status =
            view.tweaks.iter().map(|t| (t.id.clone(), Status { id: t.id.clone(), available: Ok(()), current: (t.id == "disable_ads").then(|| "on".into()), ours: None })).collect();
        view.tweaks_loaded = true;
        let mut settings = crate::config::Settings::default();
        let mut notices = Vec::new();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000.0, 1100.0)).wgpu().build_ui(move |ui| {
            super::super::style::install_fonts(ui.ctx());
            super::super::style::apply(ui.ctx(), "light", "blue");
            egui::CentralPanel::default().show(ui, |ui| {
                let mut cx = Cx { settings: &mut settings, notices: &mut notices };
                view.ui(ui, &mut cx);
            });
        });
        h.run_steps(4);
        assert!(h.query_by_label_contains("Remove (4)").is_some(), "the four recommended apps start ticked");
        assert!(h.query_all_by_label("Turn off Copilot").next().is_some());
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/gui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join("21-tools-slop.png")).unwrap();
    }
}
