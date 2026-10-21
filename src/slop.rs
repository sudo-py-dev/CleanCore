//! Microsoft slop cleaner (Tools): removes the apps Windows preinstalls or pushes (games, ads,
//! Bing apps, Copilot, Xbox…) for this user or every user, optionally with their provisioned
//! copy so they don't come back for new accounts or after feature updates. The AI, ads and
//! telemetry settings it offers are Tuner tweaks ([`TWEAKS`]), so they revert the same way.

use anyhow::Result;
use serde::Serialize;

/// A known preinstalled or promoted app, matched by package name (wildcards allowed).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BloatApp {
    pub pattern: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    /// Ticked by default: nobody needs it to use Windows.
    pub recommended: bool,
    pub note: &'static str,
}

const fn app(pattern: &'static str, name: &'static str, group: &'static str, recommended: bool, note: &'static str) -> BloatApp {
    BloatApp { pattern, name, group, recommended, note }
}

pub const ADS: &str = "Games and promoted apps";
pub const EXTRAS: &str = "Microsoft extras";
pub const AI: &str = "AI and Copilot";
pub const XBOX: &str = "Xbox";
pub const COMM: &str = "Communication";

/// The catalog. Store, App Installer (winget), frameworks and security components are never in it.
pub const CATALOG: &[BloatApp] = &[
    app("king.com.*", "Candy Crush and other King games", ADS, true, "Pushed by Windows"),
    app("Microsoft.MicrosoftSolitaireCollection", "Solitaire Collection", ADS, true, "Ads unless you pay"),
    app("SpotifyAB.SpotifyMusic", "Spotify", ADS, true, "Pushed by Windows"),
    app("*Disney*", "Disney+", ADS, true, "Pushed by Windows"),
    app("Facebook.*", "Facebook / Instagram", ADS, true, "Pushed by Windows"),
    app("BytedancePte.Ltd.TikTok", "TikTok", ADS, true, "Pushed by Windows"),
    app("AmazonVideo.PrimeVideo", "Prime Video", ADS, true, "Pushed by Windows"),
    app("*.LinkedInforWindows", "LinkedIn", ADS, true, "Pushed by Windows"),
    app("Clipchamp.Clipchamp", "Clipchamp", EXTRAS, true, "Video editor, online account needed"),
    app("Microsoft.BingNews", "News", EXTRAS, true, ""),
    app("Microsoft.BingWeather", "Weather", EXTRAS, false, ""),
    app("Microsoft.BingSearch", "Bing Search", EXTRAS, true, ""),
    app("Microsoft.BingFinance", "Money", EXTRAS, true, ""),
    app("Microsoft.BingSports", "Sports", EXTRAS, true, ""),
    app("Microsoft.GetHelp", "Get Help", EXTRAS, true, ""),
    app("Microsoft.Getstarted", "Tips", EXTRAS, true, ""),
    app("Microsoft.MicrosoftOfficeHub", "Microsoft 365 (Office) hub", EXTRAS, true, "An ad for Microsoft 365, not Office itself"),
    app("Microsoft.WindowsFeedbackHub", "Feedback Hub", EXTRAS, true, ""),
    app("Microsoft.WindowsMaps", "Maps", EXTRAS, true, ""),
    app("Microsoft.People", "People", EXTRAS, true, ""),
    app("Microsoft.MixedReality.Portal", "Mixed Reality Portal", EXTRAS, true, ""),
    app("Microsoft.Microsoft3DViewer", "3D Viewer", EXTRAS, true, ""),
    app("Microsoft.Print3D", "Print 3D", EXTRAS, true, ""),
    app("Microsoft.PowerAutomateDesktop", "Power Automate", EXTRAS, true, ""),
    app("Microsoft.Todos", "Microsoft To Do", EXTRAS, false, ""),
    app("Microsoft.MicrosoftStickyNotes", "Sticky Notes", EXTRAS, false, "Useful for many"),
    app("Microsoft.Windows.DevHome", "Dev Home", EXTRAS, true, "Retired by Microsoft"),
    app("Microsoft.Wallet", "Wallet", EXTRAS, true, ""),
    app("Microsoft.ZuneVideo", "Movies & TV", EXTRAS, false, ""),
    app("Microsoft.Office.Sway", "Sway", EXTRAS, true, ""),
    app("Microsoft.OneConnect", "Mobile Plans", EXTRAS, true, ""),
    app("Microsoft.Copilot", "Copilot", AI, true, ""),
    app("Microsoft.Windows.Ai.Copilot.Provider", "Copilot provider", AI, true, ""),
    app("Microsoft.549981C3F5F10", "Cortana", AI, true, "Retired by Microsoft"),
    app("Microsoft.MicrosoftJournal", "Journal", AI, false, ""),
    app("Microsoft.XboxApp", "Xbox Console Companion", XBOX, true, "Old Xbox app"),
    app("Microsoft.GamingApp", "Xbox", XBOX, false, "Needed for Game Pass"),
    app("Microsoft.XboxGamingOverlay", "Xbox Game Bar", XBOX, false, "Win+G overlay, screen recording"),
    app("Microsoft.XboxGameOverlay", "Xbox Game Overlay", XBOX, false, ""),
    app("Microsoft.XboxSpeechToTextOverlay", "Xbox speech-to-text", XBOX, true, ""),
    app("Microsoft.Xbox.TCUI", "Xbox TCUI", XBOX, false, "Some games need it to sign in"),
    app("MicrosoftTeams", "Teams (personal)", COMM, true, "Not Teams for work or school"),
    app("MSTeams", "Teams", COMM, false, "Needed for work or school accounts"),
    app("Microsoft.SkypeApp", "Skype", COMM, true, "Retired by Microsoft"),
    app("Microsoft.YourPhone", "Phone Link", COMM, false, "Connects an Android phone or iPhone"),
    app("Microsoft.OutlookForWindows", "Outlook (new)", COMM, false, ""),
    app("Microsoft.windowscommunicationsapps", "Mail and Calendar", COMM, true, "Retired; replaced by the new Outlook"),
    app("Microsoft.Messaging", "Messaging", COMM, true, ""),
];

