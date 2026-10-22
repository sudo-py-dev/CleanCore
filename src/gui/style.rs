//! Design system: colors, fonts, icons and the small set of widgets every page is built from.
//!
//! Fonts: Inter (SIL OFL 1.1) for text, Phosphor regular and fill (MIT) for icons; egui's
//! bundled font stays as a fallback so other scripts still render.
//!
//! Motion: hover and press states fade (`FADE`), selection indicators slide (`ease_to`), and
//! pages fade in when they open. Everything runs off egui's frame clock, so it also works in
//! the kittest harness.

use crate::i18n::tr;
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Ui, Vec2};
use std::sync::Arc;

/// Phosphor icon codepoints used by the app.
pub mod icon {
    pub const BROOM: &str = "\u{EC54}";
    pub const SLIDERS: &str = "\u{E434}";
    pub const EYE: &str = "\u{E220}";
    pub const ARCHIVE: &str = "\u{E00C}";
    pub const GEAR: &str = "\u{E272}";
    pub const SHIELD_CHECK: &str = "\u{E40C}";
    pub const GAUGE: &str = "\u{E628}";
    pub const ROCKET: &str = "\u{E3FE}";
    pub const SEARCH: &str = "\u{E30C}";
    pub const LIGHTNING: &str = "\u{E2DE}";
    pub const FUNNEL: &str = "\u{E266}";
    pub const CALENDAR: &str = "\u{E712}";
    pub const CLOCK: &str = "\u{E19A}";
    pub const ARROW_LEFT: &str = "\u{E058}";
    pub const TRASH: &str = "\u{E4A6}";
    pub const UNDO: &str = "\u{E038}";
    pub const EXPORT: &str = "\u{EAF0}";
    pub const FOLDER_OPEN: &str = "\u{E256}";
    pub const PLUS: &str = "\u{E3D4}";
    pub const CHECK_CIRCLE: &str = "\u{E184}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const X_CIRCLE: &str = "\u{E4F8}";
    pub const INFO: &str = "\u{E2CE}";
    pub const BELL: &str = "\u{E0CE}";
    pub const PLAY: &str = "\u{E3D0}";
    pub const PAUSE: &str = "\u{E39E}";
    pub const RECORD: &str = "\u{E3EE}";
    pub const FILE: &str = "\u{E230}";
    pub const FOLDER: &str = "\u{E24A}";
    pub const LIST_CHECKS: &str = "\u{EADC}";
    pub const SUN: &str = "\u{E472}";
    pub const MOON: &str = "\u{E330}";
    pub const APP_WINDOW: &str = "\u{E5DA}";
    pub const LINK: &str = "\u{E2E2}";
    pub const SPARKLE: &str = "\u{E6A2}";
    pub const REFRESH: &str = "\u{E094}";
    pub const COPY: &str = "\u{E1CA}";
    pub const X: &str = "\u{E4F6}";
    pub const DOTS: &str = "\u{E208}";
    pub const MONITOR: &str = "\u{E32E}";
    pub const LOCK: &str = "\u{E2FA}";
    pub const USERS: &str = "\u{E4D6}";
    pub const USER: &str = "\u{E4C2}";
    pub const SQUARES: &str = "\u{E464}";
    pub const FILE_TEXT: &str = "\u{E23A}";
    pub const DRIVES: &str = "\u{E2A0}";
    pub const KEY: &str = "\u{E2D6}";
    pub const HISTORY: &str = "\u{E1A0}";
    pub const CHECK: &str = "\u{E182}";
    pub const DATABASE: &str = "\u{E1DE}";
    pub const CURSOR: &str = "\u{E7C8}";
    pub const FLOPPY: &str = "\u{E248}";
    pub const TREE: &str = "\u{E67C}";
    pub const PALETTE: &str = "\u{E6C8}";
    pub const PACKAGE: &str = "\u{E390}";
    pub const CUBE: &str = "\u{E1DA}";
    pub const STOREFRONT: &str = "\u{E470}";
    pub const GLOBE: &str = "\u{E288}";
    pub const CROSSHAIR: &str = "\u{E1D6}";
    pub const TAG: &str = "\u{E478}";
    pub const CHAT: &str = "\u{E17A}";
    pub const WRENCH: &str = "\u{E5D4}";
    pub const ARROW_RIGHT: &str = "\u{E06C}";
    pub const SHIELD_WARNING: &str = "\u{E412}";
    pub const DOWNLOAD: &str = "\u{E20C}";
    pub const ARROW_UP: &str = "\u{E08E}";
    pub const ARROW_DOWN: &str = "\u{E03E}";
}

/// Category id → icon, for scan results.
/// Icons a program or trace log can be given (Revo: Change icon). Keys are stored, not glyphs.
pub const ICON_CHOICES: &[(&str, &str)] = &[
    ("app", icon::APP_WINDOW),
    ("cube", icon::CUBE),
    ("package", icon::PACKAGE),
    ("globe", icon::GLOBE),
    ("monitor", icon::MONITOR),
    ("gear", icon::GEAR),
    ("wrench", icon::WRENCH),
    ("lightning", icon::LIGHTNING),
    ("rocket", icon::ROCKET),
    ("shield", icon::SHIELD_CHECK),
    ("lock", icon::LOCK),
    ("key", icon::KEY),
    ("database", icon::DATABASE),
    ("file", icon::FILE_TEXT),
    ("folder", icon::FOLDER),
    ("palette", icon::PALETTE),
    ("chat", icon::CHAT),
    ("users", icon::USERS),
    ("tag", icon::TAG),
    ("store", icon::STOREFRONT),
    ("sparkle", icon::SPARKLE),
    ("gauge", icon::GAUGE),
    ("drives", icon::DRIVES),
    ("record", icon::RECORD),
];

pub fn icon_for_key(key: &str) -> Option<&'static str> {
    ICON_CHOICES.iter().find(|(k, _)| *k == key).map(|(_, g)| *g)
}

/// A grid of the icon choices; returns `Some(None)` for "Default", `Some(Some(key))` for a pick.
pub fn icon_picker(ui: &mut Ui, current: Option<&str>) -> Option<Option<String>> {
    let p = Palette::get(ui.ctx());
    let mut out = None;
    egui::Grid::new(ui.next_auto_id()).spacing([4.0, 4.0]).show(ui, |ui| {
        for (i, (key, glyph)) in ICON_CHOICES.iter().enumerate() {
            let sel = current == Some(*key);
            let r = ui.add(
                egui::Button::new(vis(rich(*glyph).size(18.0).color(if sel { p.accent } else { p.text })))
                    .fill(if sel { p.accent_soft } else { Color32::TRANSPARENT })
                    .min_size(Vec2::splat(34.0)),
            );
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, sel, format!("Icon {key}")));
            if r.clicked() {
                out = Some(Some(key.to_string()));
            }
            if i % 6 == 5 {
                ui.end_row();
            }
        }
    });
    if ghost(ui, icon::UNDO, "Default icon").clicked() {
        out = Some(None);
    }
    out
}

pub fn category_icon(id: &str) -> &'static str {
    match id {
        "desktop_entries" | "app_paths" => icon::APP_WINDOW,
        "startup" => icon::ROCKET,
        "file_associations" | "dangling_links" => icon::LINK,
        "mru_history" => icon::HISTORY,
        "empty_keys" => icon::FOLDER,
        "uninstaller_data" => icon::ARCHIVE,
        "include_list" => icon::LIST_CHECKS,
        "forced" => icon::SEARCH,
        "shared_dlls_missing" | "shared_dlls_unused" => icon::FILE,
        "msi_paths" => icon::DRIVES,
        "com_activex" => icon::SQUARES,
        "services" => icon::GEAR,
        "firewall_rules" => icon::LOCK,
        "file_extensions" => icon::FILE_TEXT,
        "class_keys" => icon::KEY,
        "context_menu" => icon::CURSOR,
        "system_settings" => icon::MONITOR,
        "cache_entries" => icon::DATABASE,
        _ => icon::FILE,
    }
}

// ---------------------------------------------------------------- palette

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    /// Background of a hovered borderless control.
    pub hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

