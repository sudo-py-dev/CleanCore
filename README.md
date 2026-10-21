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

- **Uninstaller**: removes programs, then finds and deletes everything they left behind
- **Quick and batch uninstall**: silent uninstallers plus automatic leftover cleanup for many programs at once
- **Install and trace**: records an installation so it can be undone exactly later
- **Cleaner**: finds broken launchers, dead startup items, orphaned settings and registry entries
- **Junk files, history and evidence remover**: temp files, browser history, free space wiping
- **Unrecoverable delete**: overwrites files before removing them
- **Autorun manager**: everything that starts with the system, in one list
- **Tuner**: reversible system tweaks
- **Watcher**: live view of every change to config files, settings or the registry
- **Backups**: everything is backed up before it is deleted, and can be restored in one click

<p align="center">
  <img src="docs/uninstall.png" width="820" alt="Leftovers after an uninstall">
</p>

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
