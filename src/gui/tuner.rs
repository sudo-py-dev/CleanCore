//! Tuner: categories, search, on/off switches and choices, revert, restart hints.

use super::style::{self, icon, Palette};
use super::Cx;
use crate::i18n::tr;
use crate::tuner::{self, Requires, State, Status, Tweak, CATEGORIES};
use eframe::egui::{self, Ui};
use std::collections::{BTreeSet, HashMap};

pub struct TunerView {
    tweaks: Vec<Tweak>,
    status: HashMap<String, Status>,
    state: State,
    loaded: bool,
    category: &'static str,
    search: String,
    pending: BTreeSet<Requires>,
    /// Tweak with a warning waiting for confirmation: (id, option).
    warn: Option<(String, String)>,
}

enum Change {
    Apply(String, String),
    Revert(String),
    /// Switched off a setting that was changed outside CleanCore: nothing recorded to restore.
    NotOurs(String),
}

impl TunerView {
    pub fn new() -> Self {
        Self {
            tweaks: tuner::tweaks(),
            status: HashMap::new(),
            state: State::default(),
            loaded: false,
            category: CATEGORIES[0].0,
            search: String::new(),
            pending: BTreeSet::new(),
            warn: None,
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn load(&mut self, cx: &mut Cx) {
        match State::load() {
            Ok(s) => self.state = s,
            Err(e) => cx.error(crate::tf!("{e:#}")),
        }
        self.status = tuner::status_all(&self.tweaks, &self.state).into_iter().map(|s| (s.id.clone(), s)).collect();
        self.loaded = true;
    }

    fn change(&mut self, c: Change, cx: &mut Cx) {
        let id = match &c {
            Change::Apply(id, _) | Change::Revert(id) => id.clone(),
            Change::NotOurs(name) => {
                return cx.error(crate::tf!("\"{name}\" was changed outside CleanCore, so there is no original value to restore. Change it in system settings instead."));
            }
        };
        let Some(t) = self.tweaks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let r = match c {
            Change::Apply(_, opt) => tuner::apply(&t, &opt, &mut self.state).map(|r| (r, crate::tf!("Applied: {}", tr(&t.name)))),
            Change::Revert(_) => tuner::revert(&t, &mut self.state).map(|r| (r, crate::tf!("Restored the original setting: {}", tr(&t.name)))),
            Change::NotOurs(_) => unreachable!(),
        };
        match r {
            Ok((req, msg)) => {
                if req != Requires::None {
                    self.pending.insert(req);
                }
                cx.info(msg);
            }
            Err(e) => cx.error(crate::tf!("{e:#}")),
        }
        if let Some(s) = tuner::status_all(std::slice::from_ref(&t), &self.state).pop() {
            self.status.insert(id, s);
        }
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        if !self.loaded {
            self.load(cx);
        }
        let p = Palette::get(ui.ctx());
        let ours = self.state.applied.len();
        let mut reset = false;
        let subtitle = if cfg!(windows) { tr("Hidden Windows settings, switched on and off safely") } else { tr("Desktop settings, switched on and off safely") };
        style::page_header(ui, "Tuner", subtitle, |ui| {
            ui.add_enabled_ui(ours > 0, |ui| {
                reset = style::secondary(ui, icon::UNDO, &crate::tf!("Reset all ({ours})")).on_hover_text(style::vis(tr("Restore every setting changed by CleanCore"))).clicked()
            });
            style::search_field(ui, &mut self.search, "Search tweaks", 200.0);
        });
        if reset {
            let ids: Vec<String> = self.state.applied.keys().cloned().collect();
            for id in ids {
                self.change(Change::Revert(id), cx);
            }
        }

        // Category pills
        if self.search.is_empty() {
            // Category tabs; the count is how many settings CleanCore changed there.
            let items: Vec<(&'static str, &str, &str, Option<usize>)> = CATEGORIES
                .iter()
                .filter(|(id, _)| self.tweaks.iter().any(|t| t.category == *id))
                .map(|(id, label)| {
                    let changed = self.tweaks.iter().filter(|t| t.category == *id && self.state.applied.contains_key(&t.id)).count();
                    (*id, "", *label, (changed > 0).then_some(changed))
                })
                .collect();
            style::tabs(ui, &mut self.category, &items);
        }

        // Restart banner
        if !self.pending.is_empty() {
            let mut restart = false;
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(p.warning.r(), p.warning.g(), p.warning.b(), 26))
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    crate::gui::style::row(ui, |ui| {
                        ui.add(style::Label::new(style::rich(icon::WARNING).size(18.0).color(p.warning)));
                        let msgs: Vec<&str> = self.pending.iter().filter_map(|r| r.message()).map(|m| m.split(" (").next().unwrap_or(m)).collect();
                        ui.add(style::Label::new(style::rich(crate::tf!("Some changes need one more step: {}.", msgs.join(", "))).color(p.text)));
                        if cfg!(windows) && self.pending.contains(&Requires::Explorer) {
                            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| restart = style::secondary(ui, icon::REFRESH, "Restart Explorer").clicked());
                        }
                    });
                });
            if restart {
                match crate::platform::restart_shell() {
                    Ok(()) => {
                        self.pending.remove(&Requires::Explorer);
                        cx.info(tr("File Explorer restarted"));
                    }
                    Err(e) => cx.error(crate::tf!("{e:#}")),
                }
            }
            ui.add_space(10.0);
        }

