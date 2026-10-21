//! Translations, gettext style: the English text is the message id, and each language is a
//! standard `.po` catalog in `locales/` (editable with Poedit, Weblate, Lokalize, …).
//! `locales/cleancore.pot` is the template listing every message.
//!
//! - `tr("Uninstall")` returns the text in the current language, or the English when there is
//!   no translation (so missing entries never break anything).
//! - `tf!("Removed {n} leftovers")` is `format!` with a translated template: the translation
//!   keeps the same `{…}` placeholders, in any order.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// A language CleanCore ships: code, name in that language, catalog.
pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    pub rtl: bool,
    po: &'static str,
}

pub const LANGUAGES: &[Language] = &[
    Language { code: "en", name: "English", rtl: false, po: "" },
    Language { code: "he", name: "עברית", rtl: true, po: include_str!("../locales/he.po") },
    Language { code: "es", name: "Español", rtl: false, po: include_str!("../locales/es.po") },
    Language { code: "fr", name: "Français", rtl: false, po: include_str!("../locales/fr.po") },
    Language { code: "de", name: "Deutsch", rtl: false, po: include_str!("../locales/de.po") },
    Language { code: "ru", name: "Русский", rtl: false, po: include_str!("../locales/ru.po") },
    Language { code: "uk", name: "Українська", rtl: false, po: include_str!("../locales/uk.po") },
];

/// Index into `LANGUAGES` of the language in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// Each language's catalog, parsed the first time it is used.
static CATALOGS: OnceLock<Vec<OnceLock<HashMap<&'static str, String>>>> = OnceLock::new();

fn catalog(i: usize) -> &'static HashMap<&'static str, String> {
    let all = CATALOGS.get_or_init(|| LANGUAGES.iter().map(|_| OnceLock::new()).collect());
    all[i].get_or_init(|| parse_po(LANGUAGES[i].po))
}

/// Use `code` ("he", "de", …), or the system language for "system" / empty. Unknown codes
/// fall back to English. `CLEANCORE_LANG` overrides both (used by the tests).
pub fn set_language(code: &str) {
    let env = std::env::var("CLEANCORE_LANG").ok();
    let code = match env.as_deref().or(Some(code)) {
        Some("") | Some("system") | None => system_language(),
        Some(c) => c.to_string(),
    };
    let i = LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0);
    CURRENT.store(i, Ordering::Relaxed);
}

pub fn current() -> &'static Language {
    &LANGUAGES[CURRENT.load(Ordering::Relaxed)]
}

pub fn is_rtl() -> bool {
    current().rtl
}

/// The two-letter code of the desktop's language, if CleanCore has it; "en" otherwise.
pub fn system_language() -> String {
    let raw = system_locale().unwrap_or_default();
    let code = raw.split(['_', '-', '.', '@']).next().unwrap_or("").to_ascii_lowercase();
    // Older systems still say "iw" for Hebrew.
    let code = if code == "iw" { "he".to_string() } else { code };
    if LANGUAGES.iter().any(|l| l.code == code) {
        code
    } else {
        "en".into()
    }
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    // The usual precedence: LANGUAGE (a list), LC_ALL, LC_MESSAGES, LANG.
    if let Ok(list) = std::env::var("LANGUAGE") {
        if let Some(first) = list.split(':').find(|l| !l.is_empty()) {
            return Some(first.to_string());
        }
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty() && v != "C" && v != "POSIX"))
}

#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide characters.
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// The text in the current language (the English itself when untranslated).
pub fn tr(text: &str) -> &str {
    let i = CURRENT.load(Ordering::Relaxed);
    if i == 0 {
        return text;
    }
    catalog(i).get(text).map(String::as_str).unwrap_or(text)
}

/// `format!` with a translated template. Same arguments as `format!`.
#[macro_export]
macro_rules! tf {
    ($fmt:literal $($args:tt)*) => {
        $crate::i18n::translate_formatted($fmt, ::std::format!($fmt $($args)*))
    };
}

/// Pieces of a format template: literal text and `{…}` placeholders (`{{`/`}}` are text).
fn pieces(template: &str) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    let mut lit = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                if !lit.is_empty() {
                    out.push((false, std::mem::take(&mut lit)));
                }
                let mut ph = String::from("{");
                for c in chars.by_ref() {
                    ph.push(c);
                    if c == '}' {
                        break;
                    }
                }
                out.push((true, ph));
            }
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        out.push((false, lit));
    }
    out
}

