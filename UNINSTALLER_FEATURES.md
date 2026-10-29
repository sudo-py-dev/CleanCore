# Revo Uninstaller Pro: feature map for CleanCore

CleanCore combines two products in one app:
- **Revo Registry Cleaner Pro** features, already built and kept: Cleaner, Tuner, Watcher, Backups, Scheduler. See [FEATURES.md](FEATURES.md).
- **Revo Uninstaller Pro** features, listed here. They come from the official online manual at revouninstaller.com/online-manual/.

The Linux column shows the equivalent behaviour. Linux has packages (deb/rpm/pacman/flatpak/snap/AppImage), config folders and dconf instead of an MSI database and a registry.

Status key: ✅ done · ⏳ later phase. Phases 1–5 are tested on Linux (CLI in a sandbox, end-to-end restore, headless GUI walkthrough); the Windows side type-checks but hasn't been run on Windows yet.

## 1. Uninstaller

### 1.1 All Programs
| Feature | Windows | Linux | Phase |
|---|---|---|---|
| Program list: Name, Size, Version, Type, Install date, Company, Comment, Uninstall string, Registry key, Website | `Uninstall` keys in HKLM, HKLM\WOW6432Node and HKCU (and every loaded user hive) | Desktop apps from dpkg/rpm/pacman, Flatpak, Snap and AppImage. All packages are shown when "show system components" is on | ✅ |
| Show Windows updates / system components | `SystemComponent`, `ParentKeyName`, KB entries | All packages, not only apps | ✅ |
| "New programs" group (last N days) and custom groups (Internet, Games, Multimedia, Utilities) | ✓ | ✓ | ✅ |
| Search by name, publisher or comment; sort; status bar count | ✓ | ✓ | ✅ |
| Export list to TXT, HTML or CSV/XLS | ✓ | ✓ | ✅ |
| Context menu: search online, About/Help/Update links, install location, open registry key, comment, add to group, change icon | ✓ | ✓ (no registry key) | ✅ (change icon: a palette of icons per program) |
| **Uninstall wizard:** analyze, restore point plus registry backup, run the built-in uninstaller, scan (Safe/Moderate/Advanced), registry leftovers, file leftovers, summary | ✓ | Package-manager removal through pkexec; leftovers are config/data/cache folders, dconf keys, autostart, .desktop files, systemd user units and residual package config | ✅ |
| Leftover rules: only entries created by the program are deleted, protected items are never touched, everything is backed up | ✓ | ✓ | ✅ |
| Delete files to the Recycle Bin, to the backup folder, or permanently | ✓ | Trash (freedesktop) / backup / permanent | ✅ |
| Scan all Windows accounts (Advanced) | ✓ | All home folders | ✅ |
| **Quick Uninstall:** silent uninstaller, automatic Moderate scan, automatic leftover removal | `QuietUninstallString`, MSI `/qn`, NSIS `/S`, Inno `/VERYSILENT` | Non-interactive package removal | ✅ |
| **Multiple Uninstall:** queue, one restore point and registry backup, summary | ✓ | ✓ | ✅ |
| **Forced MSI Uninstall** (when the uninstaller only offers Change/Repair) | `msiexec /x {ProductCode}` | n/a | ✅ |
| Change / Repair | `ModifyPath` | n/a | ✅ |
| Resume an interrupted uninstall after a reboot | ✓ | ✓ | ✅ the uninstall is remembered until its leftovers are handled; a restart request (MSI 3010/1641) starts `cleancore-gui --resume` at the next login (RunOnce / autostart); the Uninstaller page offers "Scan for leftovers"; `cleancore uninstall --resume` |
| Install-date cache, comments, groups | ✓ | ✓ | ✅ |

### 1.2 Forced Uninstall
| Feature | Phase |
|---|---|
| Inputs: program name(s) separated by `;` and/or paths to the program folder or main exe | ✅ |
| Finds matching built-in uninstallers and trace logs (local and database) | ✅ (a database log is imported into Traced Programs when used) |
| Run the selected uninstaller, then a leftover scan | ✅ |

