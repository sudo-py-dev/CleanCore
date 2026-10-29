//! Settings page. Every change is saved as it is made.

use super::style::{self, icon, Palette, ACCENTS};
use super::{apply_theme, Cx};
use crate::config::{ListEntry, Settings};
use crate::i18n::tr;
use crate::model::ScanMode;
use eframe::egui::{self, Color32, Ui, Vec2};

pub struct SettingsView {
    draft: Settings,
    new_exclude: String,
    new_include: String,
    backup_dir: String,
    older_on: bool,
    older_days: u32,
    larger_on: bool,
    larger_mb: u64,
    db_folder: String,
    db_url: String,
    /// The settings the form was last in step with, to notice changes made on other pages.
    synced: String,
    /// The dialog was just opened: the click that opened it must not close it again.
    fresh: bool,
}

impl SettingsView {
    pub fn new(s: &Settings) -> Self {
        Self {
            draft: s.clone(),
            new_exclude: String::new(),
            new_include: String::new(),
            backup_dir: s.backup_dir().display().to_string(),
            older_on: s.auto_delete_backups_older_than_days.is_some(),
            older_days: s.auto_delete_backups_older_than_days.unwrap_or(30),
            larger_on: s.auto_delete_backups_over_mb.is_some(),
            larger_mb: s.auto_delete_backups_over_mb.unwrap_or(1024),
            db_folder: crate::tracer::db_folder(s).display().to_string(),
            db_url: s.logs_db_url.clone().unwrap_or_default(),
            synced: serde_json::to_string(s).unwrap_or_default(),
            fresh: true,
        }
    }

    /// The draft with the form-only fields folded in.
    fn finished(&self) -> Settings {
        let mut d = self.draft.clone();
        d.auto_delete_backups_older_than_days = self.older_on.then_some(self.older_days);
        d.auto_delete_backups_over_mb = self.larger_on.then_some(self.larger_mb);
        let default_dir = crate::config::data_dir().join("Backups").display().to_string();
        d.backup_dir = (self.backup_dir.trim() != default_dir && !self.backup_dir.trim().is_empty()).then(|| self.backup_dir.trim().into());
        let default_db = crate::config::data_dir().join("logs-db").display().to_string();
        d.logs_db_folder = (self.db_folder.trim() != default_db && !self.db_folder.trim().is_empty()).then(|| self.db_folder.trim().into());
        d.logs_db_url = Some(self.db_url.trim().to_string()).filter(|u| !u.is_empty());
        d
    }

