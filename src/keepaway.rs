//! Microsoft Edge and OneDrive (Windows): removed, and kept away. Both come back on their own
//! (Windows Update, new accounts, feature updates), so removing one also sets Windows' own
//! blocks against reinstalling it, and while it is blocked a daily task removes it again if it
//! returns anyway (`scheduler::Action::KeepAway`). "Allow again" undoes exactly what was done
//! and reinstalls the app with winget.
//!
//! Never touched: Microsoft Edge WebView2 (many apps need it) and Edge Update, which keeps
//! WebView2 current.

use anyhow::{bail, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum App {
    Edge,
    OneDrive,
}

impl App {
    pub const ALL: [App; 2] = [App::Edge, App::OneDrive];

    pub fn name(self) -> &'static str {
        match self {
            App::Edge => "Microsoft Edge",
            App::OneDrive => "OneDrive",
        }
    }

    /// The winget package that reinstalls it.
    fn winget_id(self) -> &'static str {
        match self {
            App::Edge => "Microsoft.Edge",
            App::OneDrive => "Microsoft.OneDrive",
        }
    }
}

/// What CleanCore did, per app, so "Allow again" can undo it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Kept {
    pub edge: Option<Removed>,
    pub onedrive: Option<Removed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Removed {
    pub time: DateTime<Local>,
    /// OneDrive: the new-account setup entry taken out of the default profile, to put back.
    #[serde(default)]
    pub default_run: Option<String>,
}

impl Kept {
    pub fn get(&self, app: App) -> Option<&Removed> {
        match app {
            App::Edge => self.edge.as_ref(),
            App::OneDrive => self.onedrive.as_ref(),
        }
    }

    fn set(&mut self, app: App, r: Option<Removed>) {
        match app {
            App::Edge => self.edge = r,
            App::OneDrive => self.onedrive = r,
        }
    }

    pub fn any(&self) -> bool {
        self.edge.is_some() || self.onedrive.is_some()
    }
}

fn state_file() -> PathBuf {
    crate::config::data_dir().join("kept-away.json")
}

