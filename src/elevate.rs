//! Administrator rights, asked for once.
//!
//! Linux: removing packages needs root. Instead of one `pkexec` (and one password prompt) per
//! command, the first command starts a small root helper (`cleancore root-helper`) through
//! `pkexec`; it stays up until CleanCore exits and runs the rest. It accepts only package
//! removal commands with a plain package name, so nothing else can be run through it.
//!
//! Windows: the desktop app asks for elevation once at start (`relaunch_elevated`), so the
//! programs' uninstallers it starts don't each ask again.

#[cfg(not(windows))]
use anyhow::{bail, Context, Result};

/// Run a package-manager command as root and return its exit code. `args[0]` is the program.
#[cfg(not(windows))]
pub fn run_as_root(args: &[&str]) -> Result<Option<i32>> {
    use std::io::{BufRead, Write};

    if !allowed(args) {
        bail!("not a command CleanCore runs as administrator: {}", args.join(" "));
    }
    if is_root() {
        return run_package_sequence(args, |command| execute_package(command, false));
    }
    // The command line in a terminal uses sudo, which asks once and remembers the password for
    // a while by itself. The desktop app always uses the graphical prompt.
    if (!crate::launch::in_window() && std::io::IsTerminal::is_terminal(&std::io::stdin())) || which("pkexec").is_none() {
        return run_package_sequence(args, |command| execute_package(command, true));
    }
    let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
    // One attempt to (re)start the helper, then send the command.
    for attempt in 0..2 {
        if guard.as_mut().is_none_or(|h| h.child.try_wait().ok().flatten().is_some()) {
            *guard = Some(Helper::start()?);
        }
        let h = guard.as_mut().unwrap();
        let line = serde_json::to_string(&args)? + "\n";
        if h.stdin.write_all(line.as_bytes()).and_then(|_| h.stdin.flush()).is_err() {
            *guard = None;
            continue;
        }
        let mut reply = String::new();
        if h.stdout.read_line(&mut reply).unwrap_or(0) == 0 {
            *guard = None;
            if attempt == 0 {
                continue;
            }
            bail!("the administrator helper stopped");
        }
        let v: serde_json::Value = serde_json::from_str(reply.trim()).context("reading the administrator helper's answer")?;
        if let Some(e) = v["error"].as_str() {
            bail!("{e}");
        }
        return Ok(v["code"].as_i64().map(|c| c as i32));
    }
    bail!("could not reach the administrator helper")
}

