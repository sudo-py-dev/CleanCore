//! Self-update from GitHub releases, verified before anything is installed.
//!
//! Every release carries `SHA256SUMS` (the hash of each download) and `SHA256SUMS.minisig`,
//! a minisign (Ed25519) signature of that file made with the project's release key. The
//! public half of the key is built in below, so an update is installed only when:
//!
//! 1. `SHA256SUMS` is signed by the release key (a changed or forged list fails here), and
//! 2. the downloaded archive or package has the SHA-256 that the signed list gives for it.
//!
//! Downloads go through the system's `curl` over HTTPS (Linux, and Windows 10 and later),
//! which keeps a TLS stack out of the binary; the signature, not the connection, is what
//! makes the update trustworthy.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The GitHub repository releases come from.
pub const REPO: &str = "sudo-py-dev/CleanCore";

/// minisign public key of the release key (`minisign.pub`, second line).
pub const PUBLIC_KEY: &str = "RWTi8daL7pc8GWEO6wwOcIsus1vPWfElEFJJ+sot5nS6M+eAz4ld0m4T";

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

const SUMS: &str = "SHA256SUMS.txt";
const SIG: &str = "SHA256SUMS.txt.minisig";

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub url: String,
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&str> {
        self.assets.iter().find(|(n, _)| n == name).map(|(_, u)| u.as_str())
    }
}

/// How this copy was installed, which decides what to download and how to put it in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// The .deb package (binaries in /usr/bin, owned by dpkg).
    Deb,
    /// An unpacked .tar.gz (Linux) or .zip (Windows): the binaries are replaced in place.
    Archive,
    /// Installed with the Windows setup wizard: the newer setup runs over it, silently.
    Setup,
}

pub fn install_kind() -> Install {
    let exe = std::env::current_exe().unwrap_or_default();
    if cfg!(target_os = "linux") && exe.starts_with("/usr/bin") && Path::new("/var/lib/dpkg/info/cleancore.list").exists() {
        Install::Deb
    } else if cfg!(windows) && exe.parent().is_some_and(|d| d.join("unins000.exe").exists()) {
        Install::Setup
    } else {
        Install::Archive
    }
}

/// The file of `release` meant for this system and install kind.
pub fn asset_name(version: &str, kind: Install) -> String {
    match (cfg!(windows), kind) {
        (true, Install::Setup) => format!("cleancore-{version}-windows-x64-setup.exe"),
        (true, _) => format!("cleancore-{version}-windows-x64.zip"),
        (false, Install::Deb) => format!("cleancore_{version}_amd64.deb"),
        (false, _) => format!("cleancore-{version}-linux-x86_64.tar.gz"),
    }
}

/// `a` is a later version than `b` (dotted numbers; a "v" prefix and any "-suffix" ignored).
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim_start_matches('v').split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::platform::hidden(if cfg!(windows) { "curl.exe" } else { "curl" })
        .args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--tlsv1.2", "--max-time", "300"])
        .args(args)
        .output()
        .context("running curl (needed to check for updates)")?;
    if !out.status.success() {
        bail!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// The latest published release: from GitHub's API or, when that fails (its rate limit, an
/// outage), from where the releases page's "latest" link leads.
pub fn latest() -> Result<Release> {
    latest_from_api().or_else(|api| latest_from_page().with_context(|| format!("{api:#}; the releases page did not answer either")))
}

fn latest_from_api() -> Result<Release> {
    let body =
        curl(&["-H", "Accept: application/vnd.github+json", "-H", &format!("User-Agent: cleancore/{CURRENT}"), &format!("https://api.github.com/repos/{REPO}/releases/latest")])?;
    let v: serde_json::Value = serde_json::from_slice(&body).context("reading the release information")?;
    let tag = v["tag_name"].as_str().context("the release has no tag")?.to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        url: v["html_url"].as_str().unwrap_or("").to_string(),
        tag,
        assets,
    })
}

/// The release the releases page's "latest" link redirects to. Its downloads have fixed names,
/// so they are known without the API (its notes are not).
fn latest_from_page() -> Result<Release> {
    let out = curl(&["--head", "--write-out", "\n%{url_effective}", &format!("https://github.com/{REPO}/releases/latest")])?;
    let url = String::from_utf8_lossy(&out).lines().last().unwrap_or("").trim().to_string();
    let tag = url.rsplit_once("/releases/tag/").map(|(_, t)| t.to_string()).filter(|t| !t.is_empty()).context("the releases page shows no release")?;
    let version = tag.trim_start_matches('v').to_string();
    let assets = [SUMS.to_string(), SIG.to_string(), asset_name(&version, install_kind())]
        .into_iter()
        .map(|name| {
            let url = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
            (name, url)
        })
        .collect();
    Ok(Release { version, notes: String::new(), url, tag, assets })
}

