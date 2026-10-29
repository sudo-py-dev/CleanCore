//! Linux autorun sources: XDG autostart (user + system), systemd user and system services,
//! cron `@reboot`.

use crate::autorun::{AddMethod, AutorunEntry, Source};
use crate::model::{Fix, Issue};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn user_autostart() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".config/autostart")
}

const SYSTEM_AUTOSTART: &str = "/etc/xdg/autostart";

fn desktop_kv(text: &str) -> Vec<(String, String)> {
    let mut main = false;
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            main = l == "[Desktop Entry]";
        } else if main {
            if let Some((k, v)) = l.split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

fn get<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().rev().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

fn shown_here(kv: &[(String, String)]) -> bool {
    let desk = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    let desks: Vec<&str> = desk.split(':').collect();
    let has = |list: &str| list.split(';').filter(|s| !s.is_empty()).any(|d| desks.contains(&d.to_lowercase().as_str()));
    if let Some(only) = get(kv, "OnlyShowIn") {
        if !desk.is_empty() && !has(only) {
            return false;
        }
    }
    if let Some(not) = get(kv, "NotShowIn") {
        if has(not) {
            return false;
        }
    }
    true
}

fn is_enabled(kv: &[(String, String)]) -> bool {
    get(kv, "Hidden") != Some("true") && get(kv, "X-GNOME-Autostart-enabled") != Some("false")
}

fn systemctl(user: bool, args: &[&str]) -> Vec<String> {
    let mut c = Command::new("systemctl");
    if user {
        c.arg("--user");
    }
    c.args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default()
}

/// `ExecStart` program + arguments of many units with one `systemctl show` call.
fn exec_starts(user: bool, units: &[String]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if units.is_empty() {
        return out;
    }
    let mut args = vec!["show", "-p", "Id", "-p", "ExecStart", "--"];
    // Templates (`getty@.service`) aren't loadable units and would fail the whole call.
    args.extend(units.iter().map(String::as_str).filter(|u| !u.contains("@.")));
    let mut id = String::new();
    for l in systemctl(user, &args) {
        if let Some(i) = l.strip_prefix("Id=") {
            id = i.to_string();
        } else if let Some(x) = l.strip_prefix("ExecStart=") {
            if let Some(argv) = x.split("argv[]=").nth(1).and_then(|r| r.split(" ;").next()) {
                out.entry(id.clone()).or_insert_with(|| argv.trim().to_string());
            }
        }
    }
    out
}

fn crontab() -> String {
    Command::new("crontab").arg("-l").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

const CRON_OFF: &str = "#cleancore-disabled# ";

/// Login scripts that may start programs in the background.
fn login_scripts() -> Vec<PathBuf> {
    let h = dirs::home_dir().unwrap_or_default();
    [".xprofile", ".xsessionrc", ".profile", ".bash_profile", ".zprofile"].iter().map(|f| h.join(f)).filter(|p| p.is_file()).collect()
}

/// `prog args &` (optionally disabled by us) → the command, if the line starts a background program.
fn background_command(line: &str) -> Option<(bool, String)> {
    let (enabled, body) = match line.trim_start().strip_prefix(CRON_OFF) {
        Some(b) => (false, b),
        None => (true, line.trim_start()),
    };
    let t = body.trim();
    if t.starts_with('#') || !t.ends_with('&') || t.ends_with("&&") {
        return None;
    }
    let cmd = t.trim_end_matches('&').trim();
    // Skip shell syntax (`fi &`, `done &`, `) &`); keep plain commands.
    let first = cmd.split_whitespace().next()?;
    if cmd.is_empty() || ["fi", "done", "esac", ")", "}"].contains(&first) || first.contains('=') && !first.starts_with('/') {
        return None;
    }
    Some((enabled, cmd.to_string()))
}

fn rewrite_script(file: &Path, line: &str, f: impl Fn(&str) -> Option<String>) -> Result<()> {
    let text = fs::read_to_string(file)?;
    let dir = crate::config::data_dir().join("autorun");
    crate::config::ensure_dir(&dir)?;
    let name = file.file_name().map(|n| n.to_string_lossy().trim_start_matches('.').to_string()).unwrap_or_default();
    fs::write(dir.join(format!("{name}-{}.bak", chrono::Local::now().format("%Y%m%d-%H%M%S"))), &text)?;
    let mut found = false;
    let mut out = String::new();
    for l in text.lines() {
        if !found && l.trim_start().strip_prefix(CRON_OFF).unwrap_or(l.trim_start()) == line {
            found = true;
            if let Some(new) = f(l) {
                out += &new;
                out.push('\n');
            }
            continue;
        }
        out += l;
        out.push('\n');
    }
    anyhow::ensure!(found, "the line is no longer in {}", file.display());
    fs::write(file, out)?;
    Ok(())
}

fn script_location(e: &AutorunEntry) -> Result<(PathBuf, String)> {
    let (file, line) = e.location.split_once(" | ").context("bad location")?;
    Ok((PathBuf::from(file), line.to_string()))
}

pub fn autorun_run(e: &AutorunEntry) -> Result<()> {
    let mut c = match e.source {
        Source::UserService => {
            let mut c = Command::new("systemctl");
            c.args(["--user", "start", &e.location]);
            c
        }
        Source::Service => {
            let mut c = Command::new("pkexec");
            c.args(["systemctl", "start", &e.location]);
            c
        }
        _ => {
            let mut c = Command::new("sh");
            // Desktop-entry field codes (%u, %F, …) have no meaning here.
            let cmd: String = e.command.split_whitespace().filter(|w| !(w.len() == 2 && w.starts_with('%'))).collect::<Vec<_>>().join(" ");
            c.args(["-c", &cmd]);
            c
        }
    };
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().context("starting the program")?;
    Ok(())
}

pub fn autorun_list(show_system: bool) -> Vec<AutorunEntry> {
    let mut v = Vec::new();
    // XDG autostart: user entries override system entries with the same file name.
    let mut seen = std::collections::HashSet::new();
    for (dir, system) in [(user_autostart(), false), (PathBuf::from(SYSTEM_AUTOSTART), true)] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x == "desktop") || !seen.insert(e.file_name()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&p) else {
                continue;
            };
            let kv = desktop_kv(&text);
            if !shown_here(&kv) {
                continue;
            }
            // A user file that only hides a system entry still describes that system entry.
            let overrides_system = !system && Path::new(SYSTEM_AUTOSTART).join(e.file_name()).exists();
            let is_system = system || overrides_system;
            if is_system && !show_system {
                continue;
            }
            v.push(AutorunEntry {
                id: format!("xdg:{}", e.file_name().to_string_lossy()),
                name: get(&kv, "Name").unwrap_or(&e.file_name().to_string_lossy()).to_string(),
                command: get(&kv, "Exec").unwrap_or("").to_string(),
                location: p.display().to_string(),
                source: Source::StartupFolder,
                all_users: is_system,
                enabled: is_enabled(&kv),
                publisher: None,
                description: get(&kv, "Comment").map(str::to_string),
                exe: None,
                running: false,
                valid: true,
                removable: !is_system,
                system: is_system,
                startup: Default::default(),
            });
        }
    }
    // systemd user services that start with the session
    let user_dir = dirs::config_dir().unwrap_or_default().join("systemd/user");
    for l in systemctl(true, &["list-unit-files", "--type=service", "--no-legend", "--no-pager"]) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 2 || !matches!(f[1], "enabled" | "disabled") {
            continue;
        }
        let own = user_dir.join(f[0]).exists();
        if f[1] == "disabled" && !own {
            continue; // vendor units nobody enabled
        }
        if !own && !show_system {
            continue;
        }
        v.push(AutorunEntry {
            id: format!("systemd-user:{}", f[0]),
            name: f[0].trim_end_matches(".service").to_string(),
            command: String::new(),
            location: f[0].to_string(),
            source: Source::UserService,
            all_users: false,
            enabled: f[1] == "enabled",
            publisher: None,
            description: None,
            exe: None,
            running: false,
            valid: true,
            removable: own,
            system: !own,
            startup: Default::default(),
        });
    }
    // systemd system services (only with "show system", except custom units in /etc)
    for l in systemctl(false, &["list-unit-files", "--type=service", "--state=enabled", "--no-legend", "--no-pager"]) {
        let Some(unit) = l.split_whitespace().next() else {
            continue;
        };
        let custom = Path::new("/etc/systemd/system").join(unit).is_file() && !Path::new("/etc/systemd/system").join(unit).is_symlink();
        if !custom && !show_system {
            continue;
        }
        v.push(AutorunEntry {
            id: format!("systemd:{unit}"),
            name: unit.trim_end_matches(".service").to_string(),
            command: String::new(),
            location: unit.to_string(),
            source: Source::Service,
            all_users: true,
            enabled: true,
            publisher: None,
            description: None,
            exe: None,
            running: false,
            valid: true,
            removable: false,
            system: !custom,
            startup: Default::default(),
        });
    }
    for user in [true, false] {
        let src = if user { Source::UserService } else { Source::Service };
        let units: Vec<String> = v.iter().filter(|e| e.source == src).map(|e| e.location.clone()).collect();
        let cmds = exec_starts(user, &units);
        for e in v.iter_mut().filter(|e| e.source == src) {
            e.command = cmds.get(&e.location).cloned().unwrap_or_default();
        }
    }
    // Background programs in login scripts
    for file in login_scripts() {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for l in text.lines() {
            let Some((enabled, cmd)) = background_command(l) else {
                continue;
            };
            let body = l.trim_start().strip_prefix(CRON_OFF).unwrap_or(l.trim_start()).to_string();
            v.push(AutorunEntry {
                id: format!("profile:{}|{body}", file.display()),
                name: crate::autorun::exe_of(&cmd).and_then(|e| Path::new(&e).file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| cmd.clone()),
                command: cmd,
                location: format!("{} | {body}", file.display()),
                source: Source::LoginScript,
                all_users: false,
                enabled,
                publisher: None,
                description: None,
                exe: None,
                running: false,
                valid: true,
                removable: true,
                system: false,
                startup: Default::default(),
            });
        }
    }
    // cron @reboot
    for line in crontab().lines() {
        let (enabled, body) = match line.strip_prefix(CRON_OFF) {
            Some(b) => (false, b),
            None => (true, line),
        };
        if let Some(cmd) = body.trim().strip_prefix("@reboot") {
            let cmd = cmd.trim().to_string();
            v.push(AutorunEntry {
                id: format!("cron:{body}"),
                name: crate::autorun::exe_of(&cmd).and_then(|e| Path::new(&e).file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| cmd.clone()),
                command: cmd,
                location: body.to_string(),
                source: Source::Cron,
                all_users: false,
                enabled,
                publisher: None,
                description: None,
                exe: None,
                running: false,
                valid: true,
                removable: true,
                system: false,
                startup: Default::default(),
            });
        }
    }
    v
}