/// Given the English template and the text `format!` made from it, build the same text from
/// the translated template: each placeholder's value is read back out of `formatted` and put
/// where the translation has that placeholder.
pub fn translate_formatted(template: &'static str, formatted: String) -> String {
    let i = CURRENT.load(Ordering::Relaxed);
    if i == 0 {
        return formatted;
    }
    let Some(translated) = catalog(i).get(template) else { return formatted };
    let src = pieces(template);
    // Read the values: the text between one literal piece and the next.
    let mut values: Vec<(String, String)> = Vec::new();
    let mut rest = formatted.as_str();
    for (k, (is_ph, text)) in src.iter().enumerate() {
        if !is_ph {
            match rest.strip_prefix(text.as_str()) {
                Some(r) => rest = r,
                None => return formatted,
            }
            continue;
        }
        let value = match src.get(k + 1) {
            Some((false, next)) => match rest.find(next.as_str()) {
                Some(at) => &rest[..at],
                None => return formatted,
            },
            // Two placeholders in a row can't be told apart; the end takes the rest.
            Some((true, _)) => return formatted,
            None => rest,
        };
        values.push((text.clone(), value.to_string()));
        rest = &rest[value.len()..];
    }
    // Write the translation; repeated placeholders (`{}`) are used in their original order.
    let mut used = vec![false; values.len()];
    let mut out = String::new();
    for (is_ph, text) in pieces(translated) {
        if !is_ph {
            out.push_str(&text);
            continue;
        }
        match values.iter().enumerate().position(|(j, (ph, _))| !used[j] && *ph == text) {
            Some(j) => {
                used[j] = true;
                out.push_str(&values[j].1);
            }
            None => match values.iter().find(|(ph, _)| *ph == text) {
                Some((_, v)) => out.push_str(v),
                None => return formatted,
            },
        }
    }
    out
}

/// Parse a `.po` catalog into msgid → msgstr, skipping untranslated and fuzzy entries.
pub fn parse_po(po: &'static str) -> HashMap<&'static str, String> {
    let mut map = HashMap::new();
    let (mut id, mut s, mut fuzzy, mut field) = (String::new(), String::new(), false, 0u8);
    // A "#, fuzzy" comment belongs to the entry that follows it.
    let mut next_fuzzy = false;
    let mut flush = |id: &mut String, s: &mut String, fuzzy: &mut bool| {
        if !id.is_empty() && !s.is_empty() && !*fuzzy {
            // Leak the id once per message so lookups can be keyed by &str for the whole run.
            map.insert(&*Box::leak(std::mem::take(id).into_boxed_str()), std::mem::take(s));
        }
        id.clear();
        s.clear();
        *fuzzy = false;
    };
    for line in po.lines().map(str::trim) {
        if line.starts_with("#,") && line.contains("fuzzy") {
            next_fuzzy = true;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut s, &mut fuzzy);
            fuzzy = std::mem::take(&mut next_fuzzy);
            id.push_str(&unquote(rest));
            field = 1;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            s.push_str(&unquote(rest));
            field = 2;
        } else if line.starts_with('"') {
            match field {
                1 => id.push_str(&unquote(line)),
                2 => s.push_str(&unquote(line)),
                _ => {}
            }
        } else if line.is_empty() || line.starts_with('#') {
            field = 0;
        }
    }
    flush(&mut id, &mut s, &mut fuzzy);
    map
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').filter(|r| !r.ends_with('\\') || r.ends_with("\\\\")).unwrap_or(s);
    unescape(s)
}

/// Undo the C-style escapes of a `.po` or Rust string literal.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"
msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Uninstall"
msgstr "Deinstallieren"

msgid "Removed {n} leftover(s) from {}"
msgstr "{} : {n} Reste entfernt"

#, fuzzy
msgid "Fuzzy"
msgstr "Unsicher"