pub const ACCENTS: [(&str, &str, Color32); 6] = [
    ("blue", "Blue", Color32::from_rgb(37, 99, 235)),
    ("indigo", "Indigo", Color32::from_rgb(79, 70, 229)),
    ("violet", "Violet", Color32::from_rgb(124, 58, 237)),
    ("teal", "Teal", Color32::from_rgb(13, 148, 136)),
    ("green", "Green", Color32::from_rgb(22, 163, 74)),
    ("rose", "Rose", Color32::from_rgb(225, 29, 72)),
];

fn soft(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

impl Palette {
    pub fn new(dark: bool, accent: &str) -> Self {
        let base = ACCENTS.iter().find(|a| a.0 == accent).map(|a| a.2).unwrap_or(ACCENTS[0].2);
        // Lift the accent a little on dark backgrounds so it keeps its contrast.
        let accent = if dark { base.lerp_to_gamma(Color32::WHITE, 0.18) } else { base };
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(15, 17, 21),
                surface: Color32::from_rgb(23, 26, 33),
                surface_alt: Color32::from_rgb(31, 35, 44),
                border: Color32::from_rgb(42, 47, 58),
                text: Color32::from_rgb(230, 232, 236),
                muted: Color32::from_rgb(152, 162, 179),
                faint: Color32::from_rgb(102, 112, 133),
                accent,
                accent_soft: soft(accent, 40),
                on_accent: Color32::WHITE,
                hover: Color32::from_rgb(38, 43, 54),
                success: Color32::from_rgb(50, 213, 131),
                warning: Color32::from_rgb(253, 176, 34),
                danger: Color32::from_rgb(249, 112, 102),
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(246, 247, 249),
                surface: Color32::WHITE,
                surface_alt: Color32::from_rgb(242, 244, 247),
                border: Color32::from_rgb(228, 231, 236),
                text: Color32::from_rgb(16, 24, 40),
                muted: Color32::from_rgb(102, 112, 133),
                faint: Color32::from_rgb(152, 162, 179),
                accent,
                accent_soft: soft(accent, 26),
                on_accent: Color32::WHITE,
                hover: Color32::from_rgb(236, 238, 242),
                success: Color32::from_rgb(18, 183, 106),
                warning: Color32::from_rgb(220, 104, 3),
                danger: Color32::from_rgb(217, 45, 32),
            }
        }
    }

    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Palette>(egui::Id::NULL)).unwrap_or_else(|| Palette::new(false, "blue"))
    }
}

/// Resolve the stored theme name (older versions stored more variants) to dark/light.
pub fn wants_dark(theme: &str, ctx: &egui::Context) -> bool {
    match theme {
        "dark" | "black" | "gray" => true,
        "system" => ctx.system_theme() == Some(egui::Theme::Dark),
        _ => false,
    }
}

pub fn apply(ctx: &egui::Context, theme: &str, accent: &str) {
    let p = Palette::new(wants_dark(theme, ctx), accent);
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, p));
    ctx.set_theme(if p.dark { egui::Theme::Dark } else { egui::Theme::Light });

    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 36 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if p.dark { 90 } else { 28 }) };
    v.extreme_bg_color = if p.dark { Color32::from_rgb(18, 20, 26) } else { Color32::WHITE };
    v.faint_bg_color = p.surface_alt;
    v.code_bg_color = p.surface_alt;
    v.hyperlink_color = p.accent;
    // Soft selection: selected table rows and text keep their normal colours readable.
    v.selection.bg_fill = if p.dark { soft(p.accent, 56) } else { soft(p.accent, 34) };
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.striped = false; // tables opt in explicitly

    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.weak_bg_fill = p.surface_alt;
    w.inactive.bg_fill = if p.dark { Color32::from_rgb(52, 58, 70) } else { Color32::from_rgb(208, 213, 221) };
    // Visible outline for inputs, checkboxes and combo boxes on white cards.
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_fill = w.hovered.weak_bg_fill;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.weak_bg_fill = if p.dark { Color32::from_rgb(48, 54, 66) } else { Color32::from_rgb(225, 228, 234) };
    w.active.bg_fill = w.active.weak_bg_fill;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open = w.active;
    // Both theme slots get the palette, so nothing (a modal on the first frame, a system theme
    // switch) can draw with egui's stock visuals.
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);

    ctx.all_styles_mut(|s| {
        // Labels are information, not text to select; selectable labels also swallow row clicks.
        s.interaction.selectable_labels = false;
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size = Vec2::new(40.0, 30.0);
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.window_margin = Margin::same(20);
        s.spacing.icon_spacing = 6.0;
        s.animation_time = 0.16;
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, semibold())),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Filled icon glyphs (same codepoints as the regular ones), for selected states and logos.
pub fn icon_fill() -> FontFamily {
    FontFamily::Name("icon-fill".into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-Regular.ttf"))));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"))));
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Phosphor.ttf")).tweak(egui::FontTweak { y_offset_factor: 0.07, ..Default::default() })),
    );
    // Hebrew (Noto Sans Hebrew): Inter has no Hebrew letters. Other scripts: `system_fonts`.
    fonts.font_data.insert("hebrew".into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/NotoSansHebrew-Regular.ttf"))));
    fonts.font_data.insert("hebrew-bold".into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/NotoSansHebrew-Bold.ttf"))));
    fonts.font_data.insert(
        "phosphor-fill".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Phosphor-Fill.ttf")).tweak(egui::FontTweak { y_offset_factor: 0.07, ..Default::default() })),
    );
    fonts.families.insert(icon_fill(), vec!["phosphor-fill".into(), "phosphor".into(), "inter".into()]);
    // The bundled fonts are subset so they never overlap: Inter has no private-use glyphs and
    // Phosphor has only private-use glyphs. egui's own fonts stay last as fallback for other scripts.
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["inter".to_string(), "phosphor".to_string(), "hebrew".to_string()];
    let mut bold = vec!["inter-semibold".to_string(), "phosphor".to_string(), "hebrew-bold".to_string(), "inter".to_string()];
    for (name, data) in system_fonts() {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        prop.push(name.clone());
        bold.push(name);
    }
    prop.extend(defaults.iter().cloned());
    bold.extend(defaults);
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(semibold(), bold);
    fonts.families.entry(FontFamily::Monospace).or_default().insert(1, "phosphor".into());
    ctx.set_fonts(fonts);
}

/// The bundled fonts draw this character: Inter (Latin, Greek, Cyrillic, symbols), Noto Sans
/// Hebrew, Phosphor icons (private use).
fn bundled(c: char) -> bool {
    matches!(c,
        '\0'..='\u{052F}'
        | '\u{0590}'..='\u{05FF}'
        | '\u{1E00}'..='\u{2BFF}'
        | '\u{E000}'..='\u{F8FF}'
        | '\u{FB1D}'..='\u{FB4F}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}'..='\u{FFFF}')
}

/// System fonts for the languages in `locales/` whose script the bundled fonts don't have
/// (Arabic, Persian, Chinese, Japanese, Thai, Hindi, …), so a new catalog needs no new font.
fn system_fonts() -> Vec<(String, FontData)> {
    let mut out: Vec<(String, FontData)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for lang in crate::i18n::LANGUAGES.iter() {
        let Some(letter) = lang.catalog_text().chars().find(|c| !bundled(*c)) else { continue };
        let Some((path, index)) = system_font_file(lang.code, letter) else { continue };
        if !seen.insert((path.clone(), index)) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            let mut data = FontData::from_owned(bytes);
            data.index = index;
            out.push((format!("system-{}", lang.code), data));
        }
    }
    out
}

/// A font file (and face index) for the language, from fontconfig.
#[cfg(not(windows))]
fn system_font_file(code: &str, _letter: char) -> Option<(std::path::PathBuf, u32)> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}", &format!(":lang={}", code.replace('_', "-").to_lowercase())]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let (file, index) = text.split_once('\n')?;
    Some((file.into(), index.trim().parse().unwrap_or(0)))
}

