//! Linux uninstaller backend.
//!
//! Inventory: desktop applications mapped to their owning deb / rpm / pacman package, plus
//! Flatpak apps, Snaps, AppImages and unpackaged apps with a user launcher. "Show system
//! components" adds every package. Removal goes through the package manager (pkexec / sudo).
//! Leftovers are the per-user config / data / cache / state folders, launchers, autostart
//! entries, icons, systemd user units, file associations, dconf settings and residual package
//! configuration, never anything that belongs to a program that is still installed.

use crate::model::{Fix, Issue, ScanMode, Scope};
use crate::pathutil::{desktop_exec_program, which, PathCache};
use crate::uninstaller::{matches, Program, ProgramKind, RunMode, RunResult, Target};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CAT_SETTINGS: &str = "leftover_registry";
pub const CAT_FILES: &str = "leftover_files";

// ---------------------------------------------------------------- helpers

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn has(cmd: &str) -> bool {
    which(cmd, &PathCache::default()).is_some()
}

fn mtime_date(p: &Path) -> Option<NaiveDate> {
    let t = fs::metadata(p).and_then(|m| m.modified()).ok()?;
    Some(DateTime::<Local>::from(t).date_naive())
}

/// "1.2 GB" / "340 MB" / "12.3 MiB" → bytes.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().replace('\u{a0}', " ");
    let (num, unit) = s.split_once(' ').unwrap_or((&s, "B"));
    let n: f64 = num.replace(',', ".").parse().ok()?;
    let mult = match unit.trim().to_ascii_uppercase().as_str() {
        "B" | "BYTES" => 1.0,
        "KB" | "KIB" | "K" => 1024.0,
        "MB" | "MIB" | "M" => 1024.0 * 1024.0,
        "GB" | "GIB" | "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

fn homes(scope: Scope) -> Vec<PathBuf> {
    let me: Vec<PathBuf> = dirs::home_dir().into_iter().collect();
    if scope == Scope::CurrentUser {
        return me;
    }
    let mut out: Vec<PathBuf> = fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let uid: u32 = f.get(2)?.parse().ok()?;
            let home = PathBuf::from(f.get(5)?);
            ((1000..60000).contains(&uid) && home.is_dir()).then_some(home)
        })
        .collect();
    for h in me {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

#[derive(Default, Debug, Clone)]
struct Desktop {
    path: PathBuf,
    name: String,
    comment: String,
    exec: Option<String>,
    icon: Option<String>,
    hidden: bool,
    flatpak: Option<String>,
    snap: Option<String>,
}

fn parse_desktop(path: &Path) -> Option<Desktop> {
    let text = fs::read_to_string(path).ok()?;
    let mut d = Desktop { path: path.to_path_buf(), ..Default::default() };
    let mut main = false;
    let mut app = true;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        if !main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "Name" => d.name = v.to_string(),
            "Comment" if d.comment.is_empty() => d.comment = v.to_string(),
            "Exec" => d.exec = Some(v.to_string()),
            "Icon" => d.icon = Some(v.to_string()),
            "NoDisplay" | "Hidden" => d.hidden |= v.eq_ignore_ascii_case("true"),
            "Type" => app = v == "Application",
            "X-Flatpak" => d.flatpak = Some(v.to_string()),
            "X-SnapInstanceName" => d.snap = Some(v.to_string()),
            _ => {}
        }
    }
    (app && !d.name.is_empty()).then_some(d)
}

fn app_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into())
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|d| PathBuf::from(d).join("applications"))
        .collect();
    for d in ["/usr/share/applications", "/usr/local/share/applications", "/var/lib/snapd/desktop/applications"] {
        let p = PathBuf::from(d);
        if !v.contains(&p) {
            v.push(p);
        }
    }
    if let Some(data) = dirs::data_dir() {
        v.push(data.join("applications"));
    }
    v
}

fn desktop_entries() -> Vec<Desktop> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in app_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "desktop") && seen.insert(p.file_name().unwrap().to_os_string()) {
                if let Some(d) = parse_desktop(&p) {
                    out.push(d);
                }
            }
        }
    }
    out
}

fn exec_path(d: &Desktop) -> Option<String> {
    let prog = desktop_exec_program(d.exec.as_deref()?)?;
    if prog.starts_with('/') {
        return Some(prog);
    }
    which(&prog, &PathCache::default()).map(|p| p.display().to_string())
}