fn set_desktop_flags(path: &Path, base: &str, on: bool) -> Result<()> {
    let mut out = String::new();
    let mut main = false;
    let mut written = false;
    for l in base.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if main && !written {
                out += &format!("Hidden={}\nX-GNOME-Autostart-enabled={}\n", !on, on);
                written = true;
            }
            main = t == "[Desktop Entry]";
        } else if main && (t.starts_with("Hidden=") || t.starts_with("X-GNOME-Autostart-enabled=")) {
            continue;
        }
        out += l;
        out.push('\n');
    }
    if !written {
        out += &format!("Hidden={}\nX-GNOME-Autostart-enabled={}\n", !on, on);
    }
    if let Some(p) = path.parent() {
        crate::config::ensure_dir(p)?;
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// Rewrite crontab lines disabled under an earlier name of the app (`old` marker) to today's marker.
pub fn cron_migrate_marker(old: &str) -> Result<()> {
    let text = crontab();
    if !text.contains(old) {
        return Ok(());
    }
    write_crontab(&text.replace(old, CRON_OFF))
}

fn write_crontab(text: &str) -> Result<()> {
    // Keep the previous crontab next to CleanCore's data, just in case.
    let dir = crate::config::data_dir().join("autorun");
    crate::config::ensure_dir(&dir)?;
    fs::write(dir.join(format!("crontab-{}.bak", chrono::Local::now().format("%Y%m%d-%H%M%S"))), crontab())?;
    let mut child = Command::new("crontab").arg("-").stdin(std::process::Stdio::piped()).spawn().context("running crontab")?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("crontab rejected the change");
    }
    Ok(())
}

