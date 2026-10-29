# Feature analysis: `RevoRegCleanerProSetup.exe`

> **Note:** the installer in this folder is **Revo Registry Cleaner Pro 2.2.5**, made by VS Revo Group.
> It is *not* Revo **Uninstaller** Pro. Everything below describes the Registry Cleaner.

## How it was unpacked

| Step | Result |
|---|---|
| Installer type | Inno Setup 6.6.1, one solid LZMA1 stream (64 MB dictionary). `innoextract` 1.9 can't read this version. |
| Extraction | Custom parser: header blocks at `0x5cedf79`, data chunk at `0xeb400` → 335 MB, 664 file entries (separate x64 and x86 sets) |
| Sources used | User Guide PDF (May 2026), `Langs/English.xaml` (all UI strings), strings from the .NET and native binaries, and the driver INF |

## Original architecture

| Component | Tech | Role |
|---|---|---|
| `Revo Registry Cleaner.exe/.dll` | .NET 6 WPF, self-contained | Main GUI |
| `RRC Common.dll` | .NET | Shared models and settings |
| `RRCHelper.dll` | Native C++, 2.5 MB | Scan engine: registry walking, path validation, COM/CLSID checks |
| `RRCcmd.exe` | Native x86 | Command-line front end |
| `RRCsrv.exe` | .NET | Background service: runs scheduled Auto Clean and Auto Backup, hosts the watcher |
| `RRCRestore.exe` | .NET | Standalone restore tool, also usable from WinRE |
| `RevoSrp.exe` | Native | Creates a System Restore Point |
| `rrclp.exe` | Native COM local server (`/regserver`) | Elevated helper for privileged operations and licensing |
| `rrcflt.sys` | KMDF minifilter (ActivityMonitor class) | **Registry Watcher:** `CmRegisterCallback`, `PsSetCreateProcessNotifyRoutine`, sends events to user mode through `\RRCFltPort` |
| `Langs/*.xaml` | 28 languages | Localization |
| Settings | `HKCU\Software\VS Revo Group\Revo Registry Cleaner\General` | |
| Data | `%LOCALAPPDATA%\VS Revo Group\Revo Registry Cleaner\` (Backups, `Backups\Last\Restore.dat`, logs) | |
| Registered file types | `.rrcaf` (activation file) | |

## 1. Registry Cleaner

### Scan modes
- **Safe:** the fastest scan. Covers only the most common, low-risk locations.
- **Moderate:** Safe plus an extended scan.
- **Advanced:** Moderate plus a deep, exhaustive scan.
- **Managed (Custom):** the user picks individual categories.
- **Quick Clean:** one click scans, backs up and cleans with no prompts. The scan mode it uses is set in Options.
- **Forced Scan:** search a chosen registry location for a word or phrase.
- **Scope:** current user (default) or **all Windows accounts** (each user's hive under `HKU\<SID>`, found through `ProfileList`).

### Categories (Managed mode)
| # | Category | Registry locations (from the binaries) | Rule |
|---|---|---|---|
| 1 | Missing Shared DLLs | `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs` (+WOW6432Node) | value name is a path that does not exist |
| 2 | Unused Shared DLLs | same | reference count = 0 |
| 3 | Invalid MSI paths | `Installer\Folders`, `Installer\UserData\S-1-5-18\Products`, `Classes\Installer\Products`/`Patches` | file or folder missing |
| 4 | COM/ActiveX invalid | `Classes\CLSID\*\{InprocServer32, LocalServer32, InprocHandler32, DefaultIcon}`, `TypeLib`, `Interface`, `AppID` (+WOW6432Node) | server binary missing, or reference to a CLSID/TypeLib that doesn't exist |
| 5 | MRU / history lists | `ComDlg32\{OpenSaveMRU, OpenSavePidlMRU, LastVisitedMRU, LastVisitedPidlMRU, FirstFolder}`, `RecentDocs`, `RunMRU`, `TypedPaths`, `WordWheelQuery`, `UserAssist`, `Applets\Wordpad\Recent File List`, `Applets\Paint\Recent File List`, `Applets\Regedit` | history entries |
| 6 | Missing startup items | `Run`, `RunOnce`, `RunOnceEx`, `RunServices`, `RunServicesOnce`, `Policies\Explorer\Run`, `Explorer\StartupApproved\Run` (HKLM, HKCU, WOW6432Node) | target executable missing |
| 7 | Invalid cache entries | `Classes\Local Settings\...\Shell\MuiCache`, `AppCompatFlags\Compatibility Assistant\{Store, Persisted}`, `AppCompatFlags\Layers`, `Explorer\FileExts` | file missing |
| 8 | Unavailable services | `SYSTEM\CurrentControlSet\Services\*\ImagePath` | binary missing |
| 9 | Incorrect uninstaller data | `CurrentVersion\Uninstall\*` (+WOW6432Node) | `UninstallString`, `InstallLocation` or `DisplayIcon` missing; empty entry |
| 10 | Unused file extensions | `Classes\.ext` | no ProgID, or the ProgID doesn't exist, and no OpenWithList |
| 11 | Invalid firewall rules | `Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules` | `App=` path missing |
| 12 | Empty keys | `SOFTWARE\*` | software keys with no values and no subkeys |
| 13 | Wrong application paths | `CurrentVersion\App Paths`, `Classes\Applications` | exe missing |
| 14 | Invalid Class keys | `Classes\<ProgID>\DefaultIcon`, `shell\*\command`, `MIME\Database\Content Type` | target missing |
| 15 | Corrupt context menu commands | `*\shell\*\command`, `shellex\ContextMenuHandlers`, `Shell Extensions\Approved`/`Blocked`, Browser Helper Objects, IE Toolbar/Explorer Bars/Extensions | command or handler missing |
| 16 | Invalid system settings | `Fonts`, sound schemes (`AppEvents`), themes, `Explorer\Shell Icons` | file missing |
| 17 | Include list | user-defined keys (with an optional subkey flag) | delete all contents |

### Safety filters
- **Exclude paths on external drives:** removable or unmounted drives are never reported.
- **Exclude Windows defaults:** a built-in list of about 315 known false positives, each a key, or a key plus a value.
- **Exclude list:** user-defined keys, each with an *Include subkeys* option and an on/off checkbox.
- **Force Delete:** takes ownership of a key and resets its ACL when deletion is denied.
- Keys the user can't back up are shown greyed out.

### Results view
- Tree grouped by category, with a count per category. A value is shown as `Key | Value`.
- Select All/None, Select This, Unselect This, Collapse All, Expand All, Expand to Data.
- Open in Registry Editor: sets `Applets\Regedit\LastKey` and launches `regedit`.
- Add to exclude list, Add to include list, Copy.
- Export to TXT, HTML, CSV, XML or XLS(X).
- The **Clean (N)** button shows how many items are selected.

### Before cleaning
- Optionally create a System Restore Point.
- Optionally create a full registry backup (a WinRE-restorable `Restore.dat`, which is a batch script).
- Always back up the deleted items to a partial backup.

### Auto Clean (scheduled)
- Each task has a name, a note, a schedule (daily at a time, weekly on a day, or monthly on a date), a scan mode and a user scope.
- Duplicate tasks are rejected ("A task with the same parameters already exists!").
- A tray notification appears when a task finishes, and can be turned off.

## 2. Registry Tuner (about 55 tweaks, reversible)
Each tweak is an on/off toggle or a dropdown, tagged with the Windows versions it supports and what it needs to take effect: **Explorer restart**, **log out** or **PC restart**. A changed tweak is labelled "by Revo Registry Cleaner" and can be reset to its original value. There's a search box, and a matching action button (Restart Explorer, Log Out or Restart PC) appears when one is needed.

- **Windows Appearance:** faster Aero Snap thumbnails, faster Aero Peek, faster menus (`MenuShowDelay`), Windows 10-style context menu on Windows 11 (`{86ca1aa0-...}\InprocServer32`), disable Windows 11 ads (`ContentDeliveryManager`), disable blur on the sign-in screen, classic battery indicator, battery time estimate, disable Widgets (`SOFTWARE\Policies\Microsoft\Dsh`), remove the shortcut arrow (`Shell Icons`), disable Aero Peek, classic volume control, disable Action Center quick actions, show the Defender tray icon.
- **File Explorer:** hide the navigation pane, disable the "new apps can open this type" prompt, add "Command Prompt as Administrator" to the context menu, check "Do this for all current items" by default, AutoComplete, open Explorer to This PC/OneDrive/Downloads/Quick Access, disable pop-up descriptions, show drive letters first, show full details when deleting, Rotate Left/Right in the context menu, classic ribbon, disable tabs.
- **Desktop:** faster mouse hover, larger icon cache, disable Bing in taskbar search, "End Task" in the taskbar menu (`TaskbarDeveloperSettings`), taskbar alignment and position (`StuckRects3`), seconds in the taskbar clock, Windows version on the desktop.
- **Security and Stability:** disable AutoPlay, auto-close hung apps, disable download warnings (`Policies\Attachments`), add "Take Ownership" to the context menu, disable SmartScreen, detailed BSOD (`CrashControl`), network drives visible to elevated apps (`EnableLinkedConnections`), disable UAC, Defender PUA protection (`MpEngine`), USB write protection (`StorageDevicePolicies`), camera on/off OSD (`OEM\Device\Capture`), disable AutoEndTasks, crash dumps on program crash (`WER\LocalDumps`), allow Windows 11 on unsupported hardware (`Setup\MoSetup`).
- **Windows Usage:** enable Windows Photo Viewer, disable background apps, disable the media property handler, add classic User Accounts to Control Panel (`ControlPanel\NameSpace\{EDEEBE61-...}`), drop the "- Shortcut" suffix, disable the "This app is preventing shutdown" screen.
- **System:** legacy F8 boot menu (bcdedit), classic Alt+Tab, disable driver updates through Windows Update, Recycle Bin on removable drives, disable Store auto-updates, disable power throttling, disable telemetry (`DataCollection`), stop Edge running in the background, clear the page file at shutdown (`Memory Management`).

## 3. Registry Watcher
- Registry tree browser on the left; selected keys move to a "watched" list. Each watched key can be started, paused, resumed or deleted.
- The kernel minifilter records every operation on a watched key: **Time, Process Name, PID, Operation** (Created/Deleted/Renamed Key, Value Set, Deleted Value, Query Key, Query Value, Query Multiple Value, Enumerate Key, Enumerate Value, Set Information Key), **Key, Value, New Data, Old Data, Data Type** (String, Expandable String, Multi-String, Binary, DWord, QWord).
- A live results pane with a filter and a highlight field.
- Export one log or all logs to TXT, CSV or XLSX. Clear a log. Open the key in regedit.
- A maximum size for the log folder; when it's reached, all watching pauses automatically.

## 4. Backup Center
- A list of every backup, sorted by date, showing Name, Note, Type (Full / Partial / Auto Full / Auto Partial / Full before cleaning / Deleted items), Date and Size.
- **Create Backup:** name, note, and either *Backup Whole Registry* or a single selected key.
- **Auto Backup:** scheduled the same way as Auto Clean, with a notification.
- **Restore** a whole backup, or **Custom Restore** to pick individual entries. Delete one backup or Delete All. Archive All.
- Settings: backup folder; auto-delete backups when older than N days and/or when the total exceeds N MB, optionally only Auto backups.
- **Offline restore:** `RRCRestore.exe` from WinRE, or `Backups\Last\Restore.dat` renamed to `.bat`.

## 5. Command line (`RRCcmd.exe`)
```
-clean  -safe|-moderate|-advanced  [-all]
-backup -full | -key "HKEY_...\Path"
-watch  -start|-stop|-del "HKEY_...\Path"
-watch  -export "HKEY_..." -csv|-txt|-xlsx "out"   |  -watch -export -all -txt "dir"
```

## 6. Options and app behaviour
- **General:** language (28 options), check for updates on startup (once a day), background auto-update, skip warning messages, custom data folder.
- **Cleaner:** the safety filters above, Auto Clean notification, Force Delete, restore point before cleaning, full backup before cleaning, Quick Clean scan mode, Exclude list, Include list.
- **Watcher:** maximum log folder size in MB, log folder location.
- **Backup:** see section 4.
- **View:** themes Default/Light, Dark, Green, Gray, Black; reset view settings. Theme is chosen on first run.
- Single instance ("already running"), a portable version, and a "What's new" page.
- Licensing: 30-day trial, online or file-based activation, deactivation, update subscription. **Not cloned.**

---

# Rust clone design (CleanCore)

Goals: one native binary per OS (no .NET runtime, which is about 300 MB of the original), parallel scans with `rayon`, and no kernel driver unless one is truly needed.

| Original | CleanCore, Windows | CleanCore, Linux |
|---|---|---|
| Registry scan | `windows-registry`/`winreg` + rayon across hives and categories; one cached path-exists lookup per unique path | **Config store scan:** broken `.desktop` files, broken XDG autostart entries, stale `mimeapps.list` handlers, orphaned `~/.config`/`~/.local/share`/`~/.cache` folders for apps that aren't installed, dangling symlinks, broken systemd user units, empty config folders, recent-files/history lists (`recently-used.xbel`, shell history), dconf keys for schemas that don't exist, broken menu entries |
| Exclude/Include lists | same model, with glob or prefix matching | same |
| Backups | native `.reg` export (also readable by regedit) and a JSON manifest; full backup through `RegSaveKeyEx` of the hives | tar.zst snapshot of config folders and a `dconf dump` |
| Restore Point | `SRSetRestorePointW` / WMI `SystemRestore.CreateRestorePoint` | Btrfs/Snapper or Timeshift hook if present; otherwise skipped |
| Tuner | data-driven tweak table (TOML): key, value, on/off data, OS versions, required action | the same engine on `gsettings`/dconf (GNOME/KDE tweaks) |
| Watcher | user mode: `RegNotifyChangeKeyValue` per key, plus a diff snapshot to get old/new data. Getting the PID needs ETW (`Microsoft-Windows-Kernel-Registry` provider), so no driver is needed | `inotify` on config files, `dconf watch`, PID from `fanotify` where permitted |
| Scheduler/service | `schtasks` / Task Scheduler COM to run `cleancore --auto <task-id>` | a systemd user timer |
| CLI | the `RRCcmd` syntax is kept as an alias | same |
| GUI | `egui`/`eframe` with the same layout (command panel, tool panel, themes) | same |
| i18n | Fluent (`.ftl`) files | same |

## Implementation status (v0.1)

| Area | Status |
|---|---|
| Scan engine: parallel (rayon), cached path lookups, exclude list, OS-default filter, external-drive guard, de-duplication | ✅ done |
| Windows cleaner: all 16 categories plus the include list, Safe/Moderate/Advanced/Managed, current or all users, forced scan | ✅ written and type-checked for `x86_64-pc-windows-gnu`; **not yet run on real Windows** |
| Linux cleaner: launchers, autostart and systemd user units, mimeapps, recent files, broken symlinks, empty folders, leftover app data, include list, forced scan | ✅ tested |
| Backups: deleted-items backup before every clean; partial/full; list/show/restore/custom restore/delete/prune | ✅ (Linux round trip tested byte-for-byte; Windows uses `.reg` + `reg import`, hives + WinRE `Restore.bat`) |
| Restore point before cleaning (Windows `Checkpoint-Computer`, Linux snapper/timeshift) | ✅ |
| Export TXT/CSV/HTML/XML/JSON | ✅ (XLSX not done) |
| CLI, including the legacy `RRCcmd` syntax `-clean -safe -all` / `-backup -full` / `-backup -key K` | ✅ |
| Force Delete (take ownership and reset the ACL) | ⏳ |
| Loading the hives of signed-out users for "all users" (`RegLoadKey`) | ⏳ only loaded profiles are scanned |
| Registry Tuner: TOML tweak tables (`tweaks/windows.toml`, 54 tweaks; `tweaks/linux.toml`, 33 gsettings tweaks), toggles and choices, Windows build gating, exact revert (restores prior values, deletes keys it created), "by CleanCore" tracking, live state detection, Explorer/log-out/reboot hints, `cleancore tune list/show/set/revert/restart-explorer` | ✅ Linux tested against a sandboxed gsettings backend; Windows type-checked, **not yet run on real Windows** |
| Tuner items not ported: hide navigation pane (binary blob), legacy F8 boot menu (bcdedit), Recycle Bin on removable drives (per-volume), Explorer tabs (ViVeTool), media property handlers | ⏳ need non-registry actions |
| Registry Watcher: watch list (start/pause/resume/delete), per-watch JSONL logs, filter + highlight, live follow, export TXT/CSV/JSON, clear, log-folder size limit that auto-pauses, `cleancore watch …` plus legacy `-watch -start/-stop/-del/-export` | ✅ |
| Watcher, Linux: inotify with snapshot diffing (INI per key, JSON per field, other files fingerprinted; atomic saves and renames recognised) and `dconf watch` for the GNOME settings database | ✅ tested live. inotify/dconf don't report the PID, so the Process column is empty |
| Watcher, Windows: real-time ETW on Microsoft-Windows-Kernel-Registry (process name and PID, old/new data, optional reads, no kernel driver); without admin, falls back to `RegNotifyChangeKeyValue` plus snapshot diff | ✅ type-checked; **not yet run on real Windows** |
| Watcher gaps: XLSX export (CSV instead) | ⏳ |
| Scheduler: Auto Clean (mode, user scope) and Auto Backup (everything or one key/path) tasks; daily / weekly / monthly (1–28 or last day); name and note; duplicate rejection; edit, remove, run now, history, re-sync; notifications on success (can be turned off) and always on failure | ✅ |
| Scheduler, Linux: systemd user `.timer` + `.service` per task (`Persistent=true` catches up missed runs), `cleancore-recorder.service` at login, notify-send with a gdbus fallback | ✅ units validated with `systemd-analyze`; real run tested through a transient systemd timer |
| Scheduler, Windows: Task Scheduler entries under `\CleanCore\`, run elevated when possible through `conhost --headless` (no console flash), recorder at logon, toast notifications | ✅ type-checked; **not yet run on real Windows** |
| GUI (`cleancore-gui`, egui 0.37): commands panel + tool switcher + active tool, Menu (Options, Theme, data folder, About), first-run theme choice, 5 themes, status bar with message history and recorder indicator; background jobs keep the window responsive | ✅ |
| GUI tools: Cleaner (scan-mode cards, Custom/Forced scan, Quick Clean, results tree with tri-state categories, per-item menu, Clean with restore-point/full-backup choice, export, Auto Clean); Tuner (categories, search, switches/choices, warnings, "by CleanCore", reset all, restart hints); Watcher (key/folder/dconf browser, watch list, live events table with filter/highlight, in-app recorder, start at login, export); Backup Center (table, create with key browser, restore, custom restore, delete, Auto Backup); Options (general, cleaner, exclude/include lists, watcher, backups, view) | ✅ tested headless end to end (`tests/gui.rs`, screenshots in `target/gui-shots/`); Windows build type-checked only |
| GUI design: sidebar navigation with icons, pages with header actions, white cards on a soft gray background, toggle switches, toast notifications with a notification history, light / dark / system themes and six accent colors. Fonts are bundled in `assets/fonts/`: Inter (SIL OFL 1.1) and Phosphor icons (MIT), both subset so they don't overlap (Inter without the private-use range, Phosphor with only it) | ✅ |
| GUI gaps: translations (English only); characters missing from egui's bundled font (e.g. CJK key names) draw as boxes; native file pickers (paths are typed) | ⏳ |