/// Tuner tweaks the slop cleaner offers, in display order.
pub const TWEAKS: &[&str] = &[
    "disable_copilot",
    "disable_recall",
    "disable_click_to_do",
    "disable_ai_in_apps",
    "no_edge_bing_sidebar",
    "disable_ads",
    "no_start_recommendations",
    "no_lockscreen_ads",
    "no_advertising_id",
    "no_bing_search",
    "disable_widgets",
    "no_telemetry",
];

/// A catalog app found on this PC.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub app: BloatApp,
    /// Installed package names (`Name`), for this user or (as administrator) any user.
    pub packages: Vec<String>,
    /// Provisioned copies (`PackageName`), installed for every new account.
    pub provisioned: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Scan {
    pub found: Vec<Found>,
    /// Run as administrator: other users' copies and provisioned packages are visible and
    /// removable.
    pub admin: bool,
}

fn matching(pattern: &str, names: &[String]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().filter(|n| crate::pathutil::wildcard(pattern, n)).cloned().collect();
    v.sort();
    v.dedup();
    v
}

/// Match package names against the catalog.
pub fn match_catalog(installed: &[String], provisioned: &[(String, String)]) -> Vec<Found> {
    CATALOG
        .iter()
        .filter_map(|a| {
            let packages = matching(a.pattern, installed);
            let prov: Vec<String> = provisioned.iter().filter(|(d, _)| crate::pathutil::wildcard(a.pattern, d)).map(|(_, p)| p.clone()).collect();
            (!packages.is_empty() || !prov.is_empty()).then_some(Found { app: *a, packages, provisioned: prov })
        })
        .collect()
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    // SAFETY: no arguments.
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    false
}

