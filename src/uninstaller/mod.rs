//! Uninstaller: installed-program inventory, running the
//! built-in uninstaller, leftover scans in three depths, quick / multiple / forced uninstall,
//! groups and comments, uninstall history and list export.
//!
//! Leftovers are ordinary [`Issue`]s, so removal goes through [`crate::cleaner::clean`]: every
//! item is backed up before it is deleted and can be restored from Backups.

use crate::config::{data_dir, ensure_dir, Settings};
use crate::i18n::tr;
use crate::model::{Issue, ScanMode, Scope};
use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProgramKind {
    /// Windows desktop program with an `Uninstall` registry entry.
    Win32,
    /// Windows Installer (MSI) program.
    Msi,
    /// Windows Store / built-in app (AppX / MSIX).
    WindowsApp,
    Deb,
    Rpm,
    Pacman,
    Flatpak,
    Snap,
    AppImage,
    /// Linux app with no package (e.g. extracted to /opt or ~/.local).
    Manual,
}

impl ProgramKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Win32 => "Win32",
            Self::Msi => "MSI",
            Self::WindowsApp => "Windows App",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Pacman => "pacman",
            Self::Flatpak => "Flatpak",
            Self::Snap => "Snap",
            Self::AppImage => "AppImage",
            Self::Manual => "Manual",
        }
    }

    /// Store-style apps get their own tab (Windows Apps / Flatpak & Snap).
    pub fn is_store_app(self) -> bool {
        matches!(self, Self::WindowsApp | Self::Flatpak | Self::Snap)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    /// Stable id: `win32:<uninstall key>`, `deb:<package>`, `flatpak:<app id>`, …
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub kind: ProgramKind,
    pub install_date: Option<NaiveDate>,
    pub size: Option<u64>,
    pub install_location: Option<String>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub modify_path: Option<String>,
    /// Windows: the full `Uninstall\…` registry key. Linux: unused.
    pub registry_key: Option<String>,
    /// Package name, Flatpak ref, MSI product code or AppX package full name.
    pub package: Option<String>,
    pub website: Option<String>,
    pub help_link: Option<String>,
    pub update_link: Option<String>,
    pub icon: Option<String>,
    pub comment: Option<String>,
    /// System component, update or library (hidden unless "show system components").
    pub system: bool,
    /// Installed for the current user only.
    pub user_scope: bool,
    /// Main executables (used to find leftovers that reference the program).
    pub executables: Vec<String>,
}

impl Program {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: ProgramKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: String::new(),
            publisher: String::new(),
            kind,
            install_date: None,
            size: None,
            install_location: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            registry_key: None,
            package: None,
            website: None,
            help_link: None,
            update_link: None,
            icon: None,
            comment: None,
            system: false,
            user_scope: false,
            executables: Vec::new(),
        }
    }

    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.name, self.publisher, self.comment.as_deref().unwrap_or("")).to_lowercase()
    }

    /// Windows: 64 or 32 for a program registered machine-wide, by the registry view its
    /// uninstall entry is in (WOW6432Node is the 32-bit one). Per-user entries and other
    /// systems have no such split.
    pub fn bits(&self) -> Option<u8> {
        let key = self.registry_key.as_deref()?.to_ascii_uppercase();
        if !key.starts_with("HKLM") && !key.starts_with("HKEY_LOCAL_MACHINE") {
            return None;
        }
        Some(if key.contains(r"\WOW6432NODE\") { 32 } else { 64 })
    }

    /// The MSI product code, if this is (or wraps) an MSI install.
    pub fn msi_product_code(&self) -> Option<String> {
        let from = |s: &str| {
            let i = s.find('{')?;
            let j = s[i..].find('}')? + i;
            let g = &s[i..=j];
            (g.len() == 38).then(|| g.to_string())
        };
        if self.kind == ProgramKind::Msi {
            if let Some(g) = self.registry_key.as_deref().and_then(|k| k.rsplit('\\').next()).and_then(from) {
                return Some(g);
            }
        }
        self.uninstall_string.as_deref().filter(|u| u.to_ascii_lowercase().contains("msiexec")).and_then(from)
    }
}

/// What a leftover scan looks for. Built from a [`Program`], or from the Forced Uninstall form.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    /// The program's own id, so it is never protected against itself.
    pub id: Option<String>,
    pub names: Vec<String>,
    pub publisher: Option<String>,
    /// Program folders and/or main executables.
    pub paths: Vec<String>,
    pub package: Option<String>,
    pub kind: Option<ProgramKind>,
    /// Windows: the program's own `Uninstall\…` key (always a leftover if it remains).
    pub registry_key: Option<String>,
    pub executables: Vec<String>,
}

