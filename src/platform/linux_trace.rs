//! Linux side of the installation monitor: what to snapshot (program, config and data
//! folders; dconf; the package database) and how to undo a traced item.

use crate::model::Fix;
use crate::tracer::{h64, Excludes, ItemKind, SettingsEntry, TraceItem};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

pub fn default_trace_excludes() -> Vec<&'static str> {
    vec![
        // caches, trash, history and session state written by the desktop, not by installers
        "/.cache/",
        "/.local/share/trash",
        "/.local/share/recently-used.xbel",
        "/.local/share/gvfs-metadata",
        "/.local/share/tracker",
        "/.local/state/wireplumber",
        "/.config/pulse",
        "/.local/share/keyrings",
        "/.config/dconf/",
        "/.xsession-errors",
        "/.xauthority",
        "/.bash_history",
        "/.zsh_history",
        "/.python_history",
        "/.lesshst",
        "/.viminfo",
        "/.wget-hsts",
        "/.sudo_as_admin_successful",
        // browser profiles and big package/toolchain stores
        "/.mozilla/",
        "/.config/google-chrome",
        "/.config/chromium",
        "/.config/bravesoftware",
        "/.config/microsoft-edge",
        "/.config/vivaldi",
        "/.local/share/flatpak/",
        "/.local/share/steam",
        "/.steam/",
        "/.npm/",
        "/.cargo/registry",
        "/.cargo/git",
        "/.rustup/",
        "/.gradle/",
        "/.m2/",
        "/node_modules/",
        "/.git/",
        "/.local/share/containers",
        "/snap/",
        // this program's own data
        "/cleancore/",
        // dconf noise
        "/org/gnome/nautilus/window-state/",
        "/org/gtk/settings/file-chooser/",
        "/org/gtk/gtk4/settings/file-chooser/",
        "/org/gnome/evolution-data-server/",
        "/org/gnome/desktop/notifications/application/",
        // system files that change on their own
        "/etc/ld.so.cache",
        "/etc/mtab",
        "/etc/resolv.conf",
        "/etc/cups/",
        "/etc/.pwd.lock",
    ]
}

pub fn trace_file_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/opt", "/usr/local", "/etc"].iter().map(PathBuf::from).collect();
    if let Some(home) = dirs::home_dir() {
        for sub in [".config", ".local", "bin", "Applications", "AppImages", ".var/app", "Desktop"] {
            v.push(home.join(sub));
        }
        // Dotfiles and dot-folders directly in the home folder (~/.myapprc, ~/.myapp/)
        if let Ok(rd) = std::fs::read_dir(&home) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with('.') && ![".config", ".local", ".cache", ".var"].contains(&n.as_str()) {
                    v.push(e.path());
                }
            }
        }
    }
    v
}

/// dconf is the closest thing Linux desktops have to a registry.
pub fn walk_settings(ex: &Excludes, f: &mut dyn FnMut(SettingsEntry)) {
    let Ok(o) = Command::new("dconf").args(["dump", "/"]).output() else {
        return;
    };
    let text = String::from_utf8_lossy(&o.stdout);
    let mut key = String::new();
    let mut vals: Vec<(String, u64)> = Vec::new();
    let mut raw: Vec<Vec<u8>> = Vec::new();
    let flush = |key: &str, vals: &mut Vec<(String, u64)>, raw: &mut Vec<Vec<u8>>, f: &mut dyn FnMut(SettingsEntry)| {
        if !key.is_empty() && !ex.hit(key) {
            f(SettingsEntry { key, values: std::mem::take(vals), raw: std::mem::take(raw) });
        }
        vals.clear();
        raw.clear();
    };
    for line in text.lines() {
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            flush(&key, &mut vals, &mut raw, f);
            key = if sec == "/" { "/".into() } else { format!("/{}/", sec.trim_matches('/')) };
        } else if let Some((k, v)) = line.split_once('=') {
            vals.push((k.to_string(), h64(v.as_bytes())));
            raw.push(v.as_bytes().to_vec());
        }
    }
    flush(&key, &mut vals, &mut raw, f);
}

fn lines(cmd: &str, args: &[&str]) -> Vec<String> {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default()
}