/// A font file (and face index) for the language, from the fonts Windows ships.
#[cfg(windows)]
fn system_font_file(code: &str, letter: char) -> Option<(std::path::PathBuf, u32)> {
    let base = code.split(['_', '-']).next().unwrap_or(code);
    let candidates: &[&str] = match (base, letter as u32) {
        ("zh", _) if code.to_ascii_uppercase().ends_with("TW") || code.to_ascii_uppercase().ends_with("HK") => &["msjh.ttc"],
        ("zh", _) => &["msyh.ttc", "simsun.ttc"],
        ("ja", _) => &["yugothr.ttc", "meiryo.ttc", "msgothic.ttc"],
        ("ko", _) => &["malgun.ttf", "gulim.ttc"],
        (_, 0x0E00..=0x0EFF) => &["leelawui.ttf", "tahoma.ttf"],
        (_, 0x0900..=0x0DFF) => &["nirmala.ttf"],
        (_, 0x1200..=0x139F) => &["ebrima.ttf"],
        (_, 0x1000..=0x109F) => &["mmrtext.ttf"],
        (_, 0x1780..=0x17FF) => &["leeluisl.ttf", "leelawui.ttf"],
        // (File names in lower case: Windows matches them in any case.)
        _ => &["segoeui.ttf", "tahoma.ttf", "arial.ttf"],
    };
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
    candidates.iter().map(|f| dir.join(f)).find(|p| p.exists()).map(|p| (p, 0))
}

// ---------------------------------------------------------------- widgets

pub fn strong(text: impl Into<String>) -> RichText {
    rich(text).family(semibold())
}

pub fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    rich(text).color(Palette::get(ui.ctx()).muted)
}

fn with_icon(icon: &str, text: &str) -> String {
    if text.is_empty() {
        icon.to_string()
    } else if icon.is_empty() {
        text.to_string()
    } else {
        format!("{icon}  {text}")
    }
}

// ---------------------------------------------------------------- reading direction
//
// Right-to-left languages (Hebrew) mirror the whole interface: rows run from the right, the
// "far" side of a row is the left, columns hug the right edge, the sidebar sits on the right
// and arrows point the other way. Pages use these instead of egui's left-to-right ones.

/// The interface reads right to left.
pub fn rtl() -> bool {
    crate::i18n::is_rtl()
}

/// A row in reading order (egui's `horizontal`, mirrored for right-to-left).
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    row_with(ui, egui::Align::Center, false, add)
}

/// A row whose items line up at the top.
pub fn row_top<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    row_with(ui, egui::Align::Min, false, add)
}

/// A row that wraps onto more lines when it runs out of room.
pub fn row_wrapped<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    row_with(ui, egui::Align::Center, true, add)
}

fn row_with<R>(ui: &mut Ui, align: egui::Align, wrap: bool, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    if !rtl() {
        return if wrap {
            ui.horizontal_wrapped(add)
        } else if align == egui::Align::Min {
            ui.horizontal_top(add)
        } else {
            ui.horizontal(add)
        };
    }
    let size = Vec2::new(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(align).with_main_wrap(wrap), add)
}

/// A column whose items line up at the start of the reading direction.
pub fn column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    if rtl() {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), add)
    } else {
        ui.vertical(add)
    }
}

/// Layout for the far end of a row (the right for left-to-right, the left for right-to-left).
pub fn far_layout(align: egui::Align) -> egui::Layout {
    if rtl() {
        egui::Layout::left_to_right(align)
    } else {
        egui::Layout::right_to_left(align)
    }
}

/// Layout that starts at the near end of a row (where reading starts).
pub fn near_layout(align: egui::Align) -> egui::Layout {
    if rtl() {
        egui::Layout::right_to_left(align)
    } else {
        egui::Layout::left_to_right(align)
    }
}

/// Arrow pointing back (left for left-to-right, right for right-to-left).
pub fn icon_back() -> &'static str {
    if rtl() {
        icon::ARROW_RIGHT
    } else {
        icon::ARROW_LEFT
    }
}

/// Arrow pointing forward.
pub fn icon_forward() -> &'static str {
    if rtl() {
        icon::ARROW_LEFT
    } else {
        icon::ARROW_RIGHT
    }
}

/// Mirrors x positions inside `rect` for right-to-left painting: `x(d)` is `d` from the start
/// edge, `align` flips left/right anchors.
#[derive(Clone, Copy)]
pub struct Mirror {
    rect: egui::Rect,
    on: bool,
}

impl Mirror {
    pub fn new(rect: egui::Rect) -> Self {
        Self { rect, on: rtl() }
    }

    /// The x coordinate `d` from the start edge.
    pub fn x(&self, d: f32) -> f32 {
        if self.on {
            self.rect.right() - d
        } else {
            self.rect.left() + d
        }
    }

    /// A point `d` from the start edge at height `y`.
    pub fn pos(&self, d: f32, y: f32) -> egui::Pos2 {
        egui::pos2(self.x(d), y)
    }

    /// A rect of `size` whose start edge is `d` from the start edge, top at `y`.
    pub fn rect(&self, d: f32, y: f32, size: Vec2) -> egui::Rect {
        let x = if self.on { self.rect.right() - d - size.x } else { self.rect.left() + d };
        egui::Rect::from_min_size(egui::pos2(x, y), size)
    }

    /// Text anchor at the start side (LEFT_* for left-to-right).
    pub fn start(&self, v: egui::Align) -> egui::Align2 {
        egui::Align2([if self.on { egui::Align::Max } else { egui::Align::Min }, v])
    }

    /// Text anchor at the end side.
    pub fn end(&self, v: egui::Align) -> egui::Align2 {
        egui::Align2([if self.on { egui::Align::Min } else { egui::Align::Max }, v])
    }

    /// Where a galley of width `w` starts so that it begins `d` from the start edge.
    pub fn galley_x(&self, d: f32, w: f32) -> f32 {
        if self.on {
            self.rect.right() - d - w
        } else {
            self.rect.left() + d
        }
    }
}

/// Items in reading order: as given, or reversed for right-to-left (table columns and cells).
pub fn ordered<T>(mut items: Vec<T>) -> Vec<T> {
    if rtl() {
        items.reverse();
    }
    items
}

/// Table columns given in left-to-right order, added in reading order.
pub trait ColumnsInOrder {
    fn columns_in_order(self, columns: Vec<egui_extras::Column>) -> Self;
}

impl ColumnsInOrder for egui_extras::TableBuilder<'_> {
    fn columns_in_order(mut self, columns: Vec<egui_extras::Column>) -> Self {
        for c in ordered(columns) {
            self = self.column(c);
        }
        self
    }
}

/// A table row's cells, given in left-to-right order, added in reading order.
pub fn cells_in_order<'a>(row: &mut egui_extras::TableRow<'_, '_>, cells: Vec<Box<dyn FnOnce(&mut Ui) + 'a>>) {
    for c in ordered(cells) {
        row.col(c);
    }
}

// ---------------------------------------------------------------- right-to-left text
//
// egui places text left to right and has no bidirectional layout (it only shapes the letters of
// each Hebrew word right to left). So text with Hebrew in it is handed to egui in display order
// (`i18n::visual`), exactly once, where it becomes drawable: `rich` instead of `RichText::new`,
// `vis` for plain-text widget arguments, `vstr` for painter text, and `Label` instead of
// `egui::Label` (it also wraps right-to-left text from the right and truncates it on the left).

/// Text in display order, for the painter.
pub fn vstr(text: impl ToString) -> String {
    let text = text.to_string();
    match crate::i18n::visual(&text) {
        std::borrow::Cow::Borrowed(_) => text,
        std::borrow::Cow::Owned(v) => v,
    }
}

/// `RichText::new` with the text in display order.
pub fn rich(text: impl Into<String>) -> RichText {
    RichText::new(vstr(text.into()))
}

/// Widget text in display order. Rich text already is (it is made with `rich`).
pub fn vis(text: impl Into<egui::WidgetText>) -> egui::WidgetText {
    match text.into() {
        egui::WidgetText::Text(s) => egui::WidgetText::Text(vstr(s)),
        t => t,
    }
}

/// Display-order right-to-left text broken into lines no wider than `width`: a line reads from
/// the right, so each one takes the words at the right end of what is left. `None` when every
/// line fits already.
fn rtl_lines(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Option<String> {
    if text.split('\n').all(|l| measure(l) <= width) {
        return None;
    }
    let space = measure(" ");
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let words: Vec<&str> = para.split(' ').collect();
        let widths: Vec<f32> = words.iter().map(|w| measure(w)).collect();
        let mut end = words.len();
        while end > 0 {
            let mut start = end - 1;
            let mut w = widths[start];
            while start > 0 && w + space + widths[start - 1] <= width {
                start -= 1;
                w += space + widths[start];
            }
            lines.push(words[start..end].join(" "));
            end = start;
        }
    }
    Some(lines.join("\n"))
}