// ---------------------------------------------------------------- package managers

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pm {
    Dpkg,
    Rpm,
    Pacman,
}

fn package_manager() -> Option<Pm> {
    if has("dpkg-query") {
        Some(Pm::Dpkg)
    } else if has("rpm") && !has("pacman") {
        Some(Pm::Rpm)
    } else if has("pacman") {
        Some(Pm::Pacman)
    } else {
        None
    }
}

/// Owning package of each path (one batched query).
fn owners(pm: Pm, paths: &[PathBuf]) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    if paths.is_empty() {
        return out;
    }
    let args: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    match pm {
        Pm::Dpkg => {
            let o = Command::new("dpkg-query").arg("-S").args(&args).output();
            for line in o.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default().lines() {
                // "pkg1, pkg2: /path" or "pkg:amd64: /path"
                if let Some((pkgs, path)) = line.rsplit_once(": ") {
                    let pkg = pkgs.split(", ").next().unwrap_or(pkgs);
                    let pkg = pkg.split(':').next().unwrap_or(pkg);
                    out.insert(PathBuf::from(path.trim()), pkg.to_string());
                }
            }
        }
        Pm::Rpm | Pm::Pacman => {
            for p in paths {
                let o = if pm == Pm::Rpm { run("rpm", &["-qf", "--qf", "%{NAME}\\n", &p.display().to_string()]) } else { run("pacman", &["-Qqo", &p.display().to_string()]) };
                if let Some(name) = o.and_then(|s| s.lines().next().map(str::to_string)).filter(|s| !s.contains(' ')) {
                    out.insert(p.clone(), name);
                }
            }
        }
    }
    out
}

struct PkgInfo {
    version: String,
    size: Option<u64>,
    publisher: String,
    website: Option<String>,
    installed: Option<NaiveDate>,
    system: bool,
}

/// Details for the given packages, or for every installed package when `pkgs` is empty.
fn package_info(pm: Pm, pkgs: &[String]) -> HashMap<String, PkgInfo> {
    let mut out = HashMap::new();
    match pm {
        Pm::Dpkg => {
            let fmt = "${binary:Package}\t${Version}\t${Installed-Size}\t${Maintainer}\t${Homepage}\t${db:Status-Abbrev}\t${Priority}\t${Section}\n";
            let mut c = Command::new("dpkg-query");
            c.args(["-W", "-f", fmt]).args(pkgs);
            let text = c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            for l in text.lines() {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 8 || !f[5].starts_with("ii") {
                    continue;
                }
                let name = f[0].split(':').next().unwrap_or(f[0]).to_string();
                let list = ["", ":amd64", ":arm64", ":i386", ":all"].iter().map(|a| PathBuf::from(format!("/var/lib/dpkg/info/{name}{a}.list"))).find(|p| p.exists());
                out.insert(
                    name,
                    PkgInfo {
                        version: f[1].to_string(),
                        size: f[2].parse::<u64>().ok().map(|k| k * 1024),
                        publisher: f[3].split('<').next().unwrap_or("").trim().to_string(),
                        website: (!f[4].is_empty()).then(|| f[4].to_string()),
                        installed: list.as_deref().and_then(mtime_date),
                        system: matches!(f[6], "required" | "important" | "standard") || matches!(f[7], "libs" | "oldlibs" | "kernel" | "admin" | "libdevel"),
                    },
                );
            }
        }
        Pm::Rpm => {
            let fmt = "%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{SIZE}\\t%{VENDOR}\\t%{URL}\\t%{INSTALLTIME}\\t%{GROUP}\\n";
            let mut c = Command::new("rpm");
            if pkgs.is_empty() {
                c.arg("-qa");
            } else {
                c.arg("-q");
            }
            c.args(["--qf", fmt]).args(pkgs);
            let text = c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            for l in text.lines() {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 7 {
                    continue;
                }
                out.insert(
                    f[0].to_string(),
                    PkgInfo {
                        version: f[1].to_string(),
                        size: f[2].parse().ok(),
                        publisher: f[3].replace("(none)", ""),
                        website: (!f[4].is_empty() && f[4] != "(none)").then(|| f[4].to_string()),
                        installed: f[5].parse::<i64>().ok().and_then(|t| DateTime::from_timestamp(t, 0)).map(|d| d.with_timezone(&Local).date_naive()),
                        system: f[6].starts_with("System") || f[0].starts_with("lib"),
                    },
                );
            }
        }
        Pm::Pacman => {
            let mut c = Command::new("pacman");
            c.arg("-Qi").args(pkgs);
            let text = c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            for block in text.split("\n\n") {
                let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).map(|r| r.trim_start_matches([' ', ':']).trim().to_string()));
                let Some(name) = get("Name") else { continue };
                out.insert(
                    name.clone(),
                    PkgInfo {
                        version: get("Version").unwrap_or_default(),
                        size: get("Installed Size").and_then(|s| parse_size(&s)),
                        publisher: get("Packager").map(|p| p.split('<').next().unwrap_or("").trim().to_string()).unwrap_or_default(),
                        website: get("URL").filter(|u| u != "None"),
                        installed: None,
                        system: get("Install Reason").is_some_and(|r| r.contains("dependency")) || name.starts_with("lib"),
                    },
                );
            }
        }
    }
    out
}

