//! Windows side of the installation monitor: registry hives, program / data folders and
//! Store apps to snapshot, and how to undo a traced item.

use super::windows::{join, open, subkeys};
use crate::model::Fix;
use crate::tracer::{h64, Excludes, ItemKind, SettingsEntry, TraceItem};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

pub fn default_trace_excludes() -> Vec<&'static str> {
    vec![
        // files Windows, Explorer, browsers and Defender change all the time
        r"\appdata\local\temp\",
        r"\windows\temp\",
        r"\windows\prefetch\",
        r"\softwaredistribution\",
        r"\$recycle.bin",
        r"\appdata\local\microsoft\windows\inetcache",
        r"\appdata\local\microsoft\windows\explorer\",
        r"\appdata\local\microsoft\windows\webcache",
        r"\appdata\local\microsoft\windows\notifications",
        r"\appdata\roaming\microsoft\windows\recent",
        r"\appdata\local\connecteddevicesplatform",
        r"\appdata\local\packages\",
        r"\appdata\local\crashdumps",
        r"\microsoft\windows\wer\",
        r"\appdata\local\microsoft\onedrive",
        r"\google\chrome\user data",
        r"\microsoft\edge\user data",
        r"\bravesoftware\brave-browser\user data",
        r"\mozilla\firefox\profiles",
        r"\programdata\microsoft\windows defender",
        r"\programdata\microsoft\search",
        r"\programdata\microsoft\diagnosis",
        r"\system32\driverstore",
        r"\system32\config\",
        r"\system32\winevt",
        r"\system32\logfiles",
        r"\system32\catroot2",
        r"\system32\sru",
        r"\system32\wdi",
        r"\system32\tasks_migrated",
        r"\syswow64\config\",
        r".etl",
        r"\cleancore\",
        // registry keys that change without any installation
        r"\software\microsoft\cryptography\rng",
        r"\explorer\userassist",
        r"\explorer\recentdocs",
        r"\explorer\comdlg32",
        r"\explorer\featureusage",
        r"\explorer\sessioninfo",
        r"\explorer\streams",
        r"\shell\bagmru",
        r"\shell\bags",
        r"\software\microsoft\windows defender",
        r"\services\bam\",
        r"\services\dam\",
        r"\currentversion\notifications",
        r"\windows search",
        r"\currentversion\cloudstore",
        r"\currentversion\windowsupdate",
        r"\diagnostics\diagtrack",
        r"\deliveryoptimization",
        r"\software\microsoft\tracing\",
        r"\systemcertificates",
        r"\currentversion\mmdevices",
        r"\currentversion\appmodel\",
        r"\currentversion\capabilityaccessmanager",
    ]
}

fn env(v: &str) -> Option<PathBuf> {
    std::env::var_os(v).map(PathBuf::from)
}

pub fn trace_file_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramData", "APPDATA", "LOCALAPPDATA"].iter().filter_map(|e| env(e)).collect();
    if let Some(p) = env("USERPROFILE") {
        v.push(p.join("Desktop"));
        v.push(p.join(r"AppData\LocalLow"));
    }
    if let Some(p) = env("PUBLIC") {
        v.push(p.join("Desktop"));
    }
    if let Some(w) = env("SystemRoot") {
        for s in ["System32", "SysWOW64", "Fonts"] {
            v.push(w.join(s));
        }
    }
    v
}