/// Display-order right-to-left text cut on the left, where it ends, to fit `width`. `None`
/// when it fits, or when not even its first word does.
fn rtl_truncated(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Option<String> {
    let text = text.split('\n').next().unwrap_or_default();
    if measure(text) <= width {
        return None;
    }
    let words: Vec<&str> = text.split(' ').collect();
    (1..words.len()).map(|i| format!("…{}", words[i..].join(" "))).find(|t| measure(t) <= width)
}

/// `egui::Label` for text that may read right to left (see above).
pub struct Label {
    text: egui::WidgetText,
    wrap_mode: Option<egui::TextWrapMode>,
    sense: Option<egui::Sense>,
}

impl Label {
    pub fn new(text: impl Into<egui::WidgetText>) -> Self {
        Self { text: vis(text), wrap_mode: None, sense: None }
    }

    pub fn wrap(mut self) -> Self {
        self.wrap_mode = Some(egui::TextWrapMode::Wrap);
        self
    }

    pub fn truncate(mut self) -> Self {
        self.wrap_mode = Some(egui::TextWrapMode::Truncate);
        self
    }

    pub fn extend(mut self) -> Self {
        self.wrap_mode = Some(egui::TextWrapMode::Extend);
        self
    }

    pub fn sense(mut self, sense: egui::Sense) -> Self {
        self.sense = Some(sense);
        self
    }

    fn egui(&self, text: impl Into<egui::WidgetText>) -> egui::Label {
        let label = egui::Label::new(text);
        let label = match self.wrap_mode {
            Some(mode) => label.wrap_mode(mode),
            None => label,
        };
        match self.sense {
            Some(sense) => label.sense(sense),
            None => label,
        }
    }

    /// The text laid out by hand when it reads right to left and does not fit.
    fn fitted(&self, ui: &Ui) -> Option<egui::text::LayoutJob> {
        let mode = self.wrap_mode.unwrap_or_else(|| ui.wrap_mode());
        if !rtl() || mode == egui::TextWrapMode::Extend || !crate::i18n::has_rtl(self.text.text()) {
            return None;
        }
        let mut job = Arc::unwrap_or_clone(self.text.clone().into_layout_job(ui.style(), egui::FontSelection::Default, ui.text_valign()));
        let [section] = &mut job.sections[..] else { return None };
        let font = section.format.font_id.clone();
        let painter = ui.painter();
        let measure = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::PLACEHOLDER).size().x;
        let width = ui.available_width();
        let text = if mode == egui::TextWrapMode::Wrap { rtl_lines(&job.text, width, measure) } else { rtl_truncated(&job.text, width, measure) }?;
        section.byte_range = egui::text::ByteIndex(0)..egui::text::ByteIndex(text.len());
        job.text = text;
        Some(job)
    }
}

impl egui::Widget for Label {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        match self.fitted(ui) {
            Some(job) => {
                let truncated = self.wrap_mode.unwrap_or_else(|| ui.wrap_mode()) == egui::TextWrapMode::Truncate;
                let r = ui.add(self.egui(job).extend().halign(egui::Align::RIGHT));
                if truncated {
                    r.on_hover_text(self.text)
                } else {
                    r
                }
            }
            None => ui.add(self.egui(self.text.clone())),
        }
    }
}

/// Text laid out from the reading-direction edge: paint it at `Mirror::x(d)` (right-aligned
/// for right-to-left). `wrap` is the wrap width (`f32::INFINITY` for one line).
pub fn text_galley(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32, wrap: f32) -> Arc<egui::Galley> {
    let mut text = vstr(text.into());
    if rtl() && wrap.is_finite() && crate::i18n::has_rtl(&text) {
        let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x;
        text = rtl_lines(&text, wrap, measure).unwrap_or(text);
    }
    let mut job = egui::text::LayoutJob::simple(text, font, color, wrap);
    if rtl() {
        job.halign = egui::Align::RIGHT;
    }
    ui.painter().layout_job(job)
}

/// One line of painter text no wider than `width`, cut short with "…" where it ends (on the
/// left for right-to-left text). Also says whether it was cut.
pub fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> (Arc<egui::Galley>, bool) {
    let text = vstr(text);
    if rtl() && crate::i18n::has_rtl(&text) {
        let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x;
        return match rtl_truncated(&text, width, measure) {
            Some(cut) => (ui.painter().layout_no_wrap(cut, font, color), true),
            None => (ui.painter().layout_no_wrap(text, font, color), false),
        };
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    (galley, cut)
}

/// Height of buttons, inputs, dropdowns and search fields, so toolbars line up.
pub const CONTROL_H: f32 = 36.0;

/// Seconds a hover or press state takes to fade in or out.
pub const FADE: f32 = 0.12;

/// Seconds a selection indicator takes to slide to its new place.
pub const SLIDE: f32 = 0.22;

/// A value that glides (cubic ease-out) to `target` whenever the target changes; one per `id`.
/// The first call returns `target` as is.
pub fn ease_to(ctx: &egui::Context, id: egui::Id, target: f32, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let mut st = ctx.data(|d| d.get_temp::<(f32, f32, f64)>(id)).unwrap_or((target, target, now));
    let at = |st: (f32, f32, f64)| {
        let t = (((now - st.2) as f32) / secs).clamp(0.0, 1.0);
        (egui::lerp(st.0..=st.1, egui::emath::easing::cubic_out(t)), t)
    };
    if st.1 != target {
        st = (at(st).0, target, now);
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
    let (v, t) = at(st);
    if t < 1.0 {
        ctx.request_repaint();
    }
    v
}

/// Hover amount (0..1, faded) and pressed state of the widget that will get `id`, from the
/// previous frame, so a widget can be painted with its animated colours before it is added.
fn interaction(ui: &Ui, id: egui::Id) -> (f32, bool) {
    let r = if ui.is_enabled() { ui.ctx().read_response(id) } else { None };
    let hovered = r.as_ref().is_some_and(|r| r.hovered());
    let pressed = hovered && r.is_some_and(|r| r.is_pointer_button_down_on());
    (ui.ctx().animate_bool_with_time(id.with("hover"), hovered, FADE), pressed)
}

fn button_text(icon: &str, text: &str) -> String {
    with_icon(icon, tr(text))
}

/// Filled accent button for the main action of a page. Darkens and glows on hover.
pub fn primary(ui: &mut Ui, icon: &str, text: &str) -> egui::Response {
    let p = Palette::get(ui.ctx());
    let id = ui.next_auto_id();
    let (t, pressed) = interaction(ui, id);
    let enabled = ui.is_enabled();
    let hovered = if p.dark { p.accent.lerp_to_gamma(Color32::WHITE, 0.12) } else { p.accent.lerp_to_gamma(Color32::BLACK, 0.12) };
    let mut fill = p.accent.lerp_to_gamma(hovered, t);
    if pressed {
        fill = fill.lerp_to_gamma(Color32::BLACK, 0.1);
    }
    if !enabled {
        fill = p.accent.gamma_multiply(0.45);
    }
    let glow = ui.painter().add(egui::Shape::Noop);
    let r = ui.add(
        egui::Button::new(vis(rich(button_text(icon, text)).color(p.on_accent).family(semibold())))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(8)
            .min_size(Vec2::new(0.0, CONTROL_H)),
    );
    if enabled {
        let a = (if p.dark { 40.0 } else { 34.0 } + 50.0 * t) as u8;
        let shadow = egui::Shadow { offset: [0, 2 + (2.0 * t) as i8], blur: 6 + (8.0 * t) as u8, spread: 0, color: soft(p.accent, a) };
        ui.painter().set(glow, shadow.as_shape(r.rect, 8));
    }
    r
}

/// Outlined neutral button.
pub fn secondary(ui: &mut Ui, icon: &str, text: &str) -> egui::Response {
    let p = Palette::get(ui.ctx());
    let (t, pressed) = interaction(ui, ui.next_auto_id());
    let fill = if pressed { p.hover.lerp_to_gamma(p.border, 0.5) } else { p.surface.lerp_to_gamma(p.hover, t) };
    let stroke = p.border.lerp_to_gamma(p.faint, t * 0.6);
    ui.add(egui::Button::new(vis(button_text(icon, text))).fill(fill).stroke(Stroke::new(1.0, stroke)).corner_radius(8).min_size(Vec2::new(0.0, CONTROL_H)))
}

/// Destructive outlined button; tints red on hover.
pub fn danger(ui: &mut Ui, icon: &str, text: &str) -> egui::Response {
    let p = Palette::get(ui.ctx());
    let (t, pressed) = interaction(ui, ui.next_auto_id());
    let fill = p.surface.lerp_to_gamma(p.danger, if pressed { 0.16 } else { 0.08 * t });
    ui.add(
        egui::Button::new(vis(rich(button_text(icon, text)).color(p.danger)))
            .fill(fill)
            .stroke(Stroke::new(1.0, p.border.lerp_to_gamma(p.danger, t * 0.55)))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, CONTROL_H)),
    )
}