fn pm_kind(pm: Pm) -> ProgramKind {
    match pm {
        Pm::Dpkg => ProgramKind::Deb,
        Pm::Rpm => ProgramKind::Rpm,
        Pm::Pacman => ProgramKind::Pacman,
    }
}

fn pm_prefix(k: ProgramKind) -> &'static str {
    match k {
        ProgramKind::Deb => "deb",
        ProgramKind::Rpm => "rpm",
        ProgramKind::Pacman => "pacman",
        _ => "pkg",
    }
}

// ---------------------------------------------------------------- inventory

pub fn list_programs(show_system: bool) -> Vec<Program> {
    // Flatpak and Snap are asked in the background while the package manager is queried.
    let flatpak =
        std::thread::spawn(|| has("flatpak").then(|| run("flatpak", &["list", "--app", "--columns=application,name,version,size,installation,origin"]).unwrap_or_default()));
    let snap = std::thread::spawn(|| has("snap").then(|| run("snap", &["list"]).unwrap_or_default()));
    let entries = desktop_entries();
    let home = dirs::home_dir().unwrap_or_default();
    let user_apps = home.join(".local/share/applications");
    let mut out: Vec<Program> = Vec::new();

    // Packaged desktop apps
    if let Some(pm) = package_manager() {
        let system_entries: Vec<&Desktop> = entries.iter().filter(|d| d.flatpak.is_none() && d.snap.is_none() && !d.path.starts_with(&user_apps)).collect();
        let owner = owners(pm, &system_entries.iter().map(|d| d.path.clone()).collect::<Vec<_>>());
        let mut by_pkg: HashMap<String, Vec<&Desktop>> = HashMap::new();
        for d in &system_entries {
            if let Some(pkg) = owner.get(&d.path) {
                by_pkg.entry(pkg.clone()).or_default().push(d);
            }
        }
        let pkgs: Vec<String> = by_pkg.keys().cloned().collect();
        let info = package_info(pm, if show_system { &[] } else { &pkgs });
        let kind = pm_kind(pm);
        for (pkg, inf) in &info {
            let ds = by_pkg.get(pkg);
            if ds.is_none() && !show_system {
                continue;
            }
            // Prefer the visible launcher whose file name matches the package.
            let main = ds.and_then(|v| v.iter().filter(|d| !d.hidden).min_by_key(|d| (!d.path.to_string_lossy().contains(pkg.as_str()), d.name.len())).or(v.first()).copied());
            let mut p = Program::new(format!("{}:{pkg}", pm_prefix(kind)), main.map(|d| d.name.clone()).unwrap_or_else(|| pkg.clone()), kind);
            p.version = inf.version.clone();
            p.size = inf.size;
            p.publisher = inf.publisher.clone();
            p.website = inf.website.clone();
            p.install_date = inf.installed;
            p.package = Some(pkg.clone());
            p.system = ds.is_none() || ds.is_some_and(|v| v.iter().all(|d| d.hidden)) || inf.system && ds.is_none();
            p.uninstall_string = Some(match kind {
                ProgramKind::Deb => format!("apt-get remove {pkg}"),
                ProgramKind::Rpm => format!("dnf remove {pkg}"),
                _ => format!("pacman -R {pkg}"),
            });
            if let Some(d) = main {
                p.icon = d.icon.clone();
                p.comment = (!d.comment.is_empty()).then(|| d.comment.clone());
                p.executables = ds.unwrap().iter().filter_map(|d| exec_path(d)).collect();
                p.executables.sort_unstable();
                p.executables.dedup();
            }
            if p.system && !show_system {
                continue;
            }
            out.push(p);
        }
    }

    // Flatpak
    if let Some(text) = flatpak.join().ok().flatten() {
        for l in text.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 {
                continue;
            }
            let (id, inst) = (f[0], f[4]);
            let mut p = Program::new(format!("flatpak:{id}:{inst}"), if f[1].is_empty() { id } else { f[1] }, ProgramKind::Flatpak);
            p.version = f[2].to_string();
            p.size = parse_size(f[3]);
            p.package = Some(id.to_string());
            p.user_scope = inst == "user";
            p.publisher = f.get(5).map(|o| o.to_string()).unwrap_or_default();
            let base = if p.user_scope { home.join(".local/share/flatpak/app") } else { PathBuf::from("/var/lib/flatpak/app") };
            p.install_location = Some(base.join(id).display().to_string());
            p.install_date = mtime_date(&base.join(id));
            p.uninstall_string = Some(format!("flatpak uninstall {}{id}", if p.user_scope { "--user " } else { "" }));
            p.icon = Some(id.to_string());
            out.push(p);
        }
    }

    // Snap
    if let Some(text) = snap.join().ok().flatten() {
        for l in text.lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 5 {
                continue;
            }
            let notes = f.get(5).copied().unwrap_or("");
            let system = notes.contains("base") || notes.contains("snapd") || notes.contains("core") || f[0].starts_with("gnome-") || f[0].starts_with("gtk-") || f[0] == "bare";
            if system && !show_system {
                continue;
            }
            let mut p = Program::new(format!("snap:{}", f[0]), f[0], ProgramKind::Snap);
            p.version = f[1].to_string();
            p.publisher = f[4].trim_end_matches('*').trim_end_matches('✓').to_string();
            p.package = Some(f[0].to_string());
            p.system = system;
            p.install_location = Some(format!("/snap/{}", f[0]));
            p.install_date = mtime_date(Path::new(&format!("/snap/{}/current", f[0])));
            p.size = fs::metadata(format!("/var/lib/snapd/snaps/{}_{}.snap", f[0], f[2])).ok().map(|m| m.len());
            p.uninstall_string = Some(format!("snap remove {}", f[0]));
            // Use the launcher's display name when there is one.
            if let Some(d) = entries.iter().find(|d| d.snap.as_deref() == Some(f[0])) {
                p.name = d.name.clone();
                p.icon = d.icon.clone();
            }
            out.push(p);
        }
    }

    // AppImages
    for dir in [home.join("Applications"), home.join("AppImages"), home.join(".local/bin"), home.join("Downloads"), home.join("Desktop"), PathBuf::from("/opt")] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("appimage")) {
                continue;
            }
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let name = stem
                .split(['-', '_'])
                .take_while(|w| !w.chars().next().is_some_and(|c| c.is_ascii_digit()) && !["x86", "x64", "amd64", "aarch64", "linux"].contains(&w.to_ascii_lowercase().as_str()))
                .collect::<Vec<_>>()
                .join(" ");
            let mut p = Program::new(format!("appimage:{}", path.display()), if name.is_empty() { stem.clone() } else { name }, ProgramKind::AppImage);
            p.size = e.metadata().ok().map(|m| m.len());
            p.install_date = mtime_date(&path);
            p.install_location = Some(path.display().to_string());
            p.executables = vec![path.display().to_string()];
            p.uninstall_string = Some(format!("move {} to Trash", path.display()));
            p.user_scope = path.starts_with(&home);
            out.push(p);
        }
    }

    // Unpackaged apps with a user launcher (e.g. extracted to /opt or ~/.local/opt)
    let known: HashSet<String> = out.iter().flat_map(|p| p.executables.clone()).collect();
    for d in entries.iter().filter(|d| d.path.starts_with(&user_apps) && !d.hidden && d.flatpak.is_none() && d.snap.is_none()) {
        let Some(exe) = exec_path(d) else { continue };
        if known.contains(&exe) || exe.starts_with("/usr/") || exe.starts_with("/bin/") || exe.to_ascii_lowercase().ends_with(".appimage") {
            continue;
        }
        // ~/.local/zed.app/bin/zed → ~/.local/zed.app
        let mut dir = Path::new(&exe).parent().map(Path::to_path_buf);
        if dir.as_ref().is_some_and(|d| d.file_name().is_some_and(|n| n == "bin")) {
            dir = dir.and_then(|d| d.parent().map(Path::to_path_buf));
        }
        let dir = dir.filter(|d| d != &home && d.components().count() > 2).map(|d| d.display().to_string());
        let mut p = Program::new(format!("manual:{}", d.path.display()), &d.name, ProgramKind::Manual);
        p.install_location = dir;
        p.executables = vec![exe];
        p.icon = d.icon.clone();
        p.comment = (!d.comment.is_empty()).then(|| d.comment.clone());
        p.install_date = mtime_date(&d.path);
        p.user_scope = true;
        out.push(p);
    }
    out
}

