//! Windows watcher backend.
//!
//! Primary: a real-time ETW session on Microsoft-Windows-Kernel-Registry. It reports every
//! registry operation with the PID that made it, needs administrator rights, and needs no
//! kernel driver (the original product ships a minifilter for this).
//! Fallback when ETW can't start (not elevated): `RegNotifyChangeKeyValue` per watched key
//! plus snapshot diffing. That catches every change but can't tell which process made it.

use super::windows::{join, open, split_root, subkeys};
use crate::watcher::{diff, from_kernel_path, to_kernel_path, Op, Snapshot, Target, Typed, WatchEvent};
use anyhow::Result;
use chrono::{DateTime, Local};
use ferrisetw::parser::{Parser, Pointer};
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, UserTrace};
use ferrisetw::EventRecord;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use winreg::enums::*;
use winreg::RegValue;

const KERNEL_REGISTRY: &str = "70eb4f03-c1de-4f73-a051-33d13d5413bd";
const SESSION: &str = "CleanCore-RegistryWatcher";
const MAX_CACHED_VALUES: usize = 200_000;

// Microsoft-Windows-Kernel-Registry event ids.
const EV_CREATE_KEY: u16 = 1;
const EV_DELETE_KEY: u16 = 3;
const EV_QUERY_KEY: u16 = 4;
const EV_SET_VALUE: u16 = 5;
const EV_DELETE_VALUE: u16 = 6;
const EV_QUERY_VALUE: u16 = 7;
const EV_ENUM_KEY: u16 = 8;
const EV_ENUM_VALUE: u16 = 9;
const EV_QUERY_MULTI: u16 = 10;
const EV_SET_INFO: u16 = 11;
const REG_CREATED_NEW_KEY: u32 = 1;

// ---------------------------------------------------------------- value formatting

fn type_label(t: &RegType) -> &'static str {
    match t {
        REG_SZ => "String",
        REG_EXPAND_SZ => "Expandable String",
        REG_MULTI_SZ => "Multi-String",
        REG_BINARY => "Binary",
        REG_DWORD | REG_DWORD_BIG_ENDIAN => "DWord",
        REG_QWORD => "QWord",
        REG_NONE => "None",
        _ => "Other",
    }
}

fn format_value(v: &RegValue) -> Typed {
    let b: &[u8] = &v.bytes;
    let text = match v.vtype {
        REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => {
            let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let s = String::from_utf16_lossy(&w);
            let parts: Vec<&str> = s.trim_end_matches('\0').split('\0').collect();
            parts.join(" | ")
        }
        REG_DWORD if b.len() >= 4 => {
            let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            format!("0x{n:08x} ({n})")
        }
        REG_QWORD if b.len() >= 8 => {
            let n = u64::from_le_bytes(b[..8].try_into().unwrap());
            format!("0x{n:016x} ({n})")
        }
        _ => {
            let hex: Vec<String> = b.iter().take(64).map(|x| format!("{x:02x}")).collect();
            format!("{}{}", hex.join(" "), if b.len() > 64 { format!(" … ({} bytes)", b.len()) } else { String::new() })
        }
    };
    (type_label(&v.vtype).to_string(), text)
}

fn read_value(key: &str, name: &str) -> Option<Typed> {
    open(key)?.get_raw_value(name).ok().map(|v| format_value(&v))
}

/// Snapshot of a key tree (capped, so a huge hive can't exhaust memory).
fn read_snapshot(root: &str, cap: usize) -> Snapshot {
    let mut snap = Snapshot::new();
    let mut count = 0;
    let mut stack = vec![root.to_string()];
    while let Some(k) = stack.pop() {
        let Some(key) = open(&k) else { continue };
        let mut vals = BTreeMap::new();
        for (name, v) in key.enum_values().flatten() {
            vals.insert(name, format_value(&v));
            count += 1;
        }
        snap.insert(k.clone(), vals);
        if count >= cap {
            break;
        }
        stack.extend(subkeys(&k).into_iter().map(|s| join(&k, &s)));
    }
    snap
}

