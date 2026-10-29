// Windows: a windowed program, so opening CleanCore shows no console window behind it. The
// command line still prints to the terminal it was started from (see `attach_console`).
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use cleancore::backup::{self, Backup, BackupKind};
use cleancore::config::{ListEntry, Settings};
use cleancore::engine::{self, Selection};
use cleancore::model::{Issue, ScanMode, Scope};
use cleancore::scheduler::{self, Action, Task};
use cleancore::tuner::{self, Requires};
use cleancore::uninstaller::{self as uninst, Operation, Program, RunMode, Target};
use cleancore::watcher::{self, Watch, WatchEvent};
use cleancore::{cleaner, export, platform};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "cleancore", version, about = "Fast registry / configuration cleaner with backups")]
struct Cli {
    /// Use this folder for settings, backups, logs and schedules (default: per-user data folder)
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan and list problems without changing anything
    Scan(ScanArgs),
    /// Scan, back up and remove the problems found
    Clean {
        #[command(flatten)]
        scan: ScanArgs,
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// One-click: scan with the Quick Clean mode from settings, back up and clean
    Quick {
        #[arg(long)]
        all_users: bool,
    },
    /// Forced scan: find every key/value (or file) under ROOT that matches TEXT
    Forced {
        root: String,
        text: String,
        #[arg(long)]
        export: Option<PathBuf>,
        /// Remove the matches (after a backup)
        #[arg(long)]
        clean: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// List scan categories (for --categories)
    Categories,
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Manage the exclude list (never reported or deleted)
    #[command(subcommand)]
    Exclude(ListCmd),
    /// Manage the include list (contents always removed in a managed scan with `include_list`)
    #[command(subcommand)]
    Include(ListCmd),
    /// Show settings, or change one: `settings set restore_point_before_clean true`
    Settings {
        #[command(subcommand)]
        action: Option<SettingsCmd>,
    },
    /// Open a key in the Registry Editor (Windows) or a path in the file manager (Linux)
    Open { location: String },
    /// Registry Tuner (Windows) / settings tuner (Linux): reversible system tweaks
    #[command(subcommand)]
    Tune(TuneCmd),
    /// Registry Watcher: record changes under registry keys (Windows) or config paths / dconf (Linux)
    #[command(subcommand)]
    Watch(WatchCmd),
    /// Scheduled Auto Clean / Auto Backup tasks, and starting the watch recorder at login
    #[command(subcommand)]
    Schedule(ScheduleCmd),
    /// List installed programs
    Programs {
        /// Wildcard pattern for the name (`*` and `?`)
        #[arg(short, long)]
        r#match: Option<String>,
        /// Show the install location
        #[arg(short = 'i', long)]
        location: bool,
        /// Show the uninstall command
        #[arg(short = 'u', long)]
        uninstall_cmd: bool,
        /// Include system components, updates and libraries
        #[arg(long)]
        system: bool,
        /// Only store apps (Windows Apps / Flatpak / Snap)
        #[arg(long)]
        store: bool,
        /// One line per program, in columns (Windows: with 64-bit / 32-bit)
        #[arg(short, long)]
        table: bool,
        #[arg(long)]
        json: bool,
        /// Write the list to a file (.txt .csv .xlsx .html .json)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Uninstall programs and remove their leftovers
    Uninstall(UninstallArgs),
    /// Installation monitor and traced programs
    #[command(subcommand)]
    Trace(TraceCmd),
    /// Browser extensions (Chrome, Edge, Brave, Opera, Vivaldi, Chromium, Firefox)
    #[command(subcommand)]
    Extensions(ExtCmd),
    /// Programs, services and tasks that start with the system
    #[command(subcommand)]
    Autorun(AutorunCmd),
    /// Hunter Mode: identify a running program and act on it
    Hunter {
        /// List programs with a window
        #[arg(long)]
        list: bool,
        /// With --list: every process of the desktop session, not just applications
        #[arg(long)]
        all: bool,
        /// Open Hunter Mode in the GUI (falls back to --list without the GUI)
        #[arg(long)]
        gui: bool,
        /// Target process id
        #[arg(long)]
        pid: Option<u32>,
        /// Use the window under the mouse pointer (Windows; Linux on X11)
        #[arg(long)]
        at_pointer: bool,
        /// info | uninstall | stop-autorun | kill | kill-delete | folder | search
        #[arg(long, default_value = "info")]
        action: String,
        #[arg(short, long)]
        yes: bool,
    },
    /// Logs database: shared trace logs
    #[command(subcommand)]
    Logsdb(LogsDbCmd),
    /// Junk Files Cleaner: temporary files, editor backups, crash dumps
    #[command(subcommand)]
    Junk(JunkCmd),
    /// Duplicate Finder: files with the same content, and deleting the extra copies
    #[command(subcommand)]
    Dupes(DupesCmd),
    /// History Cleaner: browser history and caches, recent files, temp files, Office MRU
    #[command(subcommand)]
    History(HistoryCmd),
    /// Unrecoverable Delete: overwrite files / folders, then delete them
    Shred {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Overwrite passes (default from settings, 3)
        #[arg(long)]
        passes: Option<u32>,
        #[arg(short, long)]
        yes: bool,
    },
    /// Evidence Remover: overwrite the free space of a drive so deleted files can't be recovered
    WipeFree {
        /// A folder on the drive to wipe (list drives without it)
        drive: Option<PathBuf>,
        #[arg(long)]
        passes: Option<u32>,
        /// Write at most this many MB per pass
        #[arg(long)]
        max_mb: Option<u64>,
        #[arg(short, long)]
        yes: bool,
    },
    /// System Tools: open the operating system's utilities
    #[command(subcommand)]
    Tools(ToolsCmd),
    /// Internal: the administrator helper, started through pkexec (see `elevate`)
    #[command(hide = true)]
    RootHelper,
    /// Update CleanCore to the latest release (signature and checksum verified first)
    Update {
        /// Only say whether a newer version exists
        #[arg(long)]
        check: bool,
    },
    /// Microsoft slop cleaner (Windows): preinstalled apps, Copilot and other AI, ads, telemetry
    #[command(subcommand)]
    Slop(SlopCmd),
    /// Program groups
    #[command(subcommand)]
    Groups(GroupsCmd),
    /// Store apps (Windows Apps / Flatpak / Snap): run, terminate, reset
    #[command(subcommand)]
    App(AppCmd),
    /// "Install with CleanCore" / "Uninstall with CleanCore" in the file manager's right-click menu
    ShellIntegration {
        /// on | off | status
        #[arg(default_value = "status")]
        state: String,
    },
    /// Programs uninstalled with CleanCore
    UninstallHistory {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        /// Show the details of entry N (1 = newest)
        #[arg(long)]
        show: Option<usize>,
    },
}

#[derive(Subcommand)]
enum ExtCmd {
    List {
        /// Only this browser (chrome, chromium, edge, brave, opera, vivaldi, firefox)
        #[arg(long)]
        browser: Option<String>,
        /// Include extensions that ship with the browser
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Remove extensions by name (several separated by `;`); the browser must be closed
    Remove {
        names: String,
        #[arg(long)]
        browser: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AutorunCmd {
    List {
        /// Include entries that belong to the operating system
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
        /// Write the list to a file (.txt .csv .xlsx .html .json)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    Enable {
        entry: String,
    },
    Disable {
        entry: String,
    },
    /// Start the entry's program now
    Run {
        entry: String,
    },
    /// Stop the entry's running program
    Kill {
        entry: String,
    },
    /// Delete an entry (backed up); system entries are disabled instead
    Remove {
        entry: String,
        #[arg(short, long)]
        yes: bool,
    },
    /// Add a program that starts at login
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        all_users: bool,
        /// run | runonce | startup
        #[arg(long, default_value = "run")]
        method: String,
    },
    /// Remove entries whose program no longer exists
    RemoveInvalid {
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove all disabled entries
    RemoveDisabled {
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Args)]
struct JunkArgs {
    /// Folder or drive to scan (repeatable; default from settings: home / all fixed drives)
    #[arg(long = "location")]
    locations: Vec<String>,
    /// Name pattern (repeatable, e.g. "*.tmp"); replaces the patterns from settings
    #[arg(long = "pattern")]
    patterns: Vec<String>,
    /// Also files changed in the last 24 hours
    #[arg(long)]
    include_recent: bool,
}

#[derive(Args)]
struct DupesArgs {
    /// Folder or drive to scan (repeatable; default from settings: the home folder)
    #[arg(long = "location")]
    locations: Vec<String>,
    /// Ignore files smaller than this many KB (default from settings: 64)
    #[arg(long)]
    min_kb: Option<u64>,
    /// Also hidden files and folders
    #[arg(long)]
    include_hidden: bool,
}

#[derive(Subcommand)]
enum DupesCmd {
    Scan {
        #[command(flatten)]
        a: DupesArgs,
        #[arg(long)]
        json: bool,
        /// Write the groups to a file (.txt or .csv)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Delete every copy but one in each group
    Clean {
        #[command(flatten)]
        a: DupesArgs,
        /// Which copy stays: oldest | newest | shortest
        #[arg(long, default_value = "oldest")]
        keep: String,
        /// Delete permanently instead of to the Recycle Bin / Trash
        #[arg(long)]
        permanent: bool,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum JunkCmd {
    Scan {
        #[command(flatten)]
        a: JunkArgs,
        #[arg(long)]
        json: bool,
        /// Write the list to a file (.txt or .csv)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    Clean {
        #[command(flatten)]
        a: JunkArgs,
        /// Delete permanently instead of to the Recycle Bin / Trash
        #[arg(long)]
        permanent: bool,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum HistoryCmd {
    /// List what can be cleaned, with sizes (* = selected by default)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Clean the default selection, or the given item ids
    Clean {
        /// Item ids separated by commas (see `history list`); `all` for everything
        #[arg(long)]
        only: Option<String>,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AppCmd {
    Run {
        name: String,
    },
    Terminate {
        name: String,
    },
    /// Windows: reset the app's data (Reset-AppxPackage)
    Reset {
        name: String,
    },
    /// Windows: reset the Microsoft Store (wsreset)
    ResetStore,
}

#[derive(Subcommand)]
enum GroupsCmd {
    /// Groups and their programs
    List,
    Create {
        name: String,
    },
    Rename {
        old: String,
        new: String,
    },
    /// Delete a group (its programs stay installed, just ungrouped)
    Delete {
        name: String,
    },
    /// Put a program in a group
    Add {
        program: String,
        group: String,
    },
    /// Take a program out of its group
    Remove {
        program: String,
    },
}

#[derive(Subcommand)]
enum SlopCmd {
    /// Preinstalled and promoted apps found on this PC (* = recommended to remove)
    Scan,
    /// Remove apps by name (several separated by `;`), or all recommended ones
    Remove {
        names: Option<String>,
        #[arg(long)]
        recommended: bool,
        /// For every user account (administrator)
        #[arg(long)]
        all_users: bool,
        /// Also remove the preinstalled copy so it doesn't come back (administrator)
        #[arg(long)]
        deprovision: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// The AI, ads and telemetry settings and their state
    Settings,
    /// Turn settings off (`all` for every one); reversible with `revert`
    Apply { ids: Vec<String> },
    /// Restore settings changed with `apply` (`all` for every one)
    Revert { ids: Vec<String> },
}

#[derive(Subcommand)]
enum ToolsCmd {
    List,
    Run {
        name: String,
    },
    /// Add your own tool
    Add {
        name: String,
        command: String,
    },
    Remove {
        name: String,
    },
}

#[derive(Subcommand)]
enum LogsDbCmd {
    /// List logs in the database folder
    List {
        /// Also logs recorded on other operating systems / architectures
        #[arg(long)]
        all: bool,
    },
    /// Download new logs from the database URL
    Update,
    /// Share a local trace log through the database folder
    Add { log: String },
    /// Uninstall a program with a database log (imported into Traced Programs first)
    Uninstall {
        name: String,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum TraceCmd {
    /// Put a log in a group, or take it out (no GROUP)
    Group {
        log: String,
        group: Option<String>,
    },
    /// Trace an installation in one go: snapshot, run the installer, snapshot again, save the log
    Start {
        /// Installer to run (setup.exe, .msi, .deb, install.sh, any command)
        #[arg(long)]
        run: Option<String>,
        /// Log name (default: guessed from what was installed)
        #[arg(long)]
        name: Option<String>,
        /// Don't wait for Enter after the installer exits
        #[arg(long)]
        no_wait: bool,
    },
    /// Two-step tracing, part 1: record the current state
    Begin,
    /// Two-step tracing, part 2: compare with the state saved by `begin` and save the log
    End {
        #[arg(long)]
        name: Option<String>,
    },
    /// List trace logs
    List,
    /// Show what a log recorded
    Show {
        log: String,
        /// Include items that were only modified (never removed)
        #[arg(long)]
        all: bool,
    },
    /// Uninstall a traced program: undo everything its installer created
    Uninstall {
        log: String,
        /// Custom Uninstall: only items whose path contains one of these texts
        #[arg(long)]
        only: Vec<String>,
        /// Advanced Uninstall: also scan for related leftovers the log missed
        #[arg(long)]
        advanced: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Export a log (.cptrace to share, .txt or .html to read)
    Export {
        log: String,
        file: PathBuf,
    },
    /// Import a .cptrace log
    Import {
        file: PathBuf,
    },
    /// Merge logs into the oldest one
    Merge {
        #[arg(required = true, num_args = 2..)]
        logs: Vec<String>,
        #[arg(long)]
        name: String,
    },
    Rename {
        log: String,
        name: String,
    },
    /// Set a log's comment
    Comment {
        log: String,
        text: String,
    },
    Delete {
        log: String,
    },
    /// Save every log into one file
    Backup {
        file: PathBuf,
    },
    /// Restore logs from a backup file
    Restore {
        file: PathBuf,
    },
}

#[derive(Args)]
struct UninstallArgs {
    /// Program name(s) as listed by `cleancore programs`; several = Multiple Uninstall
    names: Vec<String>,
    /// Program folder or main executable (Forced Uninstall; repeatable)
    #[arg(long)]
    path: Vec<String>,
    /// Leftover scan depth
    #[arg(short, long)]
    mode: Option<ScanMode>,
    /// Silent uninstaller, automatic Moderate scan and leftover removal
    #[arg(long)]
    quick: bool,
    /// Don't run the built-in uninstaller; only remove leftovers
    #[arg(long)]
    leftovers_only: bool,
    /// Forced Uninstall: the program isn't listed; find leftovers by name and --path
    #[arg(long)]
    forced: bool,
    /// Use `msiexec /x {ProductCode}` instead of the registered command
    #[arg(long)]
    msi: bool,
    /// Store apps by name, separated by `;`
    #[arg(long)]
    app: Option<String>,
    /// Look for leftovers in every user account
    #[arg(long)]
    all_users: bool,
    /// Don't ask before deleting leftovers
    #[arg(short, long)]
    yes: bool,
    /// Continue an uninstall that was interrupted (e.g. by a restart): scan for its leftovers
    #[arg(long)]
    resume: bool,
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// Add an Auto Clean task: scan, back up and clean on a schedule
    AddClean {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        note: String,
        /// `daily HH:MM`, `weekly <mon..sun> HH:MM`, `monthly <1-28|last> HH:MM`
        #[arg(long)]
        when: String,
        #[arg(short, long, default_value = "safe")]
        mode: ScanMode,
        #[arg(long)]
        all_users: bool,
    },
    /// Add an Auto Backup task: whole registry/config store, or one key/path with --key
    AddBackup {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        note: String,
        /// `daily HH:MM`, `weekly <mon..sun> HH:MM`, `monthly <1-28|last> HH:MM`
        #[arg(long)]
        when: String,
        #[arg(long)]
        key: Option<String>,
    },
    /// Change a task (only the options you pass)
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        when: Option<String>,
        /// Auto Clean: scan mode
        #[arg(short, long)]
        mode: Option<ScanMode>,
        /// Auto Clean: true = all users, false = current user
        #[arg(long)]
        all_users: Option<bool>,
        /// Auto Backup: back up this key/path instead
        #[arg(long, conflicts_with = "whole")]
        key: Option<String>,
        /// Auto Backup: back up everything
        #[arg(long)]
        whole: bool,
    },
    /// Delete a task and its OS schedule
    Remove { id: String },
    /// List tasks with their next and last run
    List,
    /// Run a task now, in this terminal
    Run { id: String },
    /// Run a task (used by the OS scheduler)
    #[command(hide = true)]
    Exec { id: String },
    /// Results of past runs
    History {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Re-register every task with the OS (after moving cleancore or the data folder)
    Sync,
    /// Start the watch recorder automatically at login: enable | disable | status
    Recorder { action: String },
}

#[derive(Subcommand)]
enum WatchCmd {
    /// Add a key/path to the watch list, or resume it (Linux: a file, folder, or dconf:/path/)
    Start {
        path: String,
        /// Also record reads (query/enumerate); Windows ETW only, very noisy
        #[arg(long)]
        reads: bool,
    },
    /// Pause watching a key/path (its log is kept)
    Stop { path: String },
    /// Remove a key/path from the watch list and delete its log
    Del {
        path: String,
        /// Export the log to this file first (.txt .csv .xlsx .json)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Show the watch list
    List,
    /// Show recorded events (all watches, or one)
    Log {
        path: Option<String>,
        /// Only events containing this text (any column)
        #[arg(long)]
        filter: Option<String>,
        /// Highlight events containing this text
        #[arg(long)]
        highlight: Option<String>,
        /// Show at most this many of the latest events
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        /// Keep printing new events as they are recorded
        #[arg(short, long)]
        follow: bool,
    },
    /// Export a watch log: `export KEY OUT`, or every log: `export --all OUT_FOLDER`
    Export {
        /// KEY OUT, or just OUT with --all
        #[arg(num_args = 1..=2, required = true, value_names = ["KEY", "OUT"])]
        args: Vec<String>,
        #[arg(long)]
        all: bool,
        /// txt | csv | xlsx | json (default: from OUT's extension, else csv)
        #[arg(long)]
        format: Option<String>,
    },
    /// Delete all recorded events of a watch
    Clear { path: String },
    /// Run the recorder in the foreground (needs to be running for anything to be recorded)
    Run {
        /// Don't print events, only record them
        #[arg(short, long)]
        quiet: bool,
    },
}

#[derive(Subcommand)]
enum TuneCmd {
    /// List tweaks with their current state
    List {
        #[arg(short, long)]
        category: Option<String>,
        /// Filter by text in the id, name or description
        #[arg(short, long)]
        search: Option<String>,
        /// Only tweaks changed by CleanCore
        #[arg(long)]
        changed: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show what a tweak changes
    Show { id: String },
    /// Turn a tweak on, or pick an option of a choice; `off` reverts it
    Set {
        id: String,
        /// Option id (default: the first option, `on` for toggles)
        option: Option<String>,
        #[arg(short, long)]
        yes: bool,
    },
    /// Restore the original values of a tweak, or of every tweak with --all
    Revert {
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Restart File Explorer so shell tweaks take effect (Windows)
    RestartExplorer,
}

#[derive(Args)]
struct ScanArgs {
    /// safe | moderate | advanced
    #[arg(short, long, default_value = "safe")]
    mode: ScanMode,
    /// Managed scan: comma-separated category ids (overrides --mode)
    #[arg(short, long, value_delimiter = ',')]
    categories: Option<Vec<String>>,
    /// Scan every user account instead of only the current one
    #[arg(long)]
    all_users: bool,
    /// Write the results to a file (.txt .csv .xlsx .html .xml .json)
    #[arg(long)]
    export: Option<PathBuf>,
    /// Print results as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum BackupCmd {
    /// Create a backup of the whole registry/config store or of one key/path
    Create {
        #[arg(long, conflicts_with = "key")]
        full: bool,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value = "Manual backup")]
        name: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// List backups, newest first
    List,
    /// Show the entries of a backup
    Show { id: String },
    /// Restore a backup; --only restores just entries whose label contains the text (Custom Restore)
    Restore {
        id: String,
        #[arg(long)]
        only: Vec<String>,
    },
    /// Delete one backup, all with --all, or those made in a time range with --from / --to
    Delete {
        id: Option<String>,
        #[arg(long)]
        all: bool,
        /// Start of the range: 2026-10-01 or "2026-10-01 14:30"
        #[arg(long)]
        from: Option<String>,
        /// End of the range (a bare date includes that whole day; default: now)
        #[arg(long)]
        to: Option<String>,
    },
    /// Make today's daily full backup now, if the option is on and it doesn't exist yet
    Daily,
    /// Apply the auto-delete rules from settings
    Prune,
}

#[derive(Subcommand)]
enum ListCmd {
    Add {
        path: String,
        /// Don't apply to sub-keys / sub-folders
        #[arg(long)]
        no_subkeys: bool,
    },
    Remove {
        path: String,
    },
    List,
}

#[derive(Subcommand)]
enum SettingsCmd {
    Set { key: String, value: String },
    Reset,
}

fn main() {
    // The administrator helper runs as root: it only serves requests, nothing else.
    #[cfg(not(windows))]
    if std::env::args().nth(1).as_deref() == Some("root-helper") {
        if let Err(e) = cleancore::elevate::serve() {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(feature = "gui")]
    if cleancore::launch::opens_window(&std::env::args_os().skip(1).collect::<Vec<_>>()) && has_display() {
        run_window();
        return;
    }
    #[cfg(all(windows, feature = "gui"))]
    attach_console();
    let args = translate_legacy(std::env::args().collect());
    let cli = Cli::parse_from(args);
    if let Some(d) = &cli.data_dir {
        // Everything below (and child processes) reads the data folder from here.
        std::env::set_var("CLEANCORE_DATA_DIR", std::path::absolute(d).unwrap_or_else(|_| d.clone()));
    }
    cleancore::migrate::from_old_name();
    cleancore::update::cleanup_after_update();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// The window: `cleancore` with no command (or with one of the window's options).
#[cfg(feature = "gui")]
fn run_window() {
    cleancore::launch::set_window();
    // A windowed app has no console for panic messages: also keep them in the data folder.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = cleancore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let text = format!("[{}] {info}\n{}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), std::backtrace::Backtrace::force_capture());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("gui-crash.log")).and_then(|mut f| f.write_all(text.as_bytes()));
        default(info);
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = match args.first().map(String::as_str) {
        Some("--data-dir") => args.get(1).cloned(),
        Some(a) => a.strip_prefix("--data-dir=").map(str::to_string),
        None => None,
    };
    if let Some(d) = dir {
        let d = PathBuf::from(d);
        std::env::set_var("CLEANCORE_DATA_DIR", std::path::absolute(&d).unwrap_or(d));
    }
    // Windows: ask for administrator rights once, here, so the uninstallers started later
    // don't each ask. Declined: carry on without them.
    #[cfg(windows)]
    if cleancore::elevate::relaunch_elevated() {
        return;
    }
    cleancore::migrate::from_old_name();
    cleancore::migrate::from_gui_program();
    cleancore::update::cleanup_after_update();
    if let Err(e) = cleancore::gui::run() {
        // A windowed program has no console: also say it in the log and, on Windows, a message.
        let text = format!("CleanCore could not open its window: {e}");
        eprintln!("error: {text}");
        let dir = cleancore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let line = format!("[{}] {text}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("gui-crash.log")).and_then(|mut f| f.write_all(line.as_bytes()));
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
            // SAFETY: NUL-terminated wide strings that outlive the call.
            unsafe { MessageBoxW(std::ptr::null_mut(), wide(&text).as_ptr(), wide("CleanCore").as_ptr(), MB_OK | MB_ICONERROR) };
        }
        std::process::exit(1);
    }
}

/// Linux without a desktop (a server, an SSH session): `cleancore` alone shows the command
/// line's help instead of failing to open a window.
#[cfg(feature = "gui")]
fn has_display() -> bool {
    cfg!(not(target_os = "linux")) || ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

/// Windows: print to the terminal this was started from. Started through `cleancore.com`, or
/// with its output redirected, the handles it was given are kept; started as `cleancore.exe`
/// from a terminal, they are opened on that terminal's console.
#[cfg(all(windows, feature = "gui"))]
fn attach_console() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    // SAFETY: plain Win32 calls with NUL-terminated wide strings that outlive them.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for (std, name) in [(STD_INPUT_HANDLE, "CONIN$"), (STD_OUTPUT_HANDLE, "CONOUT$"), (STD_ERROR_HANDLE, "CONOUT$")] {
            let h = GetStdHandle(std);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                continue;
            }
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let h = CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut());
            if h != INVALID_HANDLE_VALUE {
                SetStdHandle(std, h);
            }
        }
    }
}

/// `programs --table`: name, version, 32/64-bit (Windows) and kind in columns, then the
/// install location and uninstall command when asked for. Long names are cut short.
fn print_programs_table(list: &[Program], location: bool, uninstall_cmd: bool) {
    const NAME: usize = 48;
    let cut = |s: &str, w: usize| if s.chars().count() > w { format!("{}…", s.chars().take(w - 1).collect::<String>()) } else { s.to_string() };
    let bits = |p: &Program| match p.bits() {
        Some(b) => format!("{b}-bit"),
        None if p.user_scope => "user".into(),
        None => String::new(),
    };
    let name_w = list.iter().map(|p| p.name.chars().count().min(NAME)).max().unwrap_or(0).max(4);
    let ver_w = list.iter().map(|p| cut(&p.version, 20).chars().count()).max().unwrap_or(0).max(7);
    let bits_w = list.iter().map(|p| bits(p).len()).max().unwrap_or(0);
    let row = |name: &str, ver: &str, b: &str, kind: &str, loc: &str, cmd: &str| {
        let mut line = format!("{name:<name_w$}  {ver:<ver_w$}  ");
        if bits_w > 0 {
            line += &format!("{b:<bits_w$}  ");
        }
        line += kind;
        if location {
            line += &format!("  {loc}");
        }
        if uninstall_cmd {
            line += &format!("  {cmd}");
        }
        println!("{}", line.trim_end());
    };
    row("Name", "Version", if bits_w > 0 { "Bits" } else { "" }, "Kind", if location { "Location" } else { "" }, if uninstall_cmd { "Uninstall" } else { "" });
    for p in list {
        row(&cut(&p.name, NAME), &cut(&p.version, 20), &bits(p), p.kind.label(), p.install_location.as_deref().unwrap_or(""), p.uninstall_string.as_deref().unwrap_or(""));
    }
}

/// Accept the classic slash-and-dash command syntax: `-clean -safe -all`, `-backup -full`, `-backup -key "HK..."`.
fn translate_legacy(args: Vec<String>) -> Vec<String> {
    let Some(first) = args.get(1) else {
        return args;
    };
    let rest: Vec<String> = args[2..].iter().map(|a| a.to_ascii_lowercase()).collect();
    let mut out = vec![args[0].clone()];
    match first.to_ascii_lowercase().as_str() {
        "-clean" => {
            out.extend(["clean".into(), "--yes".into()]);
            for m in ["-safe", "-moderate", "-advanced"] {
                if rest.iter().any(|a| a == m) {
                    out.extend(["--mode".into(), m[1..].into()]);
                }
            }
            if rest.iter().any(|a| a == "-all") {
                out.push("--all-users".into());
            }
        }
        "-backup" => {
            out.extend(["backup".into(), "create".into()]);
            if rest.first().is_some_and(|a| a == "-full") {
                out.push("--full".into());
            } else if rest.first().is_some_and(|a| a == "-key") {
                out.push("--key".into());
                out.extend(args.get(3).cloned());
            }
        }
        // Listing programs: [/m "name*"] [/i] [/u] [/t] [/p] [/winapps], in any
        // order; without /m it lists every program. /p (pause on each page) isn't needed: the
        // terminal scrolls.
        "/m" | "/i" | "/u" | "/t" | "/p" | "/winapps" => {
            out.push("programs".into());
            let mut i = 1;
            while i < args.len() {
                match args[i].to_ascii_lowercase().as_str() {
                    "/m" => {
                        out.push("--match".into());
                        out.extend(args.get(i + 1).cloned());
                        i += 1;
                    }
                    "/i" => out.push("--location".into()),
                    "/u" => out.push("--uninstall-cmd".into()),
                    "/t" => out.push("--table".into()),
                    "/winapps" => out.push("--store".into()),
                    _ => {}
                }
                i += 1;
            }
        }
        "/?" | "/help" => out.push("--help".into()),
        // /mu "name" /path "dir" /mode Moderate
        "/mu" => {
            out.extend(["uninstall".into(), "--leftovers-only".into(), "--yes".into()]);
            out.extend(args.get(2).cloned());
            let mut i = 3;
            while i < args.len() {
                match args[i].to_ascii_lowercase().as_str() {
                    "/path" => {
                        out.extend(["--path".into()]);
                        out.extend(args.get(i + 1).cloned());
                        i += 1;
                    }
                    "/mode" => {
                        out.extend(["--mode".into()]);
                        out.extend(args.get(i + 1).map(|m| m.to_ascii_lowercase()));
                        i += 1;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        // /export "file" /all /txt
        "/export" => {
            out.push("programs".into());
            if let Some(f) = args.get(2) {
                let ext = rest.iter().find_map(|a| match a.as_str() {
                    "/html" => Some("html"),
                    "/xls" => Some("csv"),
                    "/txt" => Some("txt"),
                    _ => None,
                });
                let mut path = PathBuf::from(f);
                if let Some(e) = ext {
                    if path.extension().is_none() {
                        path.set_extension(e);
                    }
                }
                out.extend(["--export".into(), path.display().to_string()]);
            }
        }
        // /wa "App1;App2"
        "/wa" => {
            out.extend(["uninstall".into(), "--yes".into(), "--app".into()]);
            out.extend(args.get(2).cloned());
        }
        // /tu "Log name"
        "/tu" => {
            out.extend(["trace".into(), "uninstall".into(), "--yes".into()]);
            out.extend(args.get(2).cloned());
        }
        // /be "Ext1;Ext2" /chrome "ProfileName"
        "/be" => {
            out.extend(["extensions".into(), "remove".into(), "--yes".into()]);
            out.extend(args.get(2).cloned());
            let mut i = 3;
            while i < args.len() {
                let a = args[i].to_ascii_lowercase();
                if let Some(b) = cleancore::extensions::Browser::from_flag(&a) {
                    out.extend(["--browser".into(), b.flag().into()]);
                    if let Some(p) = args.get(i + 1).filter(|p| !p.starts_with('/')) {
                        out.extend(["--profile".into(), p.clone()]);
                        i += 1;
                    }
                }
                i += 1;
            }
        }
        // /hunter
        "/hunter" => out.extend(["hunter".into(), "--gui".into()]),
        "-watch" => {
            // -watch [-start|-stop|-del] "KEY"   |   -watch -export ("KEY" | -all) -csv|-txt|-xlsx "OUT"
            let orig = &args[2..];
            out.push("watch".into());
            let action = rest.first().map(String::as_str).unwrap_or("");
            match action {
                "-stop" | "-del" | "-start" => {
                    out.push(action[1..].into());
                    out.extend(orig.get(1).cloned());
                }
                "-export" => {
                    out.push("export".into());
                    let mut i = 1;
                    if rest.get(1).is_some_and(|a| a == "-all") {
                        out.push("--all".into());
                        i = 2;
                    } else if let Some(k) = orig.get(1) {
                        out.push(k.clone());
                        i = 2;
                    }
                    if let Some(fmt) = rest.get(i).and_then(|f| f.strip_prefix('-')) {
                        out.extend(["--format".into(), fmt.to_string()]);
                        out.extend(orig.get(i + 1).cloned());
                    }
                }
                _ => {
                    out.push("start".into());
                    out.extend(orig.first().cloned());
                }
            }
        }
        _ => return args,
    }
    out
}

fn run(cli: Cli) -> Result<()> {
    let mut settings = Settings::load()?;
    match cli.cmd {
        Cmd::Scan(a) => {
            let issues = do_scan(&settings, &a);
            report(&issues, &a)?;
        }
        Cmd::Clean { scan, yes } => {
            let issues = do_scan(&settings, &scan);
            report(&issues, &scan)?;
            clean(&settings, &issues, yes)?;
        }
        Cmd::Quick { all_users } => {
            let a = ScanArgs { mode: settings.quick_clean_mode, categories: None, all_users, export: None, json: false };
            let issues = do_scan(&settings, &a);
            println!("Found {} item(s).", issues.len());
            clean(&settings, &issues, true)?;
        }
        Cmd::Forced { root, text, export, clean: do_clean, yes } => {
            let issues = engine::run_forced_scan(&settings, &root, &text)?;
            print!("{}", export::to_txt(&issues));
            println!("\n{} match(es)", issues.len());
            if let Some(p) = export {
                export::export(&issues, &p)?;
            }
            if do_clean {
                clean(&settings, &issues, yes)?;
            }
        }
        Cmd::Categories => {
            for c in engine::all_checks() {
                println!("{:<22} {:<9} {}: {}", c.id, format!("{:?}", c.min_mode).to_lowercase(), c.name, c.description);
            }
        }
        Cmd::Backup(b) => run_backup(&settings, b)?,
        Cmd::Exclude(l) => {
            edit_list(&mut settings.exclude_list, l)?;
            settings.save()?;
        }
        Cmd::Include(l) => {
            edit_list(&mut settings.include_list, l)?;
            settings.save()?;
        }
        Cmd::Settings { action } => {
            match action {
                None => {}
                Some(SettingsCmd::Reset) => settings = Settings::default(),
                Some(SettingsCmd::Set { key, value }) => {
                    let mut v = serde_json::to_value(&settings)?;
                    let obj = v.as_object_mut().unwrap();
                    if !obj.contains_key(&key) {
                        bail!("unknown setting '{key}'");
                    }
                    let parsed = serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value));
                    obj.insert(key, parsed);
                    settings = serde_json::from_value(v)?;
                }
            }
            settings.save()?;
            println!("{}", serde_json::to_string_pretty(&settings)?);
            println!("# stored in {}", cleancore::config::data_dir().join("settings.json").display());
        }
        Cmd::Open { location } => platform::open_in_editor(&location)?,
        Cmd::Tune(t) => run_tune(t)?,
        Cmd::Watch(w) => run_watch(&settings, w)?,
        Cmd::Schedule(c) => run_schedule(&settings, c)?,
        Cmd::Programs { r#match, location, uninstall_cmd, system, store, table, json, export } => {
            let mut s = settings.clone();
            s.show_system_components |= system;
            let mut list = uninst::list_programs(&s);
            if let Some(m) = &r#match {
                list.retain(|p| uninst::wildcard(m, &p.name));
            }
            if store {
                list.retain(|p| p.kind.is_store_app());
            }
            if let Some(f) = export {
                uninst::export(&list, &f)?;
                println!("Exported {} program(s) to {}", list.len(), f.display());
            } else if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if table {
                print_programs_table(&list, location, uninstall_cmd);
            } else {
                for p in &list {
                    println!(
                        "{}{}  [{}]{}",
                        p.name,
                        if p.version.is_empty() { String::new() } else { format!("  {}", p.version) },
                        p.kind.label(),
                        if p.publisher.is_empty() { String::new() } else { format!("  · {}", p.publisher) }
                    );
                    if location {
                        if let Some(l) = &p.install_location {
                            println!("    location: {l}");
                        }
                    }
                    if uninstall_cmd {
                        if let Some(u) = &p.uninstall_string {
                            println!("    uninstall: {u}");
                        }
                    }
                }
                let total: u64 = list.iter().filter_map(|p| p.size).sum();
                println!("{} program(s), {}", list.len(), human(total));
            }
        }
        Cmd::Uninstall(a) => run_uninstall(&settings, a)?,
        Cmd::Trace(t) => run_trace(&settings, t)?,
        Cmd::Extensions(c) => run_extensions(&settings, c)?,
        Cmd::Autorun(c) => run_autorun(&settings, c)?,
        Cmd::Hunter { list, all, gui, pid, at_pointer, action, yes } => run_hunter(gui, &settings, list, all, pid, at_pointer, &action, yes)?,
        Cmd::Logsdb(c) => run_logsdb(&settings, c)?,
        Cmd::Junk(c) => run_junk(&settings, c)?,
        Cmd::Dupes(c) => run_dupes(&settings, c)?,
        Cmd::History(c) => run_history(&mut settings, c)?,
        Cmd::Shred { paths, passes, yes } => run_shred(&settings, &paths, passes, yes)?,
        Cmd::WipeFree { drive, passes, max_mb, yes } => run_wipe(&settings, drive, passes, max_mb, yes)?,
        Cmd::Tools(c) => run_tools(&mut settings, c)?,
        Cmd::Update { check } => run_update(check)?,
        Cmd::RootHelper => anyhow::bail!("the administrator helper is started by CleanCore itself"),
        Cmd::Slop(c) => run_slop(&settings, c)?,
        Cmd::Groups(c) => run_groups(&settings, c)?,
        Cmd::App(c) => run_app(&settings, c)?,
        Cmd::ShellIntegration { state } => match state.as_str() {
            "on" => {
                cleancore::shell::integrate(true)?;
                println!("Added \"Install with CleanCore\" and \"Uninstall with CleanCore\" to the right-click menu");
            }
            "off" => {
                cleancore::shell::integrate(false)?;
                println!("Removed CleanCore from the right-click menu");
            }
            "status" => println!("{}", if cleancore::shell::is_integrated() { "on" } else { "off" }),
            other => bail!("use on, off or status (not '{other}')"),
        },
        Cmd::UninstallHistory { limit, show } => {
            let h = uninst::history();
            if let Some(n) = show {
                let e = h.get(n.saturating_sub(1)).ok_or_else(|| anyhow::anyhow!("no history entry {n}"))?;
                print!("{}", uninst::history_text(e));
            } else {
                for (i, e) in h.iter().take(limit).enumerate() {
                    println!(
                        "{:>3}. {}  {:<17} {} {}  ({} removed{})",
                        i + 1,
                        e.time.format("%Y-%m-%d %H:%M"),
                        e.operation.label(),
                        e.program,
                        e.version,
                        e.removed.len(),
                        if e.failed.is_empty() { String::new() } else { format!(", {} failed", e.failed.len()) }
                    );
                }
                if h.is_empty() {
                    println!("Nothing uninstalled yet.");
                }
            }
        }
    }
    Ok(())
}

fn do_scan(settings: &Settings, a: &ScanArgs) -> Vec<Issue> {
    let sel = match &a.categories {
        Some(ids) => Selection::Ids(ids.clone()),
        None => Selection::Mode(a.mode),
    };
    let scope = if a.all_users { Scope::AllUsers } else { Scope::CurrentUser };
    let t = Instant::now();
    let issues = engine::run_scan(settings, &sel, scope);
    eprintln!("Scan finished in {:.0?}", t.elapsed());
    issues
}

fn report(issues: &[Issue], a: &ScanArgs) -> Result<()> {
    if a.json {
        println!("{}", serde_json::to_string_pretty(issues)?);
    } else if issues.is_empty() {
        println!("No problems found.");
    } else {
        print!("{}", export::to_txt(issues));
        println!("\nFound {} item(s).", issues.len());
    }
    if let Some(p) = &a.export {
        export::export(issues, p)?;
        eprintln!("Exported to {}", p.display());
    }
    Ok(())
}

fn clean(settings: &Settings, issues: &[Issue], yes: bool) -> Result<()> {
    if issues.is_empty() {
        return Ok(());
    }
    if !yes && !settings.skip_warnings && !confirm(&format!("Delete {} item(s)? All deleted data will be backed up.", issues.len()))? {
        println!("Cancelled.");
        return Ok(());
    }
    let r = cleaner::clean(settings, issues)?;
    match &r.restore_point {
        Some(Ok(())) => println!("Restore point created."),
        Some(Err(e)) => eprintln!("warning: failed to create a restore point: {e}"),
        None => {}
    }
    if let Some(id) = &r.full_backup_id {
        println!("Full backup: {id}");
    }
    println!("Deleted {} item(s). Backup: {}", r.cleaned, r.backup_id);
    for (loc, err) in &r.failed {
        eprintln!("  failed: {loc}: {err}");
    }
    Ok(())
}

fn confirm(q: &str) -> Result<bool> {
    print!("{q} [y/N] ");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(matches!(s.trim(), "y" | "Y" | "yes"))
}

fn run_backup(settings: &Settings, cmd: BackupCmd) -> Result<()> {
    match cmd {
        BackupCmd::Create { full, key, name, note } => {
            let kind = if full { BackupKind::Full } else { BackupKind::Partial };
            let mut b = Backup::create(settings, &name, &note, kind)?;
            let entries = match (full, key) {
                (true, _) => platform::backup_full(&b.dir),
                (false, Some(k)) => platform::backup_key(&b.dir, &k),
                (false, None) => bail!("use --full or --key <KEY>"),
            };
            match entries {
                Ok(e) => b.manifest.entries = e,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&b.dir);
                    return Err(e.context("Backup creation failed!"));
                }
            }
            b.save()?;
            println!("Created backup {} ({} entries)", b.manifest.id, b.manifest.entries.len());
        }
        BackupCmd::List => {
            for m in backup::list(settings)? {
                println!("{:<20} {:<16} {:>9}  {:>4} entries  {}  {}", m.id, format!("{:?}", m.kind), human(m.size_bytes), m.entries.len(), m.name, m.note);
            }
        }
        BackupCmd::Show { id } => {
            for e in Backup::open(settings, &id)?.manifest.entries {
                println!("{}", e.label());
            }
        }
        BackupCmd::Restore { id, only } => {
            let b = Backup::open(settings, &id)?;
            let n = b.restore((!only.is_empty()).then_some(&only[..]))?;
            println!("{n} item(s) restored!");
        }
        BackupCmd::Delete { id, all, from, to } => {
            if from.is_some() || to.is_some() {
                let from = from.as_deref().map(|f| backup::parse_local(f, false)).transpose()?.unwrap_or(chrono::DateTime::<chrono::Local>::MIN_UTC.into());
                let to = to.as_deref().map(|t| backup::parse_local(t, true)).transpose()?.unwrap_or_else(chrono::Local::now);
                let removed = backup::delete_range(settings, from, to)?;
                println!("Deleted {} backup(s)", removed.len());
            } else if all {
                for m in backup::list(settings)? {
                    backup::delete(settings, &m.id)?;
                }
            } else {
                backup::delete(settings, &id.ok_or_else(|| anyhow::anyhow!("give a backup id or --all"))?)?;
            }
        }
        BackupCmd::Daily => match backup::daily_full_if_due(settings)? {
            Some(id) => println!("Created daily backup {id}"),
            None => println!("Nothing to do (the option is off, or today's backup exists)."),
        },
        BackupCmd::Prune => {
            let removed = backup::prune(settings)?;
            println!("Removed {} backup(s)", removed.len());
        }
    }
    Ok(())
}

fn edit_list(list: &mut Vec<ListEntry>, cmd: ListCmd) -> Result<()> {
    match cmd {
        ListCmd::Add { path, no_subkeys } => {
            list.retain(|e| !e.path.eq_ignore_ascii_case(&path));
            list.push(ListEntry { path, include_subkeys: !no_subkeys, enabled: true });
        }
        ListCmd::Remove { path } => {
            let before = list.len();
            list.retain(|e| !e.path.eq_ignore_ascii_case(&path));
            if list.len() == before {
                bail!("'{path}' is not in the list");
            }
        }
        ListCmd::List => {}
    }
    for e in list.iter() {
        println!("[{}] {}{}", if e.enabled { 'x' } else { ' ' }, e.path, if e.include_subkeys { "  (+subkeys)" } else { "" });
    }
    Ok(())
}

fn run_extensions(settings: &Settings, cmd: ExtCmd) -> Result<()> {
    use cleancore::extensions::{self as ext, Browser};
    let parse_b =
        |b: &Option<String>| -> Result<Option<Browser>> { b.as_deref().map(|x| Browser::from_flag(x).ok_or_else(|| anyhow::anyhow!("unknown browser '{x}'"))).transpose() };
    match cmd {
        ExtCmd::List { browser, all, json } => {
            let b = parse_b(&browser)?;
            let list: Vec<_> = ext::list(all).into_iter().filter(|e| b.is_none_or(|b| e.browser == b)).collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            let mut last = String::new();
            for e in &list {
                let head = format!("{} › {}", e.browser.label(), e.profile);
                if head != last {
                    println!("\n{head}");
                    last = head;
                }
                println!("  {:<40} {:<10} {:<9} {}{}", e.name, e.version, e.kind, if e.enabled { "" } else { "(disabled) " }, if e.builtin { "(built-in)" } else { "" });
            }
            println!("\n{} extension(s)", list.len());
        }
        ExtCmd::Remove { names, browser, profile, yes } => {
            let names: Vec<String> = names.split(';').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
            let found = ext::find(&names, parse_b(&browser)?, profile.as_deref())?;
            for e in &found {
                println!("{} › {} › {} {}", e.browser.label(), e.profile, e.name, e.version);
            }
            if !yes && !confirm(&format!("Remove {} extension(s)? They are backed up first.", found.len()))? {
                return Ok(());
            }
            let r = ext::remove(settings, &found)?;
            println!("Removed. Backup: {}", r.backup_id);
            for (loc, err) in &r.failed {
                eprintln!("  not removed: {loc}: {err}");
            }
        }
    }
    Ok(())
}

fn find_autorun(entries: &[cleancore::autorun::AutorunEntry], key: &str) -> Result<cleancore::autorun::AutorunEntry> {
    let hits: Vec<_> = entries.iter().filter(|e| e.id == key || e.name.eq_ignore_ascii_case(key)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no autorun entry \"{key}\" (see `cleancore autorun list --all`)"),
        _ => bail!("\"{key}\" matches several entries; use the id from `cleancore autorun list --json`"),
    }
}

fn run_autorun(settings: &Settings, cmd: AutorunCmd) -> Result<()> {
    use cleancore::autorun::{self, AddMethod};
    let all = autorun::list(true);
    let cmd_kind = if matches!(cmd, AutorunCmd::RemoveInvalid { .. }) { "invalid" } else { "" };
    match cmd {
        AutorunCmd::List { all: show_all, json, export } => {
            let list: Vec<_> = all.into_iter().filter(|e| show_all || settings.autorun_show_system || !e.system).collect();
            if let Some(f) = export {
                autorun::export(&list, &f)?;
                println!("Exported {} entr(ies) to {}", list.len(), f.display());
            } else if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for e in &list {
                    let flags = [(!e.enabled).then_some("disabled"), e.running.then_some("running"), (!e.valid).then_some("MISSING TARGET")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "{} [{}]{}{}",
                        e.name,
                        e.source.label(),
                        if flags.is_empty() { String::new() } else { format!("  ({flags})") },
                        e.publisher.as_deref().map(|p| format!("  · {p}")).unwrap_or_default()
                    );
                    println!("    {}", e.command);
                }
                println!("{} entr(ies)", list.len());
            }
        }
        AutorunCmd::Enable { entry } => {
            autorun::set_enabled(&find_autorun(&all, &entry)?, true)?;
            println!("Enabled");
        }
        AutorunCmd::Run { entry } => {
            autorun::run_now(&find_autorun(&all, &entry)?)?;
            println!("Started");
        }
        AutorunCmd::Kill { entry } => match autorun::kill(&find_autorun(&all, &entry)?)? {
            0 => println!("It wasn't running"),
            n => println!("Stopped {n} process(es)"),
        },
        AutorunCmd::Disable { entry } => {
            autorun::set_enabled(&find_autorun(&all, &entry)?, false)?;
            println!("Disabled");
        }
        AutorunCmd::Remove { entry, yes } => {
            let e = find_autorun(&all, &entry)?;
            if !yes && !confirm(&format!("Remove \"{}\"?", e.name))? {
                return Ok(());
            }
            let (removed, disabled) = autorun::remove(settings, &[e])?;
            println!("Removed {removed}, disabled {disabled}");
        }
        AutorunCmd::Add { name, command, all_users, method } => {
            let m = match method.as_str() {
                "run" => AddMethod::Run,
                "runonce" => AddMethod::RunOnce,
                "startup" => AddMethod::StartupFolder,
                other => bail!("unknown method '{other}' (run, runonce, startup)"),
            };
            autorun::add(&name, &command, all_users, m)?;
            println!("Added \"{name}\"");
        }
        AutorunCmd::RemoveInvalid { yes } | AutorunCmd::RemoveDisabled { yes } => {
            let invalid = matches!(cmd_kind, "invalid");
            let picked: Vec<_> = all.into_iter().filter(|e| !e.system && if invalid { !e.valid } else { !e.enabled }).collect();
            if picked.is_empty() {
                println!("Nothing to remove.");
                return Ok(());
            }
            for e in &picked {
                println!("  {}: {}", e.name, e.command);
            }
            if !yes && !confirm(&format!("Remove {} entr(ies)?", picked.len()))? {
                return Ok(());
            }
            let (removed, disabled) = autorun::remove(settings, &picked)?;
            println!("Removed {removed}, disabled {disabled}");
        }
    }
    Ok(())
}

fn run_hunter(gui: bool, settings: &Settings, list: bool, all: bool, pid: Option<u32>, at_pointer: bool, action: &str, yes: bool) -> Result<()> {
    use cleancore::hunter;
    if gui {
        if cfg!(feature = "gui") {
            std::process::Command::new(cleancore::launch::program()?).arg("--hunter").spawn().context("opening the CleanCore window")?;
            return Ok(());
        }
        eprintln!("This cleancore was built without the window; listing programs instead.");
    }
    if list || (pid.is_none() && !at_pointer) {
        for p in hunter::gui_processes(all) {
            println!("{:>7}  {:<24} {}", p.pid, p.name, p.exe.unwrap_or_default());
        }
        println!("Act on one with `cleancore hunter --pid <PID> --action <info|uninstall|stop-autorun|kill|kill-delete|folder|search>`.");
        return Ok(());
    }
    let t = if at_pointer { hunter::identify_at_pointer(settings)?.context("no window under the pointer")? } else { hunter::identify(pid.unwrap(), settings)? };
    println!("{} (pid {})", t.name, t.pid);
    for (k, v) in [
        ("Window", t.title.clone()),
        ("File", t.exe.clone()),
        ("Publisher", t.publisher.clone()),
        ("Program", t.program.as_ref().map(|p| format!("{} {} [{}]", p.name, p.version, p.kind.label()))),
    ] {
        if let Some(v) = v {
            println!("  {k}: {v}");
        }
    }
    for a in &t.autoruns {
        println!("  Starts automatically: {} [{}]{}", a.name, a.source.label(), if a.enabled { "" } else { " (disabled)" });
    }
    match action {
        "info" => {}
        "folder" => hunter::open_folder(&t)?,
        "search" => hunter::search_online(&t)?,
        "stop-autorun" => println!("Disabled {} autorun entr(ies)", hunter::stop_autostart(&t)?),
        "kill" => {
            if yes || confirm(&format!("Force-close {} process(es)? Unsaved work is lost.", t.pids.len()))? {
                println!("Stopped {} process(es)", hunter::kill(&t)?);
            }
        }
        "kill-delete" => {
            if yes || confirm("Force-close it and delete its program file (backed up)?")? {
                println!("Done. Backup: {}", hunter::kill_and_delete(&t, settings)?);
            }
        }
        "uninstall" => match &t.program {
            Some(p) => {
                let a = UninstallArgs {
                    names: vec![p.id.clone()],
                    path: vec![],
                    mode: None,
                    quick: false,
                    leftovers_only: false,
                    forced: false,
                    msi: false,
                    app: None,
                    all_users: false,
                    yes,
                    resume: false,
                };
                run_uninstall(settings, a)?;
            }
            None => {
                let exe = t.exe.clone().context("unknown executable")?;
                let dir = std::path::Path::new(&exe).parent().map(|d| d.display().to_string()).unwrap_or(exe.clone());
                let a = UninstallArgs {
                    names: vec![t.name.clone()],
                    path: vec![dir],
                    mode: Some(ScanMode::Advanced),
                    quick: false,
                    leftovers_only: false,
                    forced: true,
                    msi: false,
                    app: None,
                    all_users: false,
                    yes,
                    resume: false,
                };
                run_uninstall(settings, a)?;
            }
        },
        other => bail!("unknown action '{other}'"),
    }
    Ok(())
}

fn run_logsdb(settings: &Settings, cmd: LogsDbCmd) -> Result<()> {
    use cleancore::tracer;
    match cmd {
        LogsDbCmd::List { all } => {
            let list: Vec<_> = tracer::db_list(settings).into_iter().filter(|d| all || d.compatible()).collect();
            for d in &list {
                println!("{:<32} {} {}  {} settings  {} files", d.log.name, d.log.os, d.log.arch, d.log.count(true), d.log.count(false));
            }
            println!("{} log(s) in {}", list.len(), tracer::db_folder(settings).display());
        }
        LogsDbCmd::Update => println!("Downloaded {} new log(s)", tracer::db_update(settings)?),
        LogsDbCmd::Add { log } => println!("Added to {}", tracer::db_add(settings, &tracer::find(&log)?)?.display()),
        LogsDbCmd::Uninstall { name, yes } => {
            let d = tracer::db_list(settings).into_iter().find(|d| d.log.name.eq_ignore_ascii_case(&name)).ok_or_else(|| anyhow::anyhow!("no database log named \"{name}\""))?;
            let local = tracer::db_import(&d)?;
            println!("Imported \"{}\" into Traced Programs", local.name);
            run_trace(settings, TraceCmd::Uninstall { log: local.id, only: vec![], advanced: false, yes })?;
        }
    }
    Ok(())
}

fn junk_settings(settings: &Settings, a: &JunkArgs) -> Settings {
    let mut s = settings.clone();
    if !a.locations.is_empty() {
        s.junk_locations = a.locations.clone();
    }
    if !a.patterns.is_empty() {
        s.junk_patterns = a.patterns.iter().map(|p| cleancore::config::Toggle { value: p.clone(), enabled: true }).collect();
    }
    if a.include_recent {
        s.junk_skip_recent = false;
    }
    s
}

fn run_junk(settings: &Settings, cmd: JunkCmd) -> Result<()> {
    use cleancore::junk;
    let s = match &cmd {
        JunkCmd::Scan { a, .. } | JunkCmd::Clean { a, .. } => junk_settings(settings, a),
    };
    let started = std::time::Instant::now();
    let files = junk::scan(&s, &junk::Progress::default());
    let total: u64 = files.iter().map(|f| f.size).sum();
    match cmd {
        JunkCmd::Scan { json, export, .. } => {
            if let Some(f) = export {
                junk::export(&files, &f)?;
                println!("Exported {} file(s) to {}", files.len(), f.display());
            } else if json {
                println!("{}", serde_json::to_string_pretty(&files)?);
                return Ok(());
            } else {
                for f in &files {
                    println!("{:>10}  {}", human(f.size), f.path.display());
                }
            }
            println!("{} junk file(s), {} in {:.1}s", files.len(), human(total), started.elapsed().as_secs_f32());
        }
        JunkCmd::Clean { permanent, yes, .. } => {
            if files.is_empty() {
                println!("No junk files found.");
                return Ok(());
            }
            let mut s = s;
            if permanent {
                s.junk_to_trash = false;
            }
            let dest = if s.junk_to_trash { "to the Trash" } else { "permanently" };
            if !yes && !confirm(&format!("Delete {} junk file(s) ({}) {dest}?", files.len(), human(total)))? {
                return Ok(());
            }
            let r = junk::delete(&s, &files);
            println!("Deleted {} file(s), {}", r.deleted, human(r.bytes));
            for (p, e) in r.failed.iter().take(20) {
                eprintln!("  not deleted: {p}: {e}");
            }
        }
    }
    Ok(())
}

fn run_dupes(settings: &Settings, cmd: DupesCmd) -> Result<()> {
    use cleancore::dupes::{self, Keep};
    let mut s = settings.clone();
    let (DupesCmd::Scan { a, .. } | DupesCmd::Clean { a, .. }) = &cmd;
    if !a.locations.is_empty() {
        s.dupes_locations = a.locations.clone();
    }
    if let Some(kb) = a.min_kb {
        s.dupes_min_size = kb * 1024;
    }
    if a.include_hidden {
        s.dupes_skip_hidden = false;
    }
    let started = std::time::Instant::now();
    let groups = dupes::scan(&s, &dupes::Progress::default());
    let wasted: u64 = groups.iter().map(|g| g.wasted()).sum();
    let summary = format!("{} group(s) of duplicates, {} can be freed", groups.len(), human(wasted));
    match cmd {
        DupesCmd::Scan { json, export, .. } => {
            if let Some(f) = export {
                dupes::export(&groups, &f)?;
                println!("Exported {} group(s) to {}", groups.len(), f.display());
            } else if json {
                println!("{}", serde_json::to_string_pretty(&groups)?);
                return Ok(());
            } else {
                for g in &groups {
                    println!("{} × {}", g.files.len(), human(g.size));
                    for f in &g.files {
                        println!("    {}", f.path.display());
                    }
                }
            }
            println!("{summary} ({:.1}s)", started.elapsed().as_secs_f32());
        }
        DupesCmd::Clean { keep, permanent, yes, .. } => {
            let keep = match keep.as_str() {
                "oldest" => Keep::Oldest,
                "newest" => Keep::Newest,
                "shortest" => Keep::ShortestPath,
                k => anyhow::bail!("unknown --keep \"{k}\" (oldest, newest or shortest)"),
            };
            if groups.is_empty() {
                println!("No duplicates found.");
                return Ok(());
            }
            if permanent {
                s.dupes_to_trash = false;
            }
            let n: usize = groups.iter().map(|g| g.files.len() - 1).sum();
            let dest = if s.dupes_to_trash { "to the Trash" } else { "permanently" };
            println!("{summary}");
            if !yes && !confirm(&format!("Delete {n} extra copies ({}) {dest}, keeping one of each?", human(wasted)))? {
                return Ok(());
            }
            let r = dupes::delete(&s, &groups, &dupes::auto_select(&groups, keep));
            println!("Deleted {} file(s), {}", r.deleted, human(r.bytes));
            for (p, e) in r.failed.iter().take(20) {
                eprintln!("  not deleted: {p}: {e}");
            }
        }
    }
    Ok(())
}

fn run_history(settings: &mut Settings, cmd: HistoryCmd) -> Result<()> {
    use cleancore::history;
    let items = history::items();
    let default = history::selected_by_default(settings, &items);
    match cmd {
        HistoryCmd::List { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
                return Ok(());
            }
            let mut last = String::new();
            for i in &items {
                if i.group != last {
                    println!("\n{}", i.group);
                    last = i.group.clone();
                }
                println!("  {} {:<40} {:>10}  {}", if default.contains(&i.id) { "*" } else { " " }, i.name, if i.size > 0 { human(i.size) } else { String::new() }, i.id);
            }
        }
        HistoryCmd::Clean { only, yes } => {
            let picked: Vec<_> = match only.as_deref() {
                Some("all") => items,
                Some(ids) => {
                    let ids: Vec<&str> = ids.split(',').map(str::trim).collect();
                    if let Some(bad) = ids.iter().find(|id| !items.iter().any(|i| i.id == **id)) {
                        bail!("no history item \"{bad}\" (see `cleancore history list`)");
                    }
                    items.into_iter().filter(|i| ids.contains(&i.id.as_str())).collect()
                }
                None => items.into_iter().filter(|i| default.contains(&i.id)).collect(),
            };
            for i in &picked {
                println!("  {} › {}", i.group, i.name);
            }
            if picked.is_empty() || !yes && !confirm(&format!("Clean {} item(s)? This can't be undone.", picked.len()))? {
                return Ok(());
            }
            let r = history::clean(settings, &picked);
            println!("Cleaned {} item(s), {}", r.cleaned, human(r.bytes));
            for s in &r.skipped {
                eprintln!("  skipped (close the browser first): {s}");
            }
            for (i, e) in &r.failed {
                eprintln!("  failed: {i}: {e}");
            }
        }
    }
    Ok(())
}

/// Run `job` on a thread while printing its progress.
fn with_progress<T: Send + 'static>(ctl: std::sync::Arc<cleancore::shred::Control>, job: impl FnOnce() -> T + Send + 'static) -> T {
    use std::sync::atomic::Ordering;
    let h = std::thread::spawn(job);
    while !h.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let pass = ctl.pass.load(Ordering::Relaxed);
        eprint!(
            "\r{:5.1}%  {} of {}{}   ",
            ctl.fraction() * 100.0,
            human(ctl.done.load(Ordering::Relaxed)),
            human(ctl.total.load(Ordering::Relaxed)),
            if pass > 0 { format!("  pass {pass}") } else { String::new() }
        );
    }
    eprintln!();
    h.join().expect("worker thread panicked")
}

fn run_shred(settings: &Settings, paths: &[PathBuf], passes: Option<u32>, yes: bool) -> Result<()> {
    for p in paths {
        if std::fs::symlink_metadata(p).is_err() {
            bail!("{} doesn't exist", p.display());
        }
    }
    let passes = passes.unwrap_or(settings.shred_passes);
    if !yes && !confirm(&format!("Overwrite and delete {} item(s) with {passes} pass(es)? They can't be recovered, not even from backups.", paths.len()))? {
        return Ok(());
    }
    let ctl = std::sync::Arc::new(cleancore::shred::Control::default());
    let (c, p) = (ctl.clone(), paths.to_vec());
    let r = with_progress(ctl, move || cleancore::shred::shred(&p, passes, &c))?;
    println!("Shredded {} file(s) and {} folder(s), {}", r.files, r.folders, human(r.bytes));
    for (p, e) in &r.failed {
        eprintln!("  failed: {p}: {e}");
    }
    Ok(())
}

fn run_wipe(settings: &Settings, drive: Option<PathBuf>, passes: Option<u32>, max_mb: Option<u64>, yes: bool) -> Result<()> {
    let Some(dir) = drive else {
        for d in cleancore::platform::drives() {
            println!("{:<24} {:<8} {:>10} free of {:>10}  {}", d.label, d.fs, human(d.free), human(d.total), d.path.display());
        }
        println!("Wipe one with `cleancore wipe-free <path>`.");
        return Ok(());
    };
    let passes = passes.unwrap_or(settings.shred_passes);
    let free = cleancore::platform::free_space(&dir).unwrap_or(0);
    if !yes
        && !confirm(&format!(
            "Fill the {} of free space on the drive of {} with random data, {passes} time(s)? This takes a while; the drive is briefly full.",
            human(free),
            dir.display()
        ))?
    {
        return Ok(());
    }
    let ctl = std::sync::Arc::new(cleancore::shred::Control::default());
    let c = ctl.clone();
    let limit = max_mb.map(|m| m << 20);
    let n = with_progress(ctl, move || cleancore::shred::wipe_free_space(&dir, passes, limit, &c))?;
    if max_mb.is_some() {
        println!("Wrote {} of overwrite data (limited by --max-mb).", human(n));
    } else {
        println!("Wrote {} of overwrite data; the free space is wiped.", human(n));
    }
    Ok(())
}

fn run_app(settings: &Settings, cmd: AppCmd) -> Result<()> {
    let apps = || -> Vec<Program> {
        let mut s = settings.clone();
        s.show_system_components = true;
        uninst::list_programs(&s).into_iter().filter(|p| p.kind.is_store_app()).collect()
    };
    match cmd {
        AppCmd::Run { name } => {
            let list = apps();
            let p = find_program(&list, &name)?;
            platform::app_run(p)?;
            println!("Started {}", p.name);
        }
        AppCmd::Terminate { name } => {
            let list = apps();
            let p = find_program(&list, &name)?;
            match platform::app_terminate(p)? {
                0 => println!("{} wasn't running", p.name),
                n => println!("Stopped {} ({n})", p.name),
            }
        }
        AppCmd::Reset { name } => {
            #[cfg(windows)]
            {
                let list = apps();
                let p = find_program(&list, &name)?;
                platform::app_reset(p)?;
                println!("{} was reset", p.name);
            }
            #[cfg(not(windows))]
            bail!("resetting \"{name}\" only applies to Windows apps");
        }
        AppCmd::ResetStore => {
            platform::reset_store()?;
            println!("Resetting the Microsoft Store…");
        }
    }
    Ok(())
}

fn run_groups(settings: &Settings, cmd: GroupsCmd) -> Result<()> {
    let mut meta = uninst::Meta::load();
    let programs = || {
        let mut s = settings.clone();
        s.show_system_components = true;
        uninst::list_programs(&s)
    };
    match cmd {
        GroupsCmd::List => {
            let list = programs();
            for (g, ids) in &meta.groups {
                println!("{g} ({})", ids.len());
                for id in ids {
                    println!("  {}", list.iter().find(|p| &p.id == id).map(|p| p.name.as_str()).unwrap_or(id));
                }
            }
            return Ok(());
        }
        GroupsCmd::Create { name } => meta.create_group(&name)?,
        GroupsCmd::Rename { old, new } => meta.rename_group(&old, &new)?,
        GroupsCmd::Delete { name } => meta.delete_group(&name)?,
        GroupsCmd::Add { program, group } => {
            let list = programs();
            let p = find_program(&list, &program)?;
            meta.remove_from_groups(&p.id);
            meta.add_to_group(&p.id, &group);
        }
        GroupsCmd::Remove { program } => {
            let list = programs();
            let p = find_program(&list, &program)?;
            meta.remove_from_groups(&p.id);
        }
    }
    meta.save()?;
    println!("Done");
    Ok(())
}

fn run_slop(settings: &Settings, cmd: SlopCmd) -> Result<()> {
    use cleancore::slop;
    if !cfg!(windows) {
        bail!("the Microsoft slop cleaner is for Windows");
    }
    let pick_tweaks = |ids: &[String]| -> Result<Vec<tuner::Tweak>> {
        let all: Vec<tuner::Tweak> = tuner::tweaks().into_iter().filter(|t| slop::TWEAKS.contains(&t.id.as_str())).collect();
        if ids.iter().any(|i| i == "all") {
            return Ok(all);
        }
        ids.iter().map(|i| all.iter().find(|t| &t.id == i).cloned().ok_or_else(|| anyhow::anyhow!("no slop setting \"{i}\" (see `cleancore slop settings`)"))).collect()
    };
    match cmd {
        SlopCmd::Scan => {
            let s = slop::scan()?;
            let mut last = "";
            for f in &s.found {
                if f.app.group != last {
                    println!("\n{}", f.app.group);
                    last = f.app.group;
                }
                let prov = if f.provisioned.is_empty() { "" } else { "  (preinstalled for new users)" };
                println!("  {} {:<40}{prov}  {}", if f.app.recommended { "*" } else { " " }, f.app.name, f.app.note);
            }
            if !s.admin {
                println!("\nRun as administrator to see every user's apps and the preinstalled copies.");
            }
        }
        SlopCmd::Remove { names, recommended, all_users, deprovision, yes } => {
            let s = slop::scan()?;
            if (all_users || deprovision) && !s.admin {
                bail!("--all-users and --deprovision need an administrator console");
            }
            let wanted: Vec<String> = names.unwrap_or_default().split(';').map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).collect();
            let picked: Vec<slop::Found> = s
                .found
                .into_iter()
                .filter(|f| (recommended && f.app.recommended) || wanted.iter().any(|w| f.app.name.to_lowercase() == *w || f.app.pattern.to_lowercase() == *w))
                .collect();
            if picked.is_empty() {
                bail!("nothing to remove: name apps from `cleancore slop scan`, or use --recommended");
            }
            for f in &picked {
                println!("  {}", f.app.name);
            }
            if !yes && !confirm(&format!("Remove {} app(s){}? They can be reinstalled from the Microsoft Store.", picked.len(), if all_users { " for every user" } else { "" }))? {
                return Ok(());
            }
            let r = slop::remove(settings, &picked, all_users, deprovision);
            println!("Removed {}", r.removed.len());
            for (n, e) in &r.failed {
                eprintln!("  failed: {n}: {e}");
            }
        }
        SlopCmd::Settings => {
            let ts = pick_tweaks(&["all".into()])?;
            let state = tuner::State::load()?;
            for st in tuner::status_all(&ts, &state) {
                let t = ts.iter().find(|t| t.id == st.id).unwrap();
                let mark = match (&st.available, st.current.is_some()) {
                    (Err(_), _) => "n/a",
                    (Ok(()), true) => "off",
                    _ => "on ",
                };
                println!("  [{mark}] {:<26} {}", t.id, t.name);
            }
            println!("\n[off] = already turned off. Turn off with `cleancore slop apply <id>|all`.");
        }
        SlopCmd::Apply { ids } | SlopCmd::Revert { ids } if ids.is_empty() => {
            bail!("name setting ids (see `cleancore slop settings`) or `all`")
        }
        SlopCmd::Apply { ids } => {
            let mut state = tuner::State::load()?;
            let ts = pick_tweaks(&ids)?;
            let status = tuner::status_all(&ts, &state);
            let mut needs = std::collections::BTreeSet::new();
            for t in &ts {
                match status.iter().find(|s| s.id == t.id).map(|s| &s.available) {
                    Some(Err(why)) => println!("  skipped {}: {why}", t.id),
                    _ => match tuner::apply(t, &t.options[0].id, &mut state) {
                        Ok(r) => {
                            needs.insert(r);
                            println!("  turned off: {}", t.name);
                        }
                        Err(e) => eprintln!("  failed {}: {e:#}", t.id),
                    },
                }
            }
            for r in needs {
                if let Some(m) = r.message() {
                    println!("{m}");
                }
            }
        }
        SlopCmd::Revert { ids } => {
            let mut state = tuner::State::load()?;
            let ours: Vec<tuner::Tweak> = pick_tweaks(&ids)?.into_iter().filter(|t| state.applied.contains_key(&t.id)).collect();
            if ours.is_empty() {
                println!("Nothing to restore: CleanCore didn't change these settings.");
            }
            for t in &ours {
                match tuner::revert(t, &mut state) {
                    Ok(_) => println!("  restored: {}", t.name),
                    Err(e) => eprintln!("  failed {}: {e:#}", t.id),
                }
            }
        }
    }
    Ok(())
}

fn run_tools(settings: &mut Settings, cmd: ToolsCmd) -> Result<()> {
    use cleancore::systools;
    match cmd {
        ToolsCmd::List => {
            let mut last = "";
            for t in systools::list(settings) {
                if t.group != last {
                    println!("\n{}", t.group);
                    last = t.group;
                }
                println!("  {:<30} {:<44} {}{}", t.name, t.description, t.command, if t.admin { "  (admin)" } else { "" });
            }
        }
        ToolsCmd::Run { name } => {
            let tools = systools::list(settings);
            let t = systools::find(&tools, &name)?;
            systools::run(t)?;
            println!("Started {}", t.name);
        }
        ToolsCmd::Add { name, command } => {
            systools::add(settings, &name, &command)?;
            println!("Added \"{name}\"");
        }
        ToolsCmd::Remove { name } => {
            systools::remove(settings, &name)?;
            println!("Removed \"{name}\"");
        }
    }
    Ok(())
}

fn pending_snapshot() -> PathBuf {
    cleancore::config::data_dir().join("traces").join("pending.snapshot.json")
}

fn save_trace(settings: &Settings, items: Vec<cleancore::tracer::TraceItem>, name: Option<String>, installer: Option<String>) -> Result<()> {
    use cleancore::tracer;
    let _ = settings;
    let created = items.iter().filter(|i| i.status == tracer::ItemStatus::Created).count();
    if created == 0 {
        println!("Nothing was created during the trace; no log saved.");
        return Ok(());
    }
    let name = name.unwrap_or_else(|| tracer::suggest_name(&items, installer.as_deref()));
    let log = tracer::new_log(&name, installer, items);
    tracer::save(&log)?;
    println!("Saved trace log \"{}\" ({}): {} settings item(s), {} file/folder/package item(s), {}", log.name, log.id, log.count(true), log.count(false), human(log.size()));
    Ok(())
}

fn run_trace(settings: &Settings, cmd: TraceCmd) -> Result<()> {
    use cleancore::tracer::{self, ItemStatus};
    use std::io::IsTerminal;
    match cmd {
        TraceCmd::Start { run, name, no_wait } => {
            eprintln!("Recording the current state…");
            let t = Instant::now();
            let session = tracer::Session::start(settings, run.clone());
            eprintln!("Snapshot taken in {:.1?}. Install the program now{}.", t.elapsed(), if run.is_some() { " (starting the installer)" } else { "" });
            if run.is_some() {
                let mut child = session.run_installer(settings)?;
                let st = child.wait()?;
                eprintln!("Installer exited ({st}).");
            }
            if !no_wait && std::io::stdin().is_terminal() {
                print!("Run and configure the program, close it, then press Enter to finish the trace… ");
                std::io::stdout().flush()?;
                let mut s = String::new();
                std::io::stdin().read_line(&mut s)?;
            }
            eprintln!("Comparing…");
            let (items, left_out) = session.finish_report(settings);
            if left_out > 0 {
                eprintln!("Left out {left_out} change(s) made by other programs during the trace.");
            }
            save_trace(settings, items, name, run)?;
        }
        TraceCmd::Begin => {
            let snap = tracer::take_snapshot(settings);
            cleancore::config::ensure_dir(pending_snapshot().parent().unwrap())?;
            std::fs::write(pending_snapshot(), serde_json::to_vec(&snap)?)?;
            println!("State recorded. Install the program, then run `cleancore trace end`.");
        }
        TraceCmd::End { name } => {
            let data = std::fs::read(pending_snapshot()).context("no trace in progress: run `cleancore trace begin` first")?;
            let snap: tracer::Snapshot = serde_json::from_slice(&data)?;
            let items = tracer::diff(settings, &snap);
            std::fs::remove_file(pending_snapshot())?;
            save_trace(settings, items, name, None)?;
        }
        TraceCmd::List => {
            let all = tracer::list();
            for l in &all {
                println!(
                    "{:<36} {:<28} {}  {:>5} settings  {:>5} files  {:>9}{}",
                    l.id,
                    l.name,
                    l.created.format("%Y-%m-%d %H:%M"),
                    l.count(true),
                    l.count(false),
                    human(l.size()),
                    if l.imported { "  (imported)" } else { "" }
                );
            }
            if all.is_empty() {
                println!("No trace logs. Trace an installation with `cleancore trace start --run <installer>`.");
            }
        }
        TraceCmd::Show { log, all } => {
            let l = tracer::find(&log)?;
            let mut l2 = l.clone();
            if !all {
                l2.items.retain(|i| i.status == ItemStatus::Created);
            }
            print!("{}", tracer::to_text(&l2));
        }
        TraceCmd::Uninstall { log, only, advanced, yes } => {
            let l = tracer::find(&log)?;
            let mut issues = tracer::to_issues(&l);
            if !only.is_empty() {
                issues.retain(|i| only.iter().any(|o| i.location.to_lowercase().contains(&o.to_lowercase())));
            }
            println!("{}: {} item(s) to remove", l.name, issues.len());
            show_leftovers(&issues);
            if !issues.is_empty() {
                if !yes && !confirm("Remove them? Everything is backed up first.")? {
                    return Ok(());
                }
                prepare_uninstall(settings, &l.name, yes)?;
                let r = tracer::uninstall(settings, &l, &issues)?;
                println!("Removed {} item(s). Backup: {}", r.cleaned, r.backup_id);
                for (loc, err) in &r.failed {
                    eprintln!("  not removed: {loc}: {err}");
                }
            }
            if advanced {
                let a = UninstallArgs {
                    names: vec![],
                    path: vec![],
                    mode: Some(ScanMode::Moderate),
                    quick: false,
                    leftovers_only: true,
                    forced: false,
                    msi: false,
                    app: None,
                    all_users: false,
                    yes,
                    resume: false,
                };
                leftovers_step(settings, &l.target(), &a, &l.name, "", Operation::TracedUninstall)?;
            }
        }
        TraceCmd::Export { log, file } => {
            tracer::export(&tracer::find(&log)?, &file)?;
            println!("Exported to {}", file.display());
        }
        TraceCmd::Import { file } => {
            let l = tracer::import(&file)?;
            println!("Imported \"{}\" as {}", l.name, l.id);
        }
        TraceCmd::Merge { logs, name } => {
            let ids: Vec<String> = logs.iter().map(|l| tracer::find(l).map(|x| x.id)).collect::<Result<_>>()?;
            let m = tracer::merge(&ids, &name)?;
            println!("Merged into \"{}\" ({}), {} item(s)", m.name, m.id, m.items.len());
        }
        TraceCmd::Rename { log, name } => {
            tracer::rename(&tracer::find(&log)?.id, &name)?;
            println!("Renamed to \"{name}\"");
        }
        TraceCmd::Group { log, group } => {
            let l = tracer::find(&log)?;
            tracer::set_group(&l.id, group.as_deref())?;
            match group {
                Some(g) => println!("\"{}\" is in group \"{g}\"", l.name),
                None => println!("\"{}\" is no longer in a group", l.name),
            }
        }
        TraceCmd::Comment { log, text } => {
            let mut l = tracer::find(&log)?;
            l.comment = text;
            l.modified = chrono::Local::now();
            tracer::save(&l)?;
        }
        TraceCmd::Delete { log } => {
            let l = tracer::find(&log)?;
            tracer::delete(&l.id)?;
            println!("Deleted log \"{}\" (the program itself was not touched)", l.name);
        }
        TraceCmd::Backup { file } => println!("Saved {} log(s) to {}", tracer::backup_all(&file)?, file.display()),
        TraceCmd::Restore { file } => println!("Restored {} log(s)", tracer::restore_all(&file)?),
    }
    Ok(())
}

fn find_program<'a>(list: &'a [Program], name: &str) -> Result<&'a Program> {
    let exact: Vec<&Program> = list.iter().filter(|p| p.name.eq_ignore_ascii_case(name) || p.id == name || p.package.as_deref() == Some(name)).collect();
    match exact.as_slice() {
        [one] => Ok(one),
        [] => {
            let similar: Vec<&str> = list.iter().filter(|p| p.name.to_lowercase().contains(&name.to_lowercase())).map(|p| p.name.as_str()).take(8).collect();
            if similar.is_empty() {
                bail!("no installed program named \"{name}\" (use --forced to remove leftovers of an unlisted program)")
            }
            bail!("no program named exactly \"{name}\". Did you mean: {}", similar.join(", "))
        }
        many => bail!("\"{name}\" matches {} programs; use the id from `cleancore programs --json`", many.len()),
    }
}

fn show_leftovers(issues: &[cleancore::model::Issue]) {
    let reg = if cfg!(windows) { "Registry" } else { "Settings" };
    for (cat, label) in [("leftover_registry", reg), ("leftover_files", "Files and folders")] {
        let items: Vec<_> = issues.iter().filter(|i| i.category == cat).collect();
        if items.is_empty() {
            continue;
        }
        println!("\n{label} ({})", items.len());
        for i in items {
            println!("  {}\n      {}", i.location, i.detail);
        }
    }
    println!();
}

fn leftovers_step(settings: &Settings, target: &Target, a: &UninstallArgs, program: &str, version: &str, op: Operation) -> Result<()> {
    let mode = a.mode.unwrap_or(settings.uninstall_scan_mode);
    eprintln!("Scanning for leftovers ({} mode)…", format!("{mode:?}").to_lowercase());
    let t = Instant::now();
    let issues = uninst::scan(settings, target, mode, a.all_users);
    eprintln!("Scan finished in {:.1?}", t.elapsed());
    if issues.is_empty() {
        println!("No leftovers found.");
        uninst::remove(settings, program, version, op, &[])?;
        return Ok(());
    }
    show_leftovers(&issues);
    if !a.yes && !confirm(&format!("Delete {} leftover(s)? Everything is backed up first.", issues.len()))? {
        println!("Leftovers kept.");
        return Ok(());
    }
    let r = uninst::remove(settings, program, version, op, &issues)?;
    println!("Removed {} leftover(s). Backup: {}", r.cleaned, r.backup_id);
    for (loc, err) in &r.failed {
        eprintln!("  not removed: {loc}: {err}");
    }
    Ok(())
}

fn run_uninstall(settings: &Settings, a: UninstallArgs) -> Result<()> {
    use std::io::IsTerminal;
    let mut s = settings.clone();
    s.show_system_components = true;
    if a.resume {
        let p = uninst::pending().context("no interrupted uninstall to continue")?;
        println!("Continuing the uninstall of {} (started {})", p.label, p.started.format("%d %b %Y %H:%M"));
        return leftovers_step(settings, &p.target, &a, &p.label, &p.version, p.op);
    }
    // Forced Uninstall: names and/or paths of something that isn't (properly) listed.
    if a.forced {
        if a.names.is_empty() && a.path.is_empty() {
            bail!("give a program name and/or --path");
        }
        let names: Vec<String> = a.names.iter().flat_map(|n| n.split(';').map(|x| x.trim().to_string())).filter(|n| !n.is_empty()).collect();
        let target = Target { names: names.clone(), paths: a.path.clone(), ..Default::default() };
        let label = if names.is_empty() { a.path.join(", ") } else { names.join(", ") };
        prepare_uninstall(settings, &label, a.yes)?;
        return leftovers_step(settings, &target, &a, &label, "", Operation::ForcedUninstall);
    }
    let list = uninst::list_programs(&s);
    let programs: Vec<Program> = if let Some(apps) = &a.app {
        apps.split(';')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| find_program(&list.iter().filter(|p| p.kind.is_store_app()).cloned().collect::<Vec<_>>(), n).cloned())
            .collect::<Result<_>>()?
    } else {
        if a.names.is_empty() {
            bail!("name the program(s) to uninstall (see `cleancore programs`)");
        }
        a.names.iter().map(|n| find_program(&list, n).cloned()).collect::<Result<_>>()?
    };

    // Multiple / Quick Uninstall: one restore point for the whole queue, silent uninstallers.
    if a.quick || programs.len() > 1 || a.app.is_some() {
        if !a.yes && !confirm(&format!("Quick uninstall {} program(s): {}?", programs.len(), programs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")))? {
            return Ok(());
        }
        prepare_uninstall(&uninst::settings_for(settings, &programs), &format!("{} program(s)", programs.len()), a.yes)?;
        let mut s2 = settings.clone();
        if let Some(m) = a.mode {
            s2.uninstall_scan_mode = m;
        }
        for p in &programs {
            println!("→ {}", p.name);
            match uninst::quick_uninstall(&s2, p) {
                Ok(r) => println!("  {}", r.message(&p.name)),
                Err(e) => eprintln!("  failed: {e:#}"),
            }
        }
        return Ok(());
    }

    let p = &programs[0];
    println!("{} {}  [{}]", p.name, p.version, p.kind.label());
    let target = Target::from_program(p);
    if a.leftovers_only {
        let mut t = target;
        t.paths.extend(a.path.iter().cloned());
        return leftovers_step(settings, &t, &a, &p.name, &p.version, Operation::LeftoversOnly);
    }
    prepare_uninstall(&uninst::settings_for(settings, [p]), &p.name, a.yes)?;
    let mode =if a.msi { RunMode::ForcedMsi } else { RunMode::Normal };
    if p.uninstall_string.is_none() && !a.msi {
        println!("No built-in uninstaller: removing the program's files and leftovers.");
        return leftovers_step(settings, &target, &a, &p.name, &p.version, Operation::Uninstall);
    }
    match uninst::run_with_options(settings, p, mode, Operation::Uninstall) {
        Ok((notes, r)) => {
            println!("Built-in uninstaller finished ({}): {}", r.exit_code.map_or("no exit code".into(), |c| format!("exit {c}")), r.command);
            for n in notes {
                println!("{n}");
            }
        }
        Err(e) => eprintln!("Built-in uninstaller could not run: {e:#}"),
    }
    if settings.uninstall_builtin_only {
        uninst::remove(settings, &p.name, &p.version, Operation::Uninstall, &[])?;
        println!("Leftover scan skipped (\"only run the built-in uninstaller\" is on).");
        return Ok(());
    }
    if uninst::still_installed(p)
        && std::io::stdin().is_terminal()
        && !a.yes
        && !confirm("The program still looks installed (its uninstaller may still be running). Scan for leftovers anyway?")?
    {
        return Ok(());
    }
    leftovers_step(settings, &target, &a, &p.name, &p.version, Operation::Uninstall)
}

/// `uninst::prepare` (restore point and backup). When the restore point fails, asks before
/// uninstalling without it, and stops when it can't ask (`--yes`, or no terminal).
fn prepare_uninstall(settings: &Settings, label: &str, yes: bool) -> Result<()> {
    use std::io::IsTerminal;
    let notes = match uninst::prepare(settings, label) {
        Ok(notes) => notes,
        Err(failed) => {
            eprintln!("Restore point not created: {}", failed.0);
            if yes || !std::io::stdin().is_terminal() || !confirm("Uninstall without a restore point?")? {
                bail!("stopped before uninstalling: no restore point (switch off uninstall_restore_point to uninstall without one)");
            }
            uninst::prepare_without_restore_point(settings, label)
        }
    };
    for n in notes {
        println!("{n}");
    }
    Ok(())
}

fn add_task(mut tasks: Vec<Task>, name: String, note: String, when: &str, action: Action) -> Result<()> {
    let task = Task { id: scheduler::make_id(&name, &tasks), name, note, schedule: scheduler::parse_when(when)?, action, created: chrono::Local::now(), last_run: None };
    if let Some(dup) = tasks.iter().find(|t| t.same_params(&task)) {
        bail!("A task with the same parameters already exists: '{}' ({})", dup.name, dup.id);
    }
    scheduler::register(&task)?;
    println!("Added '{}' ({}): {}, {}", task.name, task.id, task.action, task.schedule);
    println!("Next run: {}", scheduler::next_run(&task.schedule, chrono::Local::now()).format("%a %Y-%m-%d %H:%M"));
    tasks.push(task);
    scheduler::save_tasks(&tasks)
}

fn run_schedule(settings: &Settings, cmd: ScheduleCmd) -> Result<()> {
    let mut tasks = scheduler::load_tasks()?;
    let find =
        |tasks: &[Task], id: &str| -> Result<usize> { tasks.iter().position(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("no task '{id}' (see `cleancore schedule list`)")) };
    let exec = matches!(cmd, ScheduleCmd::Exec { .. });
    match cmd {
        ScheduleCmd::AddClean { name, note, when, mode, all_users } => {
            add_task(tasks, name, note, &when, Action::AutoClean { mode, all_users })?;
        }
        ScheduleCmd::AddBackup { name, note, when, key } => {
            let key = key.map(|k| if cfg!(windows) { watcher::canonical_key(&k) } else { k });
            add_task(tasks, name, note, &when, Action::AutoBackup { key })?;
        }
        ScheduleCmd::Edit { id, name, note, when, mode, all_users, key, whole } => {
            let i = find(&tasks, &id)?;
            let mut t = tasks[i].clone();
            if let Some(n) = name {
                t.name = n;
            }
            if let Some(n) = note {
                t.note = n;
            }
            if let Some(w) = when {
                t.schedule = scheduler::parse_when(&w)?;
            }
            match &mut t.action {
                Action::AutoClean { mode: m, all_users: a } => {
                    if key.is_some() || whole {
                        bail!("--key/--whole apply to Auto Backup tasks");
                    }
                    *m = mode.unwrap_or(*m);
                    *a = all_users.unwrap_or(*a);
                }
                Action::AutoBackup { key: k } => {
                    if mode.is_some() || all_users.is_some() {
                        bail!("--mode/--all-users apply to Auto Clean tasks");
                    }
                    if whole {
                        *k = None;
                    } else if let Some(nk) = key {
                        *k = Some(nk);
                    }
                }
                Action::RemoveSlop { .. } | Action::KeepAway => {
                    if mode.is_some() || all_users.is_some() || key.is_some() || whole {
                        bail!("the automatic app removal is set up on Tools › Microsoft slop");
                    }
                }
            }
            if let Some(dup) = tasks.iter().find(|o| o.id != t.id && o.same_params(&t)) {
                bail!("A task with the same parameters already exists: '{}' ({})", dup.name, dup.id);
            }
            scheduler::register(&t)?;
            println!("Updated '{}': {}, {}", t.name, t.action, t.schedule);
            tasks[i] = t;
            scheduler::save_tasks(&tasks)?;
        }
        ScheduleCmd::Remove { id } => {
            let i = find(&tasks, &id)?;
            scheduler::unregister(&id)?;
            let t = tasks.remove(i);
            scheduler::save_tasks(&tasks)?;
            println!("Removed '{}'", t.name);
        }
        ScheduleCmd::List => {
            if tasks.is_empty() {
                println!("No scheduled tasks. Add one with `cleancore schedule add-clean` or `add-backup`.");
            }
            let now = chrono::Local::now();
            for t in &tasks {
                println!("{}: {}", t.id, t.name);
                println!("    {}  |  {}", t.action, t.schedule);
                if !t.note.is_empty() {
                    println!("    note: {}", t.note);
                }
                println!("    next: {}   os: {}", scheduler::next_run(&t.schedule, now).format("%a %Y-%m-%d %H:%M"), platform::sched_status(&t.id));
                if let Some(r) = &t.last_run {
                    println!("    last: {} {}: {}", r.time.format("%Y-%m-%d %H:%M"), if r.ok { "ok" } else { "FAILED" }, r.summary);
                }
            }
            println!("recorder: {}", platform::recorder_status());
        }
        ScheduleCmd::Run { id } | ScheduleCmd::Exec { id } => {
            find(&tasks, &id)?;
            // The run's summary and notification are shown in the app: in its language.
            if exec {
                cleancore::i18n::set_language(&settings.language);
            }
            println!("{}", scheduler::execute(settings, &id)?);
        }
        ScheduleCmd::History { limit } => {
            let h = scheduler::history()?;
            for e in &h[h.len().saturating_sub(limit)..] {
                println!("{}  {:<6} {:<24} {}", e.time.format("%Y-%m-%d %H:%M:%S"), if e.ok { "ok" } else { "FAILED" }, e.name, e.summary);
            }
            if h.is_empty() {
                println!("No runs yet.");
            }
        }
        ScheduleCmd::Sync => {
            for t in &tasks {
                scheduler::register(t)?;
                println!("registered {}", t.id);
            }
        }
        ScheduleCmd::Recorder { action } => match action.as_str() {
            "enable" => {
                let (exe, args) = scheduler::exe_args(&["watch", "run", "--quiet"])?;
                platform::recorder_enable(&exe, &args)?;
                println!("The watch recorder now starts at login. {}", platform::recorder_status());
            }
            "disable" => {
                platform::recorder_disable()?;
                println!("The watch recorder no longer starts at login.");
            }
            "status" => println!("{}", platform::recorder_status()),
            other => bail!("unknown action '{other}' (enable | disable | status)"),
        },
    }
    Ok(())
}

fn watch_hint() {
    if !watcher::recorder_running() {
        println!("Note: the recorder is not running. Start it with `cleancore watch run`{}.", if cfg!(windows) { " (as administrator for process names)" } else { "" });
    }
}

fn get_watch(watches: &mut [Watch], path: &str) -> Result<Watch> {
    watcher::find(watches, path).map(|w| w.clone()).ok_or_else(|| anyhow::anyhow!("'{path}' is not in the watch list (see `cleancore watch list`)"))
}

fn print_event(e: &WatchEvent, highlight: Option<&str>) {
    use std::io::IsTerminal;
    let proc_ = match (e.process.as_str(), e.pid) {
        ("", None) => "-".to_string(),
        (p, Some(pid)) => format!("{p}({pid})"),
        (p, None) => p.to_string(),
    };
    let mut line = format!("{}  {:<22} {:<14} {}", e.time.format("%Y-%m-%d %H:%M:%S%.3f"), proc_, e.op.label(), e.key);
    if let Some(v) = &e.value {
        line.push_str(&format!(" | {}", if v.is_empty() { "(Default)" } else { v }));
    }
    match (&e.old_data, &e.new_data) {
        (Some(o), Some(n)) => line.push_str(&format!("  {o} → {n}")),
        (None, Some(n)) => line.push_str(&format!("  = {n}")),
        (Some(o), None) => line.push_str(&format!("  (was {o})")),
        (None, None) => {}
    }
    if let Some(t) = &e.data_type {
        line.push_str(&format!("  [{t}]"));
    }
    let hit = highlight.is_some_and(|h| e.haystack().contains(&h.to_lowercase()));
    if hit && std::io::stdout().is_terminal() {
        println!("\x1b[30;43m{line}\x1b[0m");
    } else if hit {
        println!("* {line}");
    } else {
        println!("{line}");
    }
}

fn export_path(out: &std::path::Path, format: Option<&str>) -> PathBuf {
    match format {
        Some(f) if out.extension().is_none_or(|e| !e.eq_ignore_ascii_case(f)) => {
            let mut s = out.as_os_str().to_owned();
            s.push(format!(".{f}"));
            PathBuf::from(s)
        }
        _ if out.extension().is_none() => out.with_extension("csv"),
        _ => out.to_path_buf(),
    }
}

fn run_watch(settings: &Settings, cmd: WatchCmd) -> Result<()> {
    let mut watches = watcher::load_watches()?;
    match cmd {
        WatchCmd::Start { path, reads } => {
            let norm = watcher::normalize_target(&path);
            if let Some(w) = watcher::find(&mut watches, &path) {
                w.active = true;
                w.include_reads |= reads;
                println!("Resumed watching {norm}");
            } else {
                if !norm.starts_with("dconf:") && !cfg!(windows) && !std::path::Path::new(&norm).exists() {
                    bail!("{norm} does not exist");
                }
                watches.push(Watch { path: norm.clone(), active: true, include_reads: reads, created: chrono::Local::now() });
                println!("Watching {norm}");
            }
            watcher::save_watches(&watches)?;
            watch_hint();
        }
        WatchCmd::Stop { path } => {
            let w = watcher::find(&mut watches, &path).ok_or_else(|| anyhow::anyhow!("'{path}' is not in the watch list"))?;
            w.active = false;
            println!("Paused {}", w.path);
            watcher::save_watches(&watches)?;
        }
        WatchCmd::Del { path, export } => {
            let w = get_watch(&mut watches, &path)?;
            if let Some(out) = export {
                watcher::export_events(&watcher::read_log(&w)?, &out)?;
                println!("Exported to {}", out.display());
            }
            watcher::clear_log(&w)?;
            watches.retain(|x| x.path != w.path);
            watcher::save_watches(&watches)?;
            println!("Deleted watch {}", w.path);
        }
        WatchCmd::List => {
            for w in &watches {
                let n = watcher::read_log(w)?.len();
                let size = std::fs::metadata(w.log_file()).map(|m| m.len()).unwrap_or(0);
                println!(
                    "[{}] {:<60} {:>7} events {:>9}{}",
                    if w.active { "watching" } else { " paused " },
                    w.path,
                    n,
                    human(size),
                    if w.include_reads { "  (+reads)" } else { "" }
                );
            }
            println!(
                "{} watch(es); logs {} of {} MB limit; recorder {}",
                watches.len(),
                human(watcher::logs_size()),
                settings.watcher_max_log_mb,
                if watcher::recorder_running() { "running" } else { "NOT running (`cleancore watch run`)" }
            );
        }
        WatchCmd::Log { path, filter, highlight, limit, follow } => {
            let selected: Vec<Watch> = match &path {
                Some(p) => vec![get_watch(&mut watches, p)?],
                None => watches.clone(),
            };
            let keep = |e: &WatchEvent| filter.as_ref().is_none_or(|f| e.haystack().contains(&f.to_lowercase()));
            let mut events: Vec<WatchEvent> = selected.iter().flat_map(|w| watcher::read_log(w).unwrap_or_default()).filter(|e| keep(e)).collect();
            events.sort_by_key(|e| e.time);
            let skip = events.len().saturating_sub(limit);
            for e in &events[skip..] {
                print_event(e, highlight.as_deref());
            }
            if !follow {
                if events.is_empty() {
                    println!("No events recorded.");
                    watch_hint();
                }
                return Ok(());
            }
            watch_hint();
            // Tail the log files.
            let mut offsets: Vec<u64> = selected.iter().map(|w| std::fs::metadata(w.log_file()).map(|m| m.len()).unwrap_or(0)).collect();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(300));
                for (w, off) in selected.iter().zip(offsets.iter_mut()) {
                    let Ok(mut f) = std::fs::File::open(w.log_file()) else {
                        continue;
                    };
                    let len = f.metadata()?.len();
                    if len < *off {
                        *off = 0; // log was cleared
                    }
                    if len == *off {
                        continue;
                    }
                    use std::io::{Read, Seek};
                    f.seek(std::io::SeekFrom::Start(*off))?;
                    let mut buf = String::new();
                    f.read_to_string(&mut buf)?;
                    // Only consume complete lines.
                    let complete = buf.rfind('\n').map_or(0, |i| i + 1);
                    for l in buf[..complete].lines() {
                        if let Ok(e) = serde_json::from_str::<WatchEvent>(l) {
                            if keep(&e) {
                                print_event(&e, highlight.as_deref());
                            }
                        }
                    }
                    *off += complete as u64;
                }
            }
        }
        WatchCmd::Export { args, all, format } => {
            let format = format.map(|f| f.to_ascii_lowercase());
            let (path, out) = match (all, args.as_slice()) {
                (true, [out]) => (None, PathBuf::from(out)),
                (false, [key, out]) => (Some(key.clone()), PathBuf::from(out)),
                (true, _) => bail!("with --all give only the output folder"),
                (false, _) => bail!("give the key/path and the output file"),
            };
            if all {
                cleancore::config::ensure_dir(&out)?;
                let ext = format.unwrap_or_else(|| "csv".into());
                for w in &watches {
                    let file = out.join(format!("{}.{ext}", w.log_file().file_stem().unwrap().to_string_lossy()));
                    watcher::export_events(&watcher::read_log(w)?, &file)?;
                    println!("{} → {}", w.path, file.display());
                }
            } else {
                let w = get_watch(&mut watches, path.as_deref().unwrap_or_default())?;
                let file = export_path(&out, format.as_deref());
                let events = watcher::read_log(&w)?;
                watcher::export_events(&events, &file)?;
                println!("Exported {} event(s) to {}", events.len(), file.display());
            }
        }
        WatchCmd::Clear { path } => {
            let w = get_watch(&mut watches, &path)?;
            watcher::clear_log(&w)?;
            println!("Cleared log of {}", w.path);
        }
        WatchCmd::Run { quiet } => {
            if watcher::recorder_running() {
                bail!("a recorder is already running");
            }
            watcher::run_recorder(settings, |e| {
                if !quiet {
                    print_event(e, None);
                }
            })?;
        }
    }
    Ok(())
}

fn find_tweak<'a>(tweaks: &'a [tuner::Tweak], id: &str) -> Result<&'a tuner::Tweak> {
    tweaks.iter().find(|t| t.id.eq_ignore_ascii_case(id)).ok_or_else(|| anyhow::anyhow!("no tweak '{id}' (see `cleancore tune list`)"))
}

fn print_requires(r: Requires) {
    if let Some(m) = r.message() {
        println!("{m}");
    }
}

fn run_tune(cmd: TuneCmd) -> Result<()> {
    let tweaks = tuner::tweaks();
    let mut state = tuner::State::load()?;
    match cmd {
        TuneCmd::List { category, search, changed, json } => {
            let needle = search.map(|s| s.to_lowercase());
            let shown: Vec<&tuner::Tweak> = tweaks
                .iter()
                .filter(|t| category.as_ref().is_none_or(|c| t.category.eq_ignore_ascii_case(c)))
                .filter(|t| needle.as_ref().is_none_or(|n| [&t.id, &t.name, &t.description].iter().any(|f| f.to_lowercase().contains(n.as_str()))))
                .filter(|t| !changed || state.applied.contains_key(&t.id))
                .collect();
            let owned: Vec<tuner::Tweak> = shown.iter().map(|t| (*t).clone()).collect();
            let statuses = tuner::status_all(&owned, &state);
            if json {
                println!("{}", serde_json::to_string_pretty(&statuses)?);
                return Ok(());
            }
            for (cat, label) in tuner::CATEGORIES {
                let rows: Vec<_> = owned.iter().zip(&statuses).filter(|(t, _)| t.category == cat).collect();
                if rows.is_empty() {
                    continue;
                }
                println!("\n{label}");
                for (t, st) in rows {
                    let mark = match (&st.available, &st.current) {
                        (Err(_), _) => "n/a".to_string(),
                        (Ok(()), Some(_)) if t.is_toggle() => "on".to_string(),
                        (Ok(()), Some(o)) => o.clone(),
                        (Ok(()), None) => String::new(),
                    };
                    let mut extra = String::new();
                    if st.ours.is_some() {
                        extra.push_str("  · by CleanCore");
                    }
                    if let Err(why) = &st.available {
                        extra.push_str(&format!("  · {why}"));
                    } else if t.requires != Requires::None {
                        extra.push_str(&format!("  · needs {:?}", t.requires).to_lowercase());
                    }
                    println!("  [{mark:^5}] {:<28} {}{extra}", t.id, t.name);
                }
            }
            let pending = statuses.iter().filter(|s| s.ours.is_some()).count();
            println!("\n{} tweak(s), {} changed by CleanCore. `cleancore tune show <id>` for details.", owned.len(), pending);
        }
        TuneCmd::Show { id } => {
            let t = find_tweak(&tweaks, &id)?;
            let st = tuner::status_all(std::slice::from_ref(t), &state).remove(0);
            println!("{} ({})", t.name, t.id);
            if !t.description.is_empty() {
                println!("  {}", t.description);
            }
            if let Some(w) = &t.warning {
                println!("  warning: {w}");
            }
            if let Some(m) = t.requires.message() {
                println!("  after changing: {m}");
            }
            match &st.available {
                Ok(()) => println!("  current: {}", st.current.as_deref().unwrap_or("default / other")),
                Err(why) => println!("  not available: {why}"),
            }
            if let Some(a) = state.applied.get(&t.id) {
                println!("  changed by CleanCore on {} (option '{}')", a.applied_at.format("%Y-%m-%d %H:%M"), a.option);
            }
            for o in &t.options {
                println!("\n  option '{}'{}", o.id, if o.label.is_empty() { String::new() } else { format!(": {}", o.label) });
                for s in &o.set {
                    match s {
                        tuner::Setting::Reg { key, name, data } => println!("    set    {key} | {} = {data:?}", if name.is_empty() { "(Default)" } else { name }),
                        tuner::Setting::RegDelete { key, name } => {
                            println!("    delete {key} | {name}")
                        }
                        tuner::Setting::Gsettings { schema, key, value } => {
                            println!("    gsettings set {schema} {key} {value}")
                        }
                    }
                }
            }
        }
        TuneCmd::Set { id, option, yes } => {
            let t = find_tweak(&tweaks, &id)?;
            let option = option.unwrap_or_else(|| t.options[0].id.clone());
            if matches!(option.as_str(), "off" | "default") {
                print_requires(tuner::revert(t, &mut state)?);
                println!("Reverted '{}'.", t.id);
                return Ok(());
            }
            if let Some(w) = &t.warning {
                if !yes && !confirm(&format!("{w} Continue?"))? {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            let r = tuner::apply(t, &option, &mut state)?;
            println!("Applied '{}' = {option}.", t.id);
            print_requires(r);
        }
        TuneCmd::Revert { id, all } => {
            let targets: Vec<&tuner::Tweak> = if all {
                tweaks.iter().filter(|t| state.applied.contains_key(&t.id)).collect()
            } else {
                vec![find_tweak(&tweaks, &id.ok_or_else(|| anyhow::anyhow!("give a tweak id or --all"))?)?]
            };
            let mut need = Requires::None;
            let mut failed = 0;
            for t in &targets {
                match tuner::revert(t, &mut state) {
                    Ok(r) => {
                        need = need.max(r);
                        println!("Reverted '{}'.", t.id);
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("failed to revert '{}': {e:#}", t.id);
                    }
                }
            }
            print_requires(need);
            if failed > 0 {
                bail!("{failed} tweak(s) could not be reverted");
            }
        }
        TuneCmd::RestartExplorer => platform::restart_shell()?,
    }
    Ok(())
}

use cleancore::backup::human_size as human;

fn run_update(check_only: bool) -> Result<()> {
    use cleancore::update;
    println!("CleanCore {}", update::CURRENT);
    let Some(r) = update::check()? else {
        println!("You have the latest version.");
        return Ok(());
    };
    println!("Version {} is available: {}", r.version, r.url);
    if check_only {
        return Ok(());
    }
    println!("Downloading and verifying the signature and checksum…");
    let file = update::download(&r)?;
    println!("Verified {}", file.display());
    println!("{}", update::install(&file)?.message());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::translate_legacy;

    fn t(a: &[&str]) -> Vec<String> {
        translate_legacy(std::iter::once("cleancore").chain(a.iter().copied()).map(String::from).collect())[1..].to_vec()
    }

    #[test]
    fn slash_syntax_for_programs() {
        assert_eq!(t(&["/m", "Microso*", "/i", "/u"]), ["programs", "--match", "Microso*", "--location", "--uninstall-cmd"]);
        // Any order, and without /m: every program.
        assert_eq!(t(&["/U", "/t", "/m", "7-Zip*", "/p"]), ["programs", "--uninstall-cmd", "--table", "--match", "7-Zip*"]);
        assert_eq!(t(&["/i"]), ["programs", "--location"]);
        assert_eq!(t(&["/winapps", "/i"]), ["programs", "--store", "--location"]);
        assert_eq!(t(&["/?"]), ["--help"]);
        // The rest is left alone.
        assert_eq!(t(&["programs", "--table"]), ["programs", "--table"]);
    }
}