impl Target {
    pub fn from_program(p: &Program) -> Self {
        let mut paths = Vec::new();
        if let Some(l) = &p.install_location {
            if !l.trim().is_empty() {
                paths.push(l.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_string());
            }
        }
        Self {
            id: Some(p.id.clone()),
            names: vec![p.name.clone()],
            publisher: (!p.publisher.trim().is_empty()).then(|| p.publisher.clone()),
            paths,
            package: p.package.clone(),
            kind: Some(p.kind),
            registry_key: p.registry_key.clone(),
            executables: p.executables.clone(),
        }
    }

    /// Normalized words a leftover's name may equal (see [`matches`]).
    pub fn tokens(&self) -> Vec<String> {
        let mut t: Vec<String> = Vec::new();
        let mut add = |s: &str| {
            let n = normalize(s);
            if n.len() >= 3 && !GENERIC_NAMES.contains(&n.as_str()) && !t.contains(&n) {
                t.push(n);
            }
        };
        for n in &self.names {
            add(n);
            add(&strip_version(n));
        }
        for p in &self.paths {
            if let Some(last) = p.rsplit(['\\', '/']).find(|s| !s.is_empty()) {
                add(last.split('.').next().unwrap_or(last));
                add(last);
            }
        }
        for e in &self.executables {
            if let Some(f) = e.rsplit(['\\', '/']).next() {
                add(f.split('.').next().unwrap_or(f));
            }
        }
        if let Some(pkg) = &self.package {
            add(pkg);
            // Reverse-DNS app ids (org.gnome.Gedit): the last part is the name.
            if let Some(last) = pkg.rsplit('.').next() {
                add(last);
            }
        }
        t
    }
}

/// Folder / key names that must never be treated as belonging to one program.
const GENERIC_NAMES: &[&str] = &[
    "app",
    "apps",
    "bin",
    "lib",
    "data",
    "temp",
    "tmp",
    "cache",
    "config",
    "common",
    "commonfiles",
    "shared",
    "microsoft",
    "windows",
    "programfiles",
    "programdata",
    "users",
    "default",
    "public",
    "local",
    "roaming",
    "locallow",
    "software",
    "classes",
    "system",
    "setup",
    "install",
    "installer",
    "uninstall",
    "update",
    "updater",
    "share",
    "applications",
    "desktop",
    "documents",
    "downloads",
    "google",
    "mozilla",
    "apple",
    "intel",
    "nvidia",
    "amd",
    "adobe",
    "oracle",
    "x64",
    "x86",
    "win64",
    "win32",
    "64bit",
    "32bit",
    "free",
    "pro",
    "the",
    "inc",
    "ltd",
    "llc",
    "gmbh",
    "corporation",
    "corp",
    "company",
    "team",
    "project",
    "foundation",
    "packages",
    "flatpak",
    "snap",
    "gnome",
    "kde",
    "org",
    "com",
    "net",
    "io",
    "freedesktop",
    "steam",
    "python",
    "java",
    "node",
    "electron",
    "fonts",
    "icons",
];

/// Lower-case, letters and digits only: "Notepad++ 8.6 (64-bit)" → "notepad8664bit".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// "VLC media player 3.0.20 (x64)" → "VLC media player"
pub fn strip_version(name: &str) -> String {
    let mut words: Vec<&str> = name.split_whitespace().collect();
    // The first word is always the name ("7-Zip" starts with a digit but is not a version).
    while words.len() > 1 {
        let w = words.last().unwrap();
        let lw = w.trim_matches(['(', ')', '[', ']', ',']).to_ascii_lowercase();
        let versionish = lw.chars().next().is_some_and(|c| c.is_ascii_digit()) || lw.starts_with('v') && lw[1..].chars().next().is_some_and(|c| c.is_ascii_digit());
        if versionish || matches!(lw.as_str(), "x64" | "x86" | "64-bit" | "32-bit" | "amd64" | "arm64" | "bit" | "edition" | "") {
            words.pop();
        } else {
            break;
        }
    }
    if words.is_empty() {
        name.to_string()
    } else {
        words.join(" ")
    }
}

/// Does a folder / key name belong to the target? Exact normalized match only: a leftover named
/// "VLC" matches "VLC media player"'s token "vlc" only if that token exists (from the install
/// folder or executable name), never by substring.
pub fn matches(candidate: &str, tokens: &[String]) -> bool {
    matches_normalized(&normalize(candidate), tokens)
}

/// `matches` for a name already passed through `normalize`.
pub fn matches_normalized(n: &str, tokens: &[String]) -> bool {
    n.len() >= 3 && tokens.iter().any(|t| t == n)
}

/// Looser match used only by the Advanced scan: the candidate starts or ends with a long token.
pub fn matches_loose(candidate: &str, tokens: &[String]) -> bool {
    matches_loose_normalized(&normalize(candidate), tokens)
}