    /// The Settings dialog over the app. Returns false once it is closed (the close button,
    /// Escape or a click outside it).
    pub fn dialog(&mut self, ctx: &egui::Context, cx: &mut Cx, updates: &mut super::updates::Updates) -> bool {
        // Settings changed elsewhere (another page, a newer copy) are taken in first, so the form
        // never writes back stale values.
        if serde_json::to_string(&*cx.settings).unwrap_or_default() != self.synced {
            let (new_exclude, new_include) = (std::mem::take(&mut self.new_exclude), std::mem::take(&mut self.new_include));
            *self = SettingsView { new_exclude, new_include, ..SettingsView::new(cx.settings) };
        }
        let before = serde_json::to_string(&self.finished()).ok();
        let p = Palette::get(ctx);
        let screen = ctx.content_rect();
        let size = Vec2::new((screen.width() - 64.0).clamp(520.0, 980.0), (screen.height() - 150.0).clamp(320.0, 620.0));
        let mut close = false;
        let frame = egui::Frame::new().fill(p.bg).corner_radius(15).shadow(ctx.global_style().visuals.window_shadow);
        let id = egui::Id::unique("settings-dialog");
        let intro = style::dialog_intro(ctx, id);
        let shown = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
            ui.multiply_opacity(intro);
            ui.set_width(size.x);
            ui.spacing_mut().item_spacing.y = 0.0;
            // Title bar: the name in the middle, the close button at the far end.
            // Header bar, as in GNOME: the title centred, a round close button at the far end.
            egui::Frame::new().show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(size.x, 50.0), egui::Sense::hover());
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(tr("Settings")), egui::FontId::new(15.0, style::semibold()), p.text);
                let m = style::Mirror::new(rect);
                let x = m.rect(size.x - 38.0, rect.center().y - 12.0, Vec2::splat(24.0));
                if ui.put(x, |ui: &mut Ui| style::close_button(ui, "Close")).clicked() {
                    close = true;
                }
            });
            ui.add(egui::Separator::default().spacing(0.0));
            egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.set_height(size.y);
                self.body(ui, cx, updates, p);
            });
        });
        if shown.should_close() && !self.fresh {
            close = true;
        }
        self.fresh = false;
        let after = self.finished();
        if serde_json::to_string(&after).ok() != before {
            *cx.settings = after;
            cx.save_settings();
        }
        self.synced = serde_json::to_string(&*cx.settings).unwrap_or_default();
        !close
    }

    /// The category list on the side, the chosen category's settings next to it. The choice
    /// outlives the dialog, which is rebuilt each time it is opened.
    fn body(&mut self, ui: &mut Ui, cx: &mut Cx, updates: &mut super::updates::Updates, p: Palette) {
        let id = egui::Id::unique("settings-section");
        let mut section = ui.ctx().data(|d| d.get_temp::<Section>(id)).unwrap_or(Section::General);
        let side = if ui.available_width() < 700.0 { 168.0 } else { 210.0 };
        let page_h = ui.available_height();
        crate::gui::style::row_top(ui, |ui| {
            ui.set_min_height(page_h);
            crate::gui::style::column(ui, |ui| {
                ui.set_width(side);
                ui.spacing_mut().item_spacing.y = 4.0;
                for s in Section::ALL {
                    if category(ui, s, s == section, p).clicked() {
                        section = s;
                    }
                }
            });
            ui.add(egui::Separator::default().vertical().spacing(20.0));
            egui::ScrollArea::vertical().id_salt(section.title()).auto_shrink([false, false]).min_scrolled_height(page_h).max_height(page_h).show(ui, |ui| {
                crate::gui::style::column(ui, |ui| self.section(ui, cx, updates, section));
            });
        });
        ui.ctx().data_mut(|d| d.insert_temp(id, section));
    }

    /// One category's heading and settings.
    fn section(&mut self, ui: &mut Ui, cx: &mut Cx, updates: &mut super::updates::Updates, section: Section) {
        ui.set_width(ui.available_width());
        ui.add(style::Label::new(style::strong(tr(section.title())).size(20.0)));
        ui.add_space(10.0);
        match section {
            Section::General => self.general(ui, cx),
            Section::Uninstaller => self.uninstaller(ui, cx),
            Section::Cleaner => self.cleaner(ui),
            Section::Lists => self.lists(ui),
            Section::Backups => self.backups(ui),
            Section::Updates => updates.card(ui, cx, &mut self.draft.check_updates),
        }
    }

    /// Appearance, notifications and where CleanCore keeps its data.
    fn general(&mut self, ui: &mut Ui, cx: &mut Cx) {
        self.appearance(ui, cx);
        ui.add_space(14.0);
        style::card(ui, |ui| {
            style::section_title(ui, "Watcher and notifications");
            style::setting_row(ui, "Maximum log folder size", "When the logs reach this size, all watches pause. 0 means no limit.", |ui| {
                ui.add(egui::DragValue::new(&mut self.draft.watcher_max_log_mb).range(0..=100_000).suffix(" MB"));
            });
            style::divider(ui);
            style::setting_row(ui, "Notify after Auto Clean", "Failures always notify.", |ui| {
                style::toggle(ui, &mut self.draft.notify_auto_clean, "Notify after Auto Clean");
            });
            style::setting_row(ui, "Notify after Auto Backup", "", |ui| {
                style::toggle(ui, &mut self.draft.notify_auto_backup, "Notify after Auto Backup");
            });
            style::setting_row(ui, "Skip confirmation dialogs", "Clean without asking. Backups are still made.", |ui| {
                style::toggle(ui, &mut self.draft.skip_warnings, "Skip confirmation dialogs");
            });
        });
        ui.add_space(10.0);
        ui.add(style::Label::new(style::muted(ui, crate::tf!("Data folder: {}", crate::config::data_dir().display())).size(12.0)));
    }

    /// The exclude and include lists.
    fn lists(&mut self, ui: &mut Ui) {
        style::card(ui, |ui| {
            list_editor(ui, &mut self.draft.exclude_list, &mut self.new_exclude, "Exclude list", "Never reported or deleted. Subkeys / subfolders can be included.");
        });
        ui.add_space(14.0);
        style::card(ui, |ui| {
            list_editor(
                ui,
                &mut self.draft.include_list,
                &mut self.new_include,
                "Include list",
                "Contents are always deleted by a Custom Scan with the \"Include list\" category. Be careful.",
            );
        });
    }

    fn appearance(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        style::card(ui, |ui| {
            style::section_title(ui, "Appearance");
            style::setting_row(ui, "Theme", "", |ui| {
                let cur = cx.settings.gui_theme.clone().unwrap_or_else(|| "light".into());
                let norm = match cur.as_str() {
                    "dark" | "black" | "gray" => "dark",
                    "system" => "system",
                    _ => "light",
                };
                for (id, label) in
                    [("system", crate::tf!("{}  System", icon::MONITOR)), ("dark", crate::tf!("{}  Dark", icon::MOON)), ("light", crate::tf!("{}  Light", icon::SUN))]
                {
                    if style::segment(ui, norm == id, &label).clicked() {
                        cx.settings.gui_theme = Some(id.into());
                        self.draft.gui_theme = Some(id.into());
                        apply_theme(ui.ctx(), cx.settings);
                        cx.save_settings();
                    }
                }
            });
            style::divider(ui);
            style::setting_row(ui, "Language", "", |ui| {
                let names = |code: &str| -> String {
                    if code == "system" {
                        let sys = crate::i18n::system_language();
                        let n = crate::i18n::LANGUAGES.iter().find(|l| l.code == sys).map_or("English", |l| l.name.as_str());
                        crate::tf!("System ({n})")
                    } else {
                        crate::i18n::LANGUAGES.iter().find(|l| l.code == code).map_or(code.to_string(), |l| l.name.to_string())
                    }
                };
                let cur = cx.settings.language.clone();
                let mut pick = None;
                style::dropdown(ui, "language", &tf_icon(icon::GLOBE, &names(&cur)), 200.0, |ui| {
                    for code in std::iter::once("system").chain(crate::i18n::LANGUAGES.iter().map(|l| l.code)) {
                        if ui.selectable_label(cur == code, style::vis(names(code))).clicked() {
                            pick = Some(code.to_string());
                        }
                    }
                });
                if let Some(code) = pick {
                    crate::i18n::set_language(&code);
                    cx.settings.language = code.clone();
                    self.draft.language = code;
                    cx.save_settings();
                    ui.ctx().request_repaint();
                }
            });
            style::divider(ui);
            style::setting_row(ui, "Accent color", "", |ui| {
                let cur = cx.settings.gui_accent.clone().unwrap_or_else(|| "blue".into());
                for (id, label, color) in ACCENTS.iter().rev() {
                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
                    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, cur == *id, tr(label)));
                    let shown = if p.dark { color.lerp_to_gamma(egui::Color32::WHITE, 0.18) } else { *color };
                    ui.painter().circle_filled(rect.center(), 11.0, shown);
                    if cur == *id {
                        ui.painter().circle_stroke(rect.center(), 13.5, egui::Stroke::new(2.0, shown));
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::CHECK), egui::FontId::proportional(13.0), egui::Color32::WHITE);
                    }
                    let r = r.on_hover_text(style::vis(tr(label)));
                    if r.clicked() {
                        cx.settings.gui_accent = Some(id.to_string());
                        self.draft.gui_accent = Some(id.to_string());
                        apply_theme(ui.ctx(), cx.settings);
                        cx.save_settings();
                    }
                }
            });
        });
    }

    fn uninstaller(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let d = &mut self.draft;
        style::card(ui, |ui| {
            style::setting_row(
                ui,
                if cfg!(windows) { tr("Add to Explorer's right-click menu") } else { tr("Add to the file manager's right-click menu") },
                "\"Install with CleanCore\" on installers, \"Uninstall with CleanCore\" on programs, shortcuts and folders. Applies at once.",
                |ui| {
                    let mut on = crate::shell::is_integrated();
                    if style::toggle(ui, &mut on, "File manager menu").changed() {
                        match crate::shell::integrate(on) {
                            Ok(()) => cx.info(if on { tr("Added CleanCore to the right-click menu") } else { tr("Removed CleanCore from the right-click menu") }),
                            Err(e) => cx.error(crate::tf!("{e:#}")),
                        }
                    }
                },
            );
            style::setting_row(ui, "Leftover scan mode", "Used after a program's own uninstaller finishes.", |ui| {
                for (m, l) in [(ScanMode::Advanced, "Advanced"), (ScanMode::Moderate, "Moderate"), (ScanMode::Safe, "Safe")] {
                    if style::segment(ui, d.uninstall_scan_mode == m, l).clicked() {
                        d.uninstall_scan_mode = m;
                    }
                }
            });
            style::setting_row(ui, if cfg!(windows) { tr("Create a restore point before uninstalling") } else { tr("Create a system snapshot before uninstalling") }, "", |ui| {
                style::toggle(ui, &mut d.uninstall_restore_point, "Create a restore point before uninstalling");
            });
            style::setting_row(ui, "Create a full backup before uninstalling", "", |ui| {
                style::toggle(ui, &mut d.uninstall_full_backup, "Create a full backup before uninstalling");
            });
            style::setting_row(
                ui,
                if cfg!(windows) { tr("Delete leftover files to the Recycle Bin") } else { tr("Delete leftover files to the Trash") },
                "Files are also backed up either way.",
                |ui| {
                    style::toggle(ui, &mut d.delete_to_trash, "Delete leftover files to the trash");
                },
            );
            style::setting_row(ui, "Close the program before uninstalling", "Stops its running processes so the uninstaller can remove every file.", |ui| {
                style::toggle(ui, &mut d.uninstall_kill_running, "Close the program before uninstalling");
            });
            style::setting_row(ui, "Only run the built-in uninstaller", "Skip the leftover scan afterwards.", |ui| {
                style::toggle(ui, &mut d.uninstall_builtin_only, "Only run the built-in uninstaller");
            });
            style::setting_row(ui, "Tick leftovers by default", "Otherwise you pick each item to delete.", |ui| {
                style::toggle(ui, &mut d.leftovers_checked_by_default, "Tick leftovers by default");
            });
            style::setting_row(ui, "Mark programs as new for", "", |ui| {
                ui.add(egui::DragValue::new(&mut d.new_program_days).range(1..=365).suffix(" days"));
            });
            style::divider(ui);
            style::setting_row(
                ui,
                "Only record what the installer itself changes",
                if cfg!(windows) {
                    tr("Follows the installer's processes with ETW (needs administrator rights), so other programs' changes during a trace are left out.")
                } else {
                    tr("Follows the installer's processes with strace, so other programs' changes during a trace are left out.")
                },
                |ui| {
                    style::toggle(ui, &mut d.trace_follow_process, "Only record what the installer itself changes");
                },
            );
            style::setting_row(ui, "Delete a trace log after uninstalling with it", "", |ui| {
                style::toggle(ui, &mut d.delete_trace_after_uninstall, "Delete a trace log after uninstalling with it");
            });
            style::setting_row(
                ui,
                "Show system entries in Autorun",
                if cfg!(windows) { tr("Microsoft services and tasks.") } else { tr("Vendor services and desktop components.") },
                |ui| {
                    style::toggle(ui, &mut d.autorun_show_system, "Show system entries in Autorun");
                },
            );
            style::divider(ui);
            ui.add(style::Label::new(style::muted(ui, tr("Logs database folder (a shared or network folder works too)"))));
            style::path_input(ui, "logsdb-folder", &mut self.db_folder, "", ui.available_width(), style::Pick::Folder);
            ui.add_space(4.0);
            ui.add(style::Label::new(style::muted(ui, tr("Logs database URL (serves index.json and the .cptrace files; optional)"))));
            style::text_input(ui, &mut self.db_url, "https://example.org/cleancore-logs", ui.available_width());
        });
    }

    fn cleaner(&mut self, ui: &mut Ui) {
        let d = &mut self.draft;
        style::card(ui, |ui| {
            style::setting_row(ui, "Skip paths on external drives", "Items on drives that are not plugged in are never reported.", |ui| {
                style::toggle(ui, &mut d.exclude_external_drives, "Skip paths on external drives");
            });
            style::setting_row(
                ui,
                if cfg!(windows) { tr("Skip Windows defaults") } else { tr("Skip system defaults") },
                "Known entries the system recreates or keeps on purpose.",
                |ui| {
                    style::toggle(ui, &mut d.exclude_os_defaults, "Skip system defaults");
                },
            );
            if cfg!(windows) {
                style::setting_row(ui, "Force delete registry keys", "Take ownership when a key is protected.", |ui| {
                    style::toggle(ui, &mut d.force_delete, "Force delete registry keys");
                });
            }
            style::divider(ui);
            style::setting_row(
                ui,
                if cfg!(windows) { tr("Create a restore point before cleaning") } else { tr("Create a system snapshot before cleaning") },
                if cfg!(windows) { tr("Uses Windows System Restore.") } else { tr("Uses snapper or timeshift if installed.") },
                |ui| {
                    style::toggle(ui, &mut d.restore_point_before_clean, "Create a restore point before cleaning");
                },
            );
            style::setting_row(ui, "Create a full backup before cleaning", "In addition to the backup of deleted items.", |ui| {
                style::toggle(ui, &mut d.full_backup_before_clean, "Create a full backup before cleaning");
            });
            style::divider(ui);
            style::setting_row(ui, "Quick Clean scan mode", "", |ui| {
                for (m, l) in [(ScanMode::Advanced, "Advanced"), (ScanMode::Moderate, "Moderate"), (ScanMode::Safe, "Safe")] {
                    if style::segment(ui, d.quick_clean_mode == m, l).clicked() {
                        d.quick_clean_mode = m;
                    }
                }
            });
        });
    }

    fn backups(&mut self, ui: &mut Ui) {
        style::card(ui, |ui| {
            ui.add(style::Label::new(style::muted(ui, tr("Backup folder"))));
            style::path_input(ui, "backup-dir", &mut self.backup_dir, "", ui.available_width(), style::Pick::Folder);
            ui.add_space(6.0);
            style::divider(ui);
            style::setting_row(ui, "Delete backups older than", "", |ui| {
                ui.add_enabled(self.older_on, egui::DragValue::new(&mut self.older_days).range(1..=3650).suffix(" days"));
                style::toggle(ui, &mut self.older_on, "Delete backups older than");
            });
            style::setting_row(ui, "Delete oldest when all backups exceed", "", |ui| {
                ui.add_enabled(self.larger_on, egui::DragValue::new(&mut self.larger_mb).range(1..=1_000_000).suffix(" MB"));
                style::toggle(ui, &mut self.larger_on, "Delete oldest when backups exceed");
            });
            ui.add_enabled_ui(self.older_on || self.larger_on, |ui| {
                style::setting_row(
                    ui,
                    if cfg!(windows) { tr("Daily full registry backup") } else { tr("Daily full settings backup") },
                    "The first time CleanCore opens each day, it backs up everything automatically.",
                    |ui| {
                        style::toggle(ui, &mut self.draft.daily_full_backup, "Daily full backup");
                    },
                );
                style::setting_row(ui, "Only delete automatic backups", "Manual backups are always kept.", |ui| {
                    style::toggle(ui, &mut self.draft.auto_delete_only_auto_backups, "Only delete automatic backups");
                });
            });
        });
    }
}