const ROOTS: [&str; 3] = [r"HKEY_LOCAL_MACHINE\SOFTWARE", r"HKEY_CURRENT_USER\Software", r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services"];

type KeyValues = (String, Vec<(String, u64)>, Vec<Vec<u8>>);

fn walk_key(key: &str, ex: &Excludes, out: &mut Vec<KeyValues>, depth: usize) {
    if depth > 30 || ex.hit(key) {
        return;
    }
    let Some(k) = open(key) else { return };
    let mut raw = Vec::new();
    let vals: Vec<(String, u64)> = k
        .enum_values()
        .flatten()
        .map(|(n, v)| {
            // 4-byte type + data: hashed, and kept so a changed value can be restored.
            let mut bytes = (v.vtype.clone() as u32).to_le_bytes().to_vec();
            bytes.extend_from_slice(&v.bytes);
            let h = h64(&bytes);
            raw.push(bytes);
            (n, h)
        })
        .collect();
    let subs: Vec<String> = k.enum_keys().flatten().collect();
    out.push((key.to_string(), vals, raw));
    for s in subs {
        walk_key(&join(key, &s), ex, out, depth + 1);
    }
}

/// The registry, walked in parallel per first-level subkey.
pub fn walk_settings(ex: &Excludes, f: &mut dyn FnMut(SettingsEntry)) {
    let mut tops: Vec<(String, usize)> = Vec::new();
    for root in ROOTS {
        if let Some(k) = open(root) {
            let vals: Vec<(String, u64)> = k.enum_values().flatten().map(|(n, v)| (n, h64(&v.bytes))).collect();
            f(SettingsEntry { key: root, values: vals, raw: Vec::new() });
        }
        tops.extend(subkeys(root).into_iter().map(|s| (join(root, &s), 1)));
    }
    let chunks: Vec<Vec<KeyValues>> = tops
        .par_iter()
        .map(|(k, d)| {
            let mut out = Vec::new();
            walk_key(k, ex, &mut out, *d);
            out
        })
        .collect();
    for chunk in chunks {
        for (key, values, raw) in chunk {
            f(SettingsEntry { key: &key, values, raw });
        }
    }
}

/// Store apps (desktop programs are captured through the registry and files).
pub fn package_snapshot() -> BTreeSet<String> {
    let o = crate::platform::hidden("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-AppxPackage | ForEach-Object { $_.PackageFullName }"])
        .creation_flags(0x0800_0000)
        .output();
    o.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).map(|l| format!("appx:{}", l.trim())).collect()).unwrap_or_default()
}

pub fn trace_item_exists(i: &TraceItem) -> bool {
    match i.kind {
        ItemKind::File | ItemKind::Folder => std::fs::symlink_metadata(&i.path).is_ok(),
        ItemKind::SettingsKey => open(&i.path).is_some(),
        ItemKind::SettingsValue => open(&i.path).is_some_and(|k| k.get_raw_value(i.value.as_deref().unwrap_or("")).is_ok()),
        ItemKind::Package => true,
    }
}

pub fn trace_settings_fix(i: &TraceItem) -> Fix {
    match i.kind {
        ItemKind::SettingsValue => Fix::DeleteRegValue { key: i.path.clone(), value: i.value.clone().unwrap_or_default() },
        _ => Fix::DeleteRegKey { key: i.path.clone() },
    }
}

/// Start an installer: `.msi` through msiexec, anything else as typed.
pub fn launch(cmd: &str) -> anyhow::Result<std::process::Child> {
    let path = cmd.trim().trim_matches('"');
    if path.to_lowercase().ends_with(".msi") {
        return Ok(Command::new("msiexec").args(["/i", path]).spawn()?);
    }
    let line = if std::path::Path::new(path).exists() { format!("\"{path}\"") } else { cmd.to_string() };
    Ok(Command::new("cmd").arg("/C").raw_arg(format!("\"{line}\"")).spawn()?)
}

// ---------------------------------------------------------------- following the installer

/// The installer's own file activity, from a real-time ETW session on
/// Microsoft-Windows-Kernel-Process (to follow its child processes) and
/// Microsoft-Windows-Kernel-File (files they create or open for writing). Needs administrator
/// rights; without them tracing still works, just without this filter.
pub struct InstallWatch {
    trace: Option<ferrisetw::trace::UserTrace>,
    state: std::sync::Arc<std::sync::Mutex<WatchState>>,
}

#[derive(Default)]
struct WatchState {
    /// The installer and its descendants, plus Windows Installer (msiexec) processes, which
    /// do the actual work of .msi setups as a service.
    pids: std::collections::HashSet<u32>,
    /// Kernel paths (`\Device\HarddiskVolume3\…`), lower case.
    files: std::collections::HashSet<String>,
}

const KERNEL_PROCESS: &str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
const KERNEL_FILE: &str = "edd08927-9cc4-4e65-b970-c2560fb5c289";
const SESSION: &str = "CleanCore-InstallMonitor";

fn image_name(pid: u32) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: handle opened and closed here; buffer length passed in.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize]).rsplit('\\').next().unwrap_or("").to_lowercase()
    }
}