/// `matches_loose` for a name already passed through `normalize`.
pub fn matches_loose_normalized(n: &str, tokens: &[String]) -> bool {
    matches_normalized(n, tokens) || tokens.iter().any(|t| t.len() >= 6 && n.len() <= t.len() + 10 && (n.starts_with(t.as_str()) || n.ends_with(t.as_str())))
}

/// Silent variant of a Windows uninstall command ("Quick Uninstall").
pub fn quiet_command(p: &Program) -> Option<String> {
    if let Some(q) = p.quiet_uninstall_string.as_ref().filter(|q| !q.trim().is_empty()) {
        return Some(q.clone());
    }
    let u = p.uninstall_string.as_ref()?.trim().to_string();
    let lower = u.to_ascii_lowercase();
    if lower.contains("msiexec") {
        let code = p.msi_product_code()?;
        return Some(format!("MsiExec.exe /X{code} /qn /norestart"));
    }
    let exe = lower.trim_start_matches('"').split(".exe").next().unwrap_or("").rsplit(['\\', '/']).next().unwrap_or("").to_string();
    // Inno Setup: unins000.exe
    if exe.starts_with("unins") && exe[5..].chars().all(|c| c.is_ascii_digit()) && exe.len() > 5 {
        return Some(format!("{u} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"));
    }
    // NSIS: uninst.exe / uninstall.exe / Uninstall <name>.exe
    if exe.starts_with("uninst") || exe.starts_with("uninstall") {
        return Some(format!("{u} /S"));
    }
    None
}

/// Forced MSI uninstall (when the normal entry opens Change/Repair).
pub fn forced_msi_command(p: &Program) -> Option<String> {
    p.msi_product_code().map(|c| format!("MsiExec.exe /X{c}"))
}

/// `*` and `?` wildcard match, case-insensitive (`programs --match`, /m).
pub use crate::pathutil::wildcard;

// ---------------------------------------------------------------- inventory