// ---------------------------------------------------------------- helpers

/// SID of the signed-in user, to map `\REGISTRY\USER\<sid>` back to HKEY_CURRENT_USER.
fn current_user_sid() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    for sid in subkeys("HKEY_USERS").into_iter().filter(|s| s.starts_with("S-1-5-21-") && !s.ends_with("_Classes")) {
        let k = format!(r"HKEY_USERS\{sid}\Volatile Environment");
        if open(&k).and_then(|k| k.get_value::<String, _>("USERNAME").ok()).is_some_and(|u| u.eq_ignore_ascii_case(&user)) {
            return sid;
        }
    }
    // `whoami /user /fo csv /nh` → "domain\user","S-1-5-21-..."
    std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).rsplit(',').next().map(|s| s.trim().trim_matches('"').to_string()))
        .unwrap_or_default()
}

fn process_name(pid: u32) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    match pid {
        0 => return "Idle".into(),
        4 => return "System".into(),
        _ => {}
    }
    // SAFETY: plain Win32 calls on a handle we own and close; buffer length is passed in.
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
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().unwrap_or(&full).to_string()
    }
}

/// ETW timestamps are FILETIME (100 ns ticks since 1601).
fn filetime_to_local(ft: i64) -> DateTime<Local> {
    let unix_100ns = ft - 116_444_736_000_000_000;
    DateTime::from_timestamp(unix_100ns.div_euclid(10_000_000), (unix_100ns.rem_euclid(10_000_000) * 100) as u32).map(|d| d.with_timezone(&Local)).unwrap_or_else(Local::now)
}

// ---------------------------------------------------------------- backend

struct WatchedKey {
    /// Lower-case kernel path prefix, for fast matching in the ETW callback.
    kernel: String,
    include_reads: bool,
}

struct Shared {
    sid: String,
    targets: RwLock<Vec<WatchedKey>>,
    /// Last known data per (lower-case key, value name), giving "Old Data" for ETW events.
    values: Mutex<HashMap<(String, String), Typed>>,
    procs: Mutex<HashMap<u32, String>>,
    tx: Mutex<Sender<WatchEvent>>,
}

pub struct WatchBackend {
    shared: Arc<Shared>,
    etw: Option<UserTrace>,
    /// Fallback threads, one per watched key.
    notifiers: HashMap<String, Arc<AtomicBool>>,
}

impl WatchBackend {
    pub fn new(tx: Sender<WatchEvent>) -> Result<Self> {
        let shared = Arc::new(Shared {
            sid: current_user_sid(),
            targets: RwLock::new(Vec::new()),
            values: Mutex::new(HashMap::new()),
            procs: Mutex::new(HashMap::new()),
            tx: Mutex::new(tx),
        });
        let etw = match start_etw(shared.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("ETW unavailable ({e}); falling back to change notifications without process info. Run as administrator for full detail.");
                None
            }
        };
        Ok(Self { shared, etw, notifiers: HashMap::new() })
    }

    pub fn mode(&self) -> &'static str {
        if self.etw.is_some() {
            "ETW Microsoft-Windows-Kernel-Registry (with process and PID)"
        } else {
            "registry change notifications (no process info; run as administrator for ETW)"
        }
    }

    pub fn set_targets(&mut self, targets: &[Target]) -> Result<()> {
        let mut missing = Vec::new();
        for t in targets {
            if open(&t.path).is_none() {
                missing.push(t.path.clone());
            }
        }
        if self.etw.is_some() {
            let old: Vec<String> = self.shared.targets.read().unwrap().iter().map(|w| w.kernel.clone()).collect();
            let new: Vec<WatchedKey> =
                targets.iter().map(|t| WatchedKey { kernel: to_kernel_path(&t.path, &self.shared.sid).to_lowercase(), include_reads: t.include_reads }).collect();
            // Seed old-data cache for newly added keys.
            for (t, w) in targets.iter().zip(&new) {
                if !old.contains(&w.kernel) {
                    let mut cache = self.shared.values.lock().unwrap();
                    for (k, vals) in read_snapshot(&t.path, MAX_CACHED_VALUES.saturating_sub(cache.len())) {
                        for (n, v) in vals {
                            cache.insert((k.to_lowercase(), n), v);
                        }
                    }
                }
            }
            *self.shared.targets.write().unwrap() = new;
        } else {
            let wanted: Vec<&str> = targets.iter().map(|t| t.path.as_str()).collect();
            self.notifiers.retain(|path, stop| {
                let keep = wanted.contains(&path.as_str());
                if !keep {
                    stop.store(true, Ordering::Relaxed);
                }
                keep
            });
            for t in targets {
                if !self.notifiers.contains_key(&t.path) && open(&t.path).is_some() {
                    let stop = Arc::new(AtomicBool::new(false));
                    let tx = self.shared.tx.lock().unwrap().clone();
                    let (path, s) = (t.path.clone(), stop.clone());
                    std::thread::spawn(move || notify_loop(&path, s, tx));
                    self.notifiers.insert(t.path.clone(), stop);
                }
            }
        }
        if !missing.is_empty() {
            anyhow::bail!("key(s) not found or not readable: {}", missing.join(", "));
        }
        Ok(())
    }
}

