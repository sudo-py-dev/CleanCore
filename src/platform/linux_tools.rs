//! Linux side of the phase-4 tools: mounted drives, Trash, clipboard, the system's history
//! items and the built-in System Tools.

use super::Drive;
use crate::history::{HistoryItem, Target};
use crate::systools::Tool;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REAL_FS: &[&str] = &["ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "reiserfs", "jfs", "zfs", "bcachefs"];

/// Mounted real filesystems with their free space (`df`), one entry per device.
pub fn drives() -> Vec<Drive> {
    let Ok(o) = Command::new("df").args(["-B1", "--output=source,fstype,size,avail,target"]).output() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut v = Vec::new();
    for l in String::from_utf8_lossy(&o.stdout).lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 || !REAL_FS.contains(&f[1]) || !seen.insert(f[0].to_string()) {
            continue;
        }
        let target = f[4..].join(" ");
        if target.starts_with("/boot") || target.starts_with("/snap") {
            continue;
        }
        let removable = target.starts_with("/media/") || target.starts_with("/run/media/") || target.starts_with("/mnt/");
        let label = if target == "/" { "System".to_string() } else { Path::new(&target).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(target.clone()) };
        v.push(Drive { path: PathBuf::from(&target), label, fs: f[1].to_string(), total: f[2].parse().unwrap_or(0), free: f[3].parse().unwrap_or(0), removable });
    }
    v
}

/// Free bytes on the filesystem holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let o = Command::new("df").args(["-B1", "--output=avail"]).arg(path).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().nth(1)?.trim().parse().ok()
}

fn trash_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("Trash")
}

pub fn empty_trash() -> Result<()> {
    for sub in ["files", "info", "expunged"] {
        let d = trash_dir().join(sub);
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                super::files::delete_path(&e.path(), false)?;
            }
        }
    }
    Ok(())
}

