//! OS-specific backends. Each exposes the same free functions used by the engine, cleaner and backups.

/// One entry in the key / folder browser.
#[derive(Debug, Clone)]
pub struct BrowseNode {
    pub path: String,
    pub name: String,
    /// Can't be expanded (a file, a dconf key).
    pub leaf: bool,
}

pub mod files;

/// A running process (Hunter Mode, autorun "running" state, browser-closed checks).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub exe: Option<String>,
    pub cmdline: String,
    /// Has a visible window (Windows) / runs in the graphical session (Linux).
    pub gui: bool,
    /// The main process of a desktop application: has a window (Windows) / is the first
    /// process of an app launched from the desktop (Linux): what Hunter Mode lists.
    pub app: bool,
    pub title: Option<String>,
}

/// A mounted drive / filesystem (Evidence Remover, Junk Files Cleaner).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Drive {
    pub path: std::path::PathBuf,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

#[cfg(windows)]
mod windows_tools;
#[cfg(windows)]
pub use self::windows_tools::*;

#[cfg(not(windows))]
mod linux_tools;
#[cfg(not(windows))]
pub use self::linux_tools::*;

#[cfg(windows)]
mod windows_proc;
#[cfg(windows)]
pub use self::windows_proc::*;

#[cfg(not(windows))]
mod linux_proc;
#[cfg(not(windows))]
pub use self::linux_proc::*;

#[cfg(windows)]
mod windows_autorun;
#[cfg(windows)]
pub use self::windows_autorun::*;

#[cfg(not(windows))]
mod linux_autorun;
#[cfg(not(windows))]
pub use self::linux_autorun::*;

#[cfg(windows)]
mod windows_trace;
#[cfg(windows)]
pub use self::windows_trace::*;

#[cfg(not(windows))]
mod linux_trace;
#[cfg(not(windows))]
pub use self::linux_trace::*;

#[cfg(windows)]
mod windows_uninst;
#[cfg(windows)]
pub use self::windows_uninst::*;

#[cfg(not(windows))]
mod linux_uninst;
#[cfg(not(windows))]
pub use self::linux_uninst::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(windows))]
mod linux;
#[cfg(not(windows))]
pub use self::linux::*;

#[cfg(windows)]
mod windows_watch;
#[cfg(windows)]
pub use self::windows_watch::WatchBackend;

#[cfg(not(windows))]
mod linux_watch;
#[cfg(not(windows))]
pub use self::linux_watch::WatchBackend;

#[cfg(windows)]
mod windows_sched;
#[cfg(windows)]
pub use self::windows_sched::*;

#[cfg(not(windows))]
mod linux_sched;
#[cfg(not(windows))]
pub use self::linux_sched::*;

/// A command for a tool CleanCore runs in the background (curl, PowerShell, reg, schtasks, …):
/// on Windows no console window flashes up, and its output and errors come back to CleanCore.
/// Programs the user is meant to see (an uninstaller, Explorer) use `Command::new`.
pub fn hidden(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}