### 1.3 Traced Programs and the Real-Time Installation Monitor
| Feature | Windows | Linux | Phase |
|---|---|---|---|
| Install with CleanCore: snapshot, run installer, Stop, review, Save (also `cleancore trace start/begin/end`) | Registry (HKLM\SOFTWARE, HKCU\Software, Services) and Program Files / ProgramData / AppData / System32 snapshot diff; new Store apps | /opt, /usr/local, /etc, home config folders and dotfiles, dconf, and the package database (deb/rpm/pacman/Flatpak/Snap) | ✅ files are filtered to the installer's own process tree: ETW Kernel-Process/Kernel-File on Windows (with msiexec; needs administrator rights), `strace -f` on Linux (skipped for package-manager installs and when the installer switches to root). Settings and packages stay unfiltered: services write them |
| Log list (Details view), search | ✓ | ✓ | ✅ with log groups |
| Uninstall, Custom Uninstall and Advanced Uninstall from a log; restore renamed files | ✓ | ✓ | ✅ modified small config files (≤ 64 KB) and settings values (≤ 4 KB) keep their earlier content in the log and are restored on uninstall (backed up first) |
| Manage logs: delete, merge, rename, edit (properties, registry items, files), comment, icon, export/import (`.ruel`-like JSON), back up and restore all logs | ✓ | ✓ | ✅ `.cptrace` JSON, TXT/HTML export, custom log icon |
| Context-menu "Install with" / "Uninstall with" | Explorer shell verb | Nautilus script and `.desktop` action | ✅ Windows: HKCU shell verbs on .exe, .msi, shortcuts (.lnk targets are read) and folders; Linux: Nautilus scripts, a KDE/Dolphin service menu and an "Open With" entry for installers. Settings toggle or `cleancore shell-integration on` |

### 1.4 Windows Apps
| Feature | Windows | Linux | Phase |
|---|---|---|---|
| List Store and built-in apps: name, size, install date, version, publisher | `Get-AppxPackage` | Flatpak and Snap apps | ✅ also listed in All Programs, with their own icons |
| Uninstall, uninstall for all users, reset the app, run, terminate, reset Microsoft Store | `Remove-AppxPackage [-AllUsers]`, `Reset-AppxPackage`, `wsreset` | `flatpak uninstall`, `snap remove`, run, kill | ✅ uninstall, all users, reset, run, terminate, reset Microsoft Store (`cleancore app …`) |

### 1.5 Browser Extensions
| Feature | Phase |
|---|---|
| Chrome, Edge, Brave, Opera, Vivaldi (Chromium profiles) and Firefox: name, type, version, description, profile | ✅ (also Chromium and Snap/Flatpak builds on Linux; `__MSG_` names are localized; built-in extensions are hidden unless asked for) |
| Remove an extension (browser closed); per-profile command line (`/be`) | ✅ extension files, stored data and the Preferences / Secure Preferences / extensions.json entries, all backed up; refused while the browser runs |

### 1.6 Logs Database
| Feature | Phase |
|---|---|
| Revo's online database is proprietary, so CleanCore uses a shared logs folder or URL: import, filter by architecture/OS, uninstall, custom uninstall | ✅ folder (Settings › Uninstaller, default `<data>/logs-db`, may be a network share), URL with `index.json` (Update), "Add to logs database" from Traced Programs |

### 1.7 Hunter Mode
| Feature | Phase |
|---|---|
| Floating target: drag onto a window, shortcut or tray icon (Windows: `WindowFromPoint` → PID → exe; Linux X11: `_NET_WM_PID`) | ✅ floating always-on-top target window (CleanCore minimizes), tray icon (Windows notification area; Linux StatusNotifierItem) with Show target / Open CleanCore / Exit, drag-onto-window or "Pick in 3 seconds", and a running-programs list (the only way on Wayland, which hides other windows; Linux apps are found by their systemd `app-*.scope`) |
| Actions: Uninstall (Forced), Stop auto-starting, Kill, Kill and delete, Open folder, Search online, Properties | ✅ Uninstall (wizard, or Forced Uninstall when not listed), Stop autorun, Kill, Kill and delete (backed up), Open folder, Search online; details show file, publisher, owning program and autorun entries |