pub fn clear_clipboard() -> Result<()> {
    let tries: &[&[&str]] = &[&["wl-copy", "--clear"], &["xclip", "-selection", "clipboard", "-i", "/dev/null"], &["xsel", "--clipboard", "--clear"]];
    for t in tries {
        if Command::new(t[0]).args(&t[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }
    bail!("clearing the clipboard needs wl-clipboard (Wayland) or xclip / xsel (X11)")
}

pub fn system_history_items() -> Vec<HistoryItem> {
    let home = dirs::home_dir().unwrap_or_default();
    let data = dirs::data_dir().unwrap_or(home.join(".local/share"));
    let cache = dirs::cache_dir().unwrap_or(home.join(".cache"));
    let config = dirs::config_dir().unwrap_or(home.join(".config"));
    let g = "System";
    let mut v = vec![
        HistoryItem::new(
            "system:recent",
            g,
            "Recently used files",
            "The \"Recent\" list of the file manager and open dialogs",
            true,
            vec![
                Target::Rewrite { file: data.join("recently-used.xbel"), content: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\" xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\" xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\">\n</xbel>\n".into() },
                Target::Contents(data.join("RecentDocuments")),
            ],
        ),
        HistoryItem::new("system:thumbnails", g, "Thumbnail cache", "Small previews of pictures and videos", true, vec![Target::Contents(cache.join("thumbnails"))]),
        HistoryItem::new("system:trash", g, "Trash", "Files you deleted", false, vec![Target::Contents(trash_dir().join("files")), Target::Contents(trash_dir().join("info"))]),
        HistoryItem::new("system:clipboard", g, "Clipboard", "What was last copied", false, vec![Target::Clipboard]),
        HistoryItem::new(
            "system:shell",
            g,
            "Command history",
            "Commands typed in terminals (bash, zsh, fish, Python, less)",
            false,
            [".bash_history", ".zsh_history", ".local/share/fish/fish_history", ".python_history", ".lesshst", ".sqlite_history", ".node_repl_history"].iter().map(|f| Target::Path(home.join(f))).collect(),
        ),
        HistoryItem::new("system:appcache", g, "Application caches", "Everything in ~/.cache; programs rebuild it, so the first start may be slower", false, vec![Target::Contents(cache.clone())]),
    ];
    // Crash reports written for this user (Ubuntu's apport puts them in /var/crash).
    let me = std::fs::metadata(&home).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(u32::MAX);
    let crashes: Vec<Target> = std::fs::read_dir("/var/crash")
        .map(|rd| rd.flatten().filter(|e| e.metadata().is_ok_and(|m| std::os::unix::fs::MetadataExt::uid(&m) == me && m.is_file())).map(|e| Target::Path(e.path())).collect())
        .unwrap_or_default();
    if !crashes.is_empty() {
        v.push(HistoryItem::new("system:crash", g, "Crash reports", "Reports left by programs that crashed", true, crashes));
    }
    let lo = config.join("libreoffice/4/user/registrymodifications.xcu");
    v.push(HistoryItem::new(
        "office:libreoffice",
        "Office",
        "LibreOffice recent documents",
        "The Start Center and File › Recent lists",
        true,
        vec![Target::DropLines { file: lo, containing: "HistoryInfo['PickList']".into() }],
    ));
    v
}

/// A tool counts as available when its program is on PATH.
fn have(cmd: &str) -> bool {
    let prog = cmd.split_whitespace().next().unwrap_or(cmd);
    crate::pathutil::which(prog, &crate::pathutil::PathCache::default()).is_some()
}

pub fn builtin_tools() -> Vec<Tool> {
    let t = Tool::builtin;
    let candidates = vec![
        t("system-monitor", "System", "System Monitor", "Running processes and resource use", "gnome-system-monitor"),
        t("ksysguard", "System", "System Monitor", "Running processes and resource use", "plasma-systemmonitor"),
        t("htop", "System", "htop", "Processes in a terminal", "htop").in_terminal(),
        t("settings", "System", "Settings", "System settings", "gnome-control-center"),
        t("kde-settings", "System", "System Settings", "System settings", "systemsettings"),
        t("dconf-editor", "System", "dconf Editor", "Low-level desktop settings", "dconf-editor"),
        t("logs", "System", "Logs", "System journal", "gnome-logs"),
        t("journal", "System", "Journal (errors)", "Errors since the last start", "journalctl -b -p err --no-pager").in_terminal(),
        t("services", "System", "Services", "Running system services", "systemctl list-units --type=service --no-pager").in_terminal(),
        t("hardinfo", "System", "System Information", "Hardware and benchmarks", "hardinfo"),
        t("hwinfo", "System", "Hardware summary", "CPU, memory, disks", "inxi -Fxz").in_terminal(),
        t("disks", "Disks", "Disks", "Partitions, SMART data, formatting", "gnome-disks"),
        t("baobab", "Disks", "Disk Usage Analyzer", "What fills the disk", "baobab"),
        t("filelight", "Disks", "Filelight", "What fills the disk", "filelight"),
        t("gparted", "Disks", "GParted", "Partition editor", "gparted"),
        t("df", "Disks", "Free space", "Free space on every drive", "df -h").in_terminal(),
        t("network", "Network", "Network Connections", "Edit network connections", "nm-connection-editor"),
        t("ip", "Network", "Network information", "Addresses and routes", "ip -c a").in_terminal(),
        t("ping", "Network", "Ping", "Check the internet connection", "ping -c 4 1.1.1.1").in_terminal(),
        t("tracepath", "Network", "Trace route", "The path to a server", "tracepath -n 1.1.1.1").in_terminal(),
        t("firewall", "Security", "Firewall", "Firewall rules", "gufw"),
        t("seahorse", "Security", "Passwords and Keys", "Stored passwords and keys", "seahorse"),
        t("updates", "Software", "Software Updater", "Install updates", "update-manager"),
        t("software-properties", "Software", "Software & Updates", "Repositories and drivers", "software-properties-gtk"),
        t("software", "Software", "Software", "Install and remove apps", "gnome-software"),
        t("discover", "Software", "Discover", "Install and remove apps", "plasma-discover"),
        t("terminal", "System", "Terminal", "A command line", "x-terminal-emulator"),
    ];
    let mut seen = std::collections::HashSet::new();
    candidates.into_iter().filter(|t| have(&t.command) && seen.insert(t.name.clone())).collect()
}

/// A terminal emulator that can run a command and keep the window open.
fn terminal() -> Option<Vec<String>> {
    for (prog, args) in [
        ("gnome-terminal", vec!["--"]),
        ("konsole", vec!["-e"]),
        ("xfce4-terminal", vec!["-x"]),
        ("kitty", vec![]),
        ("alacritty", vec!["-e"]),
        ("xterm", vec!["-e"]),
        ("x-terminal-emulator", vec!["-e"]),
    ] {
        if have(prog) {
            let mut v = vec![prog.to_string()];
            v.extend(args.into_iter().map(String::from));
            return Some(v);
        }
    }
    None
}

pub fn run_tool(command: &str, in_terminal: bool, admin: bool) -> Result<()> {
    let command = if admin { format!("pkexec {command}") } else { command.to_string() };
    let mut c = if in_terminal {
        let term = terminal().context("no terminal emulator found")?;
        let mut c = Command::new(&term[0]);
        c.args(&term[1..]).args(["sh", "-c", &format!("{command}; echo; read -p 'Press Enter to close' _")]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &command]);
        c
    };
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().with_context(|| format!("starting {command}"))?;
    Ok(())
}