        let needle = self.search.to_lowercase();
        let shown: Vec<Tweak> = self
            .tweaks
            .iter()
            .filter(|t| if needle.is_empty() { t.category == self.category } else { crate::tf!("{} {} {}", t.id, t.name, t.description).to_lowercase().contains(&needle) })
            .cloned()
            .collect();
        let mut change = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if shown.is_empty() {
                style::card(ui, |ui| style::empty_state(ui, icon::SEARCH, "No tweaks match", "Try another word."));
            }
            style::card(ui, |ui| {
                for (i, t) in shown.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(4.0);
                        style::divider(ui);
                        ui.add_space(4.0);
                    }
                    if let Some(c) = self.row(ui, t, p) {
                        change = Some(c);
                    }
                }
            });
        });
        if let Some(c) = change {
            match &c {
                Change::Apply(id, opt) if self.tweaks.iter().any(|t| &t.id == id && t.warning.is_some()) => self.warn = Some((id.clone(), opt.clone())),
                _ => self.change(c, cx),
            }
        }
        self.warning_modal(ui.ctx(), cx);
    }

    fn row(&self, ui: &mut Ui, t: &Tweak, p: Palette) -> Option<Change> {
        let st = self.status.get(&t.id);
        let available = st.is_some_and(|s| s.available.is_ok());
        let current = st.and_then(|s| s.current.clone());
        let ours = st.and_then(|s| s.ours.clone());
        let mut out = None;
        crate::gui::style::row(ui, |ui| {
            crate::gui::style::column(ui, |ui| {
                ui.set_width((ui.available_width() - 240.0).max(240.0));
                ui.add(style::Label::new(style::rich(tr(&t.name)).color(if available { p.text } else { p.faint })));
                if !t.description.is_empty() {
                    ui.add(style::Label::new(style::rich(tr(&t.description)).size(12.5).color(p.muted)));
                }
                crate::gui::style::row_wrapped(ui, |ui| {
                    if ours.is_some() {
                        style::badge(ui, "by CleanCore", p.accent);
                    }
                    let req = match t.requires {
                        Requires::Explorer => Some("Restart Explorer"),
                        Requires::Logoff => Some("Log out"),
                        Requires::Reboot => Some("Restart PC"),
                        Requires::None => None,
                    };
                    if let Some(r) = req {
                        style::badge(ui, r, p.muted);
                    }
                    if t.warning.is_some() {
                        style::badge(ui, "Caution", p.warning);
                    }
                    if let Some(Err(why)) = st.map(|s| &s.available) {
                        ui.add(style::Label::new(style::rich(crate::tf!("Not available: {why}")).size(12.0).color(p.faint)));
                    }
                });
            });
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(available, |ui| {
                    if t.is_toggle() {
                        let mut on = current.is_some();
                        if style::toggle(ui, &mut on, &t.name).on_hover_text(style::vis(tr(&t.name))).changed() {
                            out = Some(if on { Change::Apply(t.id.clone(), t.options[0].id.clone()) } else { Change::Revert(t.id.clone()) });
                        }
                    } else {
                        let label = |id: &str| t.option(id).map(|o| if o.label.is_empty() { o.id.clone() } else { tr(&o.label).to_string() }).unwrap_or_default();
                        let text = current.as_deref().map(label).unwrap_or_else(|| "Default".into());
                        egui::ComboBox::from_id_salt(("tweak", &t.id)).width(210.0).selected_text(style::vis(text)).show_ui(ui, |ui| {
                            for o in &t.options {
                                let l = if o.label.is_empty() { o.id.clone() } else { tr(&o.label).to_string() };
                                if ui.selectable_label(current.as_deref() == Some(o.id.as_str()), style::vis(l)).clicked() {
                                    out = Some(Change::Apply(t.id.clone(), o.id.clone()));
                                }
                            }
                            if ours.is_some() && ui.selectable_label(false, style::vis(tr("Restore original"))).clicked() {
                                out = Some(Change::Revert(t.id.clone()));
                            }
                        });
                    }
                });
            });
        });
        if let Some(Change::Revert(_)) = &out {
            if ours.is_none() {
                return Some(Change::NotOurs(t.name.clone()));
            }
        }
        out
    }

    fn warning_modal(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some((id, opt)) = self.warn.clone() else {
            return;
        };
        let warning = self.tweaks.iter().find(|t| t.id == id).and_then(|t| t.warning.clone()).unwrap_or_default();
        let mut answer = None;
        let r = egui::Modal::new(egui::Id::unique("tweak-warning")).show(ctx, |ui| {
            ui.set_width(440.0);
            let p = Palette::get(ui.ctx());
            style::icon_tile(ui, icon::WARNING, p.warning, 44.0);
            ui.add_space(8.0);
            ui.add(style::Label::new(style::strong(tr("Are you sure?")).size(19.0)));
            ui.add(style::Label::new(style::muted(ui, warning)));
            ui.add_space(14.0);
            crate::gui::style::row(ui, |ui| {
                if style::primary(ui, "", "Apply anyway").clicked() {
                    answer = Some(true);
                }
                if style::secondary(ui, "", "Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if r.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.warn = None;
            if yes {
                self.change(Change::Apply(id, opt), cx);
            }
        }
    }
}