### 1.8 Uninstall History
| Feature | Phase |
|---|---|
| Entries grouped by date: operation, program, deleted and failed leftovers; export to TXT | ✅ (copy as text) |

## 2. Backup Manager
The shared backup store is already built.

| Feature | Phase |
|---|---|
| Registry backups for every uninstall and autorun change; optional file backups | ✅ (autorun removals are backed up; enable/disable is reversible by itself) |
| Restore, custom restore, delete, delete all | ✅ |
| Delete by time range | ✅ (Backups page, `cleancore backup delete --from … --to …`) |
| Restore point before uninstall; daily full registry backup | ✅ restore point, daily full backup on first start of the day (`daily_full_backup`, `cleancore backup daily`) |

## 3. Tools
| Tool | Windows | Linux | Phase |
|---|---|---|---|
| **Autorun Manager:** Run, RunOnce, Startup folder, services, scheduled tasks; enable/disable, add, remove, remove invalid/disabled, run/kill, export | ✓ (enable/disable through `StartupApproved` like Task Manager, `sc config`, `schtasks`) | XDG autostart (system entries switched with a per-user override), systemd user/system units, cron `@reboot` | ✅ run now / stop, login-script entries (`~/.xprofile`, `~/.xsessionrc`, `~/.profile` …, disabled by commenting the line, files copied first); system entries are only disabled, never deleted |
| **Junk Files Cleaner:** drives or folder, extension patterns, include/exclude lists, skip files used in the last 24 h, to Trash | ✓ (all fixed drives by default) | ✓ (home folder by default) | ✅ wildcard patterns you can switch off/add/remove, locations, excluded folders, OS folders always skipped, 24 h rule, Trash or permanent, export TXT/CSV; parallel scan |
| **Windows Tools:** 22 launchers plus custom | ✓ (31 tools; admin ones ask for elevation) | Equivalent Linux tools, shown only when installed (System Monitor, Disks, Disk Usage, GParted, journal, services, network, firewall, updates, …); console tools open in a terminal | ✅ plus your own tools |
| **Microsoft slop cleaner** (CleanCore extra) | Preinstalled and pushed apps (games, Bing apps, Clipchamp, Copilot, Xbox, Teams…) removed for this user or every user, optionally with the provisioned copy so they don't return; Copilot, Recall, Click to Do, AI in Paint/Notepad, Edge sidebar, Start/lock-screen ads, advertising ID, Bing search, Widgets and telemetry switches (Tuner tweaks, reversible) | n/a | ✅ (`cleancore slop …`) |
| **History Cleaner:** browsers (history, cache), Windows (recent docs, Run MRU, clipboard, Recycle Bin, temp files, thumbnails, crash dumps, …), MS Office MRU | ✓ (recent docs, Run / typed paths / search / Open-Save MRUs (backed up), temp, thumbnails, crash dumps + WER, Recycle Bin, clipboard, Office File/Place MRU, LibreOffice) | Recent files, thumbnails, Trash, clipboard, shell history, ~/.cache, crash reports, LibreOffice recent documents | ✅ per browser profile: cache, history (Firefox via SQL so bookmarks stay), form data, cookies and session (opt-in); open browsers are skipped; the selection is remembered |
| **Evidence Remover:** overwrite a drive's free space (3 random passes, pause/resume) | ✓ | ✓ | ✅ 1/3/7 passes (zeros, ones, random; last pass random), also fills small-file slots, pause/resume/cancel, cleans up even when cancelled |
| **Unrecoverable Delete:** shred files/folders, pause/resume | ✓ | ✓ | ✅ overwrite, truncate, rename to random names, delete; drag & drop; symlinks are removed, never followed; roots, home and OS folders refused; SSD caveat shown |

