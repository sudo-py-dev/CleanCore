//! Extracting and validating file paths embedded in registry data / desktop-entry command lines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Memoized `Path::exists`. Registry scans query the same few thousand paths many times,
/// so caching turns most checks into a hash lookup.
#[derive(Default)]
pub struct PathCache {
    map: RwLock<HashMap<PathBuf, bool>>,
}

impl PathCache {
    pub fn exists(&self, p: &Path) -> bool {
        if let Some(&v) = self.map.read().unwrap().get(p) {
            return v;
        }
        let v = p.exists();
        self.map.write().unwrap().insert(p.to_path_buf(), v);
        v
    }
}

/// Expand `%VAR%` (Windows style) and `$VAR` / `${VAR}` / leading `~` (Unix style).
pub fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if let Some(end) = s[i + 1..].find('%') {
                    let name = &s[i + 1..i + 1 + end];
                    if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'(' || c == b')') {
                        if let Some(v) = env_var_ci(name) {
                            out.push_str(&v);
                            i += end + 2;
                            continue;
                        }
                    }
                }
                out.push('%');
                i += 1;
            }
            b'$' if cfg!(unix) => {
                let rest = &s[i + 1..];
                let (name, consumed) = if let Some(r) = rest.strip_prefix('{') {
                    match r.find('}') {
                        Some(e) => (&r[..e], e + 2),
                        None => ("", 0),
                    }
                } else {
                    let n = rest.bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
                    (&rest[..n], n)
                };
                match (!name.is_empty()).then(|| std::env::var(name).ok()).flatten() {
                    Some(v) => {
                        out.push_str(&v);
                        i += 1 + consumed;
                    }
                    None => {
                        out.push('$');
                        i += 1;
                    }
                }
            }
            b'~' if cfg!(unix) && i == 0 && (b.len() == 1 || b[1] == b'/') => {
                match dirs::home_dir() {
                    Some(h) => out.push_str(&h.to_string_lossy()),
                    None => out.push('~'),
                }
                i += 1;
            }
            _ => {
                let ch = s[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

fn env_var_ci(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| std::env::vars().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
}

/// Pull the executable / file path out of a Windows command line or resource reference, e.g.
/// `"C:\Program Files\App\app.exe" --x`, `C:\x\y.dll,-101`, `@%SystemRoot%\a.dll,-5`,
/// `rundll32.exe C:\x\y.dll,Entry`, `C:\Program Files\App\app.exe /s` (unquoted with spaces).
/// Returns `None` when no absolute path can be identified (bare names, GUIDs, URLs...).
pub fn extract_windows_path(raw: &str) -> Option<String> {
    let s = expand_env(raw.trim().trim_start_matches('@').trim());
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let s = s.strip_prefix(r"\??\").unwrap_or(s);

    // Quoted path.
    if let Some(rest) = s.strip_prefix('"') {
        let p = rest.split('"').next().unwrap_or("");
        let p = expand_env(p);
        return is_abs_windows(&p).then_some(p).or_else(|| launcher_arg(s));
    }

    if let Some(p) = launcher_arg(s) {
        return Some(p);
    }

    // `\SystemRoot\...` (driver ImagePath) and `System32\drivers\x.sys` (relative to %SystemRoot%).
    let lower = s.to_ascii_lowercase();
    let windir = env_var_ci("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let s: String = if let Some(r) = lower.strip_prefix(r"\systemroot\") {
        format!(r"{windir}\{}", &s[s.len() - r.len()..])
    } else if lower.starts_with(r"system32\") {
        format!(r"{windir}\{s}")
    } else {
        s.to_string()
    };

    if !is_abs_windows(&s) {
        return None;
    }

    // Unquoted: cut at the first known extension followed by a delimiter (",", " ", end).
    let lower = s.to_ascii_lowercase();
    const EXTS: [&str; 12] = [".exe", ".dll", ".sys", ".ocx", ".cpl", ".scr", ".com", ".bat", ".cmd", ".msi", ".ico", ".drv"];
    let mut best: Option<usize> = None;
    for ext in EXTS {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(ext) {
            let end = from + pos + ext.len();
            let next = lower.as_bytes().get(end);
            if matches!(next, None | Some(b',') | Some(b' ') | Some(b'\t') | Some(b'"')) {
                best = Some(best.map_or(end, |b: usize| b.min(end)));
                break;
            }
            from = end;
        }
    }
    let p = match best {
        Some(end) => &s[..end],
        // No extension: a folder path or a file without extension. Cut at a comma (resource ids).
        None => s.split(',').next().unwrap_or(&s),
    };
    Some(p.trim_end_matches('\\').to_string())
}

/// `rundll32.exe C:\x.dll,Entry` and friends: the real target is the first argument.
fn launcher_arg(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    for launcher in ["rundll32.exe ", "rundll32 ", "regsvr32.exe ", "regsvr32 "] {
        if let Some(pos) = lower.find(launcher) {
            let mut arg = s[pos + launcher.len()..].trim();
            // Skip switches such as `regsvr32 /s /u`.
            while arg.starts_with('/') || arg.starts_with('-') {
                arg = arg.split_once(' ').map_or("", |(_, r)| r).trim_start();
            }
            let arg = arg.trim_start_matches('"');
            let p = arg.split([',', '"']).next().unwrap_or("").trim();
            let p = expand_env(p);
            return is_abs_windows(&p).then_some(p);
        }
    }
    None
}

pub fn is_abs_windows(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')) || p.starts_with(r"\\")
}

/// Drive root of a Windows path (`C:\`), or `None` for UNC paths.
pub fn windows_drive_root(p: &str) -> Option<String> {
    is_abs_windows(p).then(|| p[..3].to_string()).filter(|r| !r.starts_with(r"\\"))
}

/// First token of a freedesktop `Exec=` line, without field codes.
pub fn desktop_exec_program(exec: &str) -> Option<String> {
    let exec = exec.trim();
    let prog = if let Some(rest) = exec.strip_prefix('"') { rest.split('"').next()?.to_string() } else { exec.split_whitespace().next()?.to_string() };
    // `env FOO=1 prog` / `env prog`
    if prog == "env" || prog.ends_with("/env") {
        return exec.split_whitespace().skip(1).find(|t| !t.contains('=') && !t.starts_with('-')).map(|s| s.trim_matches('"').to_string());
    }
    Some(prog)
}

/// Resolve a program name against `$PATH` (absolute paths are returned as-is).
pub fn which(prog: &str, cache: &PathCache) -> Option<PathBuf> {
    let p = Path::new(prog);
    if p.is_absolute() {
        return cache.exists(p).then(|| p.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(prog)).find(|c| cache.exists(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths() {
        assert_eq!(extract_windows_path(r#""C:\Program Files\App\app.exe" --x"#).as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(extract_windows_path(r"C:\Program Files\App\app.exe /s").as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(extract_windows_path(r"C:\x\y.dll,-101").as_deref(), Some(r"C:\x\y.dll"));
        assert_eq!(extract_windows_path(r"rundll32.exe C:\x\y.dll,Entry").as_deref(), Some(r"C:\x\y.dll"));
        assert_eq!(extract_windows_path(r"C:\Data\Folder\").as_deref(), Some(r"C:\Data\Folder"));
        assert_eq!(extract_windows_path("notepad.exe"), None);
        assert_eq!(extract_windows_path("{0000-1111}"), None);
        assert_eq!(extract_windows_path(r"\??\C:\drv\x.sys").as_deref(), Some(r"C:\drv\x.sys"));
    }

    #[test]
    fn desktop_exec() {
        assert_eq!(desktop_exec_program("firefox %u").as_deref(), Some("firefox"));
        assert_eq!(desktop_exec_program(r#""/opt/My App/run" %F"#).as_deref(), Some("/opt/My App/run"));
        assert_eq!(desktop_exec_program("env FOO=1 /usr/bin/x --y").as_deref(), Some("/usr/bin/x"));
    }
}

/// Case-insensitive wildcard match of a file name: `*` any run, `?` one character.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod wildcard_tests {
    use super::wildcard;

    #[test]
    fn patterns() {
        assert!(wildcard("*.tmp", "A.TMP"));
        assert!(wildcard("~*", "~lock.docx"));
        assert!(wildcard("*~", "notes.txt~"));
        assert!(wildcard("core.?*", "core.1234"));
        assert!(!wildcard("core.?*", "core."));
        assert!(!wildcard("*.tmp", "a.tmpx"));
        assert!(wildcard("thumbs.db", "Thumbs.db"));
    }
}