#[cfg(windows)]
pub fn scan() -> Result<Scan> {
    let admin = is_admin();
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$pk = if ($args0) { Get-AppxPackage -AllUsers } else { Get-AppxPackage }
$prov = @()
if ($args0) { $prov = @(Get-AppxProvisionedPackage -Online | Select-Object DisplayName,PackageName) }
@{ packages = @($pk | Where-Object { -not $_.NonRemovable } | Select-Object -ExpandProperty Name); provisioned = $prov } | ConvertTo-Json -Depth 3 -Compress
"#
    .replace("$args0", if admin { "$true" } else { "$false" });
    let text = crate::platform::powershell(&script)?;
    let v: serde_json::Value = serde_json::from_str(text.trim())?;
    let list = |x: &serde_json::Value| -> Vec<serde_json::Value> {
        match x {
            serde_json::Value::Array(a) => a.clone(),
            serde_json::Value::Null => Vec::new(),
            o => vec![o.clone()],
        }
    };
    let installed: Vec<String> = list(&v["packages"]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    let provisioned: Vec<(String, String)> =
        list(&v["provisioned"]).iter().filter_map(|o| Some((o.get("DisplayName")?.as_str()?.to_string(), o.get("PackageName")?.as_str()?.to_string()))).collect();
    Ok(Scan { found: match_catalog(&installed, &provisioned), admin })
}

#[cfg(not(windows))]
pub fn scan() -> Result<Scan> {
    anyhow::bail!("the Microsoft slop cleaner is for Windows")
}

#[derive(Debug, Default, Serialize)]
pub struct RemoveReport {
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Remove the apps: for this user, or every user (administrator), and optionally their
/// provisioned copies (administrator). Each removal is recorded in the uninstall history.
pub fn remove(settings: &crate::config::Settings, apps: &[Found], all_users: bool, deprovision: bool) -> RemoveReport {
    let mut r = RemoveReport::default();
    for f in apps {
        match remove_one(f, all_users, deprovision) {
            Ok(()) => {
                let _ = crate::uninstaller::remove(settings, f.app.name, "", crate::uninstaller::Operation::Uninstall, &[]);
                r.removed.push(f.app.name.to_string());
            }
            Err(e) => r.failed.push((f.app.name.to_string(), format!("{e:#}"))),
        }
    }
    r
}

#[cfg(windows)]
fn remove_one(f: &Found, all_users: bool, deprovision: bool) -> Result<()> {
    let q = |s: &str| s.replace('\'', "''");
    let mut script = String::from("$ErrorActionPreference = 'Stop'\n");
    let scope = if all_users { " -AllUsers" } else { "" };
    for name in &f.packages {
        script += &format!("Get-AppxPackage{scope} -Name '{}' | Remove-AppxPackage{scope}\n", q(name));
    }
    if deprovision {
        for p in &f.provisioned {
            script += &format!("Remove-AppxProvisionedPackage -Online -PackageName '{}' | Out-Null\n", q(p));
        }
    }
    crate::platform::powershell(&script).map(|_| ())
}

#[cfg(not(windows))]
fn remove_one(_: &Found, _: bool, _: bool) -> Result<()> {
    anyhow::bail!("the Microsoft slop cleaner is for Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_catalog() {
        let installed: Vec<String> = ["king.com.CandyCrushSaga", "Microsoft.BingNews", "Microsoft.WindowsStore", "Microsoft.MSPaint"].map(String::from).to_vec();
        let prov = vec![("Clipchamp.Clipchamp".to_string(), "Clipchamp.Clipchamp_2.0.0.0_neutral_~_yxz26nhyzhsrt".to_string())];
        let f = match_catalog(&installed, &prov);
        let names: Vec<&str> = f.iter().map(|x| x.app.name).collect();
        assert_eq!(names, ["Candy Crush and other King games", "Clipchamp", "News"]);
        assert_eq!(f[1].provisioned.len(), 1);
        assert!(f[1].packages.is_empty());
        // The catalog never touches the Store, App Installer or security.
        for a in CATALOG {
            for p in ["Microsoft.WindowsStore", "Microsoft.DesktopAppInstaller", "Microsoft.SecHealthUI", "Microsoft.VCLibs.140.00"] {
                assert!(!crate::pathutil::wildcard(a.pattern, p), "{} matches {p}", a.pattern);
            }
        }
    }

    #[test]
    fn tweaks_exist_in_the_windows_table() {
        let all = crate::tuner::parse_table(crate::tuner::WINDOWS_TABLE).unwrap();
        for id in TWEAKS {
            assert!(all.iter().any(|t| t.id == *id), "missing tweak {id}");
        }
    }
}
