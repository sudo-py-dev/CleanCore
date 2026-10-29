//! Tools › System / Windows tools: the OS utilities as cards grouped by area, plus the
//! user's own tools.

use super::style::{self, icon, Palette};
use super::Cx;
use crate::i18n::tr;
use crate::systools::{self, Tool};
use eframe::egui::{self, Ui};

pub struct SystemToolsView {
    search: String,
    new_name: String,
    new_command: String,
}

fn group_icon(g: &str) -> &'static str {
    match g {
        "Disks" => icon::DRIVES,
        "Network" => icon::GLOBE,
        "Security" => icon::SHIELD_CHECK,
        "Software" => icon::PACKAGE,
        "My tools" => icon::USER,
        _ => icon::MONITOR,
    }
}

impl SystemToolsView {
    pub fn new() -> Self {
        Self { search: String::new(), new_name: String::new(), new_command: String::new() }
    }

    /// The "Add" popover: a name and a command line (or a picked program).
    fn add_form(&mut self, ui: &mut Ui, cx: &mut Cx) {
        // Menus pad buttons less; the form's buttons keep their usual size.
        ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        style::section_title(ui, "Add your own tool");
        ui.add(style::Label::new(style::muted(ui, tr("Any program or command line. Right-click one of your tools to remove it.")).size(12.5)).wrap());
        style::text_input(ui, &mut self.new_name, "Name", 360.0);
        let hint = if cfg!(windows) { r"C:\Tools\app.exe --flag" } else { tr("/usr/bin/app --flag") };
        crate::gui::style::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            style::text_input(ui, &mut self.new_command, hint, 360.0 - style::CONTROL_H - 6.0);
            if let Some(p) = style::browse_button(ui, "systool-command", style::Pick::File, &self.new_command) {
                if self.new_name.trim().is_empty() {
                    self.new_name = std::path::Path::new(&p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                }
                self.new_command = style::command_from_path(&p);
            }
        });
        let ready = !self.new_name.trim().is_empty() && !self.new_command.trim().is_empty();
        if ui.add_enabled_ui(ready, |ui| style::primary(ui, icon::PLUS, "Add")).inner.clicked() {
            match systools::add(cx.settings, &self.new_name, &self.new_command) {
                Ok(()) => {
                    cx.info(crate::tf!("Added \"{}\"", self.new_name.trim()));
                    self.new_name.clear();
                    self.new_command.clear();
                    ui.close();
                }
                Err(e) => cx.error(crate::tf!("{e:#}")),
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        let tools = systools::list(cx.settings);
        let q = self.search.to_lowercase();
        let shown: Vec<&Tool> = tools
            .iter()
            .filter(|t| q.is_empty() || t.name.to_lowercase().contains(&q) || t.description.to_lowercase().contains(&q) || t.command.to_lowercase().contains(&q))
            .collect();
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, "Search tools", 220.0);
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                let r = style::secondary(ui, icon::PLUS, "Add");
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Add your own tool")));
                style::menu(&r).width(380.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| self.add_form(ui, cx));
            });
        });
        ui.add_space(8.0);
        let mut run = None;
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut groups: Vec<&str> = Vec::new();
            for t in &shown {
                if !groups.contains(&t.group) {
                    groups.push(t.group);
                }
            }
            for g in groups {
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::rich(group_icon(g)).color(p.accent)));
                    ui.add(style::Label::new(style::strong(tr(g))));
                });
                ui.add_space(4.0);
                crate::gui::style::row_wrapped(ui, |ui| {
                    for t in shown.iter().filter(|t| t.group == g) {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(262.0, 64.0), egui::Sense::click());
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr(&t.name)));
                        style::tile_frame(ui, rect, &r, false);
                        let mr = crate::gui::style::Mirror::new(rect);
                        let top = rect.top() + 14.0;
                        ui.painter().text(
                            mr.pos(14.0, top + 2.0),
                            mr.start(egui::Align::Min),
                            style::vstr(if t.terminal { icon::DOTS } else { icon::APP_WINDOW }),
                            egui::FontId::proportional(18.0),
                            p.accent,
                        );
                        let mut name = tr(&t.name).to_string();
                        if t.admin {
                            name += "  \u{E2FA}"; // lock: needs administrator rights
                        }
                        ui.painter().text(mr.pos(42.0, top), mr.start(egui::Align::Min), style::vstr(name), egui::FontId::new(14.0, style::semibold()), p.text);
                        let desc: String = if t.description.is_empty() { t.command.clone() } else { tr(&t.description).to_string() };
                        let desc = if desc.chars().count() > 38 { desc.chars().take(37).collect::<String>() + "…" } else { desc };
                        ui.painter().text(mr.pos(42.0, top + 20.0), mr.start(egui::Align::Min), style::vstr(desc), egui::FontId::proportional(12.0), p.muted);
                        let r = r.on_hover_text(style::vis(crate::tf!("{}{}", t.command, if t.admin { tr(" (asks for administrator rights)") } else { "" })));
                        if r.clicked() {
                            run = Some((*t).clone());
                        }
                        if t.custom {
                            style::context_menu(&r, |ui| {
                                if style::ghost(ui, icon::TRASH, "Remove").clicked() {
                                    remove = Some(t.name.clone());
                                    ui.close();
                                }
                            });
                        }
                    }
                });
                ui.add_space(12.0);
            }
        });
        if let Some(t) = run {
            match systools::run(&t) {
                Ok(()) => cx.info(crate::tf!("Started {}", t.name)),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            }
        }
        if let Some(n) = remove {
            match systools::remove(cx.settings, &n) {
                Ok(()) => cx.info(crate::tf!("Removed \"{n}\"")),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            }
        }
    }
}
