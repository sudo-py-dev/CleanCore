//! End-to-end GUI test: drives the real app headlessly through the accessibility tree and
//! saves a screenshot of every screen to `target/gui-shots/`.
//!
//! Everything runs in a sandbox: a fake home folder, a temporary data folder, the gsettings
//! keyfile backend and unit files written to a temp folder, so the user's system is untouched.
#![cfg(all(feature = "gui", target_os = "linux"))]

use cleancore::gui::App;
use eframe::egui::{self, Role};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn shots_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/gui-shots");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    h.run_steps(4);
    let img = h.render().expect("render");
    img.save(shots_dir().join(format!("{name}.png"))).unwrap();
}

/// Step the app until a label containing `text` shows up (background work finishing).
fn wait_for(h: &mut Harness<'_, App>, text: &str) {
    let start = Instant::now();
    loop {
        h.step();
        if h.query_all_by_label_contains(text).next().is_some() {
            return;
        }
        // CI runners are slower, and their /opt holds gigabytes of toolchains to snapshot.
        let limit = if std::env::var_os("CI").is_some() { 240 } else { 20 };
        if start.elapsed() > Duration::from_secs(limit) {
            if let Ok(img) = h.render() {
                let _ = img.save(shots_dir().join("zz-timeout.png"));
            }
            panic!("timed out waiting for '{text}'");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

#[test]
fn walk_through_every_tool() {
    let root = std::env::temp_dir().join(format!("cleancore-gui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    // Things the cleaner should find.
    write(&home.join(".local/share/applications/gone.desktop"), "[Desktop Entry]\nType=Application\nName=Gone\nExec=/opt/gone/bin/gone\n");
    write(&home.join(".config/autostart/daemon.desktop"), "[Desktop Entry]\nType=Application\nExec=nonexistent-daemon\n");
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    std::os::unix::fs::symlink("/nope", home.join(".local/bin/broken")).unwrap();
    write(&home.join(".config/myapp/app.conf"), "[General]\ntheme=light\n");
    // An unpackaged app for the uninstaller.
    let app = home.join(".local/opt/fakeapp");
    write(&app.join("bin/fakeapp"), "#!/bin/sh\n");
    write(&home.join(".local/share/applications/fakeapp.desktop"), &format!("[Desktop Entry]\nType=Application\nName=Fake App\nExec={}/bin/fakeapp\n", app.display()));
    write(&home.join(".config/fakeapp/settings.ini"), "x=1\n");
    write(&home.join(".cache/fakeapp/c"), "c\n");

    // A browser profile with an extension (Vivaldi, so a running Chrome/Brave doesn't block it).
    let vivaldi = home.join(".config/vivaldi");
    let ext_id = "abcdefghijklmnopabcdefghijklmnop";
    let ext_dir = vivaldi.join("Default/Extensions").join(ext_id);
    write(&ext_dir.join("2.1_0/manifest.json"), r#"{"name":"Ad Zapper","version":"2.1","description":"Zaps ads"}"#);
    write(&vivaldi.join("Local State"), r#"{"profile":{"info_cache":{"Default":{"name":"Personal"}}}}"#);
    write(&vivaldi.join("Default/Preferences"), &format!(r#"{{"extensions":{{"settings":{{"{ext_id}":{{"state":1,"location":1}}}}}},"keep":1}}"#));
    // An autostart entry to switch off.
    write(&home.join(".config/autostart/keepme.desktop"), "[Desktop Entry]\nType=Application\nName=Keep Me\nExec=/usr/bin/true\n");

    // An uninstall interrupted by a restart, to be offered again.
    write(
        &root.join("data/pending-uninstall.json"),
        r#"{"program":null,"label":"Zapp","version":"1","target":{"names":["Zapp"]},"op":"Uninstall","started":"2026-10-21T07:00:00+03:00","reboot_needed":true}"#,
    );
    std::env::set_var("CLEANCORE_NO_TRAY", "1");
    // The walkthrough finds widgets by their English text.
    std::env::set_var("CLEANCORE_LANG", "en");
    std::env::set_var("HOME", &home);
    std::env::set_var("CLEANCORE_DATA_DIR", root.join("data"));
    std::env::set_var("CLEANCORE_SERVICE_DIR", root.join("units"));
    std::env::set_var("GSETTINGS_BACKEND", "keyfile");
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));

    // A watch with the recorder started from the GUI later.
    let target = home.join(".config/myapp");
    cleancore::watcher::save_watches(&[cleancore::watcher::Watch { path: target.display().to_string(), active: true, include_reads: false, created: chrono::Local::now() }])
        .unwrap();

    let mut h = Harness::builder().with_size(egui::Vec2::new(1280.0, 820.0)).wgpu().build_eframe(|cc| App::new(cc));

    // First start asks for a theme.
    shot(&mut h, "00-first-run");
    h.get_by_label_contains("Light").click();
    h.run_steps(3);

    // ---- Uninstaller (start page)
    wait_for(&mut h, "programs ·");
    shot(&mut h, "01a-uninstaller");
    assert!(h.query_all_by_label_contains("Finish uninstalling Zapp").next().is_some());
    h.get_by_label_contains("Dismiss").click();
    h.run_steps(3);
    assert!(!root.join("data/pending-uninstall.json").exists());
    // Groups live in the toolbar's group dropdown.
    h.get_by_role(Role::ComboBox).click();
    h.run_steps(3);
    h.get_by_label_contains("Manage groups").click();
    h.run_steps(3);
    // The dialog's field is drawn after the page's search box.
    h.query_all_by_role(Role::TextInput).last().unwrap().click();
    h.query_all_by_role(Role::TextInput).last().unwrap().type_text("Work tools");
    h.get_by_label_contains("Create").click();
    h.run_steps(3);
    shot(&mut h, "01a2-groups-manager");
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(cleancore::uninstaller::Meta::load().groups.iter().any(|(g, _)| g == "Work tools"));
    h.get_by_role(Role::TextInput).click();
    h.get_by_role(Role::TextInput).type_text("Fake");
    h.run_steps(3);
    // Clicking a program's row doesn't select it for uninstalling; only its checkbox does.
    let uninstall = format!("{}  Uninstall", cleancore::gui::style::icon::TRASH);
    h.query_all_by_label("Fake App").find(|n| n.accesskit_node().role() != Role::CheckBox).unwrap().click();
    h.run_steps(3);
    assert!(h.get_by_role_and_label(Role::Button, &uninstall).accesskit_node().is_disabled());
    h.get_by_role(Role::CheckBox).click();
    h.run_steps(3);
    assert!(!h.get_by_role_and_label(Role::Button, &uninstall).accesskit_node().is_disabled());
    shot(&mut h, "01b-uninstaller-selected");
    // Cancelling the wizard after the program's uninstaller ran (which marks the uninstall as
    // unfinished) forgets it: no "Finish uninstalling" notice afterwards.
    h.get_by_role_and_label(Role::Button, &uninstall).click();
    wait_for(&mut h, "Scan for leftovers");
    write(
        &root.join("data/pending-uninstall.json"),
        r#"{"program":null,"label":"Fake App","version":"","target":{"names":["Fake App"]},"op":"Uninstall","started":"2026-10-22T07:00:00+03:00"}"#,
    );
    h.get_by_role_and_label(Role::Button, &format!("{}  Cancel", cleancore::gui::style::icon::X)).click();
    h.run_steps(3);
    assert!(!root.join("data/pending-uninstall.json").exists());
    assert!(h.query_all_by_label_contains("Finish uninstalling").next().is_none());
    h.get_by_role_and_label(Role::Button, &uninstall).click();
    wait_for(&mut h, "Scan for leftovers");
    shot(&mut h, "01c-wizard-mode");
    h.get_by_role_and_label(Role::Button, "Moderate").click();
    h.get_by_role_and_label(Role::Button, &format!("{}  Scan for leftovers", cleancore::gui::style::icon::SEARCH)).click();
    wait_for(&mut h, "Selected to delete");
    shot(&mut h, "01d-wizard-leftovers");
    h.get_by_label_contains("Delete ").click();
    wait_for(&mut h, "is uninstalled");
    shot(&mut h, "01e-wizard-done");
    assert!(!app.exists() && !home.join(".config/fakeapp").exists() && !home.join(".local/share/applications/fakeapp.desktop").exists());
    assert!(home.join(".config/myapp/app.conf").exists());
    h.get_by_label_contains("Back to programs").click();
    h.run_steps(3);
    h.get_by_label_contains("History").click();
    h.run_steps(3);
    shot(&mut h, "01f-history");
    h.get_by_label_contains("Forced Uninstall").click();
    h.run_steps(3);
    shot(&mut h, "01g-forced");
    h.get_by_role_and_label(Role::Button, "Back").click();
    h.run_steps(2);

    // ---- Install & trace, then uninstall from the log
    let script = root.join("install-gapp.sh");
    write(
        &script,
        &format!("#!/bin/sh\nmkdir -p {0}/.local/opt/gapp/bin {0}/.config/gapp\necho x > {0}/.local/opt/gapp/bin/gapp\necho a=1 > {0}/.config/gapp/gapp.conf\n", home.display()),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    h.get_by_role_and_label(Role::Button, &format!("{}  Install & trace", cleancore::gui::style::icon::RECORD)).click();
    wait_for(&mut h, "Tracing");
    h.get_by_role(Role::TextInput).click();
    h.get_by_role(Role::TextInput).type_text(&script.display().to_string());
    h.run_steps(2);
    h.get_by_label_contains("Run installer").click();
    wait_for(&mut h, "The installer finished");
    shot(&mut h, "01h-tracing");
    h.get_by_label_contains("Stop & analyze").click();
    wait_for(&mut h, "Save log");
    shot(&mut h, "01i-trace-review");
    h.get_by_label_contains("Save log").click();
    wait_for(&mut h, "Traced programs");
    h.run_steps(3);
    shot(&mut h, "01j-traced-tab");
    h.get_by_label("gapp").click();
    h.run_steps(3);
    h.get_by_label_contains("Custom uninstall").click();
    wait_for(&mut h, "Selected to delete");
    shot(&mut h, "01k-log-custom");
    h.get_by_label_contains("Delete ").click();
    wait_for(&mut h, "is uninstalled");
    shot(&mut h, "01l-log-done");
    assert!(h.query_all_by_label_contains("Advanced uninstall").next().is_some());
    assert!(!home.join(".local/opt/gapp").exists() && !home.join(".config/gapp").exists());
    h.get_by_label_contains("Back to programs").click();
    h.run_steps(3);

    // ---- Browser extensions: remove one from a (closed) Vivaldi profile
    h.get_by_label_contains("Browser extensions").click();
    wait_for(&mut h, "Ad Zapper");
    shot(&mut h, "01m-extensions");
    h.get_by_role(Role::CheckBox).click();
    h.run_steps(3);
    h.get_by_label_contains("Remove (1)").click();
    h.run_steps(3);
    shot(&mut h, "01n-extensions-confirm");
    h.get_by_role_and_label(Role::Button, &format!("{}  Remove", cleancore::gui::style::icon::TRASH)).click();
    wait_for(&mut h, "No extensions");
    assert!(!ext_dir.exists());
    let prefs = std::fs::read_to_string(vivaldi.join("Default/Preferences")).unwrap();
    assert!(!prefs.contains(ext_id) && prefs.contains("keep"));

    // ---- Logs database: share the gapp log, then see it in the database tab
    let log = cleancore::tracer::list().into_iter().find(|l| l.name == "gapp").expect("gapp log");
    cleancore::tracer::db_add(&cleancore::config::Settings::load().unwrap(), &log).unwrap();
    h.get_by_label_contains("Logs database").click();
    wait_for(&mut h, "Select a log");
    h.get_by_label("gapp").click();
    h.run_steps(3);
    shot(&mut h, "01o-logs-database");
    assert!(h.query_all_by_label_contains("Custom uninstall").next().is_some());

    // ---- Autorun: switch an autostart entry off
    h.get_by_role_and_label(Role::Button, "Autorun").click();
    wait_for(&mut h, "start automatically");
    wait_for(&mut h, "Keep Me");
    shot(&mut h, "01p-autorun");
    h.get_by_role_and_label(Role::Switch, "Start Keep Me automatically").click();
    h.run_steps(3);
    let keep = std::fs::read_to_string(home.join(".config/autostart/keepme.desktop")).unwrap();
    assert!(keep.contains("Hidden=true"), "{keep}");
    h.get_by_label_contains("Add").click();
    h.run_steps(3);
    shot(&mut h, "01q-autorun-add");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // ---- Hunter Mode (Windows only; Linux desktops don't let apps pick other windows)
    if !cfg!(windows) {
        assert!(h.query_all_by_role_and_label(Role::Button, "Hunter Mode").next().is_none(), "Hunter Mode is shown on Linux");
    }
    if cfg!(windows) {
        // Pick a running program from the list, then kill it.
        let dummy = root.join("huntme");
        let src = root.join("huntme.rs");
        write(&src, "fn main(){std::thread::sleep(std::time::Duration::from_secs(120));}");
        assert!(std::process::Command::new("rustc").arg("-o").arg(&dummy).arg(&src).status().unwrap().success());
        let mut child = std::process::Command::new(&dummy).env("DISPLAY", ":9").spawn().unwrap();
        h.get_by_role_and_label(Role::Button, "Hunter Mode").click();
        h.run_steps(3);
        shot(&mut h, "01r-hunter-apps");
        h.get_by_label_contains("Floating target").click();
        h.run_steps(4);
        shot(&mut h, "01r2-hunter-floating");
        assert!(h.query_all_by_label("Floating hunter target").next().is_some());
        h.get_by_label("Exit Hunter Mode").click();
        h.run_steps(3);
        assert!(h.query_all_by_label("Floating hunter target").next().is_none());
        // The dummy isn't a desktop app (no app scope), so show every session process.
        h.get_by_role_and_label(Role::Switch, "Show all processes").click();
        h.run_steps(3);
        h.get_by_role(Role::TextInput).click();
        h.get_by_role(Role::TextInput).type_text("huntme");
        wait_for(&mut h, "huntme");
        h.get_by_role_and_label(Role::Button, "huntme").click();
        wait_for(&mut h, "Installed as");
        shot(&mut h, "01s-hunter-target");
        h.get_by_label_contains("Kill process").click();
        h.run_steps(3);
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "hunter didn't kill the process");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    // ---- Cleaner
    h.get_by_role_and_label(Role::Button, "Cleaner").click();
    h.run_steps(3);
    shot(&mut h, "01-cleaner-home");
    h.get_by_label("Moderate Scan").click();
    h.run_steps(2);
    shot(&mut h, "01b-cleaner-moderate");
    h.get_by_label_contains("Start scan").click();
    h.run_steps(1);
    shot(&mut h, "01c-cleaner-scanning");
    wait_for(&mut h, "categor");
    shot(&mut h, "02-cleaner-results");
    assert!(h.query_all_by_label_contains("gone.desktop").next().is_some());

    h.get_by_label_contains("Clean (").click();
    h.run_steps(3);
    shot(&mut h, "03-cleaner-confirm");
    h.get_by_role_and_label(Role::Button, &format!("{}  Delete", cleancore::gui::style::icon::TRASH)).click();
    wait_for(&mut h, "All clean");
    shot(&mut h, "04-cleaner-cleaned");
    assert!(!home.join(".local/share/applications/gone.desktop").exists());
    assert!(std::fs::symlink_metadata(home.join(".local/bin/broken")).is_err());

    // ---- Auto Clean: save a schedule (unit files go to the sandbox folder)
    h.get_by_label_contains("New scan").click();
    h.run_steps(3);
    h.get_by_label_contains("Auto Clean").click();
    h.run_steps(3);
    h.get_by_label_contains("New schedule").click();
    h.run_steps(3);
    shot(&mut h, "05-auto-clean-new");
    h.get_by_label_contains("Save").click();
    wait_for(&mut h, "Saved \"Weekly clean\"");
    shot(&mut h, "05-auto-clean");
    assert!(root.join("units/cleancore-task-weekly-clean.timer").exists());
    assert!(h.query_all_by_label_contains("Every Sunday at 03:00").next().is_some());
    h.get_by_role_and_label(Role::Button, "Back").click();
    h.run_steps(2);

    // ---- Tuner
    h.get_by_role_and_label(Role::Button, "Tuner").click();
    h.run_steps(3);
    shot(&mut h, "06-tuner");
    h.get_by_label("Desktop").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Switch, "Show seconds in the top-bar clock").click();
    wait_for(&mut h, "by CleanCore");
    shot(&mut h, "07-tuner-applied");
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "clock-show-seconds"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
    h.get_by_label_contains("Reset all").click();
    h.run_steps(3);
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "clock-show-seconds"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "false");

    // ---- Watcher: start the recorder from the GUI, change a watched file, see the event.
    h.get_by_role_and_label(Role::Button, "Watcher").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, &format!("{}  Start recording", cleancore::gui::style::icon::PLAY)).click();
    h.run_steps(2); // the click is handled on the next frame
    wait_for(&mut h, "Recording");
    std::thread::sleep(Duration::from_millis(1800)); // first reconcile tick adds the inotify watch
    write(&target.join("app.conf"), "[General]\ntheme=dark\nsize=12\n");
    wait_for(&mut h, "General/theme");
    shot(&mut h, "08-watcher");

    // ---- Backups
    h.get_by_role_and_label(Role::Button, "Backups").click();
    h.run_steps(3);
    shot(&mut h, "09-backups");
    assert!(h.query_all_by_label("Deleted items").count() >= 1);
    h.get_by_label_contains("Create backup").click();
    h.run_steps(3);
    shot(&mut h, "10-create-backup");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // ---- Tools: junk files, history, unrecoverable delete, evidence remover, system tools
    let old = std::time::SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    write(&home.join("Documents/report.docx.tmp"), "junk");
    std::fs::File::options().write(true).open(home.join("Documents/report.docx.tmp")).unwrap().set_modified(old).unwrap();
    write(&home.join(".cache/thumbnails/normal/abc.png"), "png");
    let secret = root.join("secret.txt");
    write(&secret, "top secret");
    h.get_by_role_and_label(Role::Button, "Tools").click();
    h.run_steps(3);
    shot(&mut h, "15-tools-junk-setup");
    h.get_by_label_contains("Scan for junk files").click();
    wait_for(&mut h, "junk files ·");
    shot(&mut h, "16-tools-junk-results");
    h.get_by_label_contains("Delete (").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, &format!("{}  Delete", cleancore::gui::style::icon::TRASH)).click();
    wait_for(&mut h, "Deleted 1 junk file");
    assert!(!home.join("Documents/report.docx.tmp").exists());

    let photo: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::create_dir_all(home.join("Pictures/copy")).unwrap();
    std::fs::write(home.join("Pictures/holiday.jpg"), &photo).unwrap();
    std::fs::write(home.join("Pictures/copy/holiday (1).jpg"), &photo).unwrap();
    h.get_by_label(&format!("{}  Duplicates", cleancore::gui::style::icon::COPY)).click();
    h.run_steps(3);
    shot(&mut h, "16b-tools-dupes-setup");
    h.get_by_label_contains("Find duplicates").click();
    wait_for(&mut h, "can be freed");
    shot(&mut h, "16c-tools-dupes-results");
    h.get_by_label_contains("Delete (").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, &format!("{}  Delete", cleancore::gui::style::icon::TRASH)).click();
    wait_for(&mut h, "Deleted 1 duplicate file");
    assert_eq!(home.join("Pictures/holiday.jpg").exists() as u8 + home.join("Pictures/copy/holiday (1).jpg").exists() as u8, 1);

    h.get_by_label(&format!("{}  History", cleancore::gui::style::icon::HISTORY)).click();
    wait_for(&mut h, "Thumbnail cache");
    shot(&mut h, "17-tools-history");
    h.query_all_by_label(&format!("{}  Clean", cleancore::gui::style::icon::BROOM)).next().unwrap().click();
    h.run_steps(3);
    h.query_all_by_label(&format!("{}  Clean", cleancore::gui::style::icon::BROOM)).last().unwrap().click();
    wait_for(&mut h, "Cleaned ");
    assert!(!home.join(".cache/thumbnails/normal/abc.png").exists());

    h.get_by_label_contains("Unrecoverable delete").click();
    h.run_steps(3);
    h.get_by_role(Role::TextInput).click();
    h.get_by_role(Role::TextInput).type_text(&secret.display().to_string());
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, &format!("{}  Add", cleancore::gui::style::icon::PLUS)).click();
    h.run_steps(3);
    shot(&mut h, "18-tools-shred");
    h.get_by_label_contains("Destroy 1 item").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, &format!("{}  Destroy", cleancore::gui::style::icon::TRASH)).click();
    wait_for(&mut h, "Shredded 1 file");
    assert!(!secret.exists());

    h.get_by_label_contains("Evidence remover").click();
    h.run_steps(3);
    shot(&mut h, "19-tools-evidence");
    h.get_by_label_contains("System tools").click();
    h.run_steps(3);
    shot(&mut h, "20-tools-system");

    // ---- Settings, accent color, dark mode (the sidebar entry comes first; System Tools may
    // also offer the desktop's own "Settings")
    h.query_all_by_role_and_label(Role::Button, "Settings").next().unwrap().click();
    h.run_steps(3);
    shot(&mut h, "11-settings");
    h.get_by_label("Violet").click();
    h.run_steps(3);
    h.get_by_label("Switch to dark").click();
    h.run_steps(3);
    shot(&mut h, "12-settings-dark-violet");
    h.get_by_label("Indigo").click();
    h.get_by_role_and_label(Role::Button, "Cleaner").click();
    h.run_steps(3);
    shot(&mut h, "13-dark-cleaner");
    h.get_by_role_and_label(Role::Button, "Watcher").click();
    h.run_steps(3);
    shot(&mut h, "14-dark-watcher");
}