msgid "Line \"one\"\n"
"two"
msgstr "Zeile \"eins\"\n"
"zwei"
"#;

    #[test]
    fn parses_po() {
        let m = parse_po(PO);
        assert_eq!(m.get("Uninstall").map(String::as_str), Some("Deinstallieren"));
        assert!(!m.contains_key("Fuzzy"));
        assert!(!m.contains_key(""));
        assert_eq!(m.get("Line \"one\"\ntwo").map(String::as_str), Some("Zeile \"eins\"\nzwei"));
    }

    #[test]
    fn placeholders() {
        let p = pieces("{n} of {total} · {secs:.0}s {{x}}");
        let phs: Vec<&str> = p.iter().filter(|(ph, _)| *ph).map(|(_, t)| t.as_str()).collect();
        assert_eq!(phs, ["{n}", "{total}", "{secs:.0}"]);
        assert!(p.iter().any(|(ph, t)| !ph && t.contains("{x}")));
    }

    /// The message ids of `locales/cleancore.pot`, unescaped as the program sees them.
    fn template() -> Vec<String> {
        let pot = include_str!("../locales/cleancore.pot");
        let mut ids: Vec<String> = parse_po_ids(pot);
        ids.retain(|i| !i.is_empty());
        ids
    }

    fn parse_po_ids(po: &str) -> Vec<String> {
        po.lines().filter_map(|l| l.trim().strip_prefix("msgid ")).map(unquote).collect()
    }

    #[test]
    fn every_message_is_translated() {
        let ids = template();
        assert!(ids.len() > 1000, "template looks empty");
        for (i, l) in LANGUAGES.iter().enumerate().skip(1) {
            let cat = catalog(i);
            let missing: Vec<&String> = ids.iter().filter(|m| !cat.contains_key(m.as_str())).collect();
            assert!(missing.is_empty(), "{}: {} untranslated, e.g. {:?}. Translate them in locales/{}.po.", l.code, missing.len(), &missing[..missing.len().min(5)], l.code);
        }
    }

    /// Every `tr("…")` / `tf!("…")` in the interface is in the template, so new text can't
    /// slip through untranslated: run `python3 locales/extract.py`, then translate.
    #[test]
    fn interface_strings_are_in_the_template() {
        let ids: std::collections::HashSet<String> = template().into_iter().collect();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/gui");
        let mut missing = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            for marker in ["tr(\"", "tf!(\""] {
                for (at, _) in text.match_indices(marker) {
                    let rest = &text[at + marker.len()..];
                    let Some(end) = rest.find(|c| c == '"').filter(|_| !rest.starts_with('"')) else { continue };
                    // Find the closing quote, skipping escaped ones.
                    let mut j = 0;
                    let b = rest.as_bytes();
                    while j < b.len() && !(b[j] == b'"' && (j == 0 || b[j - 1] != b'\\')) {
                        j += 1;
                    }
                    let _ = end;
                    let lit = unescape(&rest[..j]);
                    // The same rule as locales/extract.py: words, and a space or a capital.
                    let words: String = pieces(&lit).into_iter().filter(|p| !p.0).map(|p| p.1).collect();
                    let is_text = words.as_bytes().windows(2).any(|w| w[0].is_ascii_alphabetic() && w[1].is_ascii_alphabetic())
                        && (words.contains(' ') || {
                            let w = words.trim();
                            w.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && w.chars().nth(1).is_some_and(|c| c.is_ascii_lowercase())
                        });
                    if is_text && !ids.contains(&lit) {
                        missing.push(format!("{}: {lit:?}", e.file_name().to_string_lossy()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "not in locales/cleancore.pot (run python3 locales/extract.py): {missing:#?}");
    }

    #[test]
    fn translates_formatted_text() {
        set_language("de");
        assert_eq!(tr("Uninstall"), "Deinstallieren");
        let n = 3;
        assert_eq!(crate::tf!("Restore {n} item(s)"), "3 Element(e) wiederherstellen");
        assert_eq!(crate::tf!("Exported {} programs to {}", 12, "/tmp/x"), "12 Programme exportiert nach /tmp/x");
        // Unknown text and English pass through.
        assert_eq!(tr("no such message"), "no such message");
        set_language("en");
        assert_eq!(tr("Uninstall"), "Uninstall");
    }

    #[test]
    fn every_catalog_parses_and_keeps_placeholders() {
        for (i, l) in LANGUAGES.iter().enumerate().skip(1) {
            let cat = catalog(i);
            assert!(!cat.is_empty(), "{} has no translations", l.code);
            for (id, s) in cat {
                let mut a: Vec<String> = pieces(id).into_iter().filter(|p| p.0).map(|p| p.1).collect();
                let mut b: Vec<String> = pieces(s).into_iter().filter(|p| p.0).map(|p| p.1).collect();
                a.sort();
                b.sort();
                assert_eq!(a, b, "{}: placeholders differ in {id:?} -> {s:?}", l.code);
            }
        }
    }
}
