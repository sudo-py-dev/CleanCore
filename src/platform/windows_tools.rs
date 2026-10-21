//! Windows side of the phase-4 tools: drives, Recycle Bin, clipboard, the system's history
//! items (MRU lists, temp files, thumbnails, crash dumps, Office MRU) and Windows Tools.

use super::windows::{join, open, subkeys};
use super::Drive;
use crate::history::{HistoryItem, Target};
use crate::model::Fix;
use crate::systools::Tool;
use anyhow::{bail, Context, Result};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const NO_WINDOW: u32 = 0x0800_0000;
const HKCU: &str = "HKEY_CURRENT_USER";
const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn env(v: &str) -> Option<PathBuf> {
    std::env::var_os(v).map(PathBuf::from)
}

pub fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut v = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        // SAFETY: `w` is NUL-terminated.
        let ty = unsafe { GetDriveTypeW(w.as_ptr()) };
        if ty != DRIVE_FIXED && ty != DRIVE_REMOVABLE {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: valid out-pointers; the third is optional.
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } == 0 {
            continue;
        }
        let mut name = [0u16; 261];
        let mut fs = [0u16; 261];
        // SAFETY: buffers and sizes match; unused outputs are null.
        unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]);
        let label = s(&name);
        v.push(Drive {
            label: if label.is_empty() { format!("Local Disk ({})", &root[..2]) } else { format!("{label} ({})", &root[..2]) },
            path: PathBuf::from(&root),
            fs: s(&fs),
            total,
            free,
            removable: ty == DRIVE_REMOVABLE,
        });
    }
    v
}

pub fn free_space(path: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = wide(&path.display().to_string());
    let mut free = 0u64;
    // SAFETY: valid pointers; the optional outputs are null.
    (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } != 0).then_some(free)
}

pub fn empty_trash() -> Result<()> {
    use windows_sys::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
    // SAFETY: null window and root mean "all drives, no UI".
    let r = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND) };
    // S_OK, or E_UNEXPECTED when it is already empty.
    if r != 0 && r != 0x8000_FFFFu32 as i32 {
        bail!("emptying the Recycle Bin failed ({r:#x})");
    }
    Ok(())
}

pub fn clear_clipboard() -> Result<()> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
    // SAFETY: open/empty/close on the calling thread with no owner window.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            bail!("the clipboard is in use by another program");
        }
        let ok = EmptyClipboard();
        CloseClipboard();
        if ok == 0 {
            bail!("the clipboard could not be cleared");
        }
    }
    Ok(())
}

fn key_fix(sub: &str) -> Option<Target> {
    let key = join(HKCU, sub);
    open(&key).map(|_| Target::Fix(Fix::DeleteRegKey { key }))
}

fn files_matching(dir: &Path, prefix: &str) -> Vec<Target> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(prefix)).map(|e| Target::Path(e.path())).collect())
        .unwrap_or_default()
}