/// The latest release if it is newer than this build.
pub fn check() -> Result<Option<Release>> {
    let r = latest()?;
    Ok(is_newer(&r.version, CURRENT).then_some(r))
}

/// The SHA-256 the signed list gives for `name`, after checking the list's signature.
pub fn verified_sum(sums: &[u8], signature: &str, name: &str) -> Result<String> {
    let pk = minisign_verify::PublicKey::from_base64(PUBLIC_KEY).context("the built-in release key is invalid")?;
    let sig = minisign_verify::Signature::decode(signature).context("the release signature is unreadable")?;
    pk.verify(sums, &sig, false).map_err(|_| anyhow::anyhow!("the release's checksum list is not signed by the CleanCore release key; not installing"))?;
    let text = std::str::from_utf8(sums).context("the checksum list is not text")?;
    text.lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, file)| file.trim().trim_start_matches('*') == name)
        .map(|(sum, _)| sum.to_ascii_lowercase())
        .with_context(|| format!("{name} is not in the signed checksum list"))
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn work_dir() -> Result<PathBuf> {
    let d = crate::config::data_dir().join("update");
    let _ = std::fs::remove_dir_all(&d);
    crate::config::ensure_dir(&d)?;
    Ok(d)
}

/// Download this system's file of `release` and verify it. Returns the verified file.
pub fn download(release: &Release) -> Result<PathBuf> {
    let name = asset_name(&release.version, install_kind());
    let url = release.asset(&name).with_context(|| format!("the release has no {name}"))?;
    let sums_url = release.asset(SUMS).context("the release has no checksum list")?;
    let sig_url = release.asset(SIG).context("the release is not signed")?;
    let sums = curl(&[sums_url])?;
    let sig = String::from_utf8(curl(&[sig_url])?).context("the signature is not text")?;
    let want = verified_sum(&sums, &sig, &name)?;
    let file = work_dir()?.join(&name);
    curl(&["--output", &file.to_string_lossy(), url])?;
    let got = sha256_file(&file)?;
    if got != want {
        let _ = std::fs::remove_file(&file);
        bail!("{}", crate::tf!("{name} does not match its signed checksum, so it was not installed."));
    }
    Ok(file)
}

/// What comes after `install` worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The new version is in place: start it (`relaunch`) and close this one.
    InPlace,
    /// The setup wizard is updating the install: close CleanCore so it can replace the files.
    /// It starts CleanCore again when it is done.
    BySetup,
}

impl Installed {
    /// What to tell someone who updated from the command line.
    pub fn message(self) -> &'static str {
        match self {
            Installed::InPlace => crate::i18n::tr("Updated. Restart CleanCore to use the new version."),
            Installed::BySetup => crate::i18n::tr("Installing the update. CleanCore closes and starts again when it is done."),
        }
    }
}

