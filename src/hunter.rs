//! Hunter Mode: point at a program's window (or pick it from the running programs) and act on
//! it: uninstall, stop it from auto-starting, kill it, kill and delete it, open its folder.

use crate::autorun::{self, AutorunEntry};
use crate::platform::Proc;
use crate::uninstaller::Program;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub pid: u32,
    pub name: String,
    pub exe: Option<String>,
    pub title: Option<String>,
    pub publisher: Option<String>,
    /// The installed program this executable belongs to.
    pub program: Option<Program>,
    /// Autorun entries that start this executable.
    pub autoruns: Vec<AutorunEntry>,
    /// Every running process of the same executable.
    pub pids: Vec<u32>,
}

/// Desktop programs (with `all`, every process of the graphical session), one per executable,
/// without ourselves.
pub fn gui_processes(all: bool) -> Vec<Proc> {
    let me = std::process::id();
    let own_exe = std::env::current_exe().ok().map(|p| p.display().to_string());
    let mut seen = std::collections::HashSet::new();
    let mut v: Vec<Proc> = crate::platform::list_processes()
        .into_iter()
        .filter(|p| (if all { p.gui } else { p.app }) && p.pid != me && p.exe.is_some() && p.exe != own_exe)
        .filter(|p| seen.insert(p.exe.clone()))
        .collect();
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

/// Installed program owning `exe`: by install folder, known executables, or (Linux) package.
pub fn owning_program(exe: &str, programs: &[Program]) -> Option<Program> {
    let l = exe.to_lowercase();
    if let Some(p) = programs.iter().find(|p| p.executables.iter().any(|e| e.to_lowercase() == l)) {
        return Some(p.clone());
    }
    if let Some(p) = programs
        .iter()
        .filter(|p| p.install_location.as_deref().is_some_and(|loc| loc.len() > 3 && l.starts_with(&loc.to_lowercase())))
        .max_by_key(|p| p.install_location.as_ref().map(|s| s.len()))
    {
        return Some(p.clone());
    }
    // /snap/<name>/… and packaged files
    if let Some(rest) = exe.strip_prefix("/snap/") {
        let name = rest.split('/').next().unwrap_or("");
        return programs.iter().find(|p| p.kind == crate::uninstaller::ProgramKind::Snap && p.package.as_deref() == Some(name)).cloned();
    }
    let pkg = crate::platform::file_publisher(exe)?.strip_prefix("package ")?.to_string();
    programs.iter().find(|p| p.package.as_deref() == Some(pkg.as_str())).cloned()
}

/// Flatpak apps run from /app inside their sandbox: map the process back to the app id.
fn flatpak_app(pid: u32) -> Option<String> {
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let i = cg.find("app-flatpak-")?;
    let rest = &cg[i + "app-flatpak-".len()..];
    let id = rest.split('-').next()?;
    Some(id.to_string())
}

pub fn identify(pid: u32, settings: &crate::config::Settings) -> Result<Target> {
    let procs = crate::platform::list_processes();
    let p = procs.iter().find(|p| p.pid == pid).cloned().with_context(|| format!("no process {pid}"))?;
    let mut s = settings.clone();
    s.show_system_components = true;
    // The two slow lookups run side by side.
    let (programs, all_autoruns) = std::thread::scope(|sc| {
        let a = sc.spawn(|| autorun::list(true));
        (crate::uninstaller::list_programs(&s), a.join().unwrap_or_default())
    });
    let program = if cfg!(windows) {
        p.exe.as_deref().and_then(|e| owning_program(e, &programs))
    } else {
        flatpak_app(pid)
            .and_then(|id| programs.iter().find(|x| x.package.as_deref() == Some(id.as_str())).cloned())
            .or_else(|| p.exe.as_deref().and_then(|e| owning_program(e, &programs)))
    };
    let exe_l = p.exe.as_deref().map(str::to_lowercase);
    let autoruns = all_autoruns
        .into_iter()
        .filter(|a| exe_l.as_ref().is_some_and(|e| a.exe.as_deref().map(str::to_lowercase).as_ref() == Some(e) || a.command.to_lowercase().contains(e.as_str())))
        .collect();
    let pids = procs.iter().filter(|q| q.exe.is_some() && q.exe == p.exe).map(|q| q.pid).collect();
    Ok(Target {
        pid,
        publisher: p.exe.as_deref().and_then(crate::platform::file_publisher),
        name: p.name.clone(),
        exe: p.exe.clone(),
        title: p.title.clone(),
        program,
        autoruns,
        pids,
    })
}

/// The program whose window is under the mouse pointer.
pub fn identify_at_pointer(settings: &crate::config::Settings) -> Result<Option<Target>> {
    match crate::platform::window_at_pointer()? {
        Some(pid) if pid != std::process::id() => identify(pid, settings).map(Some),
        _ => Ok(None),
    }
}

/// Disable every autorun entry that starts the target.
pub fn stop_autostart(t: &Target) -> Result<usize> {
    let mut n = 0;
    for a in t.autoruns.iter().filter(|a| a.enabled) {
        autorun::set_enabled(a, false)?;
        n += 1;
    }
    Ok(n)
}

pub fn kill(t: &Target) -> Result<usize> {
    let mut n = 0;
    for pid in &t.pids {
        crate::platform::kill_process(*pid)?;
        n += 1;
    }
    Ok(n)
}

/// Kill, then delete the executable (backed up).
pub fn kill_and_delete(t: &Target, settings: &crate::config::Settings) -> Result<String> {
    let exe = t.exe.clone().context("unknown executable")?;
    kill(t)?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    let issue = crate::model::Issue::new("hunter", exe.clone(), "program file", crate::model::Fix::DeleteFile { path: exe.clone().into() });
    let mut s = settings.clone();
    s.restore_point_before_clean = false;
    s.full_backup_before_clean = false;
    let rep = crate::cleaner::clean(&s, &[issue])?;
    if let Some((_, e)) = rep.failed.first() {
        bail!("{e}");
    }
    Ok(rep.backup_id)
}

/// Open a web search for the program (its installed name, else the executable's name).
pub fn search_online(t: &Target) -> Result<()> {
    let what = t
        .program
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| t.exe.as_deref().and_then(|e| Path::new(e).file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or(t.name.clone()));
    let q: String = what.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    crate::platform::open_folder(Path::new(&format!("https://duckduckgo.com/?q={q}")))
}

pub fn open_folder(t: &Target) -> Result<()> {
    let exe = t.exe.as_deref().context("unknown executable")?;
    crate::platform::open_folder(Path::new(exe).parent().unwrap_or(Path::new(exe)))
}