#[cfg(not(windows))]
struct Helper {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

#[cfg(not(windows))]
static HELPER: std::sync::Mutex<Option<Helper>> = std::sync::Mutex::new(None);

#[cfg(not(windows))]
impl Helper {
    /// Start the helper through pkexec: this is where the password is asked for.
    fn start() -> Result<Self> {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let exe = helper_exe()?;
        let mut child = Command::new("pkexec").arg(&exe).arg("root-helper").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().context("starting pkexec")?;
        let stdin = child.stdin.take().context("helper input")?;
        let mut stdout = std::io::BufReader::new(child.stdout.take().context("helper output")?);
        // The helper says "ready" once it runs as root; pkexec exits instead if the password
        // prompt was cancelled.
        let mut first = String::new();
        stdout.read_line(&mut first).ok();
        if first.trim() != "ready" {
            let _ = child.wait();
            bail!("administrator rights were not granted");
        }
        Ok(Self { child, stdin, stdout })
    }
}

/// This program, which hosts the helper (`cleancore root-helper`).
#[cfg(not(windows))]
fn helper_exe() -> Result<std::path::PathBuf> {
    let exe = crate::launch::program().context("locating CleanCore")?;
    if !exe.exists() {
        bail!("{} is missing", exe.display());
    }
    Ok(exe)
}

/// The root side: run allowed commands read from stdin until it closes (CleanCore exited).
#[cfg(not(windows))]
pub fn serve() -> Result<()> {
    use std::io::{BufRead, Write};
    if !is_root() {
        bail!("the administrator helper must run as root (it is started through pkexec)");
    }
    let mut out = std::io::stdout().lock();
    writeln!(out, "ready")?;
    out.flush()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let reply = match serde_json::from_str::<Vec<String>>(&line) {
            Ok(args) => {
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                if !allowed(&a) {
                    serde_json::json!({ "error": format!("refused: {}", a.join(" ")) })
                } else {
                    match run_package_sequence(&a, |command| execute_package(command, false)) {
                        Ok(code) => serde_json::json!({ "code": code }),
                        Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
                    }
                }
            }
            Err(e) => serde_json::json!({ "error": format!("bad request: {e}") }),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(not(windows))]
struct CommandReport {
    code: Option<i32>,
    autoremove_suggested: bool,
}

#[cfg(not(windows))]
fn run_package_sequence(args: &[&str], mut run: impl FnMut(&[&str]) -> Result<CommandReport>) -> Result<Option<i32>> {
    anyhow::ensure!(allowed(args), "package command refused");
    let result = run(args)?;
    // Only a successful APT removal can trigger the fixed follow-up. Never execute a
    // command parsed from package output, and never retry a failed package removal.
    if matches!(args, ["apt-get", "remove" | "purge", "-y", _]) && result.code == Some(0) && result.autoremove_suggested {
        return Ok(run(&["apt-get", "autoremove", "-y"])?.code);
    }
    Ok(result.code)
}

#[cfg(not(windows))]
#[derive(Default)]
struct AptSuggestion {
    tail: Vec<u8>,
    found: bool,
}

#[cfg(not(windows))]
impl AptSuggestion {
    fn feed(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        let text = String::from_utf8_lossy(&self.tail);
        self.found |= [
            "Use 'apt autoremove' to remove them.",
            "Use 'sudo apt autoremove' to remove them.",
            "Use 'apt-get autoremove' to remove them.",
            "Use 'sudo apt-get autoremove' to remove them.",
        ]
        .iter()
        .any(|message| text.contains(message));
        // Keep enough overlap to recognize a suggestion split across reads, without
        // retaining a potentially huge package-manager log in memory.
        if self.tail.len() > 128 {
            self.tail.drain(..self.tail.len() - 128);
        }
    }
}

#[cfg(not(windows))]
fn execute_package(args: &[&str], sudo: bool) -> Result<CommandReport> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut command = Command::new(if sudo { "sudo" } else { args[0] });
    command.args(if sudo { args } else { &args[1..] }).env("DEBIAN_FRONTEND", "noninteractive").stdin(Stdio::null());
    if args[0] != "apt-get" {
        let status = command.stdout(std::io::stderr()).status().with_context(|| format!("starting {}", args[0]))?;
        return Ok(CommandReport { code: status.code(), autoremove_suggested: false });
    }
    // Keep package output in a known language for reliable detection. UI translations
    // are independent of the package-manager process locale.
    let mut child = command.env("LC_ALL", "C").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("starting APT")?;
    let reader = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || -> std::io::Result<bool> {
            let mut detector = AptSuggestion::default();
            let mut buffer = [0u8; 4096];
            loop {
                let n = pipe.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                detector.feed(&buffer[..n]);
                // stdout is the root helper's JSON protocol; command logs must use stderr.
                let _ = std::io::stderr().write_all(&buffer[..n]);
            }
            Ok(detector.found)
        })
    };
    let stdout = reader(Box::new(child.stdout.take().context("APT stdout")?));
    let stderr = reader(Box::new(child.stderr.take().context("APT stderr")?));
    let status = child.wait().context("waiting for APT")?;
    let suggested = stdout.join().map_err(|_| anyhow::anyhow!("APT output reader failed"))??;
    let suggested_err = stderr.join().map_err(|_| anyhow::anyhow!("APT output reader failed"))??;
    Ok(CommandReport { code: status.code(), autoremove_suggested: suggested || suggested_err })
}

/// The only commands that run as root: removing a package by name.
pub fn allowed(args: &[&str]) -> bool {
    fn package(s: &str) -> bool {
        // Debian/RPM/pacman/snap package names: no options, paths or shell characters.
        let s = s.split_once(':').map_or(s, |(name, arch)| if arch.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { name } else { "" });
        !s.is_empty() && s.len() <= 200 && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-._@".contains(c))
    }
    match args {
        ["apt-get", "remove" | "purge", "-y", p] => package(p),
        ["apt-get", "autoremove", "-y"] => true,
        ["dpkg", "-r" | "--purge", p] => package(p),
        ["dnf" | "dnf5" | "yum", "remove", "-y", p] => package(p),
        ["zypper", "-n", "remove", p] => package(p),
        ["rpm", "-e", p] => package(p),
        ["pacman", "-R" | "-Rns", "--noconfirm", p] => package(p),
        ["apk", "del", "--", p] => package(p),
        ["xbps-remove", "-y", p] => package(p),
        ["emerge", "--unmerge", "--ask=n", "--", atom] => atom.strip_prefix('=').and_then(|p| p.split_once('/')).is_some_and(|(category, name)| package(category) && package(name)),
        ["snap", "remove", p] | ["snap", "remove", "--purge", p] => package(p),
        _ => false,
    }
}

#[cfg(not(windows))]
fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
}

#[cfg(not(windows))]
fn which(cmd: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(cmd)).find(|f| f.is_file()))
}