/// Put a verified download in place.
pub fn install(file: &Path) -> Result<Installed> {
    if file.extension().is_some_and(|e| e == "deb") {
        // Root is needed for /usr/bin: ask through the desktop's password prompt.
        let st = Command::new("pkexec").args(["apt-get", "install", "-y", "--allow-downgrades"]).arg(file).status().context("running pkexec")?;
        if !st.success() {
            bail!("{}", crate::i18n::tr("The package manager did not install the update."));
        }
        return Ok(Installed::InPlace);
    }
    if file.extension().is_some_and(|e| e == "exe") {
        // The setup wizard, without its pages: it updates the install (and its entry in Apps &
        // features) once CleanCore has closed, then starts CleanCore again (its [Run] entry for
        // silent setups). Windows' Restart Manager isn't asked to restart it too: CleanCore
        // doesn't register with it, and the [Run] entry would start a second copy.
        Command::new(file).args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS"]).spawn().context("starting the setup")?;
        return Ok(Installed::BySetup);
    }
    let unpacked = file.with_extension("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    crate::config::ensure_dir(&unpacked)?;
    // bsdtar on Windows reads .zip; GNU tar on Linux reads .tar.gz.
    let st = crate::platform::hidden("tar").arg("-xf").arg(file).arg("-C").arg(&unpacked).status().context("running tar")?;
    if !st.success() {
        bail!("{}", crate::i18n::tr("Could not unpack the update."));
    }
    let dir = std::env::current_exe()?.parent().context("locating the program folder")?.to_path_buf();
    let names: &[&str] = if cfg!(windows) { &["cleancore.exe", "cleancore-gui.exe"] } else { &["cleancore", "cleancore-gui"] };
    for n in names {
        let Some(new) = find_file(&unpacked, n) else { continue };
        let dest = dir.join(n);
        // A running program can't be overwritten on Windows, but it can be renamed.
        let old = dest.with_extension("old");
        let _ = std::fs::remove_file(&old);
        if dest.exists() {
            std::fs::rename(&dest, &old).with_context(|| format!("replacing {}", dest.display()))?;
        }
        if let Err(e) = std::fs::copy(&new, &dest) {
            let _ = std::fs::rename(&old, &dest);
            return Err(e).with_context(|| format!("writing {}", dest.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_file(&old);
        }
    }
    let _ = std::fs::remove_dir_all(&unpacked);
    Ok(Installed::InPlace)
}

/// Start the CleanCore window again, from where this copy runs: after an update in place,
/// that is the new version.
pub fn relaunch() -> Result<()> {
    let gui = gui_beside(&std::env::current_exe().context("locating CleanCore")?);
    Command::new(&gui).spawn().with_context(|| format!("starting {}", gui.display()))?;
    Ok(())
}

/// The CleanCore window's program next to `exe`, the running one (the window, or the command
/// line tool).
fn gui_beside(exe: &Path) -> PathBuf {
    // Linux names a running program whose file was replaced "<path> (deleted)".
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)").to_string());
    exe.with_file_name(if cfg!(windows) { "cleancore-gui.exe" } else { "cleancore-gui" })
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

/// Remove the copies a Windows update left behind (`*.old` next to the program).
pub fn cleanup_after_update() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        for n in ["cleancore.old", "cleancore-gui.old"] {
            let _ = std::fs::remove_file(dir.join(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn restarts_the_window_from_a_replaced_program() {
        assert_eq!(gui_beside(Path::new("/usr/bin/cleancore-gui (deleted)")), PathBuf::from("/usr/bin/cleancore-gui"));
        assert_eq!(gui_beside(Path::new("/opt/cleancore/cleancore")), PathBuf::from("/opt/cleancore/cleancore-gui"));
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    /// A list signed with the real release key (test vector; the key itself stays secret).
    const SIGNED: &[u8] = b"f00d  cleancore-9.9.9-linux-x86_64.tar.gz\nbeef  cleancore-9.9.9-windows-x64.zip\n";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key
RUTi8daL7pc8GZY/JAgZ16zwMeUX81Pz1CjBI3/yuSwzFj7X8FevzcRbdCUcfjVi4YiI5hlPDX+vK/wEFPitHKK1DrNlYbU0RwQ=
trusted comment: CleanCore test vector
ZybNRz7XbGBsZm4R3mtwVFKgTMgpxpcRHSIjhvqpUhyaMWkMgRILvTB5aCAcPwCPjFtKLg2cLc39acK4yR3GDQ==
";

    #[test]
    fn accepts_the_release_signature() {
        assert_eq!(verified_sum(SIGNED, SIGNATURE, "cleancore-9.9.9-windows-x64.zip").unwrap(), "beef");
        assert!(verified_sum(SIGNED, SIGNATURE, "something-else.zip").is_err());
        // One changed byte in the list breaks the signature.
        let mut tampered = SIGNED.to_vec();
        tampered[0] = b'e';
        assert!(verified_sum(&tampered, SIGNATURE, "cleancore-9.9.9-linux-x86_64.tar.gz").is_err());
    }

    #[test]
    fn rejects_unsigned_or_tampered_lists() {
        let sums = b"abc123  cleancore-0.2.0-linux-x86_64.tar.gz\n";
        // Not a minisign signature at all.
        assert!(verified_sum(sums, "garbage", "cleancore-0.2.0-linux-x86_64.tar.gz").is_err());
        // A well-formed signature by some other key.
        let other = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3r3PZXvNCkLtSqxASvPsyMsE6mxsZfZNEoAvLTUZYn+wo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n";
        assert!(verified_sum(sums, other, "cleancore-0.2.0-linux-x86_64.tar.gz").is_err());
    }
}