/// Background of a borderless control at hover amount `t`.
fn hover_fill(p: &Palette, t: f32, pressed: bool) -> Color32 {
    if pressed {
        p.hover.lerp_to_gamma(p.border, 0.5)
    } else {
        p.hover.gamma_multiply(t)
    }
}

/// Borderless button for toolbars and lists.
pub fn ghost(ui: &mut Ui, icon: &str, text: &str) -> egui::Response {
    let p = Palette::get(ui.ctx());
    let (t, pressed) = interaction(ui, ui.next_auto_id());
    ui.add(egui::Button::new(vis(button_text(icon, text))).fill(hover_fill(&p, t, pressed)).stroke(Stroke::NONE).corner_radius(8).min_size(Vec2::new(30.0, CONTROL_H)))
}

/// Icon-only borderless button with a tooltip (the tooltip text is also its accessible name).
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    let tip = tr(tip);
    let p = Palette::get(ui.ctx());
    let (t, pressed) = interaction(ui, ui.next_auto_id());
    let color = p.muted.lerp_to_gamma(p.text, t);
    let r =
        ui.add(egui::Button::new(vis(rich(icon).size(17.0).color(color))).fill(hover_fill(&p, t, pressed)).stroke(Stroke::NONE).corner_radius(8).min_size(Vec2::new(32.0, 32.0)));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    r.on_hover_text(vis(tip))
}

/// Background of a clickable tile (a card-sized button): lifts a little with a soft shadow on
/// hover, accent border when hovered or selected. Returns the hover amount.
pub fn tile_frame(ui: &mut Ui, rect: egui::Rect, resp: &egui::Response, selected: bool) -> f32 {
    let p = Palette::get(ui.ctx());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("tile"), resp.hovered() && ui.is_enabled(), FADE * 1.4);
    if t > 0.0 {
        let a = ((if p.dark { 70.0 } else { 22.0 }) * t) as u8;
        ui.painter().add(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(a) }.as_shape(rect, 12));
    }
    let fill = if selected { p.surface.lerp_to_gamma(p.accent, if p.dark { 0.16 } else { 0.07 }) } else { p.surface };
    let border = if selected { p.accent } else { p.border.lerp_to_gamma(p.accent, t * 0.75) };
    ui.painter().rect(rect, 12.0, fill, Stroke::new(if selected { 1.5 } else { 1.0 + 0.5 * t }, border), egui::StrokeKind::Inside);
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    t
}

/// Indeterminate progress: an accent segment sweeping along a thin track.
pub fn busy_bar(ui: &mut Ui, width: f32) {
    let p = Palette::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, p.surface_alt.lerp_to_gamma(p.border, 0.5));
    let phase = (ui.input(|i| i.time) / 1.4).fract() as f32;
    let seg = rect.width() * 0.3;
    let x = rect.left() - seg + (rect.width() + seg) * egui::emath::easing::cubic_in_out(phase);
    let bar = egui::Rect::from_min_max(egui::pos2(x.max(rect.left()), rect.top()), egui::pos2((x + seg).min(rect.right()), rect.bottom()));
    if bar.width() > 0.5 {
        ui.painter().rect_filled(bar, 2.0, p.accent);
    }
    ui.ctx().request_repaint();
}

/// A soft diagonal gradient of the accent, as a 2×2 texture stretched with linear filtering.
fn accent_gradient(ctx: &egui::Context, p: &Palette) -> egui::TextureHandle {
    let key = egui::Id::unique(("accent-gradient", p.accent.to_array()));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return t;
    }
    let light = p.accent.lerp_to_gamma(Color32::WHITE, 0.28);
    let deep = p.accent.lerp_to_gamma(Color32::from_rgb(76, 29, 149), 0.35);
    let img = egui::ColorImage::new([2, 2], vec![light, p.accent, p.accent, deep]);
    let tex = ctx.load_texture("accent-gradient", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

/// A figure with a label under it, in a small card (summary rows).
pub fn stat_tile(ui: &mut Ui, width: f32, glyph: &str, value: &str, label: &str, color: Color32) {
    let label = tr(label);
    let p = Palette::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 74.0), egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label}: {value}")));
    ui.painter().rect(rect, 12.0, p.surface, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let m = Mirror::new(rect);
    let tile = m.rect(14.0, rect.top() + 17.0, Vec2::splat(40.0));
    ui.painter().rect_filled(tile, 11.0, soft(color, if p.dark { 44 } else { 26 }));
    ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, vstr(glyph), FontId::new(19.0, icon_fill()), color);
    let d = 14.0 + 40.0 + 12.0;
    let max_w = rect.width() - d - 10.0;
    let vg = ui.painter().layout(vstr(value.to_string()), FontId::new(20.0, semibold()), p.text, max_w);
    let (vg_h, vg_w) = (vg.size().y.min(26.0), vg.size().x);
    ui.painter().with_clip_rect(rect).galley(egui::pos2(m.galley_x(d, vg_w), rect.top() + 14.0), vg, p.text);
    ui.painter().with_clip_rect(rect).text(m.pos(d, rect.top() + 18.0 + vg_h), m.start(egui::Align::Min), vstr(label), FontId::proportional(12.0), p.muted);
}

/// Small uppercase label above a group of cards.
pub fn eyebrow(ui: &mut Ui, text: &str) {
    let text = tr(text);
    let p = Palette::get(ui.ctx());
    ui.add(Label::new(rich(text.to_uppercase()).size(11.5).family(semibold()).color(p.faint)));
    ui.add_space(2.0);
}

/// A featured card: the normal card with a faint accent gradient washed over it.
pub fn hero<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::get(ui.ctx());
    let slot = ui.painter().add(egui::Shape::Noop);
    let r = egui::Frame::new().corner_radius(16).inner_margin(Margin::same(22)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    let rect = r.response.rect;
    let tex = accent_gradient(ui.ctx(), &p);
    let uv = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
    let wash = Color32::from_white_alpha(if p.dark { 34 } else { 24 });
    ui.painter().set(
        slot,
        egui::Shape::Vec(vec![
            egui::Shape::Rect(egui::epaint::RectShape::new(rect, 16.0, p.surface, Stroke::new(1.0, p.border), egui::StrokeKind::Inside)),
            egui::Shape::Rect(egui::epaint::RectShape::filled(rect.shrink(1.0), 15.0, wash).with_texture(tex.id(), uv)),
        ]),
    );
    r.inner
}

