//! Apply/switch/revert against a sandboxed gsettings backend (never the user's real settings).
#![cfg(not(windows))]

use cleancore::tuner::{self, State};
use std::process::Command;

fn gget(schema: &str, key: &str) -> String {
    let out = Command::new("gsettings").args(["get", schema, key]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn choice_switch_then_revert_restores_original() {
    if Command::new("gsettings").arg("--version").output().is_err() {
        eprintln!("gsettings not installed, skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("cleancore-tuner-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Keyfile backend under a temp XDG_CONFIG_HOME isolates every gsettings call from dconf.
    std::env::set_var("GSETTINGS_BACKEND", "keyfile");
    std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    std::env::set_var("CLEANCORE_DATA_DIR", dir.join("data"));

    let tweaks = tuner::tweaks();
    let t = tweaks.iter().find(|t| t.id == "color_scheme").unwrap();
    if tuner::status_all(std::slice::from_ref(t), &State::default())[0].available.is_err() {
        eprintln!("org.gnome.desktop.interface not installed, skipping");
        return;
    }
    let original = gget("org.gnome.desktop.interface", "color-scheme");

    let mut state = State::load().unwrap();
    tuner::apply(t, "dark", &mut state).unwrap();
    assert_eq!(gget("org.gnome.desktop.interface", "color-scheme"), "'prefer-dark'");
    tuner::apply(t, "light", &mut state).unwrap();
    assert_eq!(gget("org.gnome.desktop.interface", "color-scheme"), "'prefer-light'");
    let st = tuner::status_all(std::slice::from_ref(t), &state).remove(0);
    assert_eq!(st.current.as_deref(), Some("light"));
    assert_eq!(st.ours.as_deref(), Some("light"));

    tuner::revert(t, &mut state).unwrap();
    assert_eq!(gget("org.gnome.desktop.interface", "color-scheme"), original);
    assert!(State::load().unwrap().applied.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