fn list_editor(ui: &mut Ui, list: &mut Vec<ListEntry>, new: &mut String, title: &str, help: &str) {
    let p = Palette::get(ui.ctx());
    style::section_title(ui, title);
    ui.add(style::Label::new(style::muted(ui, tr(help)).size(12.5)).wrap());
    ui.add_space(6.0);
    let mut remove = None;
    if list.is_empty() {
        ui.add(style::Label::new(style::rich(tr("Empty")).color(p.faint)));
    }
    for (i, e) in list.iter_mut().enumerate() {
        crate::gui::style::row(ui, |ui| {
            style::toggle(ui, &mut e.enabled, &e.path.clone());
            ui.add(style::Label::new(style::rich(&e.path).monospace().color(if e.enabled { p.text } else { p.faint })).truncate());
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if style::icon_button(ui, icon::TRASH, "Remove").clicked() {
                    remove = Some(i);
                }
                ui.checkbox(&mut e.include_subkeys, style::vis(tr("Subkeys")));
            });
        });
    }
    if let Some(i) = remove {
        list.remove(i);
    }
    ui.add_space(4.0);
    crate::gui::style::row(ui, |ui| {
        let picker = if cfg!(windows) { 0.0 } else { 2.0 * (style::CONTROL_H + 8.0) };
        style::text_input(ui, new, if cfg!(windows) { r"HKEY_LOCAL_MACHINE\SOFTWARE\..." } else { "/home/you/.config/..." }, ui.available_width() - 90.0 - picker);
        if !cfg!(windows) {
            if let Some(p) = style::browse_button(ui, &crate::tf!("list-{:p}", list as *const _), style::Pick::Any, new) {
                if !list.iter().any(|e| e.path == p) {
                    list.push(ListEntry { path: p, include_subkeys: true, enabled: true });
                }
            }
        }
        if style::secondary(ui, icon::PLUS, "Add").clicked() && !new.trim().is_empty() {
            list.push(ListEntry { path: new.trim().to_string(), include_subkeys: true, enabled: true });
            new.clear();
        }
    });
}