/// Circular progress around a glyph: `frac` 0..1, or `None` for a spinning arc. Use a new
/// `id` for each run, so the ring starts from empty.
pub fn progress_ring(ui: &mut Ui, id: egui::Id, size: f32, frac: Option<f32>, glyph: &str) {
    let p = Palette::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let c = rect.center();
    let radius = size / 2.0 - 5.0;
    let width = (size * 0.075).max(4.0);
    ui.painter().circle_stroke(c, radius, Stroke::new(width, p.surface_alt.lerp_to_gamma(p.border, 0.6)));
    let time = ui.input(|i| i.time) as f32;
    let (start, sweep) = match frac {
        Some(f) => {
            let f = ease_to(ui.ctx(), id.with("ring"), f.clamp(0.0, 1.0), 0.45);
            (-std::f32::consts::FRAC_PI_2, f * std::f32::consts::TAU)
        }
        None => (time * 4.0, 1.6),
    };
    if sweep > 0.01 {
        let n = ((sweep / std::f32::consts::TAU) * 96.0).ceil().max(2.0) as usize;
        let pts: Vec<egui::Pos2> = (0..=n)
            .map(|i| {
                let a = start + sweep * i as f32 / n as f32;
                c + Vec2::new(a.cos(), a.sin()) * radius
            })
            .collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(width, p.accent)));
    }
    // A soft breathing halo behind the glyph.
    let breathe = 0.5 + 0.5 * (time * 2.4).sin();
    ui.painter().circle_filled(c, radius * (0.56 + 0.04 * breathe), soft(p.accent, if p.dark { 40 } else { 26 }));
    ui.painter().text(c, egui::Align2::CENTER_CENTER, vstr(glyph), FontId::new(size * 0.3, icon_fill()), p.accent);
    ui.ctx().request_repaint();
}

/// Determinate progress bar; the fill glides to `frac`. Use a new `id` for each run.
pub fn progress_bar(ui: &mut Ui, id: egui::Id, width: f32, frac: f32) {
    let p = Palette::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, p.surface_alt.lerp_to_gamma(p.border, 0.5));
    let f = ease_to(ui.ctx(), id.with("bar"), frac.clamp(0.0, 1.0), 0.45);
    if f > 0.0 {
        ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height())), 3.0, p.accent);
    }
}

/// The app mark: a gradient rounded square with a white filled glyph.
pub fn logo(ui: &mut Ui, glyph: &str, size: f32) -> egui::Response {
    let p = Palette::get(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let radius = (size * 0.3) as u8;
    ui.painter().add(egui::Shadow { offset: [0, 3], blur: 10, spread: 0, color: soft(p.accent, if p.dark { 70 } else { 60 }) }.as_shape(rect, radius));
    let tex = accent_gradient(ui.ctx(), &p);
    let uv = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
    ui.painter().add(egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex.id(), uv));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, vstr(glyph), FontId::new(size * 0.52, icon_fill()), Color32::WHITE);
    r
}

/// CleanCore's logo: its mark on the accent-colored tile.
pub fn brand(ui: &mut Ui, size: f32) -> egui::Response {
    let r = logo(ui, "", size);
    let px = (size * ui.ctx().pixels_per_point()).round() as u32;
    let id = egui::Id::unique(("logo-mark", px));
    let tex = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(id)).or_else(|| {
        let tex = ui.ctx().load_texture("logo-mark", super::appicons::logo_mark(px)?, egui::TextureOptions::LINEAR);
        ui.ctx().data_mut(|d| d.insert_temp(id, tex.clone()));
        Some(tex)
    });
    if let Some(tex) = tex {
        ui.painter().image(tex.id(), r.rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
    r
}

/// White rounded card with a hairline border.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::get(ui.ctx());
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(12)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Small rounded label, e.g. a count or a status.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let text = tr(text);
    // Painted at its natural size so it never stretches inside table cells.
    let galley = ui.painter().layout_no_wrap(vstr(text.to_string()), FontId::new(12.0, semibold()), color);
    let size = galley.size() + Vec2::new(16.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    let fill = soft(color, if Palette::get(ui.ctx()).dark { 48 } else { 30 });
    ui.painter().rect_filled(rect, 10.0, fill);
    ui.painter().galley(rect.min + Vec2::new(8.0, 2.0), galley, color);
}

/// Rounded square with an icon on a soft accent background.
pub fn icon_tile(ui: &mut Ui, icon: &str, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    ui.painter().rect_filled(rect, size * 0.28, soft(color, if Palette::get(ui.ctx()).dark { 52 } else { 28 }));
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, vstr(icon), FontId::new(size * 0.5, icon_fill()), color);
}

/// Page title row: title, subtitle and right-aligned actions.
///
/// Actions are laid out right to left: add the page's main (primary) action first so it sits
/// at the far right, then secondary actions, then icon buttons.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Ui)) {
    header_row(ui, title, subtitle, |_| {}, actions);
}

/// Title row shared by the page headers. The title column is kept clear of the actions (their
/// width from the previous frame), so a long subtitle is cut short instead of running under them.
fn header_row(ui: &mut Ui, title: &str, subtitle: &str, lead: impl FnOnce(&mut Ui), actions: impl FnOnce(&mut Ui)) {
    let title = tr(title);
    let subtitle = tr(subtitle);
    let key = egui::Id::unique(("header-actions", title));
    let actions_w = ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
    row(ui, |ui| {
        ui.set_min_height(52.0);
        lead(ui);
        let title_w = (ui.available_width() - actions_w - 16.0).max(120.0);
        column(ui, |ui| {
            ui.set_max_width(title_w);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(Label::new(rich(title).size(24.0).family(semibold())).truncate());
            if !subtitle.is_empty() {
                ui.add(Label::new(muted(ui, subtitle)).truncate());
            }
        });
        let r = ui.with_layout(far_layout(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            actions(ui);
            ui.min_rect().width()
        });
        let w = r.inner.max(0.0);
        if (w - actions_w).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(key, w));
            ui.ctx().request_repaint();
        }
    });
    ui.add_space(12.0);
}

/// Header of a sub-screen: a back arrow left of the title. Returns true when it was clicked.
pub fn page_header_back(ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Ui)) -> bool {
    let mut back = false;
    header_row(
        ui,
        title,
        subtitle,
        |ui| {
            let r = ui.add(egui::Button::new(vis(rich(icon_back()).size(17.0))).frame_when_inactive(false).corner_radius(8).min_size(Vec2::splat(CONTROL_H)));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back"));
            back = r.on_hover_text(vis("Back")).clicked();
            ui.add_space(2.0);
        },
        actions,
    );
    back
}