pub fn system_history_items() -> Vec<HistoryItem> {
    let g = "Windows";
    let appdata = env("APPDATA").unwrap_or_default();
    let local = env("LOCALAPPDATA").unwrap_or_default();
    let win = env("SystemRoot").unwrap_or(PathBuf::from(r"C:\Windows"));
    let pdata = env("ProgramData").unwrap_or(PathBuf::from(r"C:\ProgramData"));
    let temp = env("TEMP").unwrap_or(local.join("Temp"));
    let reg = |subs: &[&str]| subs.iter().filter_map(|s| key_fix(&format!(r"{EXPLORER}\{s}"))).collect::<Vec<_>>();
    let mut v = vec![
        HistoryItem::new("windows:recent", g, "Recent documents", "Recent files and jump lists", true, {
            let mut t = vec![Target::Contents(appdata.join(r"Microsoft\Windows\Recent"))];
            t.extend(reg(&["RecentDocs"]));
            t
        }),
        HistoryItem::new("windows:run", g, "Run history", "Commands typed in Win+R", true, reg(&["RunMRU"])),
        HistoryItem::new("windows:typedpaths", g, "Typed paths", "Addresses typed in File Explorer", true, reg(&["TypedPaths"])),
        HistoryItem::new("windows:search", g, "File Explorer search history", "Searches typed in File Explorer", true, reg(&["WordWheelQuery"])),
        HistoryItem::new(
            "windows:dialogs",
            g,
            "Open / Save dialog history",
            "Recent files and folders in Open and Save dialogs",
            true,
            reg(&[r"ComDlg32\OpenSavePidlMRU", r"ComDlg32\LastVisitedPidlMRU"]),
        ),
        HistoryItem::new(
            "windows:temp",
            g,
            "Temporary files",
            "Your temp folder and the Windows temp folder (files in use are skipped)",
            true,
            vec![Target::Contents(temp), Target::Contents(win.join("Temp"))],
        ),
        HistoryItem::new(
            "windows:thumbnails",
            g,
            "Thumbnail cache",
            "Small previews of pictures and videos",
            true,
            files_matching(&local.join(r"Microsoft\Windows\Explorer"), "thumbcache_"),
        ),
        HistoryItem::new(
            "windows:crash",
            g,
            "Crash dumps and error reports",
            "Memory dumps and Windows Error Reporting files",
            true,
            vec![
                Target::Contents(local.join("CrashDumps")),
                Target::Contents(win.join("Minidump")),
                Target::Path(win.join("MEMORY.DMP")),
                Target::Contents(pdata.join(r"Microsoft\Windows\WER\ReportArchive")),
                Target::Contents(pdata.join(r"Microsoft\Windows\WER\ReportQueue")),
            ],
        ),
        HistoryItem::new("windows:recyclebin", g, "Recycle Bin", "Files you deleted", false, vec![Target::EmptyTrash]),
        HistoryItem::new("windows:clipboard", g, "Clipboard", "What was last copied", false, vec![Target::Clipboard]),
    ];
    // Microsoft Office: File MRU / Place MRU of every app and signed-in identity.
    let mut office = Vec::new();
    let root = join(HKCU, r"Software\Microsoft\Office");
    for ver in subkeys(&root).into_iter().filter(|v| v.contains('.')) {
        for app in subkeys(&join(&root, &ver)) {
            let base = join(&join(&root, &ver), &app);
            for sub in ["File MRU", "Place MRU"] {
                if open(&join(&base, sub)).is_some() {
                    office.push(Target::Fix(Fix::DeleteRegKey { key: join(&base, sub) }));
                }
            }
            for id in subkeys(&join(&base, "User MRU")) {
                for sub in ["File MRU", "Place MRU"] {
                    let k = join(&join(&join(&base, "User MRU"), &id), sub);
                    if open(&k).is_some() {
                        office.push(Target::Fix(Fix::DeleteRegKey { key: k }));
                    }
                }
            }
        }
    }
    if !office.is_empty() {
        v.push(HistoryItem::new(
            "office:microsoft",
            "Office",
            "Microsoft Office recent documents",
            "Recent files and places in Word, Excel, PowerPoint and the others",
            true,
            office,
        ));
    }
    if let Some(lo) = dirs::config_dir().map(|c| c.join(r"LibreOffice\4\user\registrymodifications.xcu")) {
        v.push(HistoryItem::new(
            "office:libreoffice",
            "Office",
            "LibreOffice recent documents",
            "The Start Center and File › Recent lists",
            true,
            vec![Target::DropLines { file: lo, containing: "HistoryInfo['PickList']".into() }],
        ));
    }
    // An item with no targets (no MRU keys yet) isn't shown.
    v.retain(|i| !i.targets.is_empty());
    v
}

