//! Every page at the smallest window size (960×600): screenshots to `target/gui-shots/narrow-*`
//! so layouts that overflow or overlap show up. Read-only: nothing is cleaned or removed.
#![cfg(all(feature = "gui", target_os = "linux"))]

use cleancore::gui::App;
use eframe::egui::{self, Role};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use std::path::PathBuf;

fn shot(h: &mut Harness<'_, App>, name: &str) {
    h.run_steps(6);
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/gui-shots");
    std::fs::create_dir_all(&d).unwrap();
    let lang = std::env::var("CLEANCORE_LANG").unwrap_or_default();
    let suffix = if lang == "en" { String::new() } else { format!("-{lang}") };
    h.render().expect("render").save(d.join(format!("narrow{suffix}-{name}.png"))).unwrap();
}

#[test]
fn every_page_fits_a_small_window() {
    let root = std::env::temp_dir().join(format!("cleancore-gui-narrow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".config")).unwrap();
    // A theme is already chosen, so the first-run dialog doesn't cover the pages.
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/settings.json"), r#"{"gui_theme":"light"}"#).unwrap();
    std::env::set_var("CLEANCORE_NO_TRAY", "1");
    // English by default; CLEANCORE_TEST_LANG=he (etc.) takes the screenshots in another
    // language. Widgets are found by English text, so only the tool pages are visited then.
    let lang = std::env::var("CLEANCORE_TEST_LANG").unwrap_or_else(|_| "en".into());
    std::env::set_var("CLEANCORE_LANG", &lang);
    std::env::set_var("HOME", &home);
    std::env::set_var("CLEANCORE_DATA_DIR", root.join("data"));
    std::env::set_var("CLEANCORE_SERVICE_DIR", root.join("units"));
    std::env::set_var("GSETTINGS_BACKEND", "keyfile");
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));

    let mut h = Harness::builder().with_size(egui::Vec2::new(960.0, 600.0)).wgpu().build_eframe(|cc| App::new(cc));
    h.run_steps(10);
    // Wait for the program list.
    for _ in 0..200 {
        h.step();
        if h.query_all_by_label_contains(" · ").next().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    shot(&mut h, "uninstaller");
    let english = lang == "en";
    for tab in ["Flatpak", "Traced", "Browser extensions", "Logs database", "History"].into_iter().filter(|_| english) {
        let found = match h.query_all_by_label_contains(tab).next() {
            Some(t) => {
                t.click();
                true
            }
            None => false,
        };
        if found {
            shot(&mut h, &format!("uninstaller-{}", tab.split(' ').next().unwrap().to_lowercase()));
        }
    }
    for tool in ["Autorun", "Cleaner", "Tools", "Tuner", "Watcher", "Backups", "Settings"] {
        h.get_by_role_and_label(Role::Button, cleancore::i18n::tr(tool)).click();
        shot(&mut h, &tool.to_lowercase());
    }
    let _ = std::fs::remove_dir_all(&root);
}
