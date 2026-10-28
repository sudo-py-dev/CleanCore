//! Windows uninstaller backend: `Uninstall` registry entries and Store apps, running the
//! built-in / silent / forced-MSI uninstaller, and the registry + filesystem leftover scan.

use super::windows::{get_str, get_u32, join, open, reg_string, subkeys, value_loc, values};
use crate::model::{Fix, Issue, ScanMode, Scope};
use crate::uninstaller::{forced_msi_command, normalize, quiet_command, Program, ProgramKind, RunMode, RunResult, Target};
use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CAT_REGISTRY: &str = "leftover_registry";
pub const CAT_FILES: &str = "leftover_files";

const HKLM: &str = "HKEY_LOCAL_MACHINE";
const HKCU: &str = "HKEY_CURRENT_USER";
const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL_WOW: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

// ---------------------------------------------------------------- inventory

fn key_date(key: &str) -> Option<NaiveDate> {
    if let Some(d) = get_str(key, "InstallDate") {
        let d = d.trim();
        if d.len() == 8 {
            if let Ok(nd) = NaiveDate::parse_from_str(d, "%Y%m%d") {
                return Some(nd);
            }
        }
    }
    let info = open(key)?.query_info().ok()?;
    let st = info.get_last_write_time_system();
    NaiveDate::from_ymd_opt(st.wYear as i32, st.wMonth as u32, st.wDay as u32)
}

fn icon_exe(icon: &str) -> Option<String> {
    let p = icon.trim().trim_matches('"');
    let p = p.rsplit_once(',').map_or(p, |(a, b)| if b.trim().trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) { a } else { p });
    let p = p.trim().trim_matches('"');
    p.to_ascii_lowercase().ends_with(".exe").then(|| p.to_string())
}

fn win32_programs(show_system: bool) -> Vec<Program> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (root, user) in [(join(HKLM, UNINSTALL), false), (join(HKLM, UNINSTALL_WOW), false), (join(HKCU, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"), true)] {
        for sub in subkeys(&root) {
            let key = join(&root, &sub);
            let Some(name) = get_str(&key, "DisplayName") else {
                continue;
            };
            let uninstall = get_str(&key, "UninstallString");
            let msi = get_u32(&key, "WindowsInstaller") == Some(1);
            let release = get_str(&key, "ReleaseType").unwrap_or_default();
            let system = get_u32(&key, "SystemComponent") == Some(1)
                || get_str(&key, "ParentKeyName").is_some()
                || matches!(release.as_str(), "Update" | "Hotfix" | "Security Update" | "Service Pack")
                || (uninstall.is_none() && !msi);
            if system && !show_system {
                continue;
            }
            let version = get_str(&key, "DisplayVersion").unwrap_or_default();
            if !seen.insert((name.to_lowercase(), version.clone())) {
                continue;
            }
            let mut p = Program::new(format!("win32:{key}"), name, if msi { ProgramKind::Msi } else { ProgramKind::Win32 });
            p.version = version;
            p.publisher = get_str(&key, "Publisher").unwrap_or_default();
            p.install_date = key_date(&key);
            p.size = get_u32(&key, "EstimatedSize").map(|k| k as u64 * 1024);
            p.install_location = get_str(&key, "InstallLocation").map(|l| l.trim().trim_matches('"').trim_end_matches('\\').to_string()).filter(|l| !l.is_empty());
            p.uninstall_string = uninstall;
            p.quiet_uninstall_string = get_str(&key, "QuietUninstallString");
            p.modify_path = get_str(&key, "ModifyPath");
            p.website = get_str(&key, "URLInfoAbout");
            p.help_link = get_str(&key, "HelpLink");
            p.update_link = get_str(&key, "URLUpdateInfo");
            p.icon = get_str(&key, "DisplayIcon");
            p.comment = get_str(&key, "Comments");
            p.registry_key = Some(key.clone());
            p.system = system;
            p.user_scope = user;
            p.package = p.msi_product_code();
            // Icons borrowed from Windows (msiexec, the Installer cache) say nothing about the program.
            if let Some(exe) = p.icon.as_deref().and_then(icon_exe).filter(|e| !in_windows_dir(e)) {
                if p.install_location.is_none() {
                    p.install_location = Path::new(&exe).parent().map(|d| d.display().to_string());
                }
                p.executables.push(exe);
            }
            out.push(p);
        }
    }
    out
}

/// "Microsoft.WindowsCalculator" → "Windows Calculator"
fn pretty_app_name(name: &str) -> String {
    let base = name.rsplit('.').next().unwrap_or(name);
    let mut s = String::new();
    let chars: Vec<char> = base.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars.get(i + 1).is_some_and(|n| n.is_lowercase())) {
            s.push(' ');
        }
        s.push(*c);
    }
    s
}