pub fn program_exists(p: &Program) -> bool {
    let pkg = p.package.as_deref().unwrap_or("");
    match p.kind {
        ProgramKind::Deb => run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}", pkg]).is_some_and(|s| s.starts_with("ii")),
        ProgramKind::Rpm => run("rpm", &["-q", pkg]).is_some(),
        ProgramKind::Pacman => run("pacman", &["-Q", pkg]).is_some(),
        ProgramKind::Flatpak => run("flatpak", &["info", pkg]).is_some(),
        ProgramKind::Snap => run("snap", &["list", pkg]).is_some(),
        ProgramKind::AppImage | ProgramKind::Manual => p.install_location.as_deref().is_some_and(|l| Path::new(l).exists()),
        _ => false,
    }
}

pub fn run_uninstaller(p: &Program, mode: RunMode) -> Result<RunResult> {
    let pkg = p.package.clone().unwrap_or_default();
    // Package removal runs as root through the one administrator helper (one password prompt
    // for the whole session, however many programs are removed).
    let as_root: Option<Vec<&str>> = match p.kind {
        ProgramKind::Deb if has("apt-get") => Some(vec!["apt-get", "remove", "-y", &pkg]),
        ProgramKind::Deb => Some(vec!["dpkg", "-r", &pkg]),
        ProgramKind::Rpm if has("dnf") => Some(vec!["dnf", "remove", "-y", &pkg]),
        ProgramKind::Rpm if has("zypper") => Some(vec!["zypper", "-n", "remove", &pkg]),
        ProgramKind::Rpm => Some(vec!["rpm", "-e", &pkg]),
        ProgramKind::Pacman => Some(vec!["pacman", "-R", "--noconfirm", &pkg]),
        ProgramKind::Snap => Some(vec!["snap", "remove", &pkg]),
        _ => None,
    };
    if let Some(args) = as_root {
        let code = crate::elevate::run_as_root(&args)?;
        return Ok(RunResult { command: args.join(" "), exit_code: code });
    }
    let mut cmd = match p.kind {
        ProgramKind::Flatpak => {
            let mut c = Command::new("flatpak");
            c.args(["uninstall", "-y", "--noninteractive", if p.user_scope { "--user" } else { "--system" }, &pkg]);
            c
        }
        ProgramKind::AppImage => {
            let path = p.install_location.clone().context("AppImage path unknown")?;
            super::files::delete_path(Path::new(&path), true)?;
            return Ok(RunResult { command: format!("Moved {path} to Trash"), exit_code: Some(0) });
        }
        _ => bail!("\"{}\" has no built-in uninstaller; use Forced Uninstall to remove it", p.name),
    };
    let _ = mode; // package managers are already non-interactive
    let shown = format!("{:?}", cmd).replace('"', "");
    let st = cmd.status().with_context(|| format!("starting {shown}"))?;
    Ok(RunResult { command: shown, exit_code: st.code() })
}

