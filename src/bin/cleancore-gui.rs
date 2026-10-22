//! Stand-in for `cleancore-gui`, the window's own program until 0.2.8: it opens `cleancore`,
//! which is now both the window and the command line. It ships in the .zip, .tar.gz and .deb
//! for what still starts the old name: the 0.2.8 updater (it restarts `cleancore-gui` after
//! updating), and shortcuts or login items made by hand.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let program = std::env::current_exe().map(|e| e.with_file_name(if cfg!(windows) { "cleancore.exe" } else { "cleancore" }));
    if let Err(e) = program.and_then(|p| std::process::Command::new(p).args(std::env::args_os().skip(1)).spawn()) {
        eprintln!("error: starting cleancore: {e}");
        std::process::exit(1);
    }
}