pub fn powershell(script: &str) -> Result<String> {
    powershell_within(script, Duration::from_secs(600))
}

/// Run a PowerShell script, stopping it when it takes longer than `limit`: an Appx removal
/// waiting on a busy deployment service otherwise never returns.
pub fn powershell_within(script: &str, limit: Duration) -> Result<String> {
    use std::io::Read;
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("starting PowerShell")?;
    // Read both pipes while waiting, so a full pipe can't stall the script.
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = pipe(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!("PowerShell did not finish within {} s and was stopped", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if !status.success() {
        bail!("{}", String::from_utf8_lossy(&err).trim());
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Removing one Store app: long enough for a large app, short of hanging the uninstall.
const APPX_REMOVE: Duration = Duration::from_secs(180);

/// Windows' own folder and everything in it: never a program's folder, and what runs from
/// there is never the program's to stop.
pub fn in_windows_dir(path: &str) -> bool {
    let Some(root) = env("SystemRoot") else {
        return false;
    };
    let root = root.display().to_string().trim_end_matches('\\').to_lowercase();
    let p = path.trim().trim_matches('"').to_lowercase();
    p == root || p.starts_with(&format!("{root}\\"))
}

fn store_apps(show_system: bool) -> Vec<Program> {
    let script = "Get-AppxPackage | Select-Object Name,PackageFullName,PackageFamilyName,Version,Publisher,InstallLocation,IsFramework,SignatureKind,NonRemovable | ConvertTo-Json -Compress";
    let Ok(text) = powershell(script) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };
    let items = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => return Vec::new(),
    };
    let s = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let b = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let mut out = Vec::new();
    for o in &items {
        let system = b(o, "IsFramework") || b(o, "NonRemovable") || s(o, "SignatureKind") == "System";
        if system && !show_system {
            continue;
        }
        let name = s(o, "Name");
        let mut p = Program::new(format!("appx:{}", s(o, "PackageFullName")), pretty_app_name(&name), ProgramKind::WindowsApp);
        p.version = s(o, "Version");
        p.publisher = s(o, "Publisher").split(',').next().unwrap_or("").trim_start_matches("CN=").to_string();
        p.install_location = Some(s(o, "InstallLocation")).filter(|l| !l.is_empty());
        p.install_date =
            p.install_location.as_ref().and_then(|l| std::fs::metadata(l).and_then(|m| m.modified()).ok()).map(|t| chrono::DateTime::<chrono::Local>::from(t).date_naive());
        p.package = Some(s(o, "PackageFullName"));
        p.comment = Some(s(o, "PackageFamilyName"));
        p.system = system;
        p.user_scope = true;
        p.uninstall_string = Some(format!("Remove-AppxPackage {}", s(o, "PackageFullName")));
        out.push(p);
    }
    out
}

pub fn list_programs(show_system: bool) -> Vec<Program> {
    let mut v = win32_programs(show_system);
    v.extend(store_apps(show_system));
    v
}

pub fn program_exists(p: &Program) -> bool {
    match p.kind {
        ProgramKind::WindowsApp => {
            let name = p.package.as_deref().and_then(|f| f.split('_').next()).unwrap_or("");
            powershell_within(&format!("Get-AppxPackage -Name '{}'", name.replace('\'', "''")), Duration::from_secs(30)).is_ok_and(|o| !o.trim().is_empty())
        }
        _ => p.registry_key.as_deref().is_some_and(|k| open(k).is_some()),
    }
}

pub fn run_uninstaller(p: &Program, mode: RunMode) -> Result<RunResult> {
    if p.kind == ProgramKind::WindowsApp {
        let full = p.package.clone().context("package name unknown")?;
        let script = format!("Remove-AppxPackage -Package '{}'", full.replace('\'', "''"));
        // A failed or stopped removal says why instead of ending with a bare exit code.
        powershell_within(&script, APPX_REMOVE).with_context(|| format!("removing {}", p.name))?;
        return Ok(RunResult { command: script, exit_code: Some(0) });
    }
    if mode != RunMode::ForcedMsi && crate::keepaway::is_edge(p) {
        // Files still in use go at the next restart: 3010, as an MSI asking for one.
        let reboot = crate::keepaway::uninstall_edge()?;
        return Ok(RunResult { command: "setup.exe --uninstall --msedge --system-level --force-uninstall".into(), exit_code: Some(if reboot { 3010 } else { 0 }) });
    }
    let cmd = match mode {
        RunMode::Normal => p.uninstall_string.clone(),
        RunMode::Quiet => quiet_command(p).or_else(|| p.uninstall_string.clone()),
        RunMode::ForcedMsi => forced_msi_command(p),
    }
    .with_context(|| format!("\"{}\" has no built-in uninstaller; use Forced Uninstall", p.name))?;
    // cmd /C keeps the command line exactly as the installer registered it.
    let st = Command::new("cmd").arg("/C").raw_arg(format!("\"{cmd}\"")).status().with_context(|| format!("starting {cmd}"))?;
    Ok(RunResult { command: cmd, exit_code: st.code() })
}

/// Windows Apps extras.
pub fn app_uninstall_all_users(p: &Program) -> Result<()> {
    let full = p.package.clone().context("package name unknown")?;
    powershell_within(&format!("Remove-AppxPackage -Package '{}' -AllUsers", full.replace('\'', "''")), APPX_REMOVE).map(|_| ())
}

pub fn app_reset(p: &Program) -> Result<()> {
    let name = p.package.as_deref().and_then(|f| f.split('_').next()).context("package name unknown")?;
    powershell_within(&format!("Get-AppxPackage -Name '{}' | Reset-AppxPackage", name.replace('\'', "''")), APPX_REMOVE).map(|_| ())
}

/// Start a Store app by its AppUserModelID (`<PackageFamilyName>!<AppId>`).
pub fn app_run(p: &Program) -> Result<()> {
    let pfn = p.comment.clone().filter(|c| !c.is_empty()).context("package family name unknown")?;
    let q = pfn.replace('\'', "''");
    let aumid = powershell_within(&format!("(Get-StartApps | Where-Object {{ $_.AppID -like '{q}!*' }} | Select-Object -First 1).AppID"), Duration::from_secs(30))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(format!("{pfn}!App"));
    Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{aumid}")).spawn().context("starting the app")?;
    Ok(())
}

/// Stop a Store app: every process running from its package folder.
pub fn app_terminate(p: &Program) -> Result<usize> {
    Ok(crate::uninstaller::stop_running(p).len())
}

/// Reset the Microsoft Store (wsreset clears its cache and reopens it).
pub fn reset_store() -> Result<()> {
    Command::new("wsreset.exe").spawn().context("starting wsreset")?;
    Ok(())
}

/// Remove a Store app a traced installer added.
pub fn purge(manager: &str, name: &str) -> Result<()> {
    if manager != "appx" {
        bail!("removing {manager} packages is not supported on Windows");
    }
    powershell(&format!("Remove-AppxPackage -Package '{}'", name.replace('\'', "''"))).map(|_| ())
}

// ---------------------------------------------------------------- leftovers

struct Scan<'a> {
    tokens: Vec<String>,
    others: HashSet<String>,
    other_paths: Vec<String>,
    other_keys: HashSet<String>,
    /// The target's folders and executables, lowercased once (`refers` runs for every value).
    refer_paths: Vec<String>,
    refer_exes: Vec<String>,
    target: &'a Target,
    loose: bool,
    out: Vec<Issue>,
}

impl Scan<'_> {
    fn hit(&self, name: &str) -> bool {
        let n = normalize(name);
        !self.others.contains(&n)
            && if self.loose { crate::uninstaller::matches_loose_normalized(&n, &self.tokens) } else { crate::uninstaller::matches_normalized(&n, &self.tokens) }
    }

    /// Registry data / file text that points into the program's folder or at its executables.
    fn refers(&self, data: &str) -> bool {
        if self.refer_paths.is_empty() && self.refer_exes.is_empty() {
            return false;
        }
        let d = data.to_lowercase();
        self.refer_paths.iter().any(|p| d.contains(p.as_str())) || self.refer_exes.iter().any(|e| d.contains(e.as_str()))
    }

    fn other_owns(&self, path: &str) -> bool {
        let l = path.to_lowercase();
        self.other_paths.iter().any(|o| l.starts_with(o.as_str()) || o.starts_with(&l))
    }

    fn key(&mut self, key: &str, what: &str) {
        if self.other_keys.contains(&key.to_lowercase()) {
            return;
        }
        self.out.push(Issue::new(CAT_REGISTRY, key, what, Fix::DeleteRegKey { key: key.into() }));
    }

    fn value(&mut self, key: &str, name: &str, what: &str) {
        self.out.push(Issue::new(CAT_REGISTRY, value_loc(key, name), what, Fix::DeleteRegValue { key: key.into(), value: name.into() }));
    }

    fn file(&mut self, p: &Path, what: &str) {
        let s = p.display().to_string();
        if self.other_owns(&s) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(p) else {
            return;
        };
        let size = crate::backup::dir_size(p);
        let fix = if meta.is_dir() { Fix::DeleteDirTree { path: p.into() } } else { Fix::DeleteFile { path: p.into() } };
        self.out.push(Issue::new(CAT_FILES, s, format!("{what} · {}", crate::backup::human_size(size)), fix));
    }

    /// Subkeys of `root` named after the program, plus `root\<Publisher>\<Program>`.
    fn software_keys(&mut self, root: &str) {
        let publisher = self.target.publisher.as_deref().map(normalize).unwrap_or_default();
        for sub in subkeys(root) {
            let key = join(root, &sub);
            if self.hit(&sub) {
                self.key(&key, "program settings");
            } else if !publisher.is_empty() && normalize(&sub) == publisher && !self.others.contains(&publisher) {
                // Vendor key: take the program's subkey, or the whole vendor key if nothing else is in it.
                let inner = subkeys(&key);
                let ours: Vec<String> = inner.iter().filter(|s| self.hit(s)).cloned().collect();
                if !inner.is_empty() && ours.len() == inner.len() || inner.is_empty() {
                    self.key(&key, "vendor settings");
                } else {
                    for s in ours {
                        self.key(&join(&key, &s), "program settings");
                    }
                }
            }
        }
    }

    /// Children of `dir` named after the program (and the vendor folder logic as above).
    fn folders(&mut self, dir: &Path, what: &str) {
        let publisher = self.target.publisher.as_deref().map(normalize).unwrap_or_default();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.clone());
            if self.hit(&name) || (name.to_ascii_lowercase().ends_with(".lnk") && self.hit(&stem)) {
                self.file(&e.path(), what);
            } else if !publisher.is_empty() && normalize(&name) == publisher && !self.others.contains(&publisher) && e.path().is_dir() {
                let inner: Vec<_> = std::fs::read_dir(e.path()).map(|r| r.flatten().collect()).unwrap_or_default();
                let ours: Vec<_> = inner.iter().filter(|c| self.hit(&c.file_name().to_string_lossy())).map(|c| c.path()).collect();
                if inner.is_empty() || ours.len() == inner.len() {
                    self.file(&e.path(), "vendor folder");
                } else {
                    for p in ours {
                        self.file(&p, what);
                    }
                }
            }
        }
    }
}