/// Underlined tabs that switch a page's views: (value, icon, label, count). Returns true when
/// the user picked another tab.
pub fn tabs<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str, &str, Option<usize>)]) -> bool {
    let translated: Vec<(T, &str, &str, Option<usize>)> = items.iter().map(|(v, ic, l, c)| (*v, *ic, tr(l), *c)).collect();
    let items = &translated[..];
    let p = Palette::get(ui.ctx());
    let mut changed = false;
    let mut underline = None;
    // One slide state per tab set (pages share the central panel's id).
    let anim_id = egui::Id::unique(("tabs", items.iter().map(|i| i.2).collect::<Vec<_>>()));
    // Fit the row: drop icons first, then counts, when the page is narrow.
    let measure = |icons: bool, counts: bool, pad: f32| -> f32 {
        items
            .iter()
            .map(|(_, ic, label, count)| {
                let t = if icons { with_icon(ic, label) } else { label.to_string() };
                let w = ui.painter().layout_no_wrap(vstr(t), FontId::new(14.0, semibold()), p.text).size().x;
                let c = if counts { count.map_or(0.0, |c| c.to_string().len() as f32 * 7.0 + 20.0) } else { 0.0 };
                w + c + pad + 4.0
            })
            .sum()
    };
    let avail = ui.available_width();
    // Fit the row: drop icons, then counts, then tighten the padding; scroll as a last resort.
    let (show_icons, show_counts, pad) = if measure(true, true, 22.0) <= avail {
        (true, true, 22.0)
    } else if measure(false, true, 22.0) <= avail {
        (false, true, 22.0)
    } else if measure(false, false, 22.0) <= avail {
        (false, false, 22.0)
    } else {
        (false, false, 12.0)
    };
    let scroll = measure(false, false, 12.0) > avail;
    let mut row_ui = |ui: &mut Ui| {
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (value, ic, label, count) in items {
                let sel = *current == *value;
                let count = count.filter(|_| show_counts);
                let text = if show_icons { with_icon(ic, label) } else { label.to_string() };
                let font = FontId::new(14.0, if sel { semibold() } else { FontFamily::Proportional });
                let count_galley = count.map(|c| ui.painter().layout_no_wrap(vstr(c.to_string()), FontId::new(11.5, semibold()), if sel { p.accent } else { p.muted }));
                let count_w = count_galley.as_ref().map_or(0.0, |g| g.size().x + 14.0 + 6.0);
                let probe = ui.painter().layout_no_wrap(vstr(text.clone()), font.clone(), p.text);
                let (rect, r) = ui.allocate_exact_size(Vec2::new(probe.size().x + count_w + pad, 40.0), egui::Sense::click());
                let name = with_icon(ic, label);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, sel, &name));
                let t = ui.ctx().animate_bool_with_time(r.id.with("hover"), r.hovered() && !sel, FADE);
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if t > 0.0 {
                    ui.painter().rect_filled(rect.shrink2(Vec2::new(2.0, 5.0)), 8.0, p.hover.gamma_multiply(t));
                }
                let color = if sel { p.accent } else { p.muted.lerp_to_gamma(p.text, t) };
                let galley = ui.painter().layout_no_wrap(vstr(text.clone()), font, color);
                let text_pos = egui::pos2(rect.left() + pad / 2.0 + 1.0, rect.center().y - galley.size().y / 2.0);
                let text_w = galley.size().x;
                ui.painter().galley(text_pos, galley, color);
                if let Some(g) = count_galley {
                    let pill = egui::Rect::from_min_size(egui::pos2(text_pos.x + text_w + 6.0, rect.center().y - 9.0), Vec2::new(g.size().x + 14.0, 18.0));
                    ui.painter().rect_filled(pill, 9.0, if sel { p.accent_soft } else { p.surface_alt });
                    ui.painter().galley(pill.center() - g.size() / 2.0, g, if sel { p.accent } else { p.muted });
                }
                if sel {
                    underline = Some(egui::Rect::from_min_max(egui::pos2(rect.left() + 8.0, rect.bottom() - 2.0), egui::pos2(rect.right() - 8.0, rect.bottom())));
                }
                if r.clicked() && !sel {
                    *current = *value;
                    changed = true;
                }
            }
        })
        .response
    };
    let row = if scroll {
        egui::ScrollArea::horizontal().id_salt(anim_id).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, row_ui).inner
    } else {
        row_ui(ui)
    };
    // A hairline under the whole row, and the selected tab's accent underline on top of it.
    ui.painter().hline(ui.max_rect().x_range(), row.rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    if let Some(u) = underline {
        let ctx = ui.ctx().clone();
        let l = ease_to(&ctx, anim_id.with("l"), u.left(), SLIDE);
        let r = ease_to(&ctx, anim_id.with("r"), u.right(), SLIDE);
        let u = egui::Rect::from_x_y_ranges(l..=r.max(l + 1.0), u.y_range());
        ui.painter().rect_filled(u, 1.0, p.accent);
    }
    ui.add_space(12.0);
    changed
}

/// A compact dropdown that matches buttons and inputs.
pub fn dropdown<R>(ui: &mut Ui, id: &str, selected: &str, width: f32, contents: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let p = Palette::get(ui.ctx());
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = CONTROL_H;
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = p.surface;
        w.inactive.bg_stroke = Stroke::new(1.0, p.border);
        egui::ComboBox::from_id_salt(id).selected_text(vis(selected)).width(width).height(420.0).show_ui(ui, contents).inner
    })
    .inner
}

/// Centered icon, title and text for empty pages.
pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, text: &str) {
    let title = tr(title);
    let text = tr(text);
    let p = Palette::get(ui.ctx());
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(88.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 44.0, soft(p.accent, if p.dark { 22 } else { 14 }));
        ui.painter().circle_filled(rect.center(), 32.0, soft(p.accent, if p.dark { 44 } else { 28 }));
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, vstr(icon), FontId::new(30.0, icon_fill()), p.accent);
        ui.add_space(12.0);
        ui.add(Label::new(strong(title).size(18.0)));
        ui.add(Label::new(muted(ui, text)));
    });
}

/// Label above a group of settings.
pub fn section_title(ui: &mut Ui, text: &str) {
    let text = tr(text);
    ui.add(Label::new(strong(text).size(15.0)));
    ui.add_space(2.0);
}

/// Side navigation entry. The selected entry's background is painted by `nav_indicator`, so it
/// can slide between entries.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let label = tr(label);
    let p = Palette::get(ui.ctx());
    // Shorter entries when the window is short, so the whole list fits without scrolling.
    let screen_h = ui.ctx().content_rect().height();
    let h = if screen_h < 640.0 {
        32.0
    } else if screen_h < 700.0 {
        34.0
    } else {
        38.0
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() && !selected, FADE);
    if t > 0.0 {
        ui.painter().rect_filled(rect, 9.0, p.hover.gamma_multiply(t));
    }
    let s = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, SLIDE);
    let fg = p.text.lerp_to_gamma(p.accent, s);
    let icon_color = p.muted.lerp_to_gamma(p.text, t).lerp_to_gamma(p.accent, s);
    // Icons nudge right a touch on hover, like a cursor following them.
    let m = Mirror::new(rect);
    let cy = rect.center().y;
    let family = if selected { icon_fill() } else { FontFamily::Proportional };
    ui.painter().text(m.pos(14.0 + 2.0 * t, cy), m.start(egui::Align::Center), vstr(icon), FontId::new(18.0, family), icon_color);
    ui.painter().text(m.pos(44.0 + 2.0 * t, cy), m.start(egui::Align::Center), vstr(label), FontId::new(14.5, if selected { semibold() } else { FontFamily::Proportional }), fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// The selected navigation entry's pill and accent bar, gliding to `target` (the selected
/// entry's rect). Paint it into a slot reserved before the entries so it sits behind them.
/// `clip` is the visible part of the list the entry is in (the list scrolls).
pub fn nav_indicator(ui: &Ui, slot: egui::layers::ShapeIdx, target: egui::Rect, clip: egui::Rect) {
    let p = Palette::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique("nav-indicator");
    let top = ease_to(&ctx, id.with("top"), target.top(), SLIDE);
    let rect = egui::Rect::from_min_size(egui::pos2(target.left(), top), target.size()).intersect(clip);
    if rect.height() < 4.0 {
        ui.painter().set(slot, egui::Shape::Noop);
        return;
    }
    let edge = if rtl() { rect.right() - 1.5 } else { rect.left() + 1.5 };
    let bar = egui::Rect::from_center_size(egui::pos2(edge, rect.center().y), Vec2::new(3.0, 18.0));
    ui.painter().set(slot, egui::Shape::Vec(vec![egui::Shape::rect_filled(rect, 9.0, p.accent_soft), egui::Shape::rect_filled(bar, 1.5, p.accent)]));
}

/// Pill-shaped option in a segmented control.
pub fn segment(ui: &mut Ui, selected: bool, text: &str) -> egui::Response {
    let text = tr(text);
    let p = Palette::get(ui.ctx());
    let (t, _) = interaction(ui, ui.next_auto_id());
    let text = if selected { rich(text).color(p.accent).family(semibold()) } else { rich(text).color(p.muted.lerp_to_gamma(p.text, t)) };
    let fill = if selected { p.accent_soft } else { p.hover.gamma_multiply(t) };
    let r = ui.add(egui::Button::new(vis(text)).fill(fill).stroke(Stroke::NONE).corner_radius(16).min_size(Vec2::new(0.0, 30.0)));
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, ""));
    r
}

/// On/off switch with an accessible name.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    let label = tr(label);
    let p = Palette::get(ui.ctx());
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let off_bg = if p.dark { Color32::from_rgb(60, 66, 80) } else { Color32::from_rgb(208, 213, 221) };
        let mut bg = off_bg.lerp_to_gamma(p.accent, t);
        if !ui.is_enabled() {
            bg = bg.gamma_multiply(0.5);
        }
        let h = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() && ui.is_enabled(), FADE);
        if h > 0.0 {
            ui.painter().rect_filled(rect.expand(3.0), 14.0, soft(p.accent, (30.0 * h) as u8));
        }
        ui.painter().rect_filled(rect, 11.0, bg);
        // Off sits at the start of the reading direction, on at the far end.
        let (off, on_x) = if rtl() { (rect.right() - 11.0, rect.left() + 11.0) } else { (rect.left() + 11.0, rect.right() - 11.0) };
        let x = egui::lerp(off..=on_x, t);
        let knob = egui::pos2(x, rect.center().y);
        ui.painter().circle_filled(knob + Vec2::new(0.0, 1.0), 8.5, Color32::from_black_alpha(40));
        ui.painter().circle_filled(knob, 8.0 + h * 0.5, Color32::WHITE);
        if resp.hovered() && ui.is_enabled() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    resp
}

