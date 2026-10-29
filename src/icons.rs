//! Where a program's icon is: the file named by `DisplayIcon` (Windows) or by the desktop
//! entry's `Icon=` (Linux, looked up in the icon theme). Decoding happens in the GUI.

use crate::uninstaller::{Program, ProgramKind};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconSource {
    /// An image file: PNG, SVG, ICO, BMP, XPM, JPEG.
    File(PathBuf),
    /// Windows: icon `index` inside an `.exe` / `.dll` / `.ico` (negative = resource id).
    Resource { file: PathBuf, index: i32 },
}

pub fn resolve(p: &Program) -> Option<IconSource> {
    platform_resolve(p)
}

/// An autorun entry's icon: its program's (Windows) or its desktop entry's `Icon=` (Linux).
pub fn for_autorun(e: &crate::autorun::AutorunEntry) -> Option<IconSource> {
    #[cfg(windows)]
    {
        let file = PathBuf::from(e.exe.as_deref()?);
        let ext = file.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        (file.is_file() && ext == "exe").then_some(IconSource::Resource { file, index: 0 })
    }
    #[cfg(not(windows))]
    {
        if e.source != crate::autorun::Source::StartupFolder {
            return None;
        }
        let text = std::fs::read_to_string(&e.location).ok()?;
        let name = text.lines().find_map(|l| l.trim().strip_prefix("Icon=")).map(str::trim).filter(|n| !n.is_empty())?;
        named(name)
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
fn platform_resolve(p: &Program) -> Option<IconSource> {
    if p.kind == ProgramKind::WindowsApp {
        return p.install_location.as_deref().and_then(|l| appx_logo(Path::new(l))).map(IconSource::File);
    }
    // `DisplayIcon`: `"C:\path\app.exe",0` / `C:\path\app.ico`
    if let Some(raw) = p.icon.as_deref().filter(|s| !s.trim().is_empty()) {
        let raw = crate::pathutil::expand_env(raw.trim());
        let (file, index) = match raw.rsplit_once(',') {
            Some((f, i)) if i.trim().parse::<i32>().is_ok() => (f.trim().trim_matches('"').to_string(), i.trim().parse().unwrap_or(0)),
            _ => (raw.trim_matches('"').to_string(), 0),
        };
        let path = PathBuf::from(&file);
        if path.is_file() {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            return Some(if ext == "png" || ext == "bmp" { IconSource::File(path) } else { IconSource::Resource { file: path, index } });
        }
    }
    // Otherwise the program's main executable.
    p.executables.iter().map(PathBuf::from).find(|e| e.is_file()).map(|file| IconSource::Resource { file, index: 0 })
}

/// A Store app's logo from its manifest, preferring a 48-px or 100 % scale variant.
#[cfg(windows)]
fn appx_logo(install: &Path) -> Option<PathBuf> {
    let manifest = std::fs::read_to_string(install.join("AppxManifest.xml")).ok()?;
    let attr = |name: &str| {
        manifest.find(&format!("{name}=\"")).and_then(|i| {
            let rest = &manifest[i + name.len() + 2..];
            rest.find('"').map(|j| rest[..j].to_string())
        })
    };
    let rel =
        attr("Square44x44Logo").or_else(|| attr("Square150x150Logo")).or_else(|| manifest.split("<Logo>").nth(1).and_then(|r| r.split("</Logo>").next()).map(str::to_string))?;
    let base = install.join(rel.replace('/', "\\"));
    if base.is_file() {
        return Some(base);
    }
    // Assets\Logo.png is stored as Assets\Logo.targetsize-48.png, Logo.scale-100.png, …
    let dir = base.parent()?;
    let stem = base.file_stem()?.to_string_lossy().to_lowercase();
    let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if !n.starts_with(&format!("{stem}.")) || !n.ends_with(".png") {
                return None;
            }
            let rank = if n.contains("targetsize-48") && !n.contains("contrast") {
                0
            } else if n.contains("targetsize-32") && !n.contains("contrast") {
                1
            } else if n.contains("scale-100") && !n.contains("contrast") {
                2
            } else if n.contains("contrast") {
                9
            } else {
                5
            };
            Some((rank, e.path()))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, p)| p)
}

// ---------------------------------------------------------------- Linux: icon themes

#[cfg(not(windows))]
fn platform_resolve(p: &Program) -> Option<IconSource> {
    let name = match p.icon.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(n) => n.trim().to_string(),
        None if p.kind == ProgramKind::Flatpak => p.package.clone()?,
        None if p.kind == ProgramKind::Snap => {
            // Snaps ship their icon inside the package.
            let pkg = p.package.as_deref()?;
            return ["png", "svg"].iter().map(|e| PathBuf::from(format!("/snap/{pkg}/current/meta/gui/icon.{e}"))).find(|f| f.is_file()).map(IconSource::File);
        }
        None => return None,
    };
    named(&name)
}

/// An icon by `Icon=` value: a path, or a name in the icon theme.
#[cfg(not(windows))]
fn named(name: &str) -> Option<IconSource> {
    if name.starts_with('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(IconSource::File(path));
    }
    theme_index().lookup(name).map(IconSource::File)
}

#[cfg(not(windows))]
struct ThemeIndex {
    /// icon name → (preference, path); lower preference is better.
    best: std::collections::HashMap<String, (u32, PathBuf)>,
}