impl Drop for WatchBackend {
    fn drop(&mut self) {
        for stop in self.notifiers.values() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(t) = self.etw.take() {
            let _ = t.stop();
        }
    }
}

fn start_etw(shared: Arc<Shared>) -> std::result::Result<UserTrace, String> {
    // A session left over from a crashed run blocks a new one with the same name.
    let _ = stop_trace_by_name(SESSION);
    let provider =
        Provider::by_guid(KERNEL_REGISTRY).any(u64::MAX).add_callback(move |record: &EventRecord, locator: &SchemaLocator| on_etw_event(&shared, record, locator)).build();
    UserTrace::new().named(SESSION.to_string()).enable(provider).start_and_process().map_err(|e| format!("{e:?}"))
}

fn on_etw_event(sh: &Shared, record: &EventRecord, locator: &SchemaLocator) {
    let id = record.event_id();
    let op = match id {
        EV_CREATE_KEY => Op::CreatedKey,
        EV_DELETE_KEY => Op::DeletedKey,
        EV_SET_VALUE => Op::ValueSet,
        EV_DELETE_VALUE => Op::DeletedValue,
        EV_SET_INFO => Op::SetInformationKey,
        EV_QUERY_KEY => Op::QueryKey,
        EV_QUERY_VALUE => Op::QueryValue,
        EV_QUERY_MULTI => Op::QueryMultipleValue,
        EV_ENUM_KEY => Op::EnumerateKey,
        EV_ENUM_VALUE => Op::EnumerateValue,
        _ => return, // open/close and others: cheap early exit, these are the bulk of the traffic
    };
    let want_reads = {
        let t = sh.targets.read().unwrap();
        if t.is_empty() {
            return;
        }
        t.iter().any(|w| w.include_reads)
    };
    if op.is_read() && !want_reads {
        return;
    }
    let Ok(schema) = locator.event_schema(record) else {
        return;
    };
    let p = Parser::create(record, &schema);
    if p.try_parse::<u32>("Status").is_ok_and(|s| s != 0) {
        return; // failed operation
    }
    let kernel = if id == EV_CREATE_KEY {
        if p.try_parse::<u32>("Disposition").is_ok_and(|d| d != REG_CREATED_NEW_KEY) {
            return; // opened an existing key
        }
        let base: String = p.try_parse("BaseName").unwrap_or_default();
        let rel: String = p.try_parse("RelativeName").unwrap_or_default();
        let base_obj: Option<Pointer> = p.try_parse("BaseObject").ok();
        if base.is_empty() || base_obj.is_some_and(|b| *b == 0) {
            rel
        } else {
            format!(r"{}\{rel}", base.trim_end_matches('\\'))
        }
    } else {
        p.try_parse::<String>("KeyName").unwrap_or_default()
    };
    if !kernel.starts_with('\\') {
        return; // relative name we can't resolve
    }
    let lower = kernel.to_lowercase();
    let matched = sh
        .targets
        .read()
        .unwrap()
        .iter()
        .any(|w| (lower == w.kernel || (lower.starts_with(&w.kernel) && lower.as_bytes().get(w.kernel.len()) == Some(&b'\\'))) && (!op.is_read() || w.include_reads));
    if !matched {
        return;
    }

    let key = from_kernel_path(&kernel, &sh.sid);
    let pid = record.process_id();
    let process = sh.procs.lock().unwrap().entry(pid).or_insert_with(|| process_name(pid)).clone();
    let mut ev = WatchEvent::new(op, &key);
    ev.time = filetime_to_local(record.raw_timestamp());
    ev.pid = Some(pid);
    ev.process = process;

    match op {
        Op::ValueSet | Op::DeletedValue | Op::QueryValue => {
            let name: String = p.try_parse("ValueName").unwrap_or_default();
            let cache_key = (key.to_lowercase(), name.clone());
            if op == Op::ValueSet {
                // The new data is read right away; on a fast rewrite it may already be newer.
                let new = read_value(&key, &name);
                let old = match &new {
                    Some(v) => sh.values.lock().unwrap().insert(cache_key, v.clone()),
                    None => sh.values.lock().unwrap().get(&cache_key).cloned(),
                };
                let ty = new.as_ref().or(old.as_ref()).map(|(t, _)| t.as_str());
                ev = ev.value(&name, ty, old.as_ref().map(|(_, d)| d.as_str()), new.as_ref().map(|(_, d)| d.as_str()));
            } else if op == Op::DeletedValue {
                let old = sh.values.lock().unwrap().remove(&cache_key);
                ev = ev.value(&name, old.as_ref().map(|(t, _)| t.as_str()), old.as_ref().map(|(_, d)| d.as_str()), None);
            } else {
                ev = ev.value(&name, None, None, None);
            }
        }
        Op::DeletedKey => {
            let prefix = format!("{}\\", key.to_lowercase());
            sh.values.lock().unwrap().retain(|(k, _), _| *k != key.to_lowercase() && !k.starts_with(&prefix));
        }
        _ => {}
    }
    let _ = sh.tx.lock().unwrap().send(ev);
}