/// HKCU or every loaded profile hive, and the matching profile folders.
fn user_roots(scope: Scope) -> Vec<(String, PathBuf)> {
    let me = (HKCU.to_string(), env("USERPROFILE").unwrap_or_default());
    if scope == Scope::CurrentUser {
        return vec![me];
    }
    let mut out = Vec::new();
    for sid in subkeys("HKEY_USERS").into_iter().filter(|s| s.starts_with("S-1-5-21-") && !s.ends_with("_Classes")) {
        let profile = get_str(&format!(r"{HKLM}\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}"), "ProfileImagePath").map(PathBuf::from).unwrap_or_default();
        out.push((format!(r"HKEY_USERS\{sid}"), profile));
    }
    if out.is_empty() {
        out.push(me);
    }
    out
}

pub fn scan_leftovers(target: &Target, mode: ScanMode, scope: Scope, others: &[Program]) -> Vec<Issue> {
    let mut other_tokens: HashSet<String> = others.iter().flat_map(|o| Target::from_program(o).tokens()).collect();
    for n in &target.names {
        other_tokens.remove(&normalize(n));
    }
    let other_paths: Vec<String> = others.iter().filter_map(|o| o.install_location.clone()).filter(|l| l.len() > 3).map(|l| l.to_lowercase()).collect();
    let other_keys: HashSet<String> = others.iter().filter_map(|o| o.registry_key.clone()).map(|k| k.to_lowercase()).collect();
    let refer_paths = target.paths.iter().filter(|p| p.len() > 3).map(|p| p.to_lowercase()).collect();
    let refer_exes = target.executables.iter().filter(|e| e.len() > 6).map(|e| e.to_lowercase()).collect();
    let mut s =
        Scan { tokens: target.tokens(), others: other_tokens, other_paths, other_keys, refer_paths, refer_exes, target, loose: mode == ScanMode::Advanced, out: Vec::new() };

    // The uninstall entry itself and the program folder, if they survived the uninstaller.
    if let Some(k) = &target.registry_key {
        if open(k).is_some() {
            s.key(k, "uninstall entry");
        }
    }
    for p in &target.paths {
        let path = Path::new(p);
        let protected =
            [env("ProgramFiles"), env("ProgramFiles(x86)"), env("SystemRoot"), env("ProgramData"), env("USERPROFILE")].iter().flatten().any(|d| d == path) || in_windows_dir(p);
        if path.exists() && !protected && path.components().count() > 2 {
            s.file(path, "program folder");
        }
    }

    let users = user_roots(scope);
    // --- Safe: Software keys and the usual folders
    s.software_keys(&join(HKLM, "SOFTWARE"));
    s.software_keys(&join(HKLM, r"SOFTWARE\WOW6432Node"));
    for (hive, _) in &users {
        s.software_keys(&join(hive, "Software"));
    }
    let mut machine_dirs: Vec<(PathBuf, &str)> = Vec::new();
    for (v, what) in [
        ("ProgramData", "shared data"),
        ("ProgramFiles", "program folder"),
        ("ProgramFiles(x86)", "program folder"),
        ("CommonProgramFiles", "shared files"),
        ("CommonProgramFiles(x86)", "shared files"),
    ] {
        if let Some(d) = env(v) {
            machine_dirs.push((d, what));
        }
    }
    if let Some(pd) = env("ProgramData") {
        machine_dirs.push((pd.join(r"Microsoft\Windows\Start Menu\Programs"), "Start menu entry"));
    }
    if let Some(public) = env("PUBLIC") {
        machine_dirs.push((public.join("Desktop"), "desktop shortcut"));
    }
    for (d, what) in machine_dirs {
        s.folders(&d, what);
    }
    for (_, profile) in &users {
        for (sub, what) in [
            (r"AppData\Roaming", "settings folder"),
            (r"AppData\Local", "data folder"),
            (r"AppData\LocalLow", "data folder"),
            (r"AppData\Local\Programs", "program folder"),
            (r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs", "Start menu entry"),
            ("Desktop", "desktop shortcut"),
        ] {
            s.folders(&profile.join(sub), what);
        }
        if target.kind == Some(ProgramKind::WindowsApp) {
            if let Some(fam) = target.package.as_deref().map(|f| {
                // PackageFullName "Name_Version_Arch_ResourceId_PublisherId" → family "Name_PublisherId"
                let parts: Vec<&str> = f.split('_').collect();
                format!("{}_{}", parts.first().unwrap_or(&""), parts.last().unwrap_or(&""))
            }) {
                let p = profile.join(r"AppData\Local\Packages").join(fam);
                if p.exists() {
                    s.file(&p, "app data");
                }
            }
        }
    }

    // --- Moderate: everything that points into the program folder
    if mode >= ScanMode::Moderate && (!target.paths.is_empty() || !target.executables.is_empty()) {
        let mut hives = vec![HKLM.to_string()];
        hives.extend(users.iter().map(|(h, _)| h.clone()));
        for h in &hives {
            for sub in [
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
                r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
            ] {
                let key = join(h, sub);
                for (name, v) in values(&key) {
                    if reg_string(&v).is_some_and(|d| s.refers(&d)) {
                        s.value(&key, &name, "starts with Windows");
                    }
                }
            }
            for sub in [r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths"] {
                let root = join(h, sub);
                for k in subkeys(&root) {
                    let key = join(&root, &k);
                    if get_str(&key, "").is_some_and(|d| s.refers(&d)) {
                        s.key(&key, "App Paths entry");
                    }
                }
            }
        }
        let services = join(HKLM, r"SYSTEM\CurrentControlSet\Services");
        for svc in subkeys(&services) {
            let key = join(&services, &svc);
            if get_str(&key, "ImagePath").is_some_and(|d| s.refers(&d)) {
                s.key(&key, "Windows service");
            }
        }
        for sub in [r"SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs", r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\SharedDlls"] {
            let key = join(HKLM, sub);
            for (name, _) in values(&key) {
                if s.refers(&name) {
                    s.value(&key, &name, "shared DLL counter");
                }
            }
        }
        for (h, _) in &users {
            for sub in [
                r"Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache",
                r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store",
                r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers",
            ] {
                let key = join(h, sub);
                for (name, _) in values(&key) {
                    if s.refers(&name) {
                        s.value(&key, &name, "cache entry");
                    }
                }
            }
        }
        let fw = join(HKLM, r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules");
        for (name, v) in values(&fw) {
            if reg_string(&v).is_some_and(|d| d.split('|').any(|f| f.strip_prefix("App=").is_some_and(|a| s.refers(a)))) {
                s.value(&fw, &name, "firewall rule");
            }
        }
        // COM classes and file types served from the program folder
        let mut class_roots = vec![join(HKLM, r"SOFTWARE\Classes"), join(HKLM, r"SOFTWARE\Classes\WOW6432Node")];
        class_roots.extend(users.iter().map(|(h, _)| join(h, r"Software\Classes")));
        for root in &class_roots {
            let clsid = join(root, "CLSID");
            for guid in subkeys(&clsid) {
                let key = join(&clsid, &guid);
                if ["InprocServer32", "LocalServer32"].iter().any(|srv| get_str(&join(&key, srv), "").is_some_and(|d| s.refers(&d))) {
                    s.key(&key, "COM class");
                }
            }
            for class in subkeys(root).into_iter().filter(|c| !c.starts_with('.') && c != "CLSID" && c != "WOW6432Node") {
                let key = join(root, &class);
                if get_str(&join(&key, r"shell\open\command"), "").is_some_and(|d| s.refers(&d)) {
                    s.key(&key, "file type");
                }
            }
            for exe in target.executables.iter().filter_map(|e| Path::new(e).file_name().map(|f| f.to_string_lossy().to_string())) {
                let key = join(root, &format!(r"Applications\{exe}"));
                if open(&key).is_some() {
                    s.key(&key, "Open With entry");
                }
            }
        }
    }

    // --- Advanced: deeper name search in the registry and profile folders
    if mode == ScanMode::Advanced {
        let mut roots = vec![join(HKLM, "SOFTWARE"), join(HKLM, r"SOFTWARE\WOW6432Node")];
        roots.extend(users.iter().map(|(h, _)| join(h, "Software")));
        for root in roots {
            for vendor in subkeys(&root).into_iter().filter(|v| !matches!(v.to_ascii_lowercase().as_str(), "classes" | "microsoft" | "policies" | "wow6432node")) {
                let vk = join(&root, &vendor);
                for sub in subkeys(&vk) {
                    if s.hit(&sub) {
                        s.key(&join(&vk, &sub), "program settings");
                    }
                }
            }
        }
        for (_, profile) in &users {
            for sub in ["Documents", r"AppData\Roaming", r"AppData\Local"] {
                let Ok(rd) = std::fs::read_dir(profile.join(sub)) else {
                    continue;
                };
                for vendor in rd.flatten().filter(|e| e.path().is_dir()) {
                    s.folders(&vendor.path(), "folder");
                }
            }
        }
    }
    s.out
}