/// Remove a package completely (configuration included). Used for residual deb config and for
/// packages a traced installer pulled in.
pub fn purge(manager: &str, name: &str) -> Result<()> {
    let args: Vec<&str> = match manager {
        "dpkg" if has("apt-get") => vec!["apt-get", "purge", "-y", name],
        "dpkg" => vec!["dpkg", "--purge", name],
        "rpm" if has("dnf") => vec!["dnf", "remove", "-y", name],
        "rpm" if has("zypper") => vec!["zypper", "-n", "remove", name],
        "rpm" => vec!["rpm", "-e", name],
        "pacman" => vec!["pacman", "-Rns", "--noconfirm", name],
        "flatpak" => {
            let st = Command::new("flatpak").args(["uninstall", "-y", "--noninteractive", "--delete-data", name]).status()?;
            if !st.success() {
                bail!("purge {name} failed");
            }
            return Ok(());
        }
        "snap" => vec!["snap", "remove", "--purge", name],
        other => bail!("removing {other} packages is not supported"),
    };
    if crate::elevate::run_as_root(&args)? != Some(0) {
        bail!("purge {name} failed");
    }
    Ok(())
}

// ---------------------------------------------------------------- leftovers

struct Scan<'a> {
    tokens: Vec<String>,
    /// Tokens of programs that are still installed: never treat those as leftovers.
    others: HashSet<String>,
    other_paths: Vec<PathBuf>,
    target: &'a Target,
    loose: bool,
    out: Vec<Issue>,
}

