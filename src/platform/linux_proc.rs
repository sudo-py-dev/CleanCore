//! Linux processes: /proc, kill, and the window under the pointer (X11 only).

use super::Proc;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

/// systemd puts apps started from the desktop in `app-….scope` (GNOME, KDE, Flatpak) or
/// `snap.….scope`; terminals' children go in `vte-spawn-….scope`, services in `….service`.
fn app_scope(pid: u32) -> Option<String> {
    let cg = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let last = cg.lines().find_map(|l| l.strip_prefix("0::"))?.rsplit('/').next()?.to_string();
    ((last.starts_with("app-") || last.starts_with("snap.")) && last.ends_with(".scope")).then_some(last)
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

pub fn list_processes() -> Vec<Proc> {
    let mut out = read_processes();
    let me = fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
    let ppid: std::collections::HashMap<u32, u32> = out.iter().filter_map(|p| Some((p.pid, parent_pid(p.pid)?))).collect();
    let scope: std::collections::HashMap<u32, String> =
        out.iter().filter(|p| fs::metadata(format!("/proc/{}", p.pid)).is_ok_and(|m| m.uid() == me)).filter_map(|p| Some((p.pid, app_scope(p.pid)?))).collect();
    if scope.is_empty() {
        // No systemd app scopes (other desktops): session processes whose parent isn't one.
        let gui: std::collections::HashSet<u32> = out.iter().filter(|p| p.gui).map(|p| p.pid).collect();
        for p in &mut out {
            p.app = p.gui && ppid.get(&p.pid).is_none_or(|pp| !gui.contains(pp));
        }
        return out;
    }
    // The main process of each app scope: the one with the most children in the scope (the
    // browser / Electron main process rather than its launcher script or crash handler).
    let mut children: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for (pid, s) in &scope {
        if let Some(pp) = ppid.get(pid).filter(|pp| scope.get(pp) == Some(s)) {
            *children.entry(*pp).or_default() += 1;
        }
    }
    let mut best: std::collections::HashMap<&str, (usize, std::cmp::Reverse<u32>)> = std::collections::HashMap::new();
    for (pid, s) in &scope {
        let k = (children.get(pid).copied().unwrap_or(0), std::cmp::Reverse(*pid));
        let e = best.entry(s.as_str()).or_insert(k);
        if k > *e {
            *e = k;
        }
    }
    let mains: std::collections::HashSet<u32> = best.values().map(|(_, std::cmp::Reverse(pid))| *pid).collect();
    for p in &mut out {
        if mains.contains(&p.pid) {
            p.app = true;
            p.gui = true;
        }
    }
    out
}

fn read_processes() -> Vec<Proc> {
    let me = fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
    let Ok(rd) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let dir = e.path();
        let name = fs::read_to_string(dir.join("comm")).map(|s| s.trim().to_string()).unwrap_or_default();
        let exe = fs::read_link(dir.join("exe")).ok().map(|p| p.display().to_string().trim_end_matches(" (deleted)").to_string());
        let cmdline = fs::read(dir.join("cmdline")).map(|b| String::from_utf8_lossy(&b).replace('\0', " ").trim().to_string()).unwrap_or_default();
        // A graphical-session process of this user: its environment names a display.
        let gui = fs::metadata(&dir).is_ok_and(|m| m.uid() == me)
            && fs::read(dir.join("environ")).is_ok_and(|env| env.split(|b| *b == 0).any(|v| v.starts_with(b"DISPLAY=") || v.starts_with(b"WAYLAND_DISPLAY=")));
        if name.is_empty() && exe.is_none() {
            continue;
        }
        out.push(Proc { pid, name, exe, cmdline, gui, app: false, title: None });
    }
    out
}

pub fn kill_process(pid: u32) -> Result<()> {
    let st = Command::new("kill").args(["-TERM", &pid.to_string()]).status().context("running kill")?;
    if !st.success() {
        bail!("could not stop process {pid}");
    }
    // Give it a moment, then force.
    for _ in 0..20 {
        if fs::metadata(format!("/proc/{pid}")).is_err() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    Ok(())
}

/// PID owning the top-level window under the pointer. Needs X11 with xdotool and xprop;
/// Wayland doesn't let applications inspect other windows.
pub fn window_at_pointer() -> Result<Option<u32>> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") {
        bail!("Wayland doesn't allow picking other windows; choose the program from the list instead");
    }
    let o = Command::new("xdotool").args(["getmouselocation", "--shell"]).output().context("xdotool is needed to pick a window (install the xdotool package)")?;
    let text = String::from_utf8_lossy(&o.stdout);
    let Some(win) = text.lines().find_map(|l| l.strip_prefix("WINDOW=")) else {
        return Ok(None);
    };
    let o = Command::new("xprop").args(["-id", win, "_NET_WM_PID"]).output().context("xprop is needed to pick a window")?;
    let pid = String::from_utf8_lossy(&o.stdout).rsplit('=').next().and_then(|s| s.trim().parse().ok());
    Ok(pid)
}

/// Who ships this file: the owning package, if any.
/// Publisher for many files with a single `dpkg-query` call.
pub fn file_publishers(paths: &[String]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut ask = Vec::new();
    for p in paths {
        // /snap/<name>/<rev>/...
        if let Some(name) = p.strip_prefix("/snap/").and_then(|r| r.split('/').next()).filter(|n| *n != "bin") {
            out.insert(p.clone(), format!("snap {name}"));
        } else if p.starts_with('/') {
            ask.push(p.as_str());
        }
    }
    ask.sort_unstable();
    ask.dedup();
    if ask.is_empty() {
        return out;
    }
    let Ok(o) = Command::new("dpkg-query").arg("-S").args(&ask).output() else {
        return out;
    };
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if let Some((pkg, path)) = line.split_once(": ") {
            out.insert(path.trim().to_string(), format!("package {}", pkg.split(',').next().unwrap_or(pkg).trim()));
        }
    }
    out
}

pub fn file_publisher(path: &str) -> Option<String> {
    let o = Command::new("dpkg-query").args(["-S", path]).output().ok().filter(|o| o.status.success())?;
    let line = String::from_utf8_lossy(&o.stdout);
    line.split(':').next().map(|p| format!("package {}", p.trim()))
}
