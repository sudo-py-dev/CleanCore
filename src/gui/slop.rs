//! Tools › Microsoft slop (Windows): preinstalled and promoted apps to remove, and the AI,
//! ads and telemetry settings to turn off (Tuner tweaks, so they can be turned back on).

use super::style::{self, icon, Palette};
use super::{take, Cx, Pending};
use crate::i18n::tr;
use crate::slop::{self, Found, RemoveReport, Scan};
use crate::tuner::{self, Requires, State, Status, Tweak};
use eframe::egui::{self, RichText, Ui};
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
}

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
        }
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
            self.settings_card(ui, cx, p);
        });
        self.confirm_dialog(ui.ctx(), cx);
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
            ui.label(style::muted(ui, tr("Apps Windows installs or pushes on its own. Removed apps can be reinstalled from the Microsoft Store.")).size(12.5));
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
                ui.label(RichText::new(crate::tf!("{}  {e}", icon::WARNING)).color(p.danger));
                return;
            }
            if !admin {
                ui.label(
                    RichText::new(crate::tf!("{}  Run CleanCore as administrator to remove apps for every user and stop them from coming back.", icon::INFO))
                        .size(12.5)
                        .color(p.warning),
                );
                ui.add_space(4.0);
            }
            crate::gui::style::row(ui, |ui| {
                ui.add_enabled_ui(admin, |ui| {
                    style::toggle(ui, &mut self.all_users, "For every user account");
                    ui.label(tr("For every user account"));
                    ui.add_space(16.0);
                    style::toggle(ui, &mut self.deprovision, "Also remove the preinstalled copy");
                    ui.label(tr("Also remove the preinstalled copy")).on_hover_text(tr("New accounts and feature updates won't bring the app back"));
                });
            });
            ui.add_space(6.0);
            if found.is_empty() {
                ui.label(RichText::new(crate::tf!("{}  None of the known preinstalled apps are installed.", icon::CHECK_CIRCLE)).color(p.success));
                return;
            }
            let mut last = "";
            for f in &found {
                if f.app.group != last {
                    ui.add_space(6.0);
                    ui.label(style::strong(f.app.group).size(13.5));
                    last = f.app.group;
                }
                crate::gui::style::row(ui, |ui| {
                    let mut on = self.checked.contains(f.app.name);
                    if ui.checkbox(&mut on, f.app.name).changed() {
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
                        ui.label(style::muted(ui, f.app.note).size(12.0));
                    }
                });
            }
            let _ = cx;
        });
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
            ui.label(style::muted(ui, tr("Each switch can be turned back on here or in the Tuner. Some need administrator rights.")).size(12.5));
            ui.add_space(6.0);
            if !self.pending.is_empty() {
                let msgs: Vec<&str> = self.pending.iter().filter_map(|r| r.message()).collect();
                ui.label(RichText::new(crate::tf!("{}  {}", icon::WARNING, msgs.join(" "))).size(12.5).color(p.warning));
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
                            ui.label(tr(&t.name));
                            if st.is_some_and(|s| s.ours.is_some()) {
                                style::badge(ui, "by CleanCore", p.accent);
                            }
                        });
                        let note = match st.map(|s| &s.available) {
                            Some(Err(why)) => why.clone(),
                            _ => tr(&t.description).to_string(),
                        };
                        ui.label(style::muted(ui, note).size(12.0));
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
            ui.label(style::strong(crate::tf!("Remove {} app(s)?", picked.len())).size(18.0));
            ui.add_space(6.0);
            for f in picked.iter().take(10) {
                ui.label(crate::tf!("• {}", f.app.name));
            }
            if picked.len() > 10 {
                ui.label(style::muted(ui, crate::tf!("and {} more", picked.len() - 10)));
            }
            ui.add_space(6.0);
            let scope = match (self.all_users, self.deprovision) {
                (true, true) => tr("For every user, and they won't come back for new accounts."),
                (true, false) => tr("For every user account."),
                (false, true) => tr("For you, and they won't come back for new accounts."),
                (false, false) => tr("For you only."),
            };
            ui.label(style::muted(ui, crate::tf!("{scope} They can be reinstalled from the Microsoft Store.")));
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