impl Scan<'_> {
    fn hit(&self, name: &str) -> bool {
        let n = crate::uninstaller::normalize(name);
        if self.others.contains(&n) {
            return false;
        }
        if self.loose {
            crate::uninstaller::matches_loose_normalized(&n, &self.tokens)
        } else {
            crate::uninstaller::matches_normalized(&n, &self.tokens)
        }
    }

    fn owned_by_other(&self, p: &Path) -> bool {
        self.other_paths.iter().any(|o| p.starts_with(o) || o.starts_with(p))
    }

    fn file(&mut self, p: &Path, what: &str) {
        if self.owned_by_other(p) {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(p) else {
            return;
        };
        let size = crate::backup::dir_size(p);
        let needs_root = fs::metadata(p.parent().unwrap_or(p)).is_ok_and(|m| m.uid() != fs::metadata("/proc/self").map(|s| s.uid()).unwrap_or(0));
        let detail = format!("{what} · {}{}", crate::backup::human_size(size), if needs_root { " · needs administrator" } else { "" });
        let fix = if meta.is_dir() && !meta.file_type().is_symlink() { Fix::DeleteDirTree { path: p.into() } } else { Fix::DeleteFile { path: p.into() } };
        self.out.push(Issue::new(CAT_FILES, p.display().to_string(), detail, fix));
    }

    /// Direct children of `dir` whose name matches.
    fn children(&mut self, dir: &Path, what: &str, strip_dot: bool) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if strip_dot && !name.starts_with('.') {
                continue;
            }
            let bare = name.trim_start_matches('.');
            let bare = bare.strip_suffix("rc").filter(|b| b.len() >= 3).unwrap_or(bare);
            let bare = bare.split('.').next().filter(|_| !bare.contains(' ') && strip_dot).unwrap_or(bare);
            if self.hit(&name) || self.hit(bare) {
                self.file(&e.path(), what);
            }
        }
    }

    fn refers_to_target(&self, text: &str) -> bool {
        self.target.paths.iter().any(|p| text.contains(p.as_str())) || self.target.executables.iter().any(|e| text.contains(e.as_str()))
    }
}

