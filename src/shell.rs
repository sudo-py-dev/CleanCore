//! File-manager integration (Revo: "Install with Revo Uninstaller Pro" / "Uninstall with Revo
//! Uninstaller Pro" in Explorer's right-click menu).
//!
//! Windows: shell verbs under `HKCU\Software\Classes` for programs (.exe), installers (.msi),
//! shortcuts (.lnk) and folders. Linux: Nautilus scripts, a KDE/Dolphin service menu, and an
//! "Open With" entry for installers.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

fn gui_exe() -> Result<PathBuf> {
    Ok(crate::launch::program()?)
}

/// Whether the menu entries start `cleancore-gui`, the window's own program until 0.2.8.
pub fn uses_old_program() -> bool {
    #[cfg(windows)]
    let command: Option<String> =
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(format!(r"Software\Classes\{}\command", VERBS[0].0)).and_then(|k| k.get_value("")).ok();
    #[cfg(not(windows))]
    let command = std::fs::read_to_string(&linux_files()[0]).ok();
    command.is_some_and(|c| c.contains("cleancore-gui"))
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
const VERBS: &[(&str, &str, &str, &str)] = &[
    // (class, verb key, menu text, argument)
    (r"exefile\shell\CleanCoreTrace", "Install with CleanCore", "--trace", "%1"),
    (r"Msi.Package\shell\CleanCoreTrace", "Install with CleanCore", "--trace", "%1"),
    (r"exefile\shell\CleanCoreUninstall", "Uninstall with CleanCore", "--uninstall", "%1"),
    (r"lnkfile\shell\CleanCoreUninstall", "Uninstall with CleanCore", "--uninstall", "%1"),
    (r"Directory\shell\CleanCoreUninstall", "Uninstall with CleanCore", "--uninstall", "%1"),
];

#[cfg(windows)]
pub fn integrate(on: bool) -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    let classes =
        winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Classes", winreg::enums::KEY_ALL_ACCESS).context("opening HKCU\\Software\\Classes")?;
    let exe = gui_exe()?;
    for (key, text, flag, arg) in VERBS {
        if on {
            let (k, _) = classes.create_subkey(key)?;
            k.set_value("", &text.to_string())?;
            k.set_value("Icon", &exe.display().to_string())?;
            let (c, _) = k.create_subkey("command")?;
            c.set_value("", &format!("\"{}\" {flag} \"{arg}\"", exe.display()))?;
        } else {
            let _ = classes.delete_subkey_all(key);
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn is_integrated() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(format!(r"Software\Classes\{}", VERBS[0].0)).is_ok()
}

/// Remove the menu entries added under an earlier name of the app (`old`, e.g. "cleancore").
/// Returns whether there were any.
#[cfg(windows)]
pub fn remove_legacy(old: &str) -> bool {
    let Ok(classes) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Classes", winreg::enums::KEY_ALL_ACCESS) else {
        return false;
    };
    let mut found = false;
    for (key, ..) in VERBS {
        found |= classes.delete_subkey_all(key.replace("CleanCore", old)).is_ok();
    }
    found
}

// ---------------------------------------------------------------- Linux

#[cfg(not(windows))]
fn linux_files() -> Vec<PathBuf> {
    let data = dirs::data_dir().unwrap_or_default();
    vec![
        data.join("nautilus/scripts/Install with CleanCore"),
        data.join("nautilus/scripts/Uninstall with CleanCore"),
        data.join("kio/servicemenus/cleancore.desktop"),
        data.join("applications/cleancore-install.desktop"),
    ]
}

/// Remove the menu entries added under an earlier name of the app (`old`, e.g. "cleancore").
/// Returns whether there were any.
#[cfg(not(windows))]
pub fn remove_legacy(old: &str) -> bool {
    let data = dirs::data_dir().unwrap_or_default();
    let lower = old.to_lowercase();
    let files = [
        data.join(format!("nautilus/scripts/Install with {old}")),
        data.join(format!("nautilus/scripts/Uninstall with {old}")),
        data.join(format!("kio/servicemenus/{lower}.desktop")),
        data.join(format!("applications/{lower}-install.desktop")),
    ];
    let mut found = false;
    for f in files {
        found |= std::fs::remove_file(f).is_ok();
    }
    found
}

#[cfg(not(windows))]
pub fn integrate(on: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let files = linux_files();
    if !on {
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        return Ok(());
    }
    let exe = gui_exe()?;
    let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', r"'\''"));
    // Desktop-entry Exec quoting: double quotes, with " ` $ \\ escaped.
    let dq = |p: &Path| {
        let mut out = String::from("\"");
        for c in p.display().to_string().chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out + "\""
    };
    let script = |flag: &str| format!("#!/bin/sh\n# Added by CleanCore (Settings › Uninstaller › file manager menu).\nexec {} {flag} \"$1\"\n", q(&exe));
    let installers = "application/x-executable;application/x-sharedlib;application/x-shellscript;application/vnd.debian.binary-package;application/x-rpm;application/vnd.flatpak.ref;application/vnd.appimage;application/x-msi;application/x-ms-dos-executable;";
    let contents = [
        script("--trace"),
        script("--uninstall"),
        format!(
            "[Desktop Entry]\nType=Service\nMimeType=application/octet-stream;inode/directory;application/x-desktop;{installers}\nActions=cleancoreInstall;cleancoreUninstall;\nX-KDE-Submenu=CleanCore\n\n[Desktop Action cleancoreInstall]\nName=Install with CleanCore\nIcon=system-software-install\nExec={} --trace %f\n\n[Desktop Action cleancoreUninstall]\nName=Uninstall with CleanCore\nIcon=edit-delete\nExec={} --uninstall %f\n",
            dq(&exe),
            dq(&exe)
        ),
        format!("[Desktop Entry]\nType=Application\nName=Install with CleanCore\nComment=Run the installer while CleanCore records what it changes\nExec={} --trace %f\nMimeType={installers}\nNoDisplay=true\nTerminal=false\n", dq(&exe)),
    ];
    for (f, text) in files.iter().zip(contents) {
        crate::config::ensure_dir(f.parent().unwrap())?;
        std::fs::write(f, text).with_context(|| format!("writing {}", f.display()))?;
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn is_integrated() -> bool {
    linux_files()[0].exists()
}

// ---------------------------------------------------------------- "Uninstall with": what was clicked?

/// The program a clicked file stands for: a shortcut's / launcher's target, or the file itself.
pub fn resolve_target(path: &Path) -> PathBuf {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "lnk" => lnk_target(path).unwrap_or_else(|| path.to_path_buf()),
        "desktop" => std::fs::read_to_string(path)
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=").map(str::to_string)))
            .and_then(|exec| crate::pathutil::desktop_exec_program(&exec))
            .map(|prog| if prog.starts_with('/') { PathBuf::from(prog) } else { crate::pathutil::which(&prog, &Default::default()).unwrap_or(PathBuf::from(prog)) })
            .unwrap_or_else(|| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Target path of a Windows shortcut, read from its LinkInfo (`LocalBasePath`).
/// Shell-namespace-only shortcuts (no LinkInfo) return `None`.
pub fn lnk_target(path: &Path) -> Option<PathBuf> {
    let b = std::fs::read(path).ok()?;
    let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    if u32_at(0)? != 0x4C {
        return None;
    }
    let flags = u32_at(0x14)?;
    let mut off = 0x4C;
    if flags & 0x01 != 0 {
        // HasLinkTargetIDList: skip the IDList.
        off += 2 + u16_at(off)? as usize;
    }
    if flags & 0x02 == 0 {
        return None; // no LinkInfo
    }
    let info = off;
    let info_flags = u32_at(info + 8)?;
    if info_flags & 0x01 == 0 {
        return None; // not a local path (network)
    }
    let header = u32_at(info + 4)? as usize;
    // Prefer the Unicode path when present (header ≥ 0x24).
    if header >= 0x24 {
        let o = info + u32_at(info + 0x1C)? as usize;
        let mut w = Vec::new();
        let mut i = o;
        while let Some(c) = u16_at(i).filter(|c| *c != 0) {
            w.push(c);
            i += 2;
        }
        if !w.is_empty() {
            return Some(PathBuf::from(String::from_utf16_lossy(&w)));
        }
    }
    let o = info + u32_at(info + 0x10)? as usize;
    let end = b[o..].iter().position(|c| *c == 0)? + o;
    Some(PathBuf::from(String::from_utf8_lossy(&b[o..end]).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lnk_local_base_path() {
        // Minimal shell link: header, no IDList, LinkInfo with an ANSI LocalBasePath.
        let target = b"C:\\Program Files\\App\\app.exe\0";
        let mut info = Vec::new();
        let header_size = 0x1C_u32;
        let base_off = header_size + 4; // after a 4-byte (empty) VolumeID stub
        let total = base_off + target.len() as u32 + 1;
        for v in [total, header_size, 1, header_size, base_off, 0, total - 1] {
            info.extend_from_slice(&v.to_le_bytes());
        }
        info.extend_from_slice(&[0, 0, 0, 0]);
        info.extend_from_slice(target);
        info.push(0);
        let mut b = vec![0u8; 0x4C];
        b[0] = 0x4C;
        b[0x14] = 0x02; // HasLinkInfo
        b.extend(info);
        let f = std::env::temp_dir().join(format!("cleancore-{}.lnk", std::process::id()));
        std::fs::write(&f, b).unwrap();
        assert_eq!(lnk_target(&f), Some(PathBuf::from(r"C:\Program Files\App\app.exe")));
        let _ = std::fs::remove_file(f);
    }
}