/// All installed programs, with comments from the metadata store applied, sorted by name.
pub fn list_programs(settings: &Settings) -> Vec<Program> {
    let mut v = crate::platform::list_programs(settings.show_system_components);
    let meta = Meta::load();
    for p in &mut v {
        if let Some(c) = meta.comments.get(&p.id) {
            p.comment = Some(c.clone());
        }
    }
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

// ---------------------------------------------------------------- groups and comments

pub const DEFAULT_GROUPS: [&str; 4] = ["Internet", "Games", "Multimedia", "Utilities"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub comments: BTreeMap<String, String>,
    /// Group name → program ids, in the user's group order.
    pub groups: Vec<(String, Vec<String>)>,
    /// Program id → icon chosen by the user (a key of the GUI's icon palette).
    #[serde(default)]
    pub icons: BTreeMap<String, String>,
}

impl Default for Meta {
    fn default() -> Self {
        Self { comments: BTreeMap::new(), groups: DEFAULT_GROUPS.iter().map(|g| (g.to_string(), Vec::new())).collect(), icons: BTreeMap::new() }
    }
}

fn uninst_dir() -> PathBuf {
    data_dir().join("uninstaller")
}

impl Meta {
    pub fn load() -> Self {
        std::fs::read_to_string(uninst_dir().join("meta.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        ensure_dir(&uninst_dir())?;
        std::fs::write(uninst_dir().join("meta.json"), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn set_comment(&mut self, id: &str, text: &str) {
        if text.trim().is_empty() {
            self.comments.remove(id);
        } else {
            self.comments.insert(id.to_string(), text.trim().to_string());
        }
    }

    pub fn add_to_group(&mut self, id: &str, group: &str) {
        if !self.groups.iter().any(|(g, _)| g == group) {
            self.groups.push((group.to_string(), Vec::new()));
        }
        for (g, ids) in &mut self.groups {
            if g == group && !ids.iter().any(|i| i == id) {
                ids.push(id.to_string());
            }
        }
    }

    pub fn remove_from_groups(&mut self, id: &str) {
        for (_, ids) in &mut self.groups {
            ids.retain(|i| i != id);
        }
    }

    // Groups manager

    pub fn create_group(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a group needs a name");
        anyhow::ensure!(!name.eq_ignore_ascii_case("New") && !self.groups.iter().any(|(g, _)| g.eq_ignore_ascii_case(name)), "a group named \"{name}\" already exists");
        self.groups.push((name.to_string(), Vec::new()));
        Ok(())
    }

    pub fn rename_group(&mut self, old: &str, new: &str) -> Result<()> {
        let new = new.trim();
        anyhow::ensure!(!new.is_empty(), "a group needs a name");
        anyhow::ensure!(!new.eq_ignore_ascii_case("New") && !self.groups.iter().any(|(g, _)| g != old && g.eq_ignore_ascii_case(new)), "a group named \"{new}\" already exists");
        let g = self.groups.iter_mut().find(|(g, _)| g == old).with_context(|| format!("no group \"{old}\""))?;
        g.0 = new.to_string();
        Ok(())
    }

    /// Delete a group; its programs just become ungrouped.
    pub fn delete_group(&mut self, name: &str) -> Result<()> {
        let before = self.groups.len();
        self.groups.retain(|(g, _)| g != name);
        anyhow::ensure!(self.groups.len() < before, "no group \"{name}\"");
        Ok(())
    }

    /// Move a group one place up (`-1`) or down (`1`) in the list.
    pub fn move_group(&mut self, name: &str, by: isize) {
        if let Some(i) = self.groups.iter().position(|(g, _)| g == name) {
            let j = i as isize + by;
            if j >= 0 && (j as usize) < self.groups.len() {
                self.groups.swap(i, j as usize);
            }
        }
    }

    pub fn set_icon(&mut self, id: &str, icon: Option<&str>) {
        match icon {
            Some(i) => self.icons.insert(id.to_string(), i.to_string()),
            None => self.icons.remove(id),
        };
    }

    pub fn group_of(&self, id: &str) -> Option<&str> {
        self.groups.iter().find(|(_, ids)| ids.iter().any(|i| i == id)).map(|(g, _)| g.as_str())
    }
}

/// Installed within the last `days` days (the "New Programs" group).
pub fn is_new(p: &Program, days: u32) -> bool {
    p.install_date.is_some_and(|d| (Local::now().date_naive() - d).num_days() <= days as i64)
}

// ---------------------------------------------------------------- uninstall flow

/// How a line of the uninstall report went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Done,
    Info,
    Problem,
}

/// A line of the uninstall report, in the interface language, with an optional detail shown
/// apart from it (a command or a path, which would scramble inside right-to-left text).
#[derive(Debug, Clone)]
pub struct Note {
    pub tone: Tone,
    pub text: String,
    pub detail: String,
}

impl Note {
    pub fn new(tone: Tone, text: impl Into<String>) -> Self {
        Self { tone, text: text.into(), detail: String::new() }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.text)
        } else {
            write!(f, "{} ({})", self.text, self.detail)
        }
    }
}

/// The restore point was asked for and could not be made: why.
#[derive(Debug, Clone)]
pub struct RestorePointFailed(pub String);

/// Before anything is changed: restore point and full registry backup, as configured.
/// Run once per uninstall session (once for a whole Multiple Uninstall queue). Stops before
/// anything else when the restore point fails: uninstalling without the protection the user
/// asked for is theirs to decide (`prepare_without_restore_point`).
pub fn prepare(settings: &Settings, label: &str) -> std::result::Result<Vec<Note>, RestorePointFailed> {
    let mut notes = Vec::new();
    if settings.uninstall_restore_point {
        crate::platform::create_restore_point(&format!("CleanCore: before uninstalling {label}")).map_err(|e| RestorePointFailed(format!("{e:#}")))?;
        notes.push(Note::new(Tone::Done, tr("Restore point created")));
    }
    if settings.uninstall_full_backup {
        let r = (|| -> Result<String> {
            let mut b = crate::backup::Backup::create(settings, &format!("Before uninstalling {label}"), "", crate::backup::BackupKind::Full)?;
            b.manifest.entries = crate::platform::backup_full(&b.dir)?;
            b.save()?;
            Ok(b.manifest.id)
        })();
        notes.push(match r {
            Ok(id) => Note::new(Tone::Done, crate::tf!("Full backup {id}")),
            Err(e) => Note::new(Tone::Problem, crate::tf!("Full backup failed: {e:#}")),
        });
    }
    Ok(notes)
}

/// `prepare` once the user chose to go on although the restore point failed.
pub fn prepare_without_restore_point(settings: &Settings, label: &str) -> Vec<Note> {
    let mut s = settings.clone();
    s.uninstall_restore_point = false;
    let mut notes = vec![Note::new(Tone::Problem, tr("Continued without a restore point"))];
    notes.extend(prepare(&s, label).unwrap_or_default());
    notes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// The program's own uninstaller, interactive.
    Normal,
    /// Silent where the installer technology allows it.
    Quiet,
    /// `msiexec /x {ProductCode}` instead of the registered (Change/Repair) command.
    ForcedMsi,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub command: String,
    pub exit_code: Option<i32>,
}

/// Run the built-in uninstaller (or package-manager removal) and wait for it to exit.
/// Many uninstallers hand off to a copy of themselves and exit early, so callers should
/// still let the user confirm before scanning (the GUI's "Scan" button).
pub fn run_builtin(p: &Program, mode: RunMode) -> Result<RunResult> {
    crate::platform::run_uninstaller(p, mode)
}

/// Close the program's running processes ("force-stop running executables"):
/// everything started from its install folder or one of its known executables. Returns the
/// names of what was stopped.
pub fn stop_running(p: &Program) -> Vec<String> {
    let loc = p.install_location.as_deref().map(|l| l.trim_end_matches(['/', '\\']).to_lowercase()).filter(|l| l.len() > 3);
    let exes: Vec<String> = p.executables.iter().map(|e| e.to_lowercase()).collect();
    let me = std::process::id();
    let mut stopped = Vec::new();
    for pr in crate::platform::list_processes() {
        let Some(exe) = pr.exe.as_deref().map(str::to_lowercase) else {
            continue;
        };
        let inside = loc.as_ref().is_some_and(|l| exe.starts_with(&format!("{l}/")) || exe.starts_with(&format!("{l}\\")));
        if pr.pid != me && (inside || exes.contains(&exe)) && crate::platform::kill_process(pr.pid).is_ok() {
            stopped.push(pr.name);
        }
    }
    stopped.sort();
    stopped.dedup();
    stopped
}

/// An uninstall whose leftover scan hasn't happened yet. It resumes after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUninstall {
    pub program: Option<Program>,
    pub label: String,
    pub version: String,
    pub target: Target,
    pub op: Operation,
    pub started: DateTime<Local>,
    /// The uninstaller asked for a restart (MSI exit code 3010 / 1641).
    #[serde(default)]
    pub reboot_needed: bool,
}

fn pending_file() -> PathBuf {
    data_dir().join("pending-uninstall.json")
}

pub fn pending() -> Option<PendingUninstall> {
    serde_json::from_str(&std::fs::read_to_string(pending_file()).ok()?).ok()
}

pub fn set_pending(p: &PendingUninstall) -> Result<()> {
    ensure_dir(&data_dir())?;
    std::fs::write(pending_file(), serde_json::to_string_pretty(p)?)?;
    Ok(())
}

/// Forget the interrupted uninstall (done, or dismissed) and its start-at-login entry.
pub fn clear_pending() {
    let _ = std::fs::remove_file(pending_file());
    let _ = resume_at_login(false);
}

/// Start `cleancore --resume` at the next login (Windows: RunOnce; Linux: autostart).
fn resume_at_login(on: bool) -> Result<()> {
    let exe = crate::launch::program()?;
    #[cfg(windows)]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(r"Software\Microsoft\Windows\CurrentVersion\RunOnce")?.0;
        if on {
            key.set_value("CleanCoreResume", &format!("\"{}\" --resume", exe.display()))?;
        } else {
            let _ = key.delete_value("CleanCoreResume");
        }
    }
    #[cfg(not(windows))]
    {
        let f = dirs::config_dir().unwrap_or_default().join("autostart/cleancore-resume.desktop");
        if on {
            ensure_dir(f.parent().unwrap())?;
            std::fs::write(
                &f,
                format!("[Desktop Entry]\nType=Application\nName=CleanCore: finish uninstalling\nExec=\"{}\" --resume\nX-GNOME-Autostart-enabled=true\n", exe.display()),
            )?;
        } else if f.exists() {
            std::fs::remove_file(&f)?;
        }
    }
    Ok(())
}