/// Windows: start this program again elevated (one UAC prompt) and say whether it did.
/// Returns false when already elevated, or when the prompt was declined.
#[cfg(windows)]
pub fn relaunch_elevated() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // SAFETY: no arguments.
    if unsafe { IsUserAnAdmin() } != 0 || std::env::var_os("CLEANCORE_NO_ELEVATE").is_some() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let args: Vec<String> = std::env::args().skip(1).map(|a| if a.contains([' ', '"']) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a }).collect();
    let (verb, file, params) = (wide("runas".as_ref()), wide(exe.as_os_str()), wide(args.join(" ").as_ref()));
    // SAFETY: valid, NUL-terminated wide strings that outlive the call.
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
    // Greater than 32 means the elevated copy started; declining the prompt gives an error.
    r as usize > 32
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[cfg(not(windows))]
    #[test]
    fn apt_suggestion_is_detected_across_reads_with_bounded_memory() {
        let mut detector = super::AptSuggestion::default();
        detector.feed(&vec![b'x'; 100_000]);
        assert!(detector.tail.len() <= 128);
        detector.feed(b"Use 'apt auto");
        detector.feed(b"remove' to remove them.\n");
        assert!(detector.found);
        let mut unrelated = super::AptSuggestion::default();
        unrelated.feed(b"apt autoremove is a command you could run");
        assert!(!unrelated.found);
    }

    #[cfg(not(windows))]
    #[test]
    fn autoremove_runs_once_only_after_a_successful_apt_suggestion() {
        for (args, code, suggested, expected) in [
            (vec!["apt-get", "remove", "-y", "vlc"], Some(0), true, 2),
            (vec!["apt-get", "purge", "-y", "vlc"], Some(0), true, 2),
            (vec!["apt-get", "remove", "-y", "vlc"], Some(100), true, 1),
            (vec!["apt-get", "remove", "-y", "vlc"], Some(0), false, 1),
            (vec!["snap", "remove", "vlc"], Some(0), true, 1),
            (vec!["apt-get", "autoremove", "-y"], Some(0), true, 1),
        ] {
            let mut calls = Vec::new();
            let result = super::run_package_sequence(&args, |command| {
                calls.push(command.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                Ok(super::CommandReport { code, autoremove_suggested: suggested })
            })
            .unwrap();
            assert_eq!(calls.len(), expected);
            assert_eq!(result, code);
            if expected == 2 {
                assert_eq!(calls[1], ["apt-get", "autoremove", "-y"]);
            }
        }
        let mut count = 0;
        let result = super::run_package_sequence(&["apt-get", "remove", "-y", "vlc"], |_| {
            count += 1;
            Ok(super::CommandReport { code: Some(if count == 1 { 0 } else { 100 }), autoremove_suggested: true })
        })
        .unwrap();
        assert_eq!(result, Some(100), "a failed autoremove must not be reported as success");
    }

    #[test]
    fn only_package_removal_is_allowed() {
        assert!(allowed(&["apt-get", "remove", "-y", "vlc"]));
        assert!(allowed(&["apt-get", "autoremove", "-y"]));
        assert!(!allowed(&["apt-get", "autoremove", "-y", "--purge"]));
        assert!(allowed(&["apk", "del", "--", "firefox"]));
        assert!(allowed(&["xbps-remove", "-y", "firefox"]));
        assert!(allowed(&["emerge", "--unmerge", "--ask=n", "--", "=www-client/firefox-140.0"]));
        assert!(!allowed(&["emerge", "--unmerge", "--ask=n", "--", "=../etc/passwd"]));
        assert!(!allowed(&["apk", "del", "--", "--rdepends"]));
        assert!(allowed(&["apt-get", "purge", "-y", "libfoo2:amd64"]));
        assert!(allowed(&["snap", "remove", "--purge", "firefox"]));
        assert!(allowed(&["pacman", "-Rns", "--noconfirm", "gimp"]));
        assert!(!allowed(&["apt-get", "install", "-y", "evil"]));
        assert!(!allowed(&["apt-get", "remove", "-y", "-o", "APT::Update::Pre-Invoke=sh"]));
        assert!(!allowed(&["apt-get", "remove", "-y", "-oFoo=bar"]));
        assert!(!allowed(&["apt-get", "remove", "-y", "../../bin/sh"]));
        assert!(!allowed(&["apt-get", "remove", "-y", "vlc; rm -rf /"]));
        assert!(!allowed(&["rm", "-rf", "/"]));
        assert!(!allowed(&["sh", "-c", "id"]));
        assert!(!allowed(&["dpkg", "-r", "vlc", "extra"]));
    }
}