pub fn launch_watched(cmd: &str) -> anyhow::Result<Option<(std::process::Child, InstallWatch)>> {
    use ferrisetw::parser::Parser;
    use ferrisetw::provider::Provider;
    use ferrisetw::schema_locator::SchemaLocator;
    use ferrisetw::trace::{stop_trace_by_name, UserTrace};
    use ferrisetw::EventRecord;
    use std::sync::{Arc, Mutex};
    let state = Arc::new(Mutex::new(WatchState::default()));
    let _ = stop_trace_by_name(SESSION);
    let st = state.clone();
    let procs = Provider::by_guid(KERNEL_PROCESS)
        .any(0x10) // WINEVENT_KEYWORD_PROCESS
        .add_callback(move |r: &EventRecord, loc: &SchemaLocator| {
            if r.event_id() != 1 {
                return; // ProcessStart
            }
            let Ok(schema) = loc.event_schema(r) else {
                return;
            };
            let p = Parser::create(r, &schema);
            let (Ok(pid), Ok(parent)) = (p.try_parse::<u32>("ProcessID"), p.try_parse::<u32>("ParentProcessID")) else {
                return;
            };
            let image: String = p.try_parse("ImageName").unwrap_or_default();
            let mut s = st.lock().unwrap();
            if s.pids.contains(&parent) || image.to_lowercase().ends_with("\\msiexec.exe") {
                s.pids.insert(pid);
            }
        })
        .build();
    let st = state.clone();
    // pid → "is msiexec", so a Windows Installer service that was already running counts too.
    let names: Arc<Mutex<std::collections::HashMap<u32, bool>>> = Arc::default();
    let files = Provider::by_guid(KERNEL_FILE)
        .any(0x10 | 0x80 | 0x1000) // FILENAME | CREATE | CREATE_NEW_FILE
        .add_callback(move |r: &EventRecord, loc: &SchemaLocator| {
            let id = r.event_id();
            if id != 12 && id != 30 {
                return; // Create / CreateNewFile
            }
            let pid = r.process_id();
            let ours = {
                let s = st.lock().unwrap();
                s.pids.contains(&pid)
            } || *names.lock().unwrap().entry(pid).or_insert_with(|| image_name(pid) == "msiexec.exe");
            if !ours {
                return;
            }
            let Ok(schema) = loc.event_schema(r) else {
                return;
            };
            let p = Parser::create(r, &schema);
            if let Ok(name) = p.try_parse::<String>("FileName") {
                st.lock().unwrap().files.insert(name.to_lowercase());
            }
        })
        .build();
    let trace = match UserTrace::new().named(SESSION.to_string()).enable(procs).enable(files).start_and_process() {
        Ok(t) => t,
        Err(_) => return Ok(None), // not elevated: trace without the filter
    };
    let child = launch(cmd)?;
    state.lock().unwrap().pids.insert(child.id());
    Ok(Some((child, InstallWatch { trace: Some(trace), state })))
}

/// `\Device\HarddiskVolume3` → `C:` for every drive letter.
fn dos_devices() -> Vec<(String, String)> {
    use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
    let mut v = Vec::new();
    for l in b'A'..=b'Z' {
        let drive = format!("{}:", l as char);
        let w: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = [0u16; 512];
        // SAFETY: NUL-terminated name, buffer length passed in.
        let n = unsafe { QueryDosDeviceW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if n > 0 {
            let dev = String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(0)]).to_lowercase();
            v.push((dev, drive.to_lowercase()));
        }
    }
    v
}

impl InstallWatch {
    /// Lower-case DOS paths the installer created or wrote. `None` if nothing was seen (the
    /// session didn't deliver events), so the caller keeps everything.
    pub fn touched(mut self) -> Option<std::collections::HashSet<String>> {
        if let Some(t) = self.trace.take() {
            let _ = t.stop();
        }
        let devices = dos_devices();
        let s = self.state.lock().ok()?;
        if s.files.is_empty() {
            return None;
        }
        Some(s.files.iter().filter_map(|k| devices.iter().find(|(dev, _)| k.starts_with(&format!("{dev}\\"))).map(|(dev, drive)| format!("{drive}{}", &k[dev.len()..]))).collect())
    }
}