/// A bordered single-line field of CONTROL_H height: optional leading icon, a clear button
/// when there is text (search fields), and an accent border while focused.
fn field(ui: &mut Ui, text: &mut String, hint: &str, width: f32, lead: Option<&str>) -> egui::Response {
    let hint = tr(hint);
    let p = Palette::get(ui.ctx());
    let clearable = lead.is_some();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(80.0), CONTROL_H), egui::Sense::hover());
    let bg = ui.painter().add(egui::Shape::Noop);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(10.0, 0.0))).layout(near_layout(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 6.0;
    if let Some(ic) = lead {
        child.label(rich(ic).color(p.faint));
    }
    let clear_w = if clearable && !text.is_empty() { 26.0 } else { 0.0 };
    let edit_w = (child.available_width() - clear_w).max(20.0);
    let r =
        child.add(egui::TextEdit::singleline(text).hint_text(vis(rich(hint).color(p.faint))).frame(egui::Frame::NONE).margin(Margin::ZERO).desired_width(edit_w).align(if rtl() {
            egui::Align2::RIGHT_CENTER
        } else {
            egui::Align2::LEFT_CENTER
        }));
    if clear_w > 0.0 {
        let c = child.add(egui::Button::new(vis(rich(icon::X).size(12.0).color(p.muted))).frame_when_inactive(false).min_size(Vec2::splat(20.0)));
        c.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Clear"));
        if c.clicked() {
            text.clear();
        }
    }
    let f = ui.ctx().animate_bool_with_time(r.id.with("focus"), r.has_focus(), FADE);
    let h = ui.ctx().animate_bool_with_time(r.id.with("hover"), r.hovered(), FADE);
    let stroke = Stroke::new(1.0 + 0.5 * f, p.border.lerp_to_gamma(p.faint, h * 0.6).lerp_to_gamma(p.accent, f));
    let mut shapes = Vec::new();
    if f > 0.0 {
        // A soft focus ring outside the border.
        shapes.push(egui::Shape::Rect(egui::epaint::RectShape::new(
            rect.expand(3.0),
            10.0,
            Color32::TRANSPARENT,
            Stroke::new(3.0, soft(p.accent, (50.0 * f) as u8)),
            egui::StrokeKind::Inside,
        )));
    }
    shapes.push(egui::Shape::Rect(egui::epaint::RectShape::new(rect, 8.0, p.surface, stroke, egui::StrokeKind::Inside)));
    ui.painter().set(bg, egui::Shape::Vec(shapes));
    r
}

/// Search field with a leading magnifier and a clear button. `width` is the text area's width.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    field(ui, text, hint, width + 48.0, Some(icon::SEARCH))
}

/// Text input with a border, for forms. `width` is the whole field's width.
pub fn text_input(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    field(ui, text, hint, width, None)
}

/// What a Browse button asks the native dialog for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Folder,
    File,
    /// A file to write; the field's current file name is suggested.
    Save,
    /// A file or a folder: two buttons.
    Any,
}

/// The answer of a dialog that runs on its own thread: `None` while it is open.
type DialogSlot = std::sync::Arc<std::sync::Mutex<Option<Option<std::path::PathBuf>>>>;

/// Open the native dialog on a background thread (so the window keeps drawing) and remember
/// where its answer will land.
fn open_dialog(ctx: &egui::Context, id: egui::Id, pick: Pick, current: &str) {
    let slot: DialogSlot = Default::default();
    ctx.data_mut(|d| d.insert_temp(id, slot.clone()));
    let current = std::path::PathBuf::from(current.trim());
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let mut d = rfd::FileDialog::new();
        // Start where the field points, or in its parent folder.
        if let Some(dir) = [Some(current.as_path()), current.parent()].into_iter().flatten().find(|p| p.is_dir()) {
            d = d.set_directory(dir);
        } else if let Some(home) = dirs::home_dir() {
            d = d.set_directory(home);
        }
        let picked = match pick {
            Pick::Folder => d.pick_folder(),
            Pick::File | Pick::Any => d.pick_file(),
            Pick::Save => {
                if let Some(name) = current.file_name() {
                    d = d.set_file_name(name.to_string_lossy());
                }
                d.save_file()
            }
        };
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(picked);
        ctx.request_repaint();
    });
}

/// A dialog's answer once it has closed: `Some(path)` when something was picked.
fn dialog_result(ctx: &egui::Context, id: egui::Id) -> Option<std::path::PathBuf> {
    let slot = ctx.data(|d| d.get_temp::<DialogSlot>(id))?;
    let answer = slot.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    ctx.data_mut(|d| d.remove::<DialogSlot>(id));
    answer
}

fn dialog_open(ctx: &egui::Context, id: egui::Id) -> bool {
    ctx.data(|d| d.get_temp::<DialogSlot>(id)).is_some()
}

/// Browse button(s) for a path field; returns the picked path once the dialog closes.
/// `salt` keeps buttons apart when a page has several.
pub fn browse_button(ui: &mut Ui, salt: &str, pick: Pick, current: &str) -> Option<String> {
    let base = egui::Id::unique(("browse", salt));
    let mut out = None;
    let buttons: &[(Pick, &str, &str)] = match pick {
        Pick::Folder => &[(Pick::Folder, icon::FOLDER_OPEN, "Choose a folder")],
        Pick::File => &[(Pick::File, icon::FILE, "Choose a file")],
        Pick::Save => &[(Pick::Save, icon::FOLDER_OPEN, "Choose where to save")],
        Pick::Any => &[(Pick::Folder, icon::FOLDER_OPEN, "Choose a folder"), (Pick::File, icon::FILE, "Choose a file")],
    };
    for (kind, ic, tip) in buttons {
        let id = base.with(*tip);
        let open = dialog_open(ui.ctx(), id);
        let r = ui.add_enabled_ui(!open, |ui| secondary(ui, ic, "")).inner;
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !open, tr(tip)));
        if r.on_hover_text(vis(tr(tip))).clicked() {
            open_dialog(ui.ctx(), id, *kind, current);
        }
        if let Some(p) = dialog_result(ui.ctx(), id) {
            out = Some(p.display().to_string());
        }
    }
    out
}

/// A path field with Browse button(s) on its right; `width` is the whole row's width. Picking
/// a path fills the field and marks the response changed.
pub fn path_input(ui: &mut Ui, salt: &str, text: &mut String, hint: &str, width: f32, pick: Pick) -> egui::Response {
    let buttons = if pick == Pick::Any { 2.0 } else { 1.0 };
    let r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut r = text_input(ui, text, hint, (width - buttons * (CONTROL_H + 6.0)).max(120.0));
            if let Some(p) = browse_button(ui, salt, pick, text) {
                *text = p;
                r.mark_changed();
            }
            r
        })
        .inner;
    r
}

/// Like `path_input` for fields that hold a registry key on Windows (no dialog can pick one
/// there) and a path on Linux.
pub fn key_or_path_input(ui: &mut Ui, salt: &str, text: &mut String, hint: &str, width: f32, pick: Pick) -> egui::Response {
    if cfg!(windows) {
        text_input(ui, text, hint, width)
    } else {
        path_input(ui, salt, text, hint, width, pick)
    }
}

/// A picked program as the start of a command line (quoted when the path has spaces).
pub fn command_from_path(path: &str) -> String {
    if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// A setting row: title + help text on the left, control on the right.
pub fn setting_row(ui: &mut Ui, title: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    let title = tr(title);
    let help = tr(help);
    ui.add_space(2.0);
    row(ui, |ui| {
        ui.set_min_height(42.0);
        column(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.set_width((ui.available_width() - 260.0).max(200.0));
            if help.is_empty() {
                ui.add_space(10.0); // centre a lone title in the row
            }
            ui.add(Label::new(title));
            if !help.is_empty() {
                ui.add(Label::new(muted(ui, help).size(12.5)).wrap());
            }
        });
        ui.with_layout(far_layout(egui::Align::Center), control);
    });
    ui.add_space(2.0);
}

pub fn divider(ui: &mut Ui) {
    let p = Palette::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
}
