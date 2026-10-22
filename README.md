<p align="center">
  <img src="assets/logo.svg" width="96" alt="CleanCore logo">
</p>

<h1 align="center">CleanCore</h1>

<p align="center">
  A fast uninstaller and system cleaner for Linux and Windows, written in Rust.
</p>

<p align="center">
  <img src="docs/cleaner.png" width="820" alt="CleanCore cleaner">
</p>

## Features

### Programs

- **Uninstaller**: runs a program's own uninstaller, then finds and deletes everything it left behind
- **Quick and batch uninstall**: silent uninstallers plus automatic leftover cleanup for many programs at once
- **Forced uninstall**: removes what's left of programs whose uninstaller is gone or broken
- **Install and trace**: records an installation so it can be undone exactly later; trace logs can be shared through a logs database
- **Browser extensions**: lists and removes extensions of Chrome, Edge, Brave, Opera, Vivaldi and Firefox, for every profile
- **Autorun manager**: programs, services and tasks that start with the system, in one list
- **Hunter Mode** (Windows): point at any window to uninstall its program, stop it from starting, or kill it

<p align="center">
  <img src="docs/uninstall.png" width="820" alt="Leftovers after an uninstall">
</p>

### Clean up

- **Cleaner**: finds broken launchers, dead startup items, orphaned settings and registry entries
- **Auto Clean**: scans and cleans on a schedule (systemd user timers / Task Scheduler)
- **Junk files**: temp files, editor backups and crash dumps on your drives
- **Duplicate finder**: identical files, found fast: grouped by size, then compared by a few KB from each end, and only files still alike are hashed whole (BLAKE3), all in parallel. A whole home folder takes seconds. Every copy but one is ticked (keep the oldest, the newest or the shortest path), and one copy of each file is always kept.
- **History cleaner**: browser history and caches, recent files and other traces
- **Unrecoverable delete and evidence remover**: overwrite files, or the free space, so nothing can be recovered
- **Microsoft slop cleaner** (Windows): removes preinstalled and pushed apps, turns off Copilot, ads and telemetry, and removes Edge and OneDrive and keeps them away
- **Tuner**: reversible system tweaks
- **System tools**: the operating system's own utilities in one place, plus your own

<p align="center">
  <img src="docs/duplicates.png" width="820" alt="Duplicate finder results">
</p>

### Monitor and restore

- **Watcher**: live view of every change to config files, settings or the registry
- **Backups**: everything is backed up before it is deleted, and can be restored in one click; optionally a full backup every day

## Install

Download from the [latest release](https://github.com/sudo-py-dev/CleanCore/releases/latest):

| System | File |
| --- | --- |
| Windows | `cleancore-<version>-windows-x64-setup.exe`: a setup wizard with Start menu entry, optional desktop shortcut and an uninstaller in Apps & features. Or the `.zip` to run without installing. |
| Debian, Ubuntu and derivatives | `cleancore_<version>_amd64.deb` |
| Other Linux (x86_64) | `cleancore-<version>-linux-x86_64.tar.gz` |

CleanCore updates itself from Settings › Updates (or `cleancore update`), and checks every
download against the release's signature first.

## Languages

English, עברית, Español, Français, Deutsch, Русский and Українська. CleanCore follows your
desktop's language, or pick one in Settings › Appearance.

Translations are standard gettext catalogs in [`locales/`](locales). To add or fix one, edit
the `.po` file with any PO editor (Poedit, Weblate, Lokalize). After changing interface text
in the code, run `python3 locales/extract.py` to update the catalogs.

**Adding a language** needs only its catalog; no code changes:

1. Copy `locales/cleancore.pot` to `locales/<code>.po`, using the language's code (`ar`, `it`,
   `pt_BR`, `zh_CN`, …), and set `"Language: <code>\n"` in its header.
2. Optionally add `"X-Language-Name: Italiano\n"` to the header, which is the name shown in
   Settings. Common languages already have their usual name. Right-to-left languages (Arabic,
   Persian, Urdu, Hebrew, Yiddish, …) are recognized by their code; for any other language,
   `"X-Text-Direction: rtl\n"` sets the direction.
3. Translate, then rebuild. The language appears in Settings and is picked up from the desktop
   language. Right-to-left languages mirror the whole interface and their text is laid out in
   correct reading order. Scripts the bundled fonts don't cover (Arabic, Chinese, Japanese,
   Korean, Thai, Hindi, …) use a font from the system. Untranslated entries show in English.

## Build

You need a recent [Rust toolchain](https://rustup.rs).

```bash
git clone https://github.com/sudo-py-dev/CleanCore
cd CleanCore
cargo build --release
```

This builds two programs in `target/release`:

| Program | What it is |
| --- | --- |
| `cleancore-gui` | The desktop app |
| `cleancore` | The command line tool |

## Command line

```bash
cleancore scan --mode moderate     # look for problems, change nothing
cleancore clean                    # back up, then remove what the scan finds
cleancore programs                 # list installed programs
cleancore uninstall "Program Name" # uninstall and remove leftovers
cleancore junk scan                # find junk files
cleancore dupes scan               # find duplicate files
cleancore dupes clean --keep newest # delete the extra copies, keeping one of each
cleancore --help                   # everything else
```

## Platforms

| | Linux | Windows |
| --- | --- | --- |
| Uninstall and leftovers | deb, rpm, pacman, Flatpak, Snap, AppImage | Win32, MSI, Microsoft Store |
| Settings cleaned | config files, dconf / GNOME | registry |
| Restore points | snapper / timeshift | System Restore |

## License

[Apache 2.0](LICENSE)
