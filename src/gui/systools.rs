//! Tools › System / Windows tools: the OS utilities as cards grouped by area, plus the
//! user's own tools.

use super::style::{self, icon, Palette};
use super::Cx;
use crate::i18n::tr;
use crate::systools::{self, Tool};
use eframe::egui::{self, RichText, Ui};

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

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        let tools = systools::list(cx.settings);
        let q = self.search.to_lowercase();
        let shown: Vec<&Tool> = tools
            .iter()
            .filter(|t| q.is_empty() || t.name.to_lowercase().contains(&q) || t.description.to_lowercase().contains(&q) || t.command.to_lowercase().contains(&q))
            .collect();
        ui.horizontal(|ui| {
            style::search_field(ui, &mut self.search, "Search tools", 220.0);
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
                ui.horizontal(|ui| {
                    ui.label(RichText::new(group_icon(g)).color(p.accent));
                    ui.label(style::strong(tr(g)));
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for t in shown.iter().filter(|t| t.group == g) {
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(262.0, 64.0), egui::Sense::click());
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &t.name));
                        style::tile_frame(ui, rect, &r, false);
                        let left = rect.left_top() + egui::vec2(14.0, 14.0);
                        ui.painter().text(
                            left + egui::vec2(0.0, 2.0),
                            egui::Align2::LEFT_TOP,
                            if t.terminal { icon::DOTS } else { icon::APP_WINDOW },
                            egui::FontId::proportional(18.0),
                            p.accent,
                        );
                        let mut name = tr(&t.name).to_string();
                        if t.admin {
                            name += "  \u{E2FA}"; // lock: needs administrator rights
                        }
                        ui.painter().text(left + egui::vec2(28.0, 0.0), egui::Align2::LEFT_TOP, name, egui::FontId::new(14.0, style::semibold()), p.text);
                        let desc: String = if t.description.is_empty() { t.command.clone() } else { tr(&t.description).to_string() };
                        let desc = if desc.chars().count() > 38 { desc.chars().take(37).collect::<String>() + "…" } else { desc };
                        ui.painter().text(left + egui::vec2(28.0, 20.0), egui::Align2::LEFT_TOP, desc, egui::FontId::proportional(12.0), p.muted);
                        let r = r.on_hover_text(crate::tf!("{}{}", t.command, if t.admin { tr(" (asks for administrator rights)") } else { "" }));
                        if r.clicked() {
                            run = Some((*t).clone());
                        }
                        if t.custom {
                            r.context_menu(|ui| {
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
            style::card(ui, |ui| {
                style::section_title(ui, "Add your own tool");
                ui.label(style::muted(ui, tr("Any program or command line. Right-click one of your tools to remove it.")).size(12.5));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    style::text_input(ui, &mut self.new_name, "Name", 180.0);
                    style::text_input(ui, &mut self.new_command, if cfg!(windows) { r"C:\Tools\app.exe --flag" } else { tr("/usr/bin/app --flag") }, 340.0);
                    if let Some(p) = style::browse_button(ui, "systool-command", style::Pick::File, &self.new_command) {
                        if self.new_name.trim().is_empty() {
                            self.new_name = std::path::Path::new(&p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        }
                        self.new_command = style::command_from_path(&p);
                    }
                    if style::secondary(ui, icon::PLUS, "Add").clicked() {
                        match systools::add(cx.settings, &self.new_name, &self.new_command) {
                            Ok(()) => {
                                cx.info(crate::tf!("Added \"{}\"", self.new_name.trim()));
                                self.new_name.clear();
                                self.new_command.clear();
                            }
                            Err(e) => cx.error(crate::tf!("{e:#}")),
                        }
                    }
                });
            });
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
