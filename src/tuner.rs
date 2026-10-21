//! Registry Tuner: a data-driven table of reversible system tweaks.
//!
//! Tweaks live in `tweaks/windows.toml` and `tweaks/linux.toml` (embedded at build time).
//! Before CleanCore changes a setting it records the previous state ("prior"), so a revert
//! restores exactly what was there, including deleting keys or values that didn't exist.

use crate::config::data_dir;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const WINDOWS_TABLE: &str = include_str!("../tweaks/windows.toml");
pub const LINUX_TABLE: &str = include_str!("../tweaks/linux.toml");

pub const CATEGORIES: [(&str, &str); 7] = [
    ("appearance", "Appearance"),
    ("file_explorer", "File Explorer / Files"),
    ("desktop", "Desktop"),
    ("security", "Security and Stability"),
    ("usage", "Usage"),
    ("system", "System"),
    ("ai", "AI, ads and suggestions"),
];

/// What the user must do before a change takes effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requires {
    #[default]
    None,
    Explorer,
    Logoff,
    Reboot,
}

impl Requires {
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Explorer => Some("Explorer restart required (`cleancore tune restart-explorer`)"),
            Self::Logoff => Some("Log out required"),
            Self::Reboot => Some("PC restart required"),
        }
    }
}

/// Typed registry data as written in the table: `data = { dword = 1 }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegData {
    Dword(u32),
    Qword(u64),
    Sz(String),
    ExpandSz(String),
    MultiSz(Vec<String>),
    Binary(Vec<u8>),
}

/// A raw registry value: Win32 type code + bytes, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawValue {
    pub vtype: u32,
    pub bytes: Vec<u8>,
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

impl RegData {
    pub fn to_raw(&self) -> RawValue {
        match self {
            Self::Dword(v) => RawValue { vtype: 4, bytes: v.to_le_bytes().to_vec() },
            Self::Qword(v) => RawValue { vtype: 11, bytes: v.to_le_bytes().to_vec() },
            Self::Sz(s) => RawValue { vtype: 1, bytes: utf16z(s) },
            Self::ExpandSz(s) => RawValue { vtype: 2, bytes: utf16z(s) },
            Self::MultiSz(v) => {
                let mut bytes: Vec<u8> = v.iter().flat_map(|s| utf16z(s)).collect();
                bytes.extend([0, 0]);
                RawValue { vtype: 7, bytes }
            }
            Self::Binary(b) => RawValue { vtype: 3, bytes: b.clone() },
        }
    }
}

impl RawValue {
    /// Same type and data. Strings compare without trailing NULs and case-insensitively,
    /// because Windows and other tools are inconsistent about both.
    pub fn same_as(&self, other: &RawValue) -> bool {
        let is_str = |t| matches!(t, 1 | 2);
        if is_str(self.vtype) && is_str(other.vtype) && self.vtype == other.vtype {
            let dec = |b: &[u8]| {
                let w: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&w).trim_end_matches('\0').to_lowercase()
            };
            return dec(&self.bytes) == dec(&other.bytes);
        }
        self == other
    }
}