/// Run the built-in uninstaller with the user's options: close the program first if asked,
/// remember the uninstall so it can be resumed, and note when a restart is needed.
pub fn run_with_options(settings: &Settings, p: &Program, mode: RunMode, op: Operation) -> Result<(Vec<Note>, RunResult)> {
    let mut notes = Vec::new();
    if settings.uninstall_kill_running {
        let stopped = stop_running(p);
        if !stopped.is_empty() {
            notes.push(Note::new(Tone::Done, tr("Closed the running program first")).with_detail(stopped.join(", ")));
        }
    }
    let mut state = PendingUninstall {
        program: Some(p.clone()),
        label: p.name.clone(),
        version: p.version.clone(),
        target: Target::from_program(p),
        op,
        started: Local::now(),
        reboot_needed: false,
    };
    let _ = set_pending(&state);
    let r = run_builtin(p, mode)?;
    if matches!(r.exit_code, Some(3010) | Some(1641)) {
        state.reboot_needed = true;
        let _ = set_pending(&state);
        let _ = resume_at_login(true);
        notes.push(Note::new(Tone::Info, tr("The uninstaller asks for a restart. You can scan for leftovers now, or restart: CleanCore then continues where it stopped.")));
    }
    Ok((notes, r))
}

/// Is the program still installed (its uninstall entry / package still present)?
pub fn still_installed(p: &Program) -> bool {
    crate::platform::program_exists(p)
}