pub fn package_snapshot() -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for l in lines("dpkg-query", &["-W", "-f=${binary:Package} ${db:Status-Abbrev}\n"]) {
        if let Some((p, st)) = l.split_once(' ') {
            if st.starts_with("ii") {
                s.insert(format!("dpkg:{}", p.split(':').next().unwrap_or(p)));
            }
        }
    }
    for l in lines("rpm", &["-qa", "--qf", "%{NAME}\\n"]) {
        s.insert(format!("rpm:{l}"));
    }
    for l in lines("pacman", &["-Qq"]) {
        s.insert(format!("pacman:{l}"));
    }
    for l in lines("flatpak", &["list", "--app", "--columns=application"]) {
        if !l.trim().is_empty() {
            s.insert(format!("flatpak:{}", l.trim()));
        }
    }
    for l in lines("snap", &["list"]).into_iter().skip(1) {
        if let Some(n) = l.split_whitespace().next() {
            s.insert(format!("snap:{n}"));
        }
    }
    s
}

pub fn trace_item_exists(i: &TraceItem) -> bool {
    match i.kind {
        ItemKind::File | ItemKind::Folder => std::fs::symlink_metadata(&i.path).is_ok(),
        ItemKind::SettingsKey => lines("dconf", &["list", &i.path]).iter().any(|l| !l.is_empty()),
        ItemKind::SettingsValue => lines("dconf", &["read", &format!("{}{}", i.path, i.value.as_deref().unwrap_or(""))]).iter().any(|l| !l.is_empty()),
        ItemKind::Package => package_snapshot().contains(&i.path),
    }
}

pub fn trace_settings_fix(i: &TraceItem) -> Fix {
    match i.kind {
        ItemKind::SettingsValue => Fix::DconfResetKey { key: format!("{}{}", i.path, i.value.as_deref().unwrap_or("")) },
        _ => Fix::DconfResetDir { dir: i.path.clone() },
    }
}

/// Start an installer: `.deb`/`.rpm` through the package manager, anything else directly.
pub fn launch(cmd: &str) -> anyhow::Result<std::process::Child> {
    let lower = cmd.to_lowercase();
    let path = cmd.trim().trim_matches('"');
    let child = if lower.ends_with(".deb") {
        Command::new("pkexec").args(["apt-get", "install", "-y", path]).spawn()?
    } else if lower.ends_with(".rpm") {
        Command::new("pkexec").args(["dnf", "install", "-y", path]).spawn()?
    } else if lower.ends_with(".flatpakref") || lower.ends_with(".flatpak") {
        Command::new("flatpak").args(["install", "-y", path]).spawn()?
    } else {
        Command::new("sh").args(["-c", cmd]).spawn()?
    };
    Ok(child)
}

// ---------------------------------------------------------------- following the installer

/// The installer's own file activity, recorded with `strace -f` (children included).
pub struct InstallWatch {
    log: PathBuf,
    cwd: PathBuf,
}

/// Start the installer under `strace` when that can see what it does. `None` when it can't:
/// no strace, or the work happens in a system service (`.deb` / `.rpm` / Flatpak installs go
/// through the package manager), in which case the caller starts it normally.
pub fn launch_watched(cmd: &str) -> anyhow::Result<Option<(std::process::Child, InstallWatch)>> {
    let lower = cmd.to_lowercase();
    if [".deb", ".rpm", ".flatpakref", ".flatpak"].iter().any(|e| lower.trim().trim_matches('"').ends_with(e)) {
        return Ok(None);
    }
    if crate::pathutil::which("strace", &Default::default()).is_none() {
        return Ok(None);
    }
    let log = std::env::temp_dir().join(format!("cleancore-trace-{}-{}.strace", std::process::id(), chrono::Local::now().format("%H%M%S%f")));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let child = Command::new("strace").args(["-f", "-qq", "-e", "trace=%file,%process", "-e", "signal=none", "-o"]).arg(&log).args(["--", "sh", "-c", cmd]).spawn()?;
    Ok(Some((child, InstallWatch { log, cwd })))
}

impl InstallWatch {
    /// Paths the installer (and its children) created or wrote. `None` when the record can't
    /// be trusted: it switched to root through sudo / pkexec, which strace can't follow.
    pub fn touched(self) -> Option<std::collections::HashSet<String>> {
        let text = std::fs::read_to_string(&self.log).ok();
        let _ = std::fs::remove_file(&self.log);
        parse_strace(&text?, &self.cwd)
    }
}