/// One low-level change that a tweak option makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Setting {
    /// Windows: set a registry value, creating the key if needed. `name = ""` is the default value.
    Reg {
        key: String,
        #[serde(default)]
        name: String,
        data: RegData,
    },
    /// Windows: make sure a registry value does not exist.
    RegDelete {
        key: String,
        #[serde(default)]
        name: String,
    },
    /// Linux: `gsettings set <schema> <key> <value>`; `value` is GVariant text as `gsettings get` prints it.
    Gsettings { schema: String, key: String, value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TweakOption {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub set: Vec<Setting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweak {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requires: Requires,
    /// Shown before applying (e.g. disabling UAC).
    #[serde(default)]
    pub warning: Option<String>,
    /// Windows build range (`CurrentBuildNumber`), inclusive.
    #[serde(default)]
    pub min_build: Option<u32>,
    #[serde(default)]
    pub max_build: Option<u32>,
    /// A toggle has one option (`on`); a choice has several.
    #[serde(rename = "option")]
    pub options: Vec<TweakOption>,
}

impl Tweak {
    pub fn is_toggle(&self) -> bool {
        self.options.len() == 1
    }

    pub fn option(&self, id: &str) -> Option<&TweakOption> {
        self.options.iter().find(|o| o.id.eq_ignore_ascii_case(id))
    }
}

#[derive(Deserialize)]
struct Table {
    #[serde(rename = "tweak")]
    tweaks: Vec<Tweak>,
}

pub fn parse_table(text: &str) -> Result<Vec<Tweak>> {
    let t: Table = toml::from_str(text).context("parsing tweak table")?;
    validate(&t.tweaks)?;
    Ok(t.tweaks)
}

fn validate(tweaks: &[Tweak]) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    for t in tweaks {
        if !ids.insert(t.id.as_str()) {
            bail!("duplicate tweak id '{}'", t.id);
        }
        if !CATEGORIES.iter().any(|(c, _)| *c == t.category) {
            bail!("tweak '{}': unknown category '{}'", t.id, t.category);
        }
        if t.options.is_empty() || t.options.iter().any(|o| o.set.is_empty()) {
            bail!("tweak '{}': every option needs at least one setting", t.id);
        }
        if t.options.iter().any(|o| matches!(o.id.as_str(), "off" | "default")) {
            bail!("tweak '{}': 'off'/'default' are reserved for revert", t.id);
        }
    }
    Ok(())
}

/// The tweaks for the OS this binary runs on.
pub fn tweaks() -> Vec<Tweak> {
    let text = if cfg!(windows) { WINDOWS_TABLE } else { LINUX_TABLE };
    parse_table(text).expect("embedded tweak table is validated by tests")
}

// ---------------------------------------------------------------- state

/// What CleanCore needs to undo a setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Prior {
    Reg {
        key: String,
        name: String,
        value: Option<RawValue>,
        /// Topmost key that CleanCore created; reverting deletes this whole key.
        created_key: Option<String>,
    },
    Gsettings {
        schema: String,
        key: String,
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub option: String,
    pub applied_at: DateTime<Local>,
    /// In the order the settings were applied; restored in reverse.
    pub priors: Vec<Prior>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub applied: BTreeMap<String, Applied>,
}

impl State {
    fn file() -> std::path::PathBuf {
        data_dir().join("tuner.json")
    }

    pub fn load() -> Result<Self> {
        match std::fs::read_to_string(Self::file()) {
            Ok(s) => serde_json::from_str(&s).context("parsing tuner.json"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let f = Self::file();
        std::fs::create_dir_all(f.parent().unwrap())?;
        std::fs::write(&f, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- operations

#[derive(Debug, Serialize)]
pub struct Status {
    pub id: String,
    /// `Err(reason)` when the tweak doesn't apply to this system.
    pub available: std::result::Result<(), String>,
    /// Option whose settings all match the live system, if any.
    pub current: Option<String>,
    /// Option CleanCore applied (the "by CleanCore" label).
    pub ours: Option<String>,
}

pub fn status_all(tweaks: &[Tweak], state: &State) -> Vec<Status> {
    let backend = crate::platform::TuneBackend::new();
    tweaks
        .par_iter()
        .map(|t| {
            let available = backend.available(t);
            let current = if available.is_ok() { t.options.iter().find(|o| o.set.iter().all(|s| backend.matches(s).unwrap_or(false))).map(|o| o.id.clone()) } else { None };
            Status { id: t.id.clone(), available, current, ours: state.applied.get(&t.id).map(|a| a.option.clone()) }
        })
        .collect()
}

/// Apply `option` of `tweak`. Switching between options of a choice reverts the old one first,
/// so the priors always describe the system as it was before CleanCore touched it.
pub fn apply(tweak: &Tweak, option: &str, state: &mut State) -> Result<Requires> {
    let opt = tweak.option(option).with_context(|| {
        let ids: Vec<_> = tweak.options.iter().map(|o| o.id.as_str()).collect();
        format!("tweak '{}' has no option '{option}' (options: {}, off)", tweak.id, ids.join(", "))
    })?;
    let backend = crate::platform::TuneBackend::new();
    if let Err(why) = backend.available(tweak) {
        bail!("'{}' is not available on this system: {why}", tweak.id);
    }
    if state.applied.get(&tweak.id).is_some_and(|a| a.option == opt.id) {
        return Ok(Requires::None);
    }
    if state.applied.contains_key(&tweak.id) {
        // Same backend instance, so its cache sees the reverted values before we capture priors.
        revert_with(&backend, tweak, state)?;
    }

    let mut priors = Vec::new();
    for s in &opt.set {
        // Capture right before each write so nested keys created by earlier settings are seen.
        let res = backend.capture(s).and_then(|p| {
            priors.push(p);
            backend.apply(s)
        });
        if let Err(e) = res {
            // Roll back what was already changed so the system isn't left half-tweaked.
            for p in priors.iter().rev() {
                let _ = backend.restore(p);
            }
            return Err(e.context(format!("applying '{}'", tweak.id)));
        }
    }
    state.applied.insert(tweak.id.clone(), Applied { option: opt.id.clone(), applied_at: Local::now(), priors });
    state.save()?;
    Ok(tweak.requires)
}

/// Restore the values that were there before CleanCore applied the tweak.
pub fn revert(tweak: &Tweak, state: &mut State) -> Result<Requires> {
    revert_with(&crate::platform::TuneBackend::new(), tweak, state)
}

fn revert_with(backend: &crate::platform::TuneBackend, tweak: &Tweak, state: &mut State) -> Result<Requires> {
    let Some(applied) = state.applied.get(&tweak.id) else {
        bail!("'{}' was not changed by CleanCore, nothing to revert", tweak.id);
    };
    for p in applied.priors.iter().rev() {
        backend.restore(p).with_context(|| format!("reverting '{}'", tweak.id))?;
    }
    state.applied.remove(&tweak.id);
    state.save()?;
    Ok(tweak.requires)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_valid() {
        let win = parse_table(WINDOWS_TABLE).unwrap();
        let lin = parse_table(LINUX_TABLE).unwrap();
        assert!(win.len() >= 40, "windows table has {}", win.len());
        assert!(lin.len() >= 20, "linux table has {}", lin.len());
        for t in &win {
            for o in &t.options {
                for s in &o.set {
                    assert!(!matches!(s, Setting::Gsettings { .. }), "{}: gsettings in windows table", t.id);
                    if let Setting::Reg { key, .. } | Setting::RegDelete { key, .. } = s {
                        assert!(key.starts_with("HKEY_LOCAL_MACHINE\\") || key.starts_with("HKEY_CURRENT_USER\\"), "{}: bad root {key}", t.id);
                    }
                }
            }
        }
        for t in &lin {
            assert!(t.options.iter().flat_map(|o| &o.set).all(|s| matches!(s, Setting::Gsettings { .. })), "{}", t.id);
        }
    }

    #[test]
    fn raw_values() {
        assert_eq!(RegData::Dword(1).to_raw(), RawValue { vtype: 4, bytes: vec![1, 0, 0, 0] });
        assert_eq!(RegData::Sz("a".into()).to_raw().bytes, vec![b'a', 0, 0, 0]);
        let a = RegData::Sz("Yes".into()).to_raw();
        let b = RawValue { vtype: 1, bytes: utf16z("yes").into_iter().chain([0, 0]).collect() };
        assert!(a.same_as(&b));
        assert!(!a.same_as(&RegData::ExpandSz("yes".into()).to_raw()));
    }
}