pub fn scan_leftovers(target: &Target, mode: ScanMode, scope: Scope, others: &[Program]) -> Vec<Issue> {
    let mut other_tokens: HashSet<String> = others.iter().flat_map(|o| Target::from_program(o).tokens()).collect();
    let tokens = target.tokens();
    // A name that is ours wins over the same name of another program only if it came from
    // our own package/app id (e.g. both "gedit" and "gedit-plugins" are installed).
    if let Some(pkg) = &target.package {
        other_tokens.remove(&crate::uninstaller::normalize(pkg));
    }
    let other_paths: Vec<PathBuf> = others.iter().filter_map(|o| o.install_location.clone()).filter(|l| l.len() > 4).map(PathBuf::from).collect();
    let mut s = Scan { tokens, others: other_tokens, other_paths, target, loose: mode == ScanMode::Advanced, out: Vec::new() };

    // The program's own files, if any remain. Package-managed folders (/snap, Flatpak, /usr)
    // are only ever removed by the package manager.
    let own_files = matches!(target.kind, None | Some(ProgramKind::Manual) | Some(ProgramKind::AppImage));
    for p in target.paths.iter().filter(|_| own_files) {
        let path = Path::new(p);
        let is_system_dir = ["/usr", "/usr/bin", "/usr/lib", "/usr/share", "/opt", "/etc", "/", "/home"].iter().any(|d| path == Path::new(d));
        if path.exists() && !is_system_dir && dirs::home_dir().as_deref() != Some(path) {
            s.file(path, "program files");
        }
    }

    for home in homes(scope) {
        let cfg = home.join(".config");
        let data = home.join(".local/share");
        // Safe
        s.children(&cfg, "settings folder", false);
        s.children(&data, "data folder", false);
        s.children(&home.join(".cache"), "cache", false);
        s.children(&home.join(".local/state"), "state folder", false);
        if target.kind == Some(ProgramKind::Flatpak) {
            if let Some(id) = &target.package {
                let p = home.join(".var/app").join(id);
                if p.exists() {
                    s.file(&p, "Flatpak app data");
                }
            }
        }
        if target.kind == Some(ProgramKind::Snap) {
            if let Some(n) = &target.package {
                let p = home.join("snap").join(n);
                if p.exists() {
                    s.file(&p, "Snap data");
                }
            }
        }
        for (dir, what) in [(data.join("applications"), "launcher"), (cfg.join("autostart"), "autostart entry")] {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x == "desktop") {
                    continue;
                }
                let stem = p.file_stem().unwrap().to_string_lossy().to_string();
                let stem_name =
                    stem.trim_start_matches("appimagekit_").rsplit_once('-').filter(|_| stem.starts_with("appimagekit_")).map(|(_, n)| n.to_string()).unwrap_or(stem.clone());
                let text = fs::read_to_string(&p).unwrap_or_default();
                if s.hit(&stem) || s.hit(&stem_name) || s.refers_to_target(&text) {
                    s.file(&p, what);
                }
            }
        }
        if mode >= ScanMode::Moderate {
            s.children(&home, "dotfile", true);
            // Icons installed for the user
            let icons = data.join("icons/hicolor");
            if let Ok(sizes) = fs::read_dir(&icons) {
                for sz in sizes.flatten() {
                    s.children(&sz.path().join("apps"), "icon", false);
                }
            }
            // systemd user units
            let units = cfg.join("systemd/user");
            if let Ok(rd) = fs::read_dir(&units) {
                for e in rd.flatten() {
                    let p = e.path();
                    let stem = p.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
                    if e.file_type().is_ok_and(|t| t.is_file()) && (s.hit(&stem) || s.refers_to_target(&fs::read_to_string(&p).unwrap_or_default())) {
                        s.file(&p, "systemd user service");
                    }
                }
            }
            // File associations pointing to the app's launchers
            for list in [cfg.join("mimeapps.list"), data.join("applications/mimeapps.list")] {
                let Ok(text) = fs::read_to_string(&list) else {
                    continue;
                };
                let mut ids: Vec<String> = text
                    .lines()
                    .filter_map(|l| l.split_once('=').map(|(_, v)| v))
                    .flat_map(|v| v.split(';').map(str::trim).filter(|x| x.ends_with(".desktop")).map(str::to_string).collect::<Vec<_>>())
                    .collect();
                ids.sort();
                ids.dedup();
                for id in ids {
                    if s.hit(id.trim_end_matches(".desktop")) {
                        s.out.push(Issue::new(
                            CAT_SETTINGS,
                            format!("{} | {id}", list.display()),
                            "file association",
                            Fix::RemoveMimeHandler { file: list.clone(), desktop_id: id },
                        ));
                    }
                }
            }
        }
        if mode == ScanMode::Advanced {
            // Deeper look: matching folders up to three levels below config/data/Documents.
            let mut found = Vec::new();
            for root in [cfg.clone(), data.clone(), home.join("Documents"), home.join(".local/opt"), home.join("opt")] {
                walk_dirs(&root, 3, &mut |p| {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if matches(&name, &s.tokens) && !s.others.contains(&crate::uninstaller::normalize(&name)) {
                        found.push(p.to_path_buf());
                        return false; // don't descend into a match
                    }
                    true
                });
            }
            for p in found {
                s.file(&p, "folder");
            }
        }
    }

    // GNOME settings (dconf), per current user only
    if mode >= ScanMode::Moderate && has("dconf") {
        if let Some(dump) = run("dconf", &["dump", "/"]) {
            let mut dirs_found: Vec<String> = Vec::new();
            for line in dump.lines() {
                let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
                    continue;
                };
                let segs: Vec<&str> = sec.split('/').filter(|x| !x.is_empty() && *x != "/").collect();
                for (i, seg) in segs.iter().enumerate() {
                    // Skip vendor prefixes (org/gnome/…, com/…): they are generic.
                    if s.hit(seg) {
                        let dir = format!("/{}/", segs[..=i].join("/"));
                        if !dirs_found.iter().any(|d| dir.starts_with(d.as_str())) {
                            dirs_found.retain(|d| !d.starts_with(&dir));
                            dirs_found.push(dir);
                        }
                        break;
                    }
                }
            }
            for d in dirs_found {
                s.out.push(Issue::new(CAT_SETTINGS, format!("dconf:{d}"), "GNOME settings", Fix::DconfResetDir { dir: d }));
            }
        }
    }

    // Residual package configuration (deb "rc" state)
    if mode >= ScanMode::Moderate && target.kind == Some(ProgramKind::Deb) {
        if let Some(pkg) = &target.package {
            if run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}", pkg]).is_some_and(|st| st.starts_with("rc")) {
                s.out.push(Issue::new(
                    CAT_SETTINGS,
                    format!("dpkg:{pkg}"),
                    "configuration files left by the package · needs administrator",
                    Fix::PurgePackage { manager: "dpkg".into(), name: pkg.clone() },
                ));
            }
        }
    }
    if mode == ScanMode::Advanced {
        // Unpackaged system-wide copies (/opt/<app>, /usr/local/share/<app>), only if no package owns them.
        let mut cand = Vec::new();
        for root in ["/opt", "/usr/local/share", "/usr/local/lib"] {
            if let Ok(rd) = fs::read_dir(root) {
                for e in rd.flatten() {
                    if s.hit(&e.file_name().to_string_lossy()) {
                        cand.push(e.path());
                    }
                }
            }
        }
        let owned = package_manager().map(|pm| owners(pm, &cand)).unwrap_or_default();
        for p in cand.into_iter().filter(|p| !owned.contains_key(p)) {
            s.file(&p, "system-wide program files");
        }
    }
    s.out
}

