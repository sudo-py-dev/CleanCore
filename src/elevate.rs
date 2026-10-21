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
        let st = std::process::Command::new(args[0]).args(&args[1..]).env("DEBIAN_FRONTEND", "noninteractive").status().with_context(|| format!("starting {}", args[0]))?;
        return Ok(st.code());
    }
    // The command line in a terminal uses sudo, which asks once and remembers the password for
    // a while by itself. The desktop app always uses the graphical prompt.
    let gui = std::env::current_exe().is_ok_and(|e| e.file_stem().is_some_and(|s| s == "cleancore-gui"));
    if (!gui && std::io::IsTerminal::is_terminal(&std::io::stdin())) || which("pkexec").is_none() {
        let st = std::process::Command::new("sudo").args(args).env("DEBIAN_FRONTEND", "noninteractive").status().context("starting sudo")?;
        return Ok(st.code());
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

/// The command-line binary, which hosts the helper (it sits next to the desktop app).
#[cfg(not(windows))]
fn helper_exe() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("locating CleanCore")?;
    let cli = if exe.file_stem().is_some_and(|s| s == "cleancore-gui") { exe.with_file_name("cleancore") } else { exe };
    if !cli.exists() {
        bail!("{} is missing", cli.display());
    }
    Ok(cli)
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
                    // The command's own output goes to stderr, so it can't disturb the replies.
                    match std::process::Command::new(a[0])
                        .args(&a[1..])
                        .env("DEBIAN_FRONTEND", "noninteractive")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::io::stderr())
                        .status()
                    {
                        Ok(st) => serde_json::json!({ "code": st.code() }),
                        Err(e) => serde_json::json!({ "error": format!("starting {}: {e}", a[0]) }),
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

/// The only commands that run as root: removing a package by name.
pub fn allowed(args: &[&str]) -> bool {
    fn package(s: &str) -> bool {
        // Debian/RPM/pacman/snap package names: no options, paths or shell characters.
        let s = s.split_once(':').map_or(s, |(name, arch)| if arch.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { name } else { "" });
        !s.is_empty() && s.len() <= 200 && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-._@".contains(c))
    }
    match args {
        ["apt-get", "remove" | "purge", "-y", p] => package(p),
        ["dpkg", "-r" | "--purge", p] => package(p),
        ["dnf", "remove", "-y", p] => package(p),
        ["zypper", "-n", "remove", p] => package(p),
        ["rpm", "-e", p] => package(p),
        ["pacman", "-R" | "-Rns", "--noconfirm", p] => package(p),
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

    #[test]
    fn only_package_removal_is_allowed() {
        assert!(allowed(&["apt-get", "remove", "-y", "vlc"]));
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