fn tf_icon(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// A group of settings in the side list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Uninstaller,
    Cleaner,
    Lists,
    Backups,
    Updates,
}

impl Section {
    const ALL: [Section; 6] = [Self::General, Self::Uninstaller, Self::Cleaner, Self::Lists, Self::Backups, Self::Updates];

    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Uninstaller => "Uninstaller",
            Self::Cleaner => "Cleaner",
            Self::Lists => "Lists",
            Self::Backups => "Backups",
            Self::Updates => "Updates",
        }
    }

    fn icon(self) -> (&'static str, Color32) {
        match self {
            Self::General => (icon::SLIDERS, Color32::from_rgb(37, 99, 235)),
            Self::Uninstaller => (icon::TRASH, Color32::from_rgb(225, 29, 72)),
            Self::Cleaner => (icon::BROOM, Color32::from_rgb(22, 163, 74)),
            Self::Lists => (icon::FUNNEL, Color32::from_rgb(217, 119, 6)),
            Self::Backups => (icon::ARCHIVE, Color32::from_rgb(124, 58, 237)),
            Self::Updates => (icon::DOWNLOAD, Color32::from_rgb(13, 148, 136)),
        }
    }
}

/// A side-list entry: a coloured icon tile and the name, highlighted when chosen.
fn category(ui: &mut Ui, s: Section, selected: bool, p: Palette) -> egui::Response {
    let label = tr(s.title());
    let (glyph, color) = s.icon();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), egui::Sense::click());
    // Tabs, so assistive tools (and the translation audit) see them as one group.
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    let h = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() && !selected, style::FADE);
    let s_t = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, style::FADE);
    if s_t > 0.0 || h > 0.0 {
        ui.painter().rect_filled(rect, 8.0, p.hover.gamma_multiply(h).lerp_to_gamma(p.selected, s_t));
    }
    let m = style::Mirror::new(rect);
    let tile = m.rect(10.0, rect.center().y - 15.0, Vec2::splat(30.0));
    let fill = if selected { color } else { Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if p.dark { 52 } else { 32 }) };
    ui.painter().rect_filled(tile, 8.0, fill);
    let fg = if selected { Color32::WHITE } else { color };
    ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, style::vstr(glyph), egui::FontId::new(16.0, style::icon_fill()), fg);
    let font = egui::FontId::new(15.0, if selected { style::semibold() } else { egui::FontFamily::Proportional });
    ui.painter().text(m.pos(52.0, rect.center().y), m.start(egui::Align::Center), style::vstr(label), font, if selected { p.text } else { p.muted.lerp_to_gamma(p.text, 0.5) });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}