pub fn builtin_tools() -> Vec<Tool> {
    let t = Tool::builtin;
    vec![
        t("msinfo32", "System", "System Information", "Hardware, drivers and software summary", "msinfo32"),
        t("taskmgr", "System", "Task Manager", "Running programs and performance", "taskmgr"),
        t("resmon", "System", "Resource Monitor", "CPU, memory, disk and network use per process", "resmon"),
        t("perfmon", "System", "Performance Monitor", "Performance counters and reports", "perfmon"),
        t("eventvwr", "System", "Event Viewer", "System and application logs", "eventvwr.msc"),
        t("services", "System", "Services", "Start, stop and configure services", "services.msc"),
        t("devmgmt", "System", "Device Manager", "Hardware and drivers", "devmgmt.msc"),
        t("compmgmt", "System", "Computer Management", "Users, shares, disks, services", "compmgmt.msc"),
        t("msconfig", "System", "System Configuration", "Boot options and startup", "msconfig").as_admin(),
        t("regedit", "System", "Registry Editor", "Edit the registry", "regedit").as_admin(),
        t("gpedit", "System", "Group Policy Editor", "Local policies (Pro editions)", "gpedit.msc"),
        t("dxdiag", "System", "DirectX Diagnostic Tool", "Graphics and sound details", "dxdiag"),
        t("rstrui", "System", "System Restore", "Go back to a restore point", "rstrui"),
        t("mdsched", "System", "Memory Diagnostic", "Test the RAM at the next restart", "mdsched"),
        t("sfc", "System", "System File Checker", "Find and repair damaged Windows files", "sfc /scannow").in_terminal().as_admin(),
        t("cleanmgr", "Disks", "Disk Cleanup", "Windows' own cleanup of system files", "cleanmgr"),
        t("dfrgui", "Disks", "Optimize Drives", "Defragment / TRIM drives", "dfrgui"),
        t("diskmgmt", "Disks", "Disk Management", "Partitions and volumes", "diskmgmt.msc"),
        t("chkdsk", "Disks", "Check Disk", "Check the system drive for errors (read-only)", "chkdsk").in_terminal().as_admin(),
        t("ncpa", "Network", "Network Connections", "Network adapters", "ncpa.cpl"),
        t("ipconfig", "Network", "Network information", "IP addresses, DNS and gateways", "ipconfig /all").in_terminal(),
        t("ping", "Network", "Ping", "Check the internet connection", "ping 1.1.1.1").in_terminal(),
        t("tracert", "Network", "Trace route", "The path to a server", "tracert 1.1.1.1").in_terminal(),
        t("firewall", "Security", "Windows Firewall", "Firewall rules", "wf.msc"),
        t("security", "Security", "Windows Security", "Antivirus and protection", "windowsdefender:"),
        t("appwiz", "Software", "Programs and Features", "Windows' own uninstaller list", "appwiz.cpl"),
        t("features", "Software", "Windows Features", "Turn Windows features on or off", "optionalfeatures"),
        t("update", "Software", "Windows Update", "Install updates", "ms-settings:windowsupdate"),
        t("power", "System", "Power Options", "Power plans", "powercfg.cpl"),
        t("mstsc", "Network", "Remote Desktop", "Connect to another PC", "mstsc"),
        t("cmd", "System", "Command Prompt", "A command line", "cmd"),
    ]
}

pub fn run_tool(command: &str, in_terminal: bool, admin: bool) -> Result<()> {
    let mut c = Command::new(if admin { "powershell" } else { "cmd" });
    if admin {
        // Start-Process -Verb RunAs shows the UAC prompt.
        let (file, args) = if in_terminal {
            ("cmd".to_string(), format!("/K {command}"))
        } else {
            command.split_once(' ').map(|(f, a)| (f.to_string(), a.to_string())).unwrap_or((command.to_string(), String::new()))
        };
        let q = |s: &str| s.replace('\'', "''");
        let args = if args.is_empty() { String::new() } else { format!(" -ArgumentList '{}'", q(&args)) };
        c.args(["-NoProfile", "-Command", &format!("Start-Process -Verb RunAs -FilePath '{}'{args}", q(&file))]);
    } else if in_terminal {
        c.args(["/C", "start", "", "cmd", "/K", command]);
    } else {
        c.args(["/C", "start", ""]).raw_arg(command);
    }
    c.creation_flags(NO_WINDOW).spawn().with_context(|| format!("starting {command}"))?;
    Ok(())
}