/// Quoted string arguments of a syscall line, in order.
fn quoted_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let mut j = i + 1;
            let mut cur = Vec::new();
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' && j + 1 < b.len() {
                    j += 1;
                    // strace escapes: \n, \t, \\, \", octal \123
                    match b[j] {
                        b'n' => cur.push(b'\n'),
                        b't' => cur.push(b'\t'),
                        c @ b'0'..=b'7' => {
                            let mut v = (c - b'0') as u32;
                            let mut k = 0;
                            while k < 2 && j + 1 < b.len() && (b'0'..=b'7').contains(&b[j + 1]) {
                                j += 1;
                                k += 1;
                                v = v * 8 + (b[j] - b'0') as u32;
                            }
                            cur.push(v as u8);
                        }
                        c => cur.push(c),
                    }
                } else {
                    cur.push(b[j]);
                }
                j += 1;
            }
            out.push(String::from_utf8_lossy(&cur).to_string());
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn parse_strace(text: &str, start_cwd: &std::path::Path) -> Option<std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    let mut cwd: HashMap<u32, PathBuf> = HashMap::new();
    let mut touched = HashSet::new();
    for line in text.lines() {
        let Some((pid, rest)) = line.split_once(' ') else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        let rest = rest.trim_start();
        let call = rest.split('(').next().unwrap_or("");
        let here = cwd.get(&pid).cloned().unwrap_or_else(|| start_cwd.to_path_buf());
        let abs = |p: &str| -> String {
            let pb = if p.starts_with('/') { PathBuf::from(p) } else { here.join(p) };
            // Normalize "a/./b" and "a/../b" without touching the disk.
            let mut out = PathBuf::new();
            for c in pb.components() {
                match c {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => out.push(c),
                }
            }
            out.display().to_string()
        };
        let args = quoted_args(rest);
        match call {
            "execve" => {
                // Becoming root: everything after that is invisible to strace.
                if let Some(prog) = args.first() {
                    let name = prog.rsplit('/').next().unwrap_or(prog);
                    if ["sudo", "pkexec", "su", "doas", "run0"].contains(&name) && !rest.contains("= -1") {
                        return None;
                    }
                }
            }
            "chdir" => {
                if let Some(p) = args.first().filter(|_| rest.trim_end().ends_with("= 0")) {
                    cwd.insert(pid, PathBuf::from(abs(p)));
                }
            }
            "clone" | "clone3" | "fork" | "vfork" => {
                if let Some(child) = rest.rsplit("= ").next().and_then(|r| r.trim().parse::<u32>().ok()) {
                    cwd.insert(child, here.clone());
                }
            }
            "open" | "openat" | "creat" | "openat2" => {
                let writes = call == "creat" || ["O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC"].iter().any(|f| rest.contains(f));
                if writes {
                    if let Some(p) = args.first() {
                        touched.insert(abs(p));
                    }
                }
            }
            "mkdir" | "mkdirat" | "truncate" | "unlink" | "unlinkat" | "rmdir" => {
                if let Some(p) = args.first() {
                    touched.insert(abs(p));
                }
            }
            "rename" | "renameat" | "renameat2" | "link" | "linkat" | "symlink" | "symlinkat" => {
                // The last path is the new name; for rename the old one changes too.
                for p in &args {
                    touched.insert(abs(p));
                }
            }
            _ => {}
        }
    }
    Some(touched)
}

#[cfg(test)]
mod strace_tests {
    use super::*;

    #[test]
    fn parses_writes_and_cwd() {
        let log = r#"100 execve("/bin/sh", ["sh", "-c", "x"], 0x0 /* 3 vars */) = 0
100 openat(AT_FDCWD, "/etc/ld.so.cache", O_RDONLY|O_CLOEXEC) = 3
100 clone(child_stack=NULL, flags=CLONE_CHILD_CLEARTID|SIGCHLD) = 101
101 chdir("/opt/app") = 0
101 mkdir("lib", 0777) = 0
101 openat(AT_FDCWD, "lib/a.so", O_WRONLY|O_CREAT|O_TRUNC, 0666) = 3
100 openat(AT_FDCWD, "conf", O_WRONLY|O_CREAT|O_TRUNC, 0644 <unfinished ...>
101 renameat2(AT_FDCWD, "../tmp/x", AT_FDCWD, "/opt/app/x", RENAME_NOREPLACE) = 0
"#;
        let t = parse_strace(log, std::path::Path::new("/home/me")).unwrap();
        for p in ["/opt/app/lib", "/opt/app/lib/a.so", "/home/me/conf", "/opt/tmp/x", "/opt/app/x"] {
            assert!(t.contains(p), "{p} missing from {t:?}");
        }
        assert!(!t.contains("/etc/ld.so.cache"));
        assert!(parse_strace("5 execve(\"/usr/bin/sudo\", [\"sudo\", \"make\"], 0x0) = 0\n", std::path::Path::new("/")).is_none());
    }
}