/// Leftover scan for a program or forced-uninstall target.
pub fn scan(settings: &Settings, target: &Target, mode: ScanMode, all_users: bool) -> Vec<Issue> {
    let scope = if all_users { Scope::AllUsers } else { Scope::CurrentUser };
    // Never touch what belongs to programs that are still installed.
    let others: Vec<Program> = crate::platform::list_programs(true)
        .into_iter()
        .filter(|o| target.id.as_ref() != Some(&o.id))
        .filter(|o| target.registry_key.is_none() || o.registry_key != target.registry_key)
        .filter(|o| target.package.is_none() || o.package != target.package)
        .collect();
    let mut issues = crate::platform::scan_leftovers(target, mode, scope, &others);
    // A trace log recorded for this program adds everything its installer created.
    for log in crate::tracer::matching(&target.names) {
        for mut i in crate::tracer::to_issues(&log) {
            i.detail = format!("{} · from trace log \"{}\"", i.detail, log.name);
            issues.push(i);
        }
    }
    issues.retain(|i| !settings.is_excluded(&i.location) && !shared_runtime(&i.location));
    let mut seen = std::collections::HashSet::new();
    issues.retain(|i| seen.insert(i.fix.clone()));
    issues.sort_by(|a, b| a.category.cmp(b.category).then_with(|| a.location.cmp(&b.location)));
    issues
}

/// Parts of a location that belong to Microsoft runtimes other programs need, though they look
/// like Edge's leftovers: WebView2 (Widgets, Teams, Outlook and many more use it), Edge Update
/// (it keeps WebView2 current; its services and tasks too) and EdgeCore, which both share.
fn shared_runtime(location: &str) -> bool {
    location.split(['\\', '/', '|']).map(|part| part.to_lowercase().replace([' ', '_', '-'], "")).any(|part| {
        matches!(part.as_str(), "edgewebview" | "edgecore" | "edgeupdate" | "edgeupdatem" | "microsoftedgewebview" | "microsoftedgewebview2runtime")
            || part.starts_with("microsoftedgeupdate")
    })
}

/// Delete the selected leftovers (backed up first) and record the uninstall in the history.
pub fn remove(settings: &Settings, program: &str, version: &str, op: Operation, leftovers: &[Issue]) -> Result<crate::cleaner::CleanReport> {
    // Restore point / full backup were already handled by `prepare`.
    let mut s = settings.clone();
    s.restore_point_before_clean = false;
    s.full_backup_before_clean = false;
    let report = if leftovers.is_empty() {
        crate::cleaner::CleanReport { backup_id: String::new(), cleaned: 0, failed: Vec::new(), restore_point: None, full_backup_id: None }
    } else {
        crate::cleaner::clean(&s, leftovers)?
    };
    if pending().is_some_and(|p| p.label == program) {
        clear_pending();
    }
    let failed: std::collections::HashSet<&str> = report.failed.iter().map(|(l, _)| l.as_str()).collect();
    record(&HistoryEntry {
        time: Local::now(),
        program: program.to_string(),
        version: version.to_string(),
        operation: op,
        backup_id: (!report.backup_id.is_empty()).then(|| report.backup_id.clone()),
        removed: leftovers.iter().filter(|i| !failed.contains(i.location.as_str())).map(|i| i.location.clone()).collect(),
        failed: report.failed.clone(),
    })?;
    Ok(report)
}

/// Quick Uninstall of one program: silent uninstaller, Moderate scan, remove all leftovers.
/// What a Quick Uninstall of one program did.
#[derive(Debug, Clone)]
pub struct QuickReport {
    pub exit_code: Option<i32>,
    /// False when the leftover scan is switched off in settings.
    pub scanned: bool,
    pub leftovers_removed: usize,
    pub leftovers_failed: usize,
    pub backup_id: String,
    pub still_installed: bool,
}

impl QuickReport {
    /// One line for the command line and logs.
    pub fn message(&self, name: &str) -> String {
        let exit = self.exit_code.map_or("?".into(), |c| c.to_string());
        let mut msg = if self.scanned {
            format!("{name}: uninstaller exit {exit}, removed {} leftover(s)", self.leftovers_removed)
        } else {
            format!("{name}: uninstaller exit {exit} (leftover scan off in settings)")
        };
        if self.leftovers_failed > 0 {
            msg.push_str(&format!(", {} could not be removed", self.leftovers_failed));
        }
        if self.still_installed {
            msg.push_str(" (the program still appears to be installed)");
        }
        msg
    }
}

