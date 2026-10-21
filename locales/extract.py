#!/usr/bin/env python3
"""Collect CleanCore's interface text into locales/cleancore.pot and merge it into each .po.

Run from the repository root after changing interface text:

    python3 locales/extract.py

Existing translations are kept; new messages are added untranslated (empty msgstr) and
messages that no longer exist are dropped. Then translate the empty entries (Poedit, Weblate
or any .po editor). `cargo test` fails while a catalog misses a message.
"""
import glob
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANGS = ["he", "es", "fr", "de", "ru", "uk"]
LIT = r'"((?:[^"\\]|\\.)*)"'


def ui_text(t):
    """Looks like words a person reads: a letter, and a space or a capitalised word."""
    words = re.sub(r"\{[^{}]*\}", "", t)
    return re.search(r"[A-Za-z]{2}", words) is not None and (" " in words or re.match(r"[A-Z][a-z]", words.strip()) is not None)


# Literals in these positions are ids, salts or paths, not text.
NOT_TEXT = re.compile(
    r"(Id::unique\(|id_salt\(|Id::new\(|include_bytes!\(|include_str!\(|Grid::new\(|\.join\(|with\(|"
    r"browse_button\(ui,\s*$|path_input\(ui,\s*$|key_or_path_input\(ui,\s*$|dropdown\(ui,\s*$|"
    r"Command::new\(|\.arg\(|\.args\(\[|env::var|set_var\(|starts_with\(|ends_with\(|contains\(|"
    r"strip_prefix\(|strip_suffix\(|eq_ignore_ascii_case\(|#\[|load_texture\()\s*$"
)


def literals(text):
    for m in re.finditer(LIT, text):
        before = text[max(0, m.start() - 60) : m.start()]
        # Raw strings (r"C:\Users\…") are path examples, not text to translate.
        if before.endswith(("r", "#")):
            continue
        yield m.group(1), before


def gui_messages():
    out = []
    for f in sorted(glob.glob(str(ROOT / "src/gui/*.rs"))):
        text = Path(f).read_text()
        # Skip the test modules.
        text = text.split("#[cfg(test)]")[0]
        for lit, before in literals(text):
            if ui_text(lit) and not NOT_TEXT.search(before):
                out.append(lit)
    return out


def backend_messages():
    out = []
    # Scan categories: check!(Type, "id", "Name", "Description", mode) and CheckInfo { … }.
    for f in ("src/platform/linux.rs", "src/platform/windows.rs"):
        text = (ROOT / f).read_text()
        for m in re.finditer(r'check!\(\s*\w+,\s*"[^"]*",\s*' + LIT + r",\s*" + LIT, text):
            out += [m.group(1), m.group(2)]
        for m in re.finditer(r"name:\s*" + LIT + r",\s*description:\s*" + LIT, text):
            out += [m.group(1), m.group(2)]
    # `fn label` of enums shown in the interface.
    for f in ("src/uninstaller/mod.rs", "src/autorun.rs", "src/tracer/mod.rs", "src/backup.rs", "src/model.rs"):
        text = (ROOT / f).read_text()
        for m in re.finditer(r"fn label\(self\)[^{]*\{(.*?)\n    \}", text, re.S):
            out += [lit for lit, _ in literals(m.group(1)) if ui_text(lit)]
    # Built-in system tools, history items and Windows "slop" entries.
    for f in ("src/platform/linux_tools.rs", "src/platform/windows_tools.rs", "src/slop.rs", "src/history.rs"):
        text = (ROOT / f).read_text().split("#[cfg(test)]")[0]
        out += [lit for lit, before in literals(text) if ui_text(lit) and not NOT_TEXT.search(before) and "{" not in lit and "\\" not in lit and "/" not in lit]
    # Tuner tweaks.
    for f in ("tweaks/linux.toml", "tweaks/windows.toml"):
        for line in (ROOT / f).read_text().splitlines():
            m = re.match(r'\s*(name|description|label)\s*=\s*' + LIT, line)
            if m and m.group(2):
                out.append(m.group(2))
    return out


def unique(items):
    seen, out = set(), []
    for i in items:
        if i not in seen:
            seen.add(i)
            out.append(i)
    return out


def po_quote(s):
    return '"' + s + '"'  # already escaped as in Rust source


def read_po(path):
    """msgid -> msgstr (escaped form, as written in the file)."""
    entries, cur_id, cur_str, field = {}, None, None, None
    if not path.exists():
        return entries
    for line in path.read_text().splitlines():
        line = line.strip()
        if line.startswith("msgid "):
            if cur_id is not None:
                entries[cur_id] = cur_str or ""
            cur_id, cur_str, field = line[7:-1], "", "id"
        elif line.startswith("msgstr "):
            cur_str, field = line[8:-1], "str"
        elif line.startswith('"') and field:
            if field == "id":
                cur_id += line[1:-1]
            else:
                cur_str += line[1:-1]
        elif not line or line.startswith("#"):
            field = None
    if cur_id is not None:
        entries[cur_id] = cur_str or ""
    entries.pop("", None)
    return entries


HEADER = 'msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n"Language: {lang}\\n"\n\n'


def main():
    msgs = unique(gui_messages() + backend_messages())
    pot = HEADER.format(lang="") + "".join(f"msgid {po_quote(m)}\nmsgstr \"\"\n\n" for m in msgs)
    (ROOT / "locales/cleancore.pot").write_text(pot)
    for lang in LANGS:
        path = ROOT / f"locales/{lang}.po"
        old = read_po(path)
        body = "".join(f"msgid {po_quote(m)}\nmsgstr {po_quote(old.get(m, ''))}\n\n" for m in msgs)
        path.write_text(HEADER.format(lang=lang) + body)
        missing = sum(1 for m in msgs if not old.get(m))
        print(f"{lang}: {len(msgs) - missing}/{len(msgs)} translated")


if __name__ == "__main__":
    main()