/// Fallback: wait for change notifications on `path` (whole subtree) and diff snapshots.
fn notify_loop(path: &str, stop: Arc<AtomicBool>, tx: Sender<WatchEvent>) {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Registry::{RegNotifyChangeKeyValue, HKEY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let Some((root, rest)) = split_root(path) else {
        return;
    };
    let Ok(key) = winreg::RegKey::predef(root).open_subkey_with_flags(rest, KEY_NOTIFY | KEY_READ) else {
        return;
    };
    let mut snap = read_snapshot(path, MAX_CACHED_VALUES);
    // SAFETY: event handle is created and closed here; the key handle outlives every wait.
    unsafe {
        let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
        if event.is_null() {
            return;
        }
        while !stop.load(Ordering::Relaxed) {
            let hkey = key.raw_handle() as HKEY;
            if RegNotifyChangeKeyValue(hkey, 1, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, event, 1) != 0 {
                break; // key deleted or access lost
            }
            loop {
                if stop.load(Ordering::Relaxed) {
                    CloseHandle(event);
                    return;
                }
                if WaitForSingleObject(event, 1000) == WAIT_OBJECT_0 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100)); // let a burst of writes settle
            let now = read_snapshot(path, MAX_CACHED_VALUES);
            for ev in diff(&snap, &now) {
                if tx.send(ev).is_err() {
                    CloseHandle(event);
                    return;
                }
            }
            snap = now;
        }
        CloseHandle(event);
    }
}