#[cfg(not(windows))]
impl ThemeIndex {
    fn lookup(&self, name: &str) -> Option<PathBuf> {
        // `Icon=foo.png` names a file in the theme too.
        let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.to_string());
        self.best.get(name).or_else(|| self.best.get(&stem)).map(|(_, p)| p.clone())
    }
}

#[cfg(not(windows))]
fn data_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(d) = dirs::data_dir() {
        v.push(d.clone());
        v.push(d.join("flatpak/exports/share"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    v.push("/var/lib/flatpak/exports/share".into());
    v.push("/var/lib/snapd/desktop".into());
    v.dedup();
    v
}

/// The desktop's icon theme, then its fallbacks.
#[cfg(not(windows))]
fn themes() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(o) = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "icon-theme"]).output() {
        let t = String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string();
        if !t.is_empty() {
            v.push(t);
        }
    }
    for t in ["hicolor", "Adwaita", "breeze", "Papirus", "elementary"] {
        if !v.iter().any(|x| x == t) {
            v.push(t.to_string());
        }
    }
    v
}

/// How well a theme folder's size fits a ~32 px list icon (lower is better).
#[cfg(not(windows))]
fn size_rank(dir: &str) -> u32 {
    let size: Option<u32> = dir.split('@').next().and_then(|s| s.split('x').next()).and_then(|s| s.parse().ok());
    match (dir, size) {
        (_, Some(48)) => 0,
        (_, Some(64)) => 1,
        ("scalable", _) => 2,
        (_, Some(32)) => 3,
        (_, Some(96)) | (_, Some(128)) => 4,
        (_, Some(256)) | (_, Some(512)) => 5,
        (_, Some(s)) if s < 32 => 8,
        _ => 6,
    }
}

#[cfg(not(windows))]
fn theme_index() -> &'static ThemeIndex {
    static INDEX: std::sync::OnceLock<ThemeIndex> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let mut best: std::collections::HashMap<String, (u32, PathBuf)> = std::collections::HashMap::new();
        let mut offer = |name: String, rank: u32, path: PathBuf| {
            let e = best.entry(name).or_insert((u32::MAX, PathBuf::new()));
            if rank < e.0 {
                *e = (rank, path);
            }
        };
        let is_image = |p: &Path| p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "png" | "svg" | "svgz" | "xpm"));
        for (ti, theme) in themes().iter().enumerate() {
            for base in data_dirs() {
                let root = base.join("icons").join(theme);
                let Ok(level1) = std::fs::read_dir(&root) else {
                    continue;
                };
                // Two layouts: `48x48/apps/x.png` (hicolor, Adwaita, Yaru) and `apps/48/x.svg` (Breeze).
                for a in level1.flatten().filter(|e| e.path().is_dir()) {
                    let Ok(level2) = std::fs::read_dir(a.path()) else {
                        continue;
                    };
                    let an = a.file_name().to_string_lossy().to_string();
                    for b in level2.flatten().filter(|e| e.path().is_dir()) {
                        let bn = b.file_name().to_string_lossy().to_string();
                        let (size, ctx) = if an.contains('x') || an == "scalable" || an.parse::<u32>().is_ok() { (an.as_str(), bn.as_str()) } else { (bn.as_str(), an.as_str()) };
                        let size = if size.parse::<u32>().is_ok() { format!("{size}x{size}") } else { size.to_string() };
                        let ctx_rank = match ctx {
                            "apps" | "applications" => 0,
                            "legacy" | "preferences" | "categories" => 1,
                            _ => 2,
                        };
                        let Ok(rd) = std::fs::read_dir(b.path()) else {
                            continue;
                        };
                        for f in rd.flatten() {
                            let p = f.path();
                            if is_image(&p) {
                                let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                                let name = name.strip_suffix("-symbolic").map(str::to_string).unwrap_or(name);
                                // Monochrome symbolic icons only as a last resort, then theme order,
                                // folder kind and size.
                                let sym = if ctx.contains("symbolic") || p.to_string_lossy().contains("-symbolic") { 50_000 } else { 0 };
                                offer(name, sym + ti as u32 * 1000 + ctx_rank * 10 + size_rank(&size), p);
                            }
                        }
                    }
                }
            }
        }
        for base in data_dirs() {
            if let Ok(rd) = std::fs::read_dir(base.join("pixmaps")) {
                for f in rd.flatten() {
                    let p = f.path();
                    if is_image(&p) {
                        offer(p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), 90_000, p);
                    }
                }
            }
        }
        ThemeIndex { best }
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    #[test]
    fn size_preference() {
        use super::size_rank;
        assert!(size_rank("48x48") < size_rank("scalable"));
        assert!(size_rank("scalable") < size_rank("256x256"));
        assert!(size_rank("256x256") < size_rank("16x16"));
        assert_eq!(size_rank("48x48@2"), 0);
    }
}

#[cfg(test)]
mod coverage {
    /// `cargo test --lib icon_coverage -- --ignored --nocapture`: how many installed programs
    /// get an icon on this machine.
    #[test]
    #[ignore]
    fn icon_coverage() {
        let programs = crate::uninstaller::list_programs(&crate::config::Settings::default());
        let t = std::time::Instant::now();
        let found: Vec<_> = programs.iter().map(|p| (p, super::resolve(p))).collect();
        let n = found.iter().filter(|(_, s)| s.is_some()).count();
        for (p, s) in &found {
            if s.is_none() {
                println!("no icon: {} [{:?}] icon={:?}", p.name, p.kind, p.icon);
            }
        }
        println!("{n} of {} programs have an icon ({:?})", programs.len(), t.elapsed());
    }
}
