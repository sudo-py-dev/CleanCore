//! `cleancore.com` on Windows (packaged under that name): what a terminal runs for `cleancore`,
//! since `.com` comes before `.exe`. `cleancore.exe` is a windowed program, which a terminal
//! doesn't wait for; this console program starts it with the same arguments and the terminal's
//! input and output, waits for it and passes its exit code on. Opening the window
//! (`cleancore` alone) doesn't wait. See src/launch.rs.

#[cfg(windows)]
fn main() {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // Ctrl+C reaches cleancore.exe too (it attaches to this console): it stops there, and this
    // one stays to pass its exit code on. A handler, not "ignore": that would be inherited.
    unsafe extern "system" fn wait_for_child(_: u32) -> BOOL {
        1
    }
    // SAFETY: a valid handler function for the life of the process.
    unsafe { SetConsoleCtrlHandler(Some(wait_for_child), 1) };

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut child = match std::env::current_exe().and_then(|e| std::process::Command::new(e.with_extension("exe")).args(&args).spawn()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: starting cleancore.exe: {e}");
            std::process::exit(1);
        }
    };
    if cleancore::launch::opens_window(&args) {
        return;
    }
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("cleancore-console is the Windows terminal launcher (cleancore.com). Run cleancore.");
    std::process::exit(1);
}