## 4. Options
| Section | Phase |
|---|---|
| General: backup folder, daily registry backup, skip messages, shell context menu | partly ✅ |
| Uninstaller: restore point, registry backup, back up deleted registry items, force-stop running exes, delete files to Recycle Bin/backup/permanent, "new programs" window | ✅ incl. closing the program's running processes first |
| All Programs: show updates/system components, run only the built-in uninstaller, check all leftovers by default, registry and folder exclude lists (wildcards) | ✅ incl. "only run the built-in uninstaller" and `*` / `?` wildcards in exclude lists |
| Traced Programs: exclude list (`trace_excludes`), delete log after uninstall | ✅ |
| Logs Database (folder, URL), Autorun "show system entries" | ✅ |
| Junk cleaner (patterns, locations, exclusions, 24 h, Trash), overwrite passes | ✅ |
| Groups manager | ✅ create, rename, reorder, delete (GUI dialog, `cleancore groups …`) |

## 5. Command line
| Revo | CleanCore | Phase |
|---|---|---|
| `RevoCmd /m "name*" /i /u` | `cleancore programs [--match "name*"] [--location] [--uninstall-cmd]` (also `cleancore /m …`) | ✅ |
| `/mu "name" /path "dir" /mode Moderate` | `cleancore uninstall "name" [--path dir] --mode moderate [--quick]` | ✅ |
| `/tu "log"` | `cleancore trace uninstall "log"` (also `/tu`) | ✅ |
| `/export "file" /all /txt` | `cleancore programs --export file.txt` | ✅ |
| `/wa "App1;App2"` | `cleancore uninstall --app "App1;App2"` | ✅ |
| `/be "ext" /chrome "Profile"` | `cleancore extensions remove "Ext1;Ext2" --browser chrome --profile "Profile"` (also `/be`; browsers `/chrome /edge /brave /opera /vivaldi /chromium /firefox`) | ✅ |
| `/hunter` | `cleancore-gui --hunter` (also `cleancore /hunter`); scripting: `cleancore hunter --list [--all]`, `--pid N` or `--at-pointer` with `--action info|uninstall|stop-autorun|kill|kill-delete|folder` | ✅ |
| (no Revo equivalent) | `cleancore autorun list [--all] [--json] [--export f]`, `enable/disable/remove ENTRY`, `add --name --command [--method run|runonce|startup] [--all-users]`, `remove-invalid`, `remove-disabled` | ✅ |
| (no Revo equivalent) | `cleancore extensions list [--browser b] [--all] [--json]` | ✅ |
| (no Revo equivalent) | `cleancore logsdb list [--all] / update / add LOG / uninstall NAME` | ✅ |
| (no Revo equivalent) | `cleancore junk scan|clean [--location P] [--pattern "*.tmp"] [--include-recent] [--permanent] [--export f]` | ✅ |
| (no Revo equivalent) | `cleancore history list` / `history clean [--only id,id|all]` | ✅ |
| (no Revo equivalent) | `cleancore shred PATH… [--passes N]`, `cleancore wipe-free [PATH] [--passes N]` | ✅ |
| (no Revo equivalent) | `cleancore tools list / run NAME / add NAME CMD / remove NAME` | ✅ |

## Delivery phases
1. **Uninstaller core** (✅ done; Windows side type-checked, not yet run on real Windows): All Programs, the uninstall wizard with leftover scans, Quick/Multiple/Forced/Forced MSI uninstall, Windows Apps listing and uninstall, history, export, groups and comments, CLI, and the GUI page.
2. **Install tracing** (✅ done): the real-time monitor, Traced Programs and log management, Custom/Advanced uninstall.
3. **Browser Extensions, Autorun Manager, Hunter Mode, Logs Database** (✅ done; Windows side type-checked, not yet run on real Windows).
4. **Junk Files Cleaner, History Cleaner, Evidence Remover, Unrecoverable Delete, Windows Tools** (✅ done; Windows side type-checked, not yet run on real Windows). Backup Manager extras (delete by time range, daily full backup) followed in phase 5.
5. **Remaining items** (✅ done): Backup Manager extras, uninstaller options (close running program, built-in only, wildcard excludes, resume after restart), groups manager and icons, log groups and icons, restoring modified items, following the installer's processes, Store app run/terminate/reset, Hunter floating target, tray icon and search, autorun run/kill and login scripts, file-manager integration.
