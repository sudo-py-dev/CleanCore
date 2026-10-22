//! One program for both: `cleancore` alone (or with one of the window's options) opens the
//! window, and with a command (`cleancore scan`) it is the command-line tool.
//!
//! Windows: `cleancore.exe` is a windowed program, so it has no console of its own. Next to it,
//! `cleancore.com` (src/bin/cleancore-console.rs) is what a terminal runs for `cleancore`
//! (`.com` comes before `.exe`): it passes the command on and waits for it, as Visual Studio's
//! `devenv.com` does.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// The options the window takes: Hunter Mode, Explorer's "Install / Uninstall with CleanCore",
/// and finishing an uninstall after a restart.
const WINDOW_OPTIONS: &[&str] = &["--hunter", "/hunter", "--trace", "--uninstall", "--resume"];

/// Whether these arguments (without the program's name) open the window.
pub fn opens_window(args: &[OsString]) -> bool {
    let mut rest = args;
    // `--data-dir DIR` alone still opens the window, on that data folder.
    match rest.first().and_then(|a| a.to_str()) {
        Some("--data-dir") => rest = rest.get(2..).unwrap_or_default(),
        Some(a) if a.starts_with("--data-dir=") => rest = &rest[1..],
        _ => {}
    }
    match rest.first() {
        None => true,
        Some(a) => a.to_str().is_some_and(|a| WINDOW_OPTIONS.iter().any(|o| a.eq_ignore_ascii_case(o))),
    }
}

static WINDOW: AtomicBool = AtomicBool::new(false);

/// Note that this process is the window (not the command line).
pub fn set_window() {
    WINDOW.store(true, Ordering::Relaxed);
}

/// Whether this process is the window.
pub fn in_window() -> bool {
    WINDOW.load(Ordering::Relaxed)
}

/// The program, as the window and as the command line: `cleancore.exe` on Windows (also when
/// started through `cleancore.com`), `cleancore` elsewhere.
pub fn program() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(if cfg!(windows) { exe.with_extension("exe") } else { exe })
}

#[cfg(test)]
mod tests {
    use super::opens_window;

    fn args(a: &[&str]) -> Vec<std::ffi::OsString> {
        a.iter().map(Into::into).collect()
    }

    #[test]
    fn window_or_command_line() {
        assert!(opens_window(&args(&[])));
        assert!(opens_window(&args(&["--hunter"])));
        assert!(opens_window(&args(&["/HUNTER"])));
        assert!(opens_window(&args(&["--uninstall", "C:\\Program Files\\App"])));
        assert!(opens_window(&args(&["--data-dir", "/tmp/cc"])));
        assert!(opens_window(&args(&["--data-dir=/tmp/cc", "--resume"])));
        assert!(!opens_window(&args(&["scan"])));
        assert!(!opens_window(&args(&["--help"])));
        assert!(!opens_window(&args(&["--version"])));
        assert!(!opens_window(&args(&["--data-dir", "/tmp/cc", "scan"])));
        assert!(!opens_window(&args(&["-clean", "-safe"])));
        assert!(!opens_window(&args(&["root-helper"])));
    }
}