pub fn autorun_set_enabled(e: &AutorunEntry, on: bool) -> Result<()> {
    match e.source {
        Source::StartupFolder => {
            let name = Path::new(&e.location).file_name().context("bad path")?;
            let user_file = user_autostart().join(name);
            // System entries are switched with a per-user override file.
            let base = if user_file.exists() { fs::read_to_string(&user_file)? } else { fs::read_to_string(&e.location)? };
            set_desktop_flags(&user_file, &base, on)
        }
        Source::UserService => {
            let st = Command::new("systemctl").args(["--user", if on { "enable" } else { "disable" }, &e.location]).status()?;
            if !st.success() {
                bail!("systemctl failed");
            }
            Ok(())
        }
        Source::Service => {
            let st = Command::new("pkexec").args(["systemctl", if on { "enable" } else { "disable" }, &e.location]).status()?;
            if !st.success() {
                bail!("systemctl failed (administrator rights are needed)");
            }
            Ok(())
        }
        Source::Cron => {
            let text: String = crontab()
                .lines()
                .map(|l| {
                    let body = l.strip_prefix(CRON_OFF).unwrap_or(l);
                    if body == e.location {
                        if on {
                            body.to_string()
                        } else {
                            format!("{CRON_OFF}{body}")
                        }
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            write_crontab(&text)
        }
        Source::LoginScript => {
            let (file, line) = script_location(e)?;
            rewrite_script(&file, &line, |orig| {
                let indent: String = orig.chars().take_while(|c| c.is_whitespace()).collect();
                Some(if on { format!("{indent}{line}") } else { format!("{indent}{CRON_OFF}{line}") })
            })
        }
        _ => bail!("not a Linux autorun entry"),
    }
}

pub fn autorun_removal(e: &AutorunEntry) -> Result<Vec<Issue>> {
    match e.source {
        Source::StartupFolder | Source::UserService => {
            let path = if e.source == Source::UserService { dirs::config_dir().unwrap_or_default().join("systemd/user").join(&e.location) } else { PathBuf::from(&e.location) };
            if e.source == Source::UserService {
                let _ = Command::new("systemctl").args(["--user", "disable", &e.location]).status();
            }
            Ok(vec![Issue::new("autorun", path.display().to_string(), format!("autorun entry {}", e.name), Fix::DeleteFile { path })])
        }
        Source::Cron => {
            let text: String = crontab().lines().filter(|l| l.strip_prefix(CRON_OFF).unwrap_or(l) != e.location).map(|l| format!("{l}\n")).collect();
            write_crontab(&text)?;
            Ok(Vec::new())
        }
        Source::LoginScript => {
            // The script is copied to CleanCore's data folder first, then the line is dropped.
            let (file, line) = script_location(e)?;
            rewrite_script(&file, &line, |_| None)?;
            Ok(Vec::new())
        }
        _ => bail!("\"{}\" can't be deleted, only disabled", e.name),
    }
}

pub fn autorun_add(name: &str, command: &str, all_users: bool, method: AddMethod) -> Result<()> {
    if all_users {
        bail!("adding an autostart entry for all users needs administrator rights; add it for the current user instead");
    }
    if method == AddMethod::RunOnce {
        bail!("Linux has no run-once autostart; use an autostart entry");
    }
    let file: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let path = user_autostart().join(format!("{file}.desktop"));
    if path.exists() {
        bail!("an autostart entry named {name} already exists");
    }
    crate::config::ensure_dir(&user_autostart())?;
    fs::write(&path, format!("[Desktop Entry]\nType=Application\nName={name}\nExec={command}\nX-GNOME-Autostart-enabled=true\n"))?;
    Ok(())
}

/// `systemd-analyze blame` lines (`1min 2.345s foo.service`, `812ms bar.service`) → unit → ms.
fn parse_blame(text: &str) -> std::collections::HashMap<String, u64> {
    let mut out = std::collections::HashMap::new();
    for l in text.lines() {
        let words: Vec<&str> = l.split_whitespace().collect();
        let Some((unit, times)) = words.split_last() else { continue };
        let mut ms = 0.0;
        for t in times {
            let (n, scale) = if let Some(n) = t.strip_suffix("ms") {
                (n, 1.0)
            } else if let Some(n) = t.strip_suffix("us").or_else(|| t.strip_suffix("µs")) {
                (n, 0.001)
            } else if let Some(n) = t.strip_suffix("min") {
                (n, 60_000.0)
            } else if let Some(n) = t.strip_suffix('s') {
                (n, 1000.0)
            } else if let Some(n) = t.strip_suffix('h') {
                (n, 3_600_000.0)
            } else {
                (*t, f64::NAN)
            };
            ms += n.parse::<f64>().unwrap_or(f64::NAN) * scale;
        }
        if !times.is_empty() && ms.is_finite() {
            out.insert(unit.to_lowercase(), ms.round() as u64);
        }
    }
    out
}

/// How long each systemd unit took at the last startup. Autostart entries have no record; they
/// are estimated from their program's size.
pub fn boot_data() -> crate::autorun::BootData {
    let mut d = crate::autorun::BootData::default();
    for user in [false, true] {
        let mut c = Command::new("systemd-analyze");
        if user {
            c.arg("--user");
        }
        if let Ok(o) = c.args(["blame", "--no-pager"]).output() {
            d.delay.extend(parse_blame(&String::from_utf8_lossy(&o.stdout)));
        }
    }
    d
}

#[cfg(test)]
mod boot_tests {
    use super::*;

    #[test]
    fn reads_systemd_blame() {
        let m = parse_blame("1min 2.500s apt-daily.service\n    812ms docker.service\n  950us foo.service\nbroken line here\n");
        assert_eq!(m.get("apt-daily.service"), Some(&62_500));
        assert_eq!(m.get("docker.service"), Some(&812));
        assert_eq!(m.get("foo.service"), Some(&1));
        assert!(!m.contains_key("here"));
    }
}