/// Visit directories below `root` (not following symlinks); `f` returns whether to descend.
fn walk_dirs(root: &Path, depth: usize, f: &mut dyn FnMut(&Path) -> bool) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let p = e.path();
        if f(&p) && depth > 1 {
            walk_dirs(&p, depth - 1, f);
        }
    }
}

/// Start a Flatpak / Snap app.
pub fn app_run(p: &Program) -> Result<()> {
    let pkg = p.package.clone().context("package name unknown")?;
    let (prog, args): (&str, Vec<String>) = match p.kind {
        ProgramKind::Flatpak => ("flatpak", vec!["run".into(), pkg]),
        ProgramKind::Snap => ("snap", vec!["run".into(), pkg]),
        _ => anyhow::bail!("only Flatpak and Snap apps can be started from here"),
    };
    Command::new(prog)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("starting {prog}"))?;
    Ok(())
}

/// Stop a Flatpak / Snap app's processes. Returns how many were stopped (Flatpak: 1 when it ran).
pub fn app_terminate(p: &Program) -> Result<usize> {
    match p.kind {
        ProgramKind::Flatpak => {
            let pkg = p.package.clone().context("package name unknown")?;
            let st = Command::new("flatpak").args(["kill", &pkg]).stderr(std::process::Stdio::null()).status()?;
            Ok(usize::from(st.success()))
        }
        _ => Ok(crate::uninstaller::stop_running(p).len()),
    }
}

pub fn reset_store() -> Result<()> {
    anyhow::bail!("resetting the Microsoft Store only applies to Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1.5 MB"), Some(1572864));
        assert_eq!(parse_size("12.3 MiB"), Some(12897484));
        assert_eq!(parse_size("2 GB"), Some(2147483648));
        assert_eq!(parse_size("abc"), None);
    }
}
