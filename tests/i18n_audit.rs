//! Every text the interface shows is translated: the app is opened in another language
//! (Hebrew unless CLEANCORE_TEST_LANG says otherwise), every page and tab is visited, and any
//! text that is one of CleanCore's own messages (`locales/cleancore.pot`) but still shows in
//! English fails the test: it is displayed without `tr`. Other English-looking text is printed
//! for review: names (CleanCore, Windows, Flatpak, …), paths and what comes from the system
//! (program names, file contents) are not interface text.

#![cfg(feature = "gui")]

use cleancore::gui::App;
use cleancore::i18n::tr;
use eframe::egui::{self, Role};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use std::collections::BTreeSet;

/// Words that stay in English in every language: names of things.
const NAMES: &[&str] = &[
    "CleanCore",
    "Windows",
    "Linux",
    "Microsoft",
    "Flatpak",
    "Snap",
    "AppImage",
    "Edge",
    "OneDrive",
    "WebView2",
    "Copilot",
    "Xbox",
    "Store",
    "dconf",
    "GNOME",
    "KDE",
    "systemd",
    "MSI",
    "DISM",
    "Wayland",
    "PowerShell",
    "Bing",
    "Office",
    "Teams",
    "Skype",
    "Outlook",
    "Chrome",
    "Chromium",
    "Firefox",
    "Brave",
    "Opera",
    "Vivaldi",
    "CSV",
    "JSON",
    "HTML",
    "XML",
    "TXT",
    "GB",
    "MB",
    "KB",
    "SSD",
    "SSDs",
    "btrfs",
    "ZFS",
    "deb",
    "rpm",
    "pacman",
    "winget",
    "GitHub",
    "PDF",
    "URL",
    "ID",
    "AI",
    "Recall",
    "Cortana",
    "Spotify",
    "TikTok",
    "Disney",
    "LinkedIn",
    "Clipchamp",
    "Candy",
    "Crush",
    "King",
    "Solitaire",
    "Instagram",
    "Facebook",
    "Prime",
    "Video",
];

/// Product names that stay in English but are made of everyday words.
const PRODUCTS: &[&str] = &["Dev Home", "Microsoft To Do", "Power Automate", "Xbox Console Companion", "Xbox Game Bar", "Xbox Game Overlay"];

/// Text of the sandbox itself (names the test made up), not interface text.
const SANDBOX: &[&str] = &["Fake App", "Keep Me", "Zapp", "myapp", "Gone", "daemon"];

fn english_words(text: &str) -> Vec<String> {
    let names: BTreeSet<&str> = NAMES.iter().copied().collect();
    text.split(|c: char| !(c.is_ascii_alphabetic() || c == '\''))
        .filter(|w| w.len() >= 2 && w.chars().all(|c| c.is_ascii_alphabetic() || c == '\''))
        .filter(|w| !names.contains(w))
        .map(str::to_string)
        .collect()
}

/// Interface text left in English: two or more English words that aren't names, in text that
/// isn't a path, a command or a made-up sandbox name.
fn untranslated(text: &str) -> bool {
    let t = text.trim();
    // Any letter of another alphabet: translated (English names inside it are fine).
    if t.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) {
        return false;
    }
    if PRODUCTS.contains(&t) {
        return false;
    }
    if t.is_empty() || t.contains('/') || t.contains('\\') || t.contains("::") || SANDBOX.iter().any(|s| t.contains(s)) {
        return false;
    }
    english_words(t).len() >= 2
}

/// Click the widget with this role and label; false when there is none.
fn click(h: &Harness<'_, App>, role: Role, label: &str) -> bool {
    match h.query_all_by_role_and_label(role, label).next() {
        Some(n) => {
            n.click();
            true
        }
        None => false,
    }
}

/// Every text of the page, with the role of the widget it belongs to.
fn texts(h: &Harness<'_, App>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in h.root().children_recursive() {
        let node = n.accesskit_node();
        let role = format!("{:?}", node.role());
        for t in [node.label(), node.value(), node.description()].into_iter().flatten() {
            out.push((role.clone(), t));
        }
    }
    out
}

#[test]
fn every_page_is_translated() {
    let root = std::env::temp_dir().join(format!("cleancore-i18n-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/settings.json"), r#"{"gui_theme":"light"}"#).unwrap();
    let lang = std::env::var("CLEANCORE_TEST_LANG").unwrap_or_else(|_| "he".into());
    std::env::set_var("CLEANCORE_NO_TRAY", "1");
    std::env::set_var("CLEANCORE_LANG", &lang);
    std::env::set_var("HOME", &home);
    std::env::set_var("CLEANCORE_DATA_DIR", root.join("data"));
    std::env::set_var("CLEANCORE_SERVICE_DIR", root.join("units"));
    std::env::set_var("GSETTINGS_BACKEND", "keyfile");
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));

    let mut h = Harness::builder().with_size(egui::Vec2::new(1400.0, 900.0)).wgpu().build_eframe(|cc| App::new(cc));
    let settle = |h: &mut Harness<'_, App>| {
        for _ in 0..20 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    };
    settle(&mut h);

    let pot = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/cleancore.pot")).unwrap();
    let ours: BTreeSet<String> =
        pot.lines().filter_map(|l| l.strip_prefix("msgid \"")?.strip_suffix('"').map(|m| m.replace("\\\"", "\""))).filter(|m| english_words(m).len() >= 2).collect();
    // (page, text): ours, untranslated; and other English text to look at.
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    let mut review: BTreeSet<String> = BTreeSet::new();
    let look = |h: &Harness<'_, App>, page: &str, found: &mut BTreeSet<(String, String)>, review: &mut BTreeSet<String>| {
        for (role, t) in texts(h) {
            if !untranslated(&t) {
                continue;
            }
            // Commands are typed as they are.
            if t.contains(" -") {
                continue;
            }
            // An icon in front ("⚙  Settings") is not part of the message.
            let bare = t.trim_start_matches(|c: char| !c.is_ascii()).trim().to_string();
            if ours.contains(&bare) || ours.contains(t.trim()) {
                found.insert((format!("{page} [{role}]"), t));
            } else {
                review.insert(t);
            }
        }
    };

    let tools = ["Uninstaller", "Autorun", "Hunter Mode", "Cleaner", "Tools", "Tuner", "Watcher", "Backups", "Settings"];
    for tool in tools {
        if !click(&h, Role::Button, tr(tool)) {
            continue;
        }
        settle(&mut h);
        look(&h, tool, &mut found, &mut review);
        // Every tab of the page, and the tabs inside those.
        let tabs: Vec<String> = h.query_all_by_role(Role::Tab).filter_map(|n| n.accesskit_node().label()).collect();
        for tab in tabs {
            if click(&h, Role::Tab, &tab) {
                settle(&mut h);
                look(&h, &format!("{tool} › {tab}"), &mut found, &mut review);
                let inner: Vec<String> = h.query_all_by_role(Role::Tab).filter_map(|n| n.accesskit_node().label()).collect();
                for t2 in inner.into_iter().filter(|t| t != &tab) {
                    if click(&h, Role::Tab, &t2) {
                        settle(&mut h);
                        look(&h, &format!("{tool} › {tab} › {t2}"), &mut found, &mut review);
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    for t in &review {
        println!("REVIEW\t{t}");
    }
    for (page, t) in &found {
        println!("UNTRANSLATED\t{page}\t{t}");
    }
    assert!(found.is_empty(), "{} untranslated text(s); see UNTRANSLATED lines above", found.len());
}