pub fn load() -> Kept {
    std::fs::read_to_string(state_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(k: &Kept) -> Result<()> {
    crate::config::ensure_dir(&crate::config::data_dir())?;
    std::fs::write(state_file(), serde_json::to_string_pretty(k)?)?;
    Ok(())
}

/// The app is on this PC (for this user or every user).
pub fn installed(app: App) -> bool {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let candidates: Vec<PathBuf> = match app {
        App::Edge => ["ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(|k| env(k)).map(|d| d.join(r"Microsoft\Edge\Application\msedge.exe")).collect(),
        App::OneDrive => {
            let mut v: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|k| env(k)).map(|d| d.join(r"Microsoft OneDrive\OneDrive.exe")).collect();
            v.extend(env("LOCALAPPDATA").map(|d| d.join(r"Microsoft\OneDrive\OneDrive.exe")));
            v
        }
    };
    cfg!(windows) && candidates.iter().any(|p| p.exists())
}

// ---------------------------------------------------------------- the scripts

/// OneDrive: its own uninstallers (this user's copy, then the machine-wide one).
const ONEDRIVE_REMOVE: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Stop-Process -Name OneDrive, OneDriveSetup -Force
Start-Sleep -Seconds 1
Get-ChildItem "$env:LOCALAPPDATA\Microsoft\OneDrive\*\OneDriveSetup.exe" | ForEach-Object { Start-Process $_.FullName '/uninstall' -Wait }
foreach ($d in $env:ProgramFiles, ${env:ProgramFiles(x86)}) {
  Get-ChildItem "$d\Microsoft OneDrive\*\OneDriveSetup.exe" | ForEach-Object { Start-Process $_.FullName '/uninstall /allusers' -Wait }
}
foreach ($s in "$env:SystemRoot\System32\OneDriveSetup.exe", "$env:SystemRoot\SysWOW64\OneDriveSetup.exe") {
  if (Test-Path $s) { Start-Process $s '/uninstall' -Wait }
}
"#;

/// OneDrive: the blocks. Prints the default profile's setup entry it removed (to restore).
const ONEDRIVE_BLOCK: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
# The policy "Prevent the usage of OneDrive for file storage": OneDrive won't run for anyone.
$pol = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive'
New-Item $pol -Force | Out-Null
Set-ItemProperty $pol -Name DisableFileSyncNGSC -Value 1 -Type DWord
# New accounts: the default profile installs it at first sign-in.
$hive = "$env:SystemDrive\Users\Default\NTUSER.DAT"
$old = ''
if (Test-Path $hive) {
  reg.exe load 'HKU\CleanCoreDefault' $hive | Out-Null
  $old = (Get-ItemProperty 'Registry::HKEY_USERS\CleanCoreDefault\Software\Microsoft\Windows\CurrentVersion\Run').OneDriveSetup
  Remove-ItemProperty 'Registry::HKEY_USERS\CleanCoreDefault\Software\Microsoft\Windows\CurrentVersion\Run' -Name OneDriveSetup
  [gc]::Collect()
  reg.exe unload 'HKU\CleanCoreDefault' | Out-Null
}
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name OneDrive, OneDriveSetup
Get-ScheduledTask -TaskName 'OneDrive*' | Unregister-ScheduledTask -Confirm:$false
# Explorer's sidebar entry.
foreach ($c in 'HKCU:\Software\Classes\CLSID\{018D5C66-4533-4307-9B53-224DE2ED1FE6}', 'HKCU:\Software\Classes\WOW6432Node\CLSID\{018D5C66-4533-4307-9B53-224DE2ED1FE6}') {
  New-Item $c -Force | Out-Null
  Set-ItemProperty $c -Name 'System.IsPinnedToNameSpaceTree' -Value 0 -Type DWord
}
Write-Output $old
"#;

/// OneDrive: undo the blocks. `$old` is the default profile's setup entry to put back.
const ONEDRIVE_ALLOW: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Remove-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive' -Name DisableFileSyncNGSC
foreach ($c in 'HKCU:\Software\Classes\CLSID\{018D5C66-4533-4307-9B53-224DE2ED1FE6}', 'HKCU:\Software\Classes\WOW6432Node\CLSID\{018D5C66-4533-4307-9B53-224DE2ED1FE6}') {
  Remove-ItemProperty $c -Name 'System.IsPinnedToNameSpaceTree'
}
$hive = "$env:SystemDrive\Users\Default\NTUSER.DAT"
if ($old -and (Test-Path $hive)) {
  reg.exe load 'HKU\CleanCoreDefault' $hive | Out-Null
  New-Item 'Registry::HKEY_USERS\CleanCoreDefault\Software\Microsoft\Windows\CurrentVersion\Run' -Force | Out-Null
  Set-ItemProperty 'Registry::HKEY_USERS\CleanCoreDefault\Software\Microsoft\Windows\CurrentVersion\Run' -Name OneDriveSetup -Value $old
  [gc]::Collect()
  reg.exe unload 'HKU\CleanCoreDefault' | Out-Null
}
"#;

/// Edge: its own uninstaller, then its folder. Microsoft lets the uninstaller run outside the
/// EEA only with Edge Update's developer switch set and the legacy Edge file present, so both
/// are set for the run. What it leaves of the program folder (it often does, while Edge Update
/// or Edge's elevation service still run from it) is taken over by Administrators and deleted;
/// files still in use are deleted when Windows restarts. Prints `REBOOT` in that case.
///
/// Only a folder named Edge is touched: EdgeWebView, EdgeUpdate and EdgeCore next to it are
/// WebView2's, which other programs need (see `uninstaller::shared_runtime`). Edge Update is
/// paused, not removed, for the same reason: Windows starts it again when WebView2 needs it.
const EDGE_REMOVE: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
# Where Edge is: the usual folders, and wherever its uninstall entry says (…\Edge\Application).
$roots = @("${env:ProgramFiles(x86)}\Microsoft\Edge", "$env:ProgramFiles\Microsoft\Edge")
foreach ($k in 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft Edge', 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft Edge') {
  $l = (Get-ItemProperty $k).InstallLocation
  if ($l) { $roots += Split-Path ($l.Trim('"').TrimEnd('\')) -Parent }
}
$roots = @($roots | Where-Object { $_ -and (Split-Path $_ -Leaf) -eq 'Edge' -and (Test-Path -LiteralPath $_) } | ForEach-Object { (Get-Item -LiteralPath $_).FullName.TrimEnd('\') } | Sort-Object -Unique)

# Everything that runs from Edge's folder, and Edge Update, which starts Edge's processes again.
function Stop-Edge {
  Stop-Service edgeupdate, edgeupdatem, MicrosoftEdgeElevationService -Force
  Get-Process msedge, msedge_proxy, identity_helper, elevation_service, notification_helper, pwahelper, cookie_exporter, ie_to_edge_stub, 'MicrosoftEdgeUpdate*' | Stop-Process -Force
  Get-Process | Where-Object { $p = $_.Path; $p -and @($roots | Where-Object { $p.StartsWith("$_\", 'OrdinalIgnoreCase') }).Count } | Stop-Process -Force
  Start-Sleep -Seconds 2
}
Stop-Edge

$dev = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev'
New-Item $dev -Force | Out-Null
Set-ItemProperty $dev -Name AllowUninstall -Value 1 -Type DWord
$legacy = "$env:SystemRoot\SystemApps\Microsoft.MicrosoftEdge_8wekyb3d8bbwe"
$made = -not (Test-Path "$legacy\MicrosoftEdge.exe")
if ($made) { New-Item $legacy -ItemType Directory -Force | Out-Null; New-Item "$legacy\MicrosoftEdge.exe" -ItemType File -Force | Out-Null }
foreach ($r in $roots) {
  Get-ChildItem "$r\Application\*\Installer\setup.exe" | ForEach-Object {
    Start-Process $_.FullName '--uninstall --msedge --system-level --verbose-logging --force-uninstall' -Wait
  }
}
if ($made) { Remove-Item "$legacy\MicrosoftEdge.exe" -Force }
Remove-ItemProperty $dev -Name AllowUninstall

# What the uninstaller left of the program folder.
Stop-Edge
$svc = Get-CimInstance Win32_Service -Filter "Name='MicrosoftEdgeElevationService'"
if ($svc -and @($roots | Where-Object { $svc.PathName.Trim('"').StartsWith("$_\", 'OrdinalIgnoreCase') }).Count) { sc.exe delete MicrosoftEdgeElevationService | Out-Null }
Add-Type -Namespace CleanCore -Name Kernel32 -MemberDefinition '[DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern bool MoveFileEx(string from, string to, int flags);'
foreach ($r in $roots) {
  if (-not (Test-Path -LiteralPath $r)) { continue }
  # Ownership to Administrators, then full control for them. takeown's prompt (for folders it
  # can't list) gets no answer, so it skips them: icacls sets the owner of those, in any language.
  cmd.exe /c "takeown /F `"$r`" /A /R <nul" | Out-Null
  icacls.exe $r /setowner '*S-1-5-32-544' /T /C /Q | Out-Null
  icacls.exe $r /grant '*S-1-5-32-544:(OI)(CI)F' /T /C /Q | Out-Null
  Get-ChildItem -LiteralPath $r -Recurse -Force -File | ForEach-Object { $_.Attributes = 'Normal' }
  Remove-Item -LiteralPath $r -Recurse -Force
  if (Test-Path -LiteralPath $r) {
    # Still in use: deleted when Windows restarts, deepest first so each folder is empty by then.
    @(Get-ChildItem -LiteralPath $r -Recurse -Force) + @(Get-Item -LiteralPath $r) | Sort-Object { $_.FullName.Length } -Descending | ForEach-Object { [CleanCore.Kernel32]::MoveFileEx($_.FullName, [NullString]::Value, 4) | Out-Null }
    'REBOOT'
  }
}

# Its Store placeholder and shortcuts.
Get-AppxPackage -AllUsers 'Microsoft.MicrosoftEdge.Stable' | Remove-AppxPackage -AllUsers
Remove-Item "$env:PUBLIC\Desktop\Microsoft Edge.lnk", "$env:USERPROFILE\Desktop\Microsoft Edge.lnk", "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Microsoft Edge.lnk" -Force
"#;

/// Edge's app id in Edge Update (the stable channel; WebView2 has its own and stays allowed).
const EDGE_BLOCK: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
# Microsoft's blocker for the automatic switch to the new Edge.
$eu = 'HKLM:\SOFTWARE\Microsoft\EdgeUpdate'
New-Item $eu -Force | Out-Null
Set-ItemProperty $eu -Name DoNotUpdateToEdgeWithChromium -Value 1 -Type DWord
# Edge Update policies: don't install Edge, nor put its shortcut on the desktop.
$pol = 'HKLM:\SOFTWARE\Policies\Microsoft\EdgeUpdate'
New-Item $pol -Force | Out-Null
Set-ItemProperty $pol -Name 'Install{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}' -Value 0 -Type DWord
Set-ItemProperty $pol -Name 'CreateDesktopShortcut{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}' -Value 0 -Type DWord
"#;

const EDGE_ALLOW: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Remove-ItemProperty 'HKLM:\SOFTWARE\Microsoft\EdgeUpdate' -Name DoNotUpdateToEdgeWithChromium
Remove-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\EdgeUpdate' -Name 'Install{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}', 'CreateDesktopShortcut{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}'
"#;

// ---------------------------------------------------------------- doing it

fn ps(script: &str) -> Result<String> {
    #[cfg(windows)]
    {
        crate::platform::powershell(script)
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        bail!("this is for Windows")
    }
}

fn need_admin() -> Result<()> {
    if !crate::slop::is_admin() {
        bail!("{}", crate::i18n::tr("Run CleanCore as administrator to remove it for every user and keep it away."));
    }
    Ok(())
}

/// The Uninstaller's entry for Edge itself (WebView2 is listed apart, as "Microsoft Edge
/// WebView2 Runtime", and never goes this way).
pub fn is_edge(p: &crate::uninstaller::Program) -> bool {
    cfg!(windows) && p.name == "Microsoft Edge" && p.uninstall_string.as_deref().is_some_and(|c| c.to_lowercase().contains(r"\edge\application\"))
}

/// Uninstall Edge without blocking it: what the Uninstaller runs for it. Edge's registered
/// uninstall command is refused outside the EEA (exit code 93); this sets Microsoft's switches
/// for the run, as `remove` does. Says whether files in use are left for the next restart.
pub fn uninstall_edge() -> Result<bool> {
    if !crate::slop::is_admin() {
        bail!("{}", crate::i18n::tr("Run CleanCore as administrator to uninstall Microsoft Edge."));
    }
    let reboot = remove_edge()?;
    if !reboot && installed(App::Edge) {
        bail!("{}", crate::tf!("{}'s uninstaller did not remove it.", App::Edge.name()));
    }
    Ok(reboot)
}

/// Run `EDGE_REMOVE`; true when some files are deleted at the next restart.
fn remove_edge() -> Result<bool> {
    Ok(ps(EDGE_REMOVE)?.lines().any(|l| l.trim() == "REBOOT"))
}

/// Remove the app and block it from coming back. Keeps the daily check running.
pub fn remove(app: App) -> Result<String> {
    need_admin()?;
    let mut k = load();
    let default_run = match app {
        App::Edge => {
            let reboot = remove_edge()?;
            ps(EDGE_BLOCK)?;
            if reboot {
                k.set(app, Some(Removed { time: Local::now(), default_run: None }));
                save(&k)?;
                ensure_daily_check(&k)?;
                return Ok(crate::tf!("{} is removed and won't come back. Some of its files were in use: they are deleted when Windows restarts.", app.name()));
            }
            None
        }
        App::OneDrive => {
            ps(ONEDRIVE_REMOVE)?;
            let old = ps(ONEDRIVE_BLOCK)?.trim().to_string();
            // Keep an entry saved by an earlier removal: this run found it already gone.
            Some(old).filter(|s| !s.is_empty()).or_else(|| k.get(app).and_then(|r| r.default_run.clone()))
        }
    };
    k.set(app, Some(Removed { time: Local::now(), default_run }));
    save(&k)?;
    ensure_daily_check(&k)?;
    if installed(app) {
        bail!("{}", crate::tf!("{}'s uninstaller did not remove it. It stays blocked, and the daily check tries again.", app.name()));
    }
    Ok(crate::tf!("{} is removed and won't come back.", app.name()))
}

/// Undo the blocks and reinstall the app with winget.
pub fn allow(app: App) -> Result<String> {
    need_admin()?;
    let mut k = load();
    match app {
        App::Edge => {
            ps(EDGE_ALLOW)?;
        }
        App::OneDrive => {
            let old = k.get(app).and_then(|r| r.default_run.clone()).unwrap_or_default();
            ps(&format!("$old = '{}'\n{ONEDRIVE_ALLOW}", old.replace('\'', "''")))?;
        }
    }
    k.set(app, None);
    save(&k)?;
    ensure_daily_check(&k)?;
    let st =
        crate::platform::hidden("winget").args(["install", "--id", app.winget_id(), "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements"]).status();
    match st {
        Ok(s) if s.success() => Ok(crate::tf!("{} is allowed again and reinstalled.", app.name())),
        _ => Ok(crate::tf!("{} is allowed again. Reinstall it from microsoft.com or the Microsoft Store.", app.name())),
    }
}

/// The daily check: remove again whatever came back despite the blocks. Run by the
/// scheduled task.
pub fn enforce() -> Result<String> {
    let k = load();
    let mut done = Vec::new();
    for app in App::ALL {
        if k.get(app).is_some() && installed(app) {
            remove(app)?;
            done.push(app.name());
        }
    }
    Ok(if done.is_empty() { "nothing came back".into() } else { format!("removed again: {}", done.join(", ")) })
}

/// The scheduled task behind the daily check.
pub const TASK: &str = "keep-edge-onedrive-away";

/// Register the daily check while anything is kept away, and remove it when nothing is.
fn ensure_daily_check(k: &Kept) -> Result<()> {
    use crate::scheduler::{self, Action, Schedule, Task};
    let mut all = scheduler::load_tasks()?;
    let pos = all.iter().position(|t| t.id == TASK);
    match (k.any(), pos) {
        (true, None) => {
            let task = Task {
                id: TASK.into(),
                name: "Keep Edge and OneDrive away".into(),
                note: String::new(),
                schedule: Schedule::Daily { time: chrono::NaiveTime::from_hms_opt(12, 30, 0).unwrap() },
                action: Action::KeepAway,
                created: Local::now(),
                last_run: None,
            };
            scheduler::register(&task)?;
            all.push(task);
        }
        (false, Some(i)) => {
            scheduler::unregister(TASK)?;
            all.remove(i);
        }
        _ => return Ok(()),
    }
    scheduler::save_tasks(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_says_what_is_kept_away() {
        let mut k = Kept::default();
        assert!(!k.any());
        k.set(App::OneDrive, Some(Removed { time: Local::now(), default_run: Some(r"C:\Windows\System32\OneDriveSetup.exe /thfirstsetup".into()) }));
        let back: Kept = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
        assert!(back.any() && back.get(App::Edge).is_none());
        assert_eq!(back.get(App::OneDrive).unwrap().default_run.as_deref(), Some(r"C:\Windows\System32\OneDriveSetup.exe /thfirstsetup"));
    }

    #[test]
    fn never_touches_webview2() {
        // WebView2's app id in Edge Update; only Edge's own is blocked.
        for s in [EDGE_REMOVE, EDGE_BLOCK, EDGE_ALLOW] {
            assert!(!s.contains("F3017226-FE2A-4295-8BDF-00C3A9A7E4C5"));
            assert!(!s.to_lowercase().contains("webview"));
        }
        // Edge's uninstaller is told to remove Edge only.
        assert!(EDGE_REMOVE.contains("--msedge"));
        // The folders deleted afterwards are only ever named Edge (not EdgeUpdate, EdgeCore…),
        // and Edge Update is stopped, never deleted.
        assert!(EDGE_REMOVE.contains("(Split-Path $_ -Leaf) -eq 'Edge'"));
        assert!(!EDGE_REMOVE.contains("sc.exe delete edgeupdate"));
    }

    /// The scripts parse (PowerShell's own parser: nothing runs).
    #[test]
    #[cfg(windows)]
    fn scripts_parse() {
        for s in [EDGE_REMOVE, EDGE_BLOCK, EDGE_ALLOW, ONEDRIVE_REMOVE, ONEDRIVE_BLOCK, ONEDRIVE_ALLOW] {
            let check = "$e = $null; [void][System.Management.Automation.Language.Parser]::ParseInput($env:CLEANCORE_SCRIPT, [ref]$null, [ref]$e); if ($e) { $e | ForEach-Object { $_.Message }; exit 1 }";
            let o = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", check]).env("CLEANCORE_SCRIPT", s).output().unwrap();
            assert!(o.status.success(), "{}\n{s}", String::from_utf8_lossy(&o.stdout));
        }
    }
}