pub fn quick_uninstall(settings: &Settings, p: &Program) -> Result<QuickReport> {
    let (_, run) = run_with_options(settings, p, RunMode::Quiet, Operation::QuickUninstall)?;
    if settings.uninstall_builtin_only {
        let rep = remove(settings, &p.name, &p.version, Operation::QuickUninstall, &[])?;
        return Ok(QuickReport {
            exit_code: run.exit_code,
            scanned: false,
            leftovers_removed: 0,
            leftovers_failed: 0,
            backup_id: rep.backup_id,
            still_installed: still_installed(p),
        });
    }
    // Give hand-off uninstallers a moment to finish.
    for _ in 0..30 {
        if !still_installed(p) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let leftovers = scan(settings, &Target::from_program(p), settings.uninstall_scan_mode, false);
    let rep = remove(settings, &p.name, &p.version, Operation::QuickUninstall, &leftovers)?;
    Ok(QuickReport {
        exit_code: run.exit_code,
        scanned: true,
        leftovers_removed: rep.cleaned,
        leftovers_failed: rep.failed.len(),
        backup_id: rep.backup_id,
        still_installed: still_installed(p),
    })
}

// ---------------------------------------------------------------- history

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Uninstall,
    QuickUninstall,
    ForcedUninstall,
    LeftoversOnly,
    TracedUninstall,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Uninstall => "Uninstall",
            Self::QuickUninstall => "Quick Uninstall",
            Self::ForcedUninstall => "Forced Uninstall",
            Self::LeftoversOnly => "Leftovers removal",
            Self::TracedUninstall => "Traced Uninstall",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub time: DateTime<Local>,
    pub program: String,
    pub version: String,
    pub operation: Operation,
    pub backup_id: Option<String>,
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

fn record(e: &HistoryEntry) -> Result<()> {
    ensure_dir(&uninst_dir())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(uninst_dir().join("history.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(e)?)?;
    Ok(())
}

/// Newest first.
pub fn history() -> Vec<HistoryEntry> {
    let Ok(f) = std::fs::File::open(uninst_dir().join("history.jsonl")) else {
        return Vec::new();
    };
    let mut v: Vec<HistoryEntry> = std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    v.reverse();
    v
}

pub fn history_text(e: &HistoryEntry) -> String {
    let mut s = format!("{}: {} {}\n{}\n", e.operation.label(), e.program, e.version, e.time.format("%Y-%m-%d %H:%M:%S"));
    if let Some(b) = &e.backup_id {
        s += &format!("Backup: {b}\n");
    }
    s += &format!("\nRemoved ({}):\n", e.removed.len());
    for r in &e.removed {
        s += &format!("  {r}\n");
    }
    if !e.failed.is_empty() {
        s += &format!("\nNot removed ({}):\n", e.failed.len());
        for (l, why) in &e.failed {
            s += &format!("  {l}: {why}\n");
        }
    }
    s
}

// ---------------------------------------------------------------- export

pub fn export(programs: &[Program], path: &std::path::Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_ascii_lowercase();
    let cols = |p: &Program| -> [String; 9] {
        [
            p.name.clone(),
            p.version.clone(),
            p.publisher.clone(),
            p.kind.label().to_string(),
            p.install_date.map(|d| d.to_string()).unwrap_or_default(),
            p.size.map(crate::backup::human_size).unwrap_or_default(),
            p.install_location.clone().unwrap_or_default(),
            p.uninstall_string.clone().unwrap_or_default(),
            p.comment.clone().unwrap_or_default(),
        ]
    };
    const HEAD: [&str; 9] = ["Name", "Version", "Company", "Type", "Install date", "Size", "Install location", "Uninstall string", "Comment"];
    let body = match ext.as_str() {
        "csv" | "xls" => {
            let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            let mut s = HEAD.join(",") + "\r\n";
            for p in programs {
                s += &cols(p).iter().map(|c| q(c)).collect::<Vec<_>>().join(",");
                s += "\r\n";
            }
            s
        }
        "html" | "htm" => {
            let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let mut s = String::from("<!doctype html><meta charset=utf-8><title>Installed programs</title><style>body{font:14px system-ui;margin:2em}td,th{padding:4px 8px;border-bottom:1px solid #ddd;text-align:left}</style><table><tr>");
            for h in HEAD {
                s += &format!("<th>{h}</th>");
            }
            s += "</tr>";
            for p in programs {
                s += "<tr>";
                for c in cols(p) {
                    s += &format!("<td>{}</td>", esc(&c));
                }
                s += "</tr>";
            }
            s + "</table>"
        }
        "json" => serde_json::to_string_pretty(programs)?,
        _ => {
            let mut s = String::new();
            for p in programs {
                let c = cols(p);
                s += &format!("{}\n", c[0]);
                for (h, v) in HEAD.iter().zip(c.iter()).skip(1) {
                    if !v.is_empty() {
                        s += &format!("    {h}: {v}\n");
                    }
                }
            }
            s
        }
    };
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_offers_the_runtimes_edge_shares() {
        for kept in [
            r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
            r"C:\Program Files (x86)\Microsoft\EdgeUpdate",
            r"C:\Program Files (x86)\Microsoft\EdgeCore\131.0.2903.70",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients",
            r"HKLM\SYSTEM\CurrentControlSet\Services\edgeupdatem",
            r"C:\Windows\System32\Tasks\MicrosoftEdgeUpdateTaskMachineCore",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run | MicrosoftEdgeUpdate",
        ] {
            assert!(shared_runtime(kept), "{kept}");
        }
        for offered in [
            r"C:\Program Files (x86)\Microsoft\Edge",
            r"C:\Users\me\AppData\Local\Microsoft\Edge\User Data",
            r"HKLM\SOFTWARE\Microsoft\Edge",
            r"C:\Program Files\KnowledgeCore",
            r"C:\Users\me\AppData\Local\EdgeCoreTools",
        ] {
            assert!(!shared_runtime(offered), "{offered}");
        }
    }

    #[test]
    fn version_stripping_and_tokens() {
        assert_eq!(strip_version("VLC media player 3.0.20 (x64)"), "VLC media player");
        assert_eq!(strip_version("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(strip_version("Git"), "Git");
        let mut p = Program::new("x", "Notepad++ 8.6 (64-bit x64)", ProgramKind::Win32);
        p.install_location = Some(r"C:\Program Files\Notepad++\".into());
        p.publisher = "Notepad++ Team".into();
        let t = Target::from_program(&p).tokens();
        assert!(t.contains(&"notepad".to_string()));
        assert!(matches("Notepad++", &t));
        assert!(!matches("Common Files", &t));
        assert!(!matches("Microsoft", &t));
        let mut f = Program::new("f", "Text Editor", ProgramKind::Flatpak);
        f.package = Some("org.gnome.TextEditor".into());
        let t = Target::from_program(&f).tokens();
        assert!(matches("TextEditor", &t) && matches("org.gnome.TextEditor", &t) && !matches("gnome", &t));
    }

    #[test]
    fn quiet_commands() {
        let mut p = Program::new("x", "App", ProgramKind::Win32);
        p.uninstall_string = Some(r#""C:\Program Files\App\unins000.exe""#.into());
        assert_eq!(quiet_command(&p).unwrap(), r#""C:\Program Files\App\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"#);
        p.uninstall_string = Some(r"C:\Program Files\App\uninst.exe".into());
        assert!(quiet_command(&p).unwrap().ends_with(" /S"));
        p.uninstall_string = Some("MsiExec.exe /I{23170F69-40C1-2702-2301-000001000000}".into());
        p.kind = ProgramKind::Msi;
        assert_eq!(quiet_command(&p).unwrap(), "MsiExec.exe /X{23170F69-40C1-2702-2301-000001000000} /qn /norestart");
        assert_eq!(forced_msi_command(&p).unwrap(), "MsiExec.exe /X{23170F69-40C1-2702-2301-000001000000}");
        p.quiet_uninstall_string = Some("x.exe /quiet".into());
        assert_eq!(quiet_command(&p).unwrap(), "x.exe /quiet");
    }

    #[test]
    fn registry_view() {
        let mut p = Program::new("x", "x", ProgramKind::Win32);
        let at = |p: &mut Program, k: &str| {
            p.registry_key = Some(k.into());
            p.bits()
        };
        assert_eq!(at(&mut p, r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\7-Zip"), Some(64));
        assert_eq!(at(&mut p, r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\VLC"), Some(32));
        assert_eq!(at(&mut p, r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\Zoom"), None);
        p.registry_key = None;
        assert_eq!(p.bits(), None);
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("Microso*", "Microsoft Edge"));
        assert!(wildcard("*zip*", "7-Zip 23.01"));
        assert!(wildcard("V?C*", "VLC media player"));
        assert!(!wildcard("Microso*", "Mozilla Firefox"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod stop_tests {
    use super::*;

    #[test]
    fn stops_processes_from_install_folder() {
        let dir = std::env::temp_dir().join(format!("cleancore-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // `sleep` keeps its name so multicall builds (uutils, busybox) still work from a copy.
        let exe = dir.join("sleep");
        std::fs::copy("/usr/bin/sleep", &exe).unwrap();
        let mut child = std::process::Command::new(&exe).arg("30").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(child.try_wait().unwrap().is_none(), "the sleeper should be running");
        let mut p = Program::new("t", "T", ProgramKind::Manual);
        p.install_location = Some(dir.display().to_string());
        assert_eq!(stop_running(&p), ["sleep"]);
        let start = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "still running");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
