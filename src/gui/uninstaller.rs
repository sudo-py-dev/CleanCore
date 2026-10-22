//! Uninstaller page (Revo Uninstaller Pro): All Programs / Store apps / History, the uninstall
//! wizard (built-in uninstaller → scan mode → leftovers → summary), Quick / Multiple uninstall
//! queue and Forced Uninstall.

use super::style::{self, icon, Palette};
use super::traced::{TraceScreen, TracedAction, TracedTab};
use super::{take, tree::Browser, Cx, Pending};
use crate::cleaner::CleanReport;
use crate::gui::style::ColumnsInOrder;
use crate::i18n::tr;
use crate::model::{Issue, ScanMode};
use crate::uninstaller::{self as uninst, HistoryEntry, Meta, Operation, Program, ProgramKind, RunMode, RunResult, Target};
use eframe::egui::{self, Color32, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    All,
    Store,
    Traced,
    History,
    Extensions,
    LogsDb,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Step {
    Running,
    ChooseMode,
    Scanning,
    Leftovers,
    Removing,
    Done,
}

struct Wizard {
    program: Option<Program>,
    label: String,
    target: Target,
    op: Operation,
    step: Step,
    run: Option<Pending<Result<(Vec<String>, Option<RunResult>), String>>>,
    run_info: Vec<String>,
    mode: ScanMode,
    all_users: bool,
    scan: Option<Pending<Vec<Issue>>>,
    leftovers: Vec<Issue>,
    checked: Vec<bool>,
    removing: Option<Pending<Result<CleanReport, String>>>,
    summary: Option<(usize, String, Vec<(String, String)>)>,
    /// Uninstalling with a trace log (deleted afterwards if so configured).
    log_id: Option<String>,
    /// The Advanced Uninstall follow-up scan was already offered and run.
    advanced_done: bool,
    /// Leftovers review: only rows whose location or detail contains this.
    filter: String,
}

#[derive(Clone)]
enum QState {
    Waiting,
    Running,
    Done(uninst::QuickReport),
    Failed(String),
}

struct QItem {
    prog: Program,
    state: QState,
    /// Seconds it took (set when it finishes).
    secs: f32,
}

struct Queue {
    items: Vec<QItem>,
    job: Option<Pending<Result<uninst::QuickReport, String>>>,
    prepared: Option<Pending<Vec<String>>>,
    notes: Vec<String>,
    started: std::time::Instant,
    /// Total seconds, once every program is done.
    took: Option<f32>,
}

struct Forced {
    names: String,
    paths: String,
    browser: Browser,
    found: Option<Vec<Program>>,
    use_uninstaller: Option<usize>,
    logs: Vec<crate::tracer::TraceLog>,
    /// For logs found in the logs database: the file to import before using it.
    log_files: Vec<Option<std::path::PathBuf>>,
    use_log: Option<usize>,
}

enum Screen {
    List,
    Wizard(Box<Wizard>),
    Queue(Queue),
    Forced(Box<Forced>),
    Trace(Box<TraceScreen>),
}

pub struct UninstallerView {
    tab: Tab,
    screen: Screen,
    programs: Vec<Program>,
    /// Lowercase search text of each program (same order), built when the list changes so
    /// filtering doesn't allocate per frame.
    haystacks: Vec<String>,
    loading: Option<Pending<Vec<Program>>>,
    loaded: bool,
    meta: Meta,
    search: String,
    group: Option<String>,
    checked: BTreeSet<String>,
    history: Vec<HistoryEntry>,
    history_sel: Option<usize>,
    export_path: String,
    comment_edit: Option<(String, String)>,
    new_group: String,
    traced: TracedTab,
    /// The programs' own icons, loaded in the background.
    app_icons: super::appicons::IconCache,
    /// "Uninstall with CleanCore": the clicked path being resolved to a program.
    from_path: Option<Pending<(Option<Program>, String, String)>>,
    groups_dlg: Option<GroupsDialog>,
    /// The interrupted uninstall the user dismissed this session.
    dismissed_pending: Option<chrono::DateTime<chrono::Local>>,
    extensions: super::extensions::ExtensionsTab,
    logsdb: super::logsdb::LogsDbTab,
}

impl UninstallerView {
    pub fn new() -> Self {
        Self {
            tab: Tab::All,
            screen: Screen::List,
            programs: Vec::new(),
            haystacks: Vec::new(),
            loading: None,
            loaded: false,
            meta: Meta::load(),
            search: String::new(),
            group: None,
            checked: BTreeSet::new(),
            history: Vec::new(),
            history_sel: None,
            export_path: super::export_dir().join("installed-programs.html").display().to_string(),
            comment_edit: None,
            new_group: String::new(),
            traced: TracedTab::new(),
            app_icons: Default::default(),
            from_path: None,
            groups_dlg: None,
            dismissed_pending: None,
            extensions: super::extensions::ExtensionsTab::new(),
            logsdb: super::logsdb::LogsDbTab::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
    }

    fn load(&mut self, ctx: &egui::Context, cx: &Cx) {
        if self.loading.is_none() {
            let s = cx.settings.clone();
            self.loading = Some(Pending::spawn(ctx, "Reading installed programs", move || uninst::list_programs(&s)));
        }
        self.loaded = true;
        self.history = uninst::history();
        self.meta = Meta::load();
        self.traced.refresh();
        self.logsdb.refresh();
    }

    /// Indexes (into `programs`) of the rows the tab, search and group show.
    fn visible(&self, cx: &Cx) -> Vec<usize> {
        let q = self.search.to_lowercase();
        self.programs
            .iter()
            .enumerate()
            .filter(|(_, p)| match self.tab {
                Tab::Store => p.kind.is_store_app(),
                // All programs includes apps from the Microsoft Store (also on their own tab).
                _ => true,
            })
            .filter(|(i, _)| q.is_empty() || self.haystacks[*i].contains(&q))
            .filter(|(_, p)| match self.group.as_deref() {
                None => true,
                Some("New") => uninst::is_new(p, cx.settings.new_program_days),
                Some(g) => self.meta.group_of(&p.id) == Some(g),
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn set_programs(&mut self, list: Vec<Program>) {
        self.haystacks = list.iter().map(Program::haystack).collect();
        self.programs = list;
    }

    // ------------------------------------------------------------ actions

    fn start_wizard(&mut self, ctx: &egui::Context, cx: &Cx, p: Program, run_mode: RunMode) {
        let settings = cx.settings.clone();
        let prog = p.clone();
        let has_uninstaller = p.uninstall_string.is_some() || run_mode == RunMode::ForcedMsi;
        let run = Pending::spawn(ctx, crate::tf!("Uninstalling {}", p.name), move || {
            let mut notes = uninst::prepare(&settings, &prog.name);
            if !has_uninstaller {
                return Ok((notes, None));
            }
            let (more, r) = uninst::run_with_options(&settings, &prog, run_mode, Operation::Uninstall).map_err(|e| crate::tf!("{e:#}"))?;
            notes.extend(more);
            Ok((notes, Some(r)))
        });
        self.screen = Screen::Wizard(Box::new(Wizard {
            label: p.name.clone(),
            target: Target::from_program(&p),
            program: Some(p),
            op: Operation::Uninstall,
            step: Step::Running,
            run: Some(run),
            run_info: Vec::new(),
            mode: cx.settings.uninstall_scan_mode,
            all_users: false,
            scan: None,
            leftovers: Vec::new(),
            checked: Vec::new(),
            removing: None,
            summary: None,
            log_id: None,
            advanced_done: false,
            filter: String::new(),
        }));
    }

    /// Uninstall with a trace log: straight away (Uninstall) or after review (Custom Uninstall).
    fn start_log(&mut self, ctx: &egui::Context, cx: &Cx, log: crate::tracer::TraceLog, custom: bool) {
        let items = crate::tracer::to_issues(&log);
        let mut w = Wizard {
            program: None,
            label: log.name.clone(),
            target: log.target(),
            op: Operation::TracedUninstall,
            step: if custom { Step::Leftovers } else { Step::Removing },
            run: None,
            run_info: Vec::new(),
            mode: ScanMode::Moderate,
            all_users: false,
            scan: None,
            checked: vec![true; items.len()],
            leftovers: items.clone(),
            removing: None,
            summary: None,
            log_id: Some(log.id.clone()),
            advanced_done: false,
            filter: String::new(),
        };
        if !custom {
            let (s, name) = (cx.settings.clone(), log.name.clone());
            w.removing = Some(Pending::spawn(ctx, "Removing", move || {
                let _ = uninst::prepare(&s, &name);
                uninst::remove(&s, &name, "", Operation::TracedUninstall, &items).map_err(|e| crate::tf!("{e:#}"))
            }));
        }
        self.screen = Screen::Wizard(Box::new(w));
    }

    fn start_queue(&mut self, ctx: &egui::Context, cx: &Cx, items: Vec<Program>) {
        let settings = cx.settings.clone();
        let label = crate::tf!("{} program(s)", items.len());
        let prepared = Pending::spawn(ctx, "Creating restore point and backup", move || uninst::prepare(&settings, &label));
        self.screen = Screen::Queue(Queue {
            items: items.into_iter().map(|prog| QItem { prog, state: QState::Waiting, secs: 0.0 }).collect(),
            job: None,
            prepared: Some(prepared),
            notes: Vec::new(),
            started: std::time::Instant::now(),
            took: None,
        });
    }

    /// "Install with CleanCore" from the file manager.
    pub fn trace_installer(&mut self, ctx: &egui::Context, cx: &Cx, installer: &str) {
        self.screen = Screen::Trace(Box::new(TraceScreen::with_installer(ctx, cx, installer)));
    }

    /// "Uninstall with CleanCore" from the file manager: find the program the clicked file
    /// (program, shortcut, launcher or folder) belongs to, then uninstall it or, when it isn't
    /// listed, open Forced Uninstall with its name and folder.
    pub fn uninstall_path(&mut self, ctx: &egui::Context, cx: &Cx, path: &str) {
        let (s, path) = (cx.settings.clone(), std::path::PathBuf::from(path));
        self.from_path = Some(Pending::spawn(ctx, "Finding the program", move || {
            let target = crate::shell::resolve_target(&path);
            let mut s = s;
            s.show_system_components = true;
            let programs = uninst::list_programs(&s);
            let owner = crate::hunter::owning_program(&target.display().to_string(), &programs);
            let folder = if target.is_dir() { target.clone() } else { target.parent().map(|p| p.to_path_buf()).unwrap_or(target.clone()) };
            let name = target.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (owner, name, folder.display().to_string())
        }));
    }

    /// Hunter Mode › Uninstall on a listed program.
    pub fn uninstall_from_hunter(&mut self, ctx: &egui::Context, cx: &Cx, p: Program) {
        self.start_wizard(ctx, cx, p, RunMode::Normal);
    }

    /// Hunter Mode › Uninstall on a program that isn't listed: Forced Uninstall, filled in.
    pub fn forced_from_hunter(&mut self, name: String, folder: String) {
        self.screen = Screen::Forced(Box::new(Forced {
            names: name,
            paths: folder,
            browser: Browser::default(),
            found: None,
            use_uninstaller: None,
            logs: Vec::new(),
            log_files: Vec::new(),
            use_log: None,
        }));
    }

    fn open_url(cx: &mut Cx, url: &str) {
        if let Err(e) = crate::platform::open_folder(std::path::Path::new(url)) {
            cx.error(crate::tf!("{e:#}"));
        }
    }

    // ------------------------------------------------------------ page

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        self.app_icons.poll(ui.ctx());
        if !self.loaded {
            self.load(ui.ctx(), cx);
        }
        if let Some(list) = take(&mut self.loading) {
            self.set_programs(list);
            self.app_icons.load(ui.ctx(), &self.programs);
            let ids: BTreeSet<&String> = self.programs.iter().map(|p| &p.id).collect();
            self.checked.retain(|i| ids.contains(i));
        }
        if let Some((owner, name, folder)) = take(&mut self.from_path) {
            match owner {
                Some(p) => self.start_wizard(ui.ctx(), cx, p, RunMode::Normal),
                None => {
                    cx.info(crate::tf!("\"{name}\" isn't in the list of installed programs: Forced Uninstall finds what it left."));
                    self.forced_from_hunter(name, folder);
                }
            }
        }
        if let Some(f) = &self.from_path {
            f.show_progress(ui);
            return;
        }
        let mut screen = std::mem::replace(&mut self.screen, Screen::List);
        let keep = match &mut screen {
            Screen::List => {
                self.list_page(ui, cx);
                true
            }
            Screen::Wizard(w) => self.wizard_page(ui, cx, w),
            Screen::Queue(q) => self.queue_page(ui, cx, q),
            Screen::Forced(f) => self.forced_page(ui, cx, f),
            Screen::Trace(t) => {
                let keep = t.ui(ui, cx);
                if !keep {
                    self.tab = Tab::Traced;
                }
                keep
            }
        };
        // A page may have switched to another screen while running; only restore ours if not.
        if keep && matches!(self.screen, Screen::List) {
            self.screen = screen;
        } else if !keep {
            self.loaded = false;
        }
    }

    fn list_page(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        let total: u64 = self.programs.iter().filter_map(|p| p.size).sum();
        let subtitle = if self.loading.is_some() {
            "Reading installed programs…".to_string()
        } else {
            crate::tf!("{} programs · {}", self.programs.len(), crate::backup::human_size(total))
        };
        let (mut forced, mut refresh, mut trace) = (false, false, false);
        style::page_header(ui, "Uninstaller", &subtitle, |ui| {
            forced = style::secondary(ui, icon::CROSSHAIR, "Forced Uninstall").on_hover_text(style::vis(tr("Remove a program that isn't listed or was partly removed"))).clicked();
            if style::secondary(ui, icon::RECORD, "Install & trace").on_hover_text(style::vis(tr("Record an installation so it can be undone exactly"))).clicked() {
                trace = true;
            }
            refresh = style::icon_button(ui, icon::REFRESH, "Refresh").clicked();
            let r = style::icon_button(ui, icon::EXPORT, "Export list");
            let shown = self.visible(cx);
            let programs = &self.programs;
            super::export_menu(&r, cx, &mut self.export_path, "File (.txt .html .csv .json)", |f| {
                let list: Vec<Program> = shown.iter().map(|i| programs[*i].clone()).collect();
                uninst::export(&list, f)?;
                Ok(crate::tf!("Exported {} programs to {}", list.len(), f.display()))
            });
        });
        if forced {
            self.screen = Screen::Forced(Box::new(Forced {
                names: String::new(),
                paths: String::new(),
                browser: Browser::default(),
                found: None,
                use_uninstaller: None,
                logs: Vec::new(),
                log_files: Vec::new(),
                use_log: None,
            }));
            return;
        }
        if trace {
            self.screen = Screen::Trace(Box::new(TraceScreen::new(ui.ctx(), cx)));
            return;
        }
        if refresh {
            self.loaded = false;
        }

        // An uninstall interrupted by a restart (or by closing CleanCore) can be continued.
        if let Some(pend) = uninst::pending().filter(|p| self.dismissed_pending != Some(p.started)) {
            let (mut resume, mut dismiss) = (false, false);
            egui::Frame::new().fill(p.accent_soft).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::rich(icon::INFO).size(18.0).color(p.accent)));
                    crate::gui::style::column(ui, |ui| {
                        ui.add(style::Label::new(style::strong(crate::tf!("Finish uninstalling {}", pend.label))));
                        let why = if pend.reboot_needed { tr("Its uninstaller asked for a restart") } else { tr("The uninstall was interrupted") };
                        ui.add(style::Label::new(
                            style::muted(ui, crate::tf!("{why} ({}). Scan for the leftovers it left behind.", pend.started.format("%d %b %H:%M"))).size(12.5),
                        ));
                    });
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        dismiss = style::ghost(ui, icon::X, "Dismiss").clicked();
                        resume = style::primary(ui, icon::SEARCH, "Scan for leftovers").clicked();
                    });
                });
            });
            ui.add_space(10.0);
            if dismiss {
                uninst::clear_pending();
                self.dismissed_pending = Some(pend.started);
            }
            if resume {
                self.screen = Screen::Wizard(Box::new(Wizard {
                    label: pend.label.clone(),
                    target: pend.target.clone(),
                    program: pend.program.clone(),
                    op: pend.op,
                    step: Step::ChooseMode,
                    run: None,
                    run_info: vec![crate::tf!("Continuing the uninstall started {}.", pend.started.format("%d %b %Y %H:%M"))],
                    mode: cx.settings.uninstall_scan_mode,
                    all_users: false,
                    scan: None,
                    leftovers: Vec::new(),
                    checked: Vec::new(),
                    removing: None,
                    summary: None,
                    log_id: None,
                    advanced_done: false,
                    filter: String::new(),
                }));
                return;
            }
        }

        // Tabs
        let store_n = self.programs.iter().filter(|p| p.kind.is_store_app()).count();
        let store_label = if cfg!(windows) { tr("Windows Apps") } else { tr("Flatpak & Snap") };
        let (n_logs, n_ext, n_db) = (self.traced.count(), self.extensions.count(), self.logsdb.count(cx));
        let n_hist = self.history.len();
        if style::tabs(
            ui,
            &mut self.tab,
            &[
                (Tab::All, icon::APP_WINDOW, "All programs", Some(self.programs.len())),
                (Tab::Store, icon::STOREFRONT, store_label, Some(store_n)),
                (Tab::Traced, icon::RECORD, "Traced programs", Some(n_logs)),
                (Tab::Extensions, icon::GLOBE, "Browser extensions", Some(n_ext)),
                (Tab::LogsDb, icon::DATABASE, "Logs database", Some(n_db)),
                (Tab::History, icon::HISTORY, "History", Some(n_hist)),
            ],
        ) {
            match self.tab {
                Tab::Traced => self.traced.refresh(),
                Tab::History => self.history = uninst::history(),
                Tab::Extensions => self.extensions.refresh(),
                Tab::LogsDb => self.logsdb.refresh(),
                _ => {}
            }
        }
        if self.tab == Tab::History {
            self.history_ui(ui, p);
            return;
        }
        if self.tab == Tab::Extensions {
            self.extensions.ui(ui, cx);
            return;
        }
        if matches!(self.tab, Tab::Traced | Tab::LogsDb) {
            let action = if self.tab == Tab::Traced { self.traced.ui(ui, cx) } else { self.logsdb.ui(ui, cx) };
            match action {
                Some(TracedAction::Uninstall(l)) => self.start_log(ui.ctx(), cx, l, false),
                Some(TracedAction::Custom(l)) => self.start_log(ui.ctx(), cx, l, true),
                None => {}
            }
            return;
        }

        // Toolbar: search, group and filters on the left; actions for the selection on the right.
        let visible: Vec<usize> = self.visible(cx);
        let n_checked = self.checked.len();
        // Only ticked programs are ever uninstalled: the single-program actions apply to the one
        // ticked program, and are off when none is.
        let ticked_one = match n_checked {
            1 => self.programs.iter().find(|x| self.checked.contains(&x.id)).cloned(),
            _ => None,
        };
        let mut start: Option<(Program, RunMode)> = None;
        let mut queue: Option<Vec<Program>> = None;
        // Narrow windows: shorter search and group fields, Quick uninstall as an icon.
        let narrow = ui.available_width() < 900.0;
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut self.search, if narrow { tr("Search programs") } else { tr("Search name, publisher or comment") }, if narrow { 150.0 } else { 240.0 });
            let new_n = self.programs.iter().filter(|x| uninst::is_new(x, cx.settings.new_program_days)).count();
            let current = match self.group.as_deref() {
                None => tr("All groups").to_string(),
                Some("New") => crate::tf!("New ({new_n})"),
                Some(g) => g.to_string(),
            };
            let mut manage = false;
            style::dropdown(ui, "program-group", &crate::tf!("{}  {current}", icon::TAG), if narrow { 130.0 } else { 170.0 }, |ui| {
                ui.selectable_value(&mut self.group, None, style::vis(tr("All groups")));
                ui.selectable_value(&mut self.group, Some("New".into()), style::vis(crate::tf!("New ({new_n})")));
                for (g, ids) in self.meta.groups.clone() {
                    ui.selectable_value(&mut self.group, Some(g.clone()), style::vis(crate::tf!("{g} ({})", ids.len())));
                }
                ui.separator();
                manage = ui.button(style::vis(crate::tf!("{}  Manage groups…", icon::GEAR))).clicked();
            });
            if manage {
                self.groups_dlg = Some(GroupsDialog::default());
            }
            let r = style::icon_button(ui, icon::FUNNEL, "Filters");
            egui::Popup::menu(&r).width(260.0).show(|ui| {
                crate::gui::style::row(ui, |ui| {
                    if style::toggle(ui, &mut cx.settings.show_system_components, "Show system components").changed() {
                        cx.save_settings();
                        self.loaded = false;
                    }
                    ui.add(style::Label::new(tr("Show system components")));
                });
                ui.add(style::Label::new(
                    style::muted(ui, if cfg!(windows) { tr("Updates, runtimes and Windows parts.") } else { tr("Libraries and packages without an app.") }).size(12.0),
                ));
            });
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if n_checked > 1 {
                    if style::primary(ui, icon::LIGHTNING, &crate::tf!("Quick uninstall ({n_checked})")).clicked() {
                        queue = Some(self.programs.iter().filter(|x| self.checked.contains(&x.id)).cloned().collect());
                    }
                    if style::ghost(ui, icon::X, "Clear selection").clicked() {
                        self.checked.clear();
                    }
                } else {
                    ui.add_enabled_ui(ticked_one.is_some(), |ui| {
                        if style::primary(ui, icon::TRASH, "Uninstall").on_hover_text(style::vis(tr("Run its uninstaller, then scan for leftovers"))).clicked() {
                            start = ticked_one.clone().map(|f| (f, RunMode::Normal));
                        }
                        let r = if narrow {
                            let r = style::secondary(ui, icon::LIGHTNING, "");
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tr("Quick uninstall")));
                            r
                        } else {
                            style::secondary(ui, icon::LIGHTNING, "Quick uninstall")
                        };
                        if r.on_hover_text(style::vis(tr("Quick uninstall: silent uninstall and automatic leftover removal"))).clicked() {
                            queue = ticked_one.clone().map(|f| vec![f]);
                        }
                    });
                }
            });
        });
        ui.add_space(10.0);

        // Before the list (which may still be loading): toolbar clicks are never lost.
        if let Some((prog, mode)) = start {
            self.start_wizard(ui.ctx(), cx, prog, mode);
            return;
        }
        if let Some(items) = queue {
            self.start_queue(ui.ctx(), cx, items);
            return;
        }
        if let Some(pend) = &self.loading {
            style::card(ui, |ui| {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(32.0).color(p.accent));
                    ui.add(style::Label::new(style::muted(ui, &pend.label)));
                });
                ui.add_space(24.0);
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }

        let mut action: Option<RowAction> = None;
        style::card(ui, |ui| {
            if visible.is_empty() {
                style::empty_state(ui, icon::SEARCH, "No programs match", "Try another search or group.");
                return;
            }
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .cell_layout(crate::gui::style::near_layout(egui::Align::Center))
                .auto_shrink([false, false])
                .columns_in_order(vec![
                    Column::exact(28.0),
                    Column::exact(34.0),
                    Column::remainder().at_least(if narrow { 150.0 } else { 220.0 }).clip(true),
                    Column::exact(if narrow { 96.0 } else { 130.0 }).clip(true),
                    Column::exact(if narrow { 76.0 } else { 90.0 }),
                    Column::exact(if narrow { 92.0 } else { 100.0 }),
                    Column::exact(if narrow { 72.0 } else { 90.0 }),
                ])
                .header(26.0, |mut h| {
                    for t in crate::gui::style::ordered(vec!["", "", "Name", "Version", "Size", "Installed", "Type"]) {
                        h.col(|ui| {
                            ui.add(style::Label::new(style::rich(tr(t)).size(12.5).color(p.muted)));
                        });
                    }
                })
                .body(|body| {
                    body.rows(44.0, visible.len(), |mut row| {
                        let prog = &self.programs[visible[row.index()]];
                        let ticked = self.checked.contains(&prog.id);
                        row.set_selected(ticked);
                        crate::gui::style::cells_in_order(
                            &mut row,
                            vec![
                                Box::new(|ui: &mut Ui| {
                                    let mut c = self.checked.contains(&prog.id);
                                    if ui.checkbox(&mut c, style::vis("")).changed() {
                                        if c {
                                            self.checked.insert(prog.id.clone());
                                        } else {
                                            self.checked.remove(&prog.id);
                                        }
                                    }
                                }),
                                Box::new(|ui: &mut Ui| {
                                    program_icon(ui, &self.app_icons, &self.meta, prog, 22.0, p.accent);
                                }),
                                Box::new(|ui: &mut Ui| {
                                    crate::gui::style::column(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.spacing_mut().interact_size.y = 18.0;
                                        crate::gui::style::row(ui, |ui| {
                                            ui.add(style::Label::new(style::strong(&prog.name)).truncate());
                                            if uninst::is_new(prog, cx.settings.new_program_days) {
                                                style::badge(ui, "New", p.success);
                                            }
                                        });
                                        let sub = match &prog.comment {
                                            Some(c) if !prog.publisher.is_empty() => {
                                                crate::tf!("{} · {c}", prog.publisher)
                                            }
                                            Some(c) => c.clone(),
                                            None => prog.publisher.clone(),
                                        };
                                        ui.add(style::Label::new(style::rich(sub).size(12.0).color(p.muted)).truncate());
                                    });
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(&prog.version).color(p.muted)).truncate());
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(prog.size.map(crate::backup::human_size).unwrap_or_default()).color(p.muted)));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    ui.add(style::Label::new(style::rich(prog.install_date.map(|d| d.format("%d %b %Y").to_string()).unwrap_or_default()).color(p.muted)));
                                }),
                                Box::new(|ui: &mut Ui| {
                                    style::badge(ui, prog.kind.label(), if prog.system { p.warning } else { p.accent });
                                }),
                            ],
                        );
                        // Clicking a row selects nothing: only its checkbox does.
                        row.response().context_menu(|ui| {
                            if let Some(a) = context_menu(ui, prog, ticked, &self.meta, &mut self.new_group) {
                                action = Some(a);
                            }
                        });
                    });
                });
        });
        if let Some(a) = action {
            self.apply(ui.ctx(), cx, a);
        }
        self.comment_dialog(ui.ctx(), cx);
        self.groups_dialog(ui.ctx(), cx);
    }

    fn apply(&mut self, ctx: &egui::Context, cx: &mut Cx, a: RowAction) {
        match a {
            RowAction::Uninstall(p, mode) => self.start_wizard(ctx, cx, p, mode),
            RowAction::Quick(p) => self.start_queue(ctx, cx, vec![p]),
            RowAction::Open(target) => Self::open_url(cx, &target),
            RowAction::RegKey(k) => {
                if let Err(e) = crate::platform::open_in_editor(&k) {
                    cx.error(crate::tf!("{e:#}"));
                }
            }
            RowAction::Modify(cmd) => {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    if let Err(e) = std::process::Command::new("cmd").arg("/C").raw_arg(crate::tf!("\"{cmd}\"")).spawn() {
                        cx.error(crate::tf!("{e:#}"));
                    }
                }
                #[cfg(not(windows))]
                cx.error(crate::tf!("Change/Repair is a Windows feature ({cmd})"));
            }
            RowAction::Comment(id, current) => self.comment_edit = Some((id, current)),
            RowAction::Group(id, g) => {
                self.meta.remove_from_groups(&id);
                self.meta.add_to_group(&id, &g);
                let _ = self.meta.save();
                cx.info(crate::tf!("Added to group {g}"));
            }
            RowAction::Ungroup(id) => {
                self.meta.remove_from_groups(&id);
                let _ = self.meta.save();
            }
            RowAction::RunApp(p) => match crate::platform::app_run(&p) {
                Ok(()) => cx.info(crate::tf!("Started {}", p.name)),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            },
            RowAction::TerminateApp(p) => match crate::platform::app_terminate(&p) {
                Ok(0) => cx.info(crate::tf!("{} wasn't running", p.name)),
                Ok(_) => cx.info(crate::tf!("Stopped {}", p.name)),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            },
            #[cfg(windows)]
            RowAction::ResetStore => match crate::platform::reset_store() {
                Ok(()) => cx.info(tr("Resetting the Microsoft Store…")),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            },
            RowAction::Icon(id, icon) => {
                self.meta.set_icon(&id, icon.as_deref());
                let _ = self.meta.save();
            }
            #[cfg(windows)]
            RowAction::AllUsers(p) => match crate::platform::app_uninstall_all_users(&p) {
                Ok(()) => {
                    cx.info(crate::tf!("{} removed for all users", p.name));
                    self.loaded = false;
                }
                Err(e) => cx.error(crate::tf!("{e:#}")),
            },
            #[cfg(windows)]
            RowAction::Reset(p) => match crate::platform::app_reset(&p) {
                Ok(()) => cx.info(crate::tf!("{} was reset", p.name)),
                Err(e) => cx.error(crate::tf!("{e:#}")),
            },
        }
    }

    fn comment_dialog(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some((id, text)) = &mut self.comment_edit else {
            return;
        };
        let mut done = None;
        egui::Modal::new(egui::Id::unique("comment")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.add(style::Label::new(style::strong(tr("Comment")).size(18.0)));
            style::text_input(ui, text, "Shown under the program name", 400.0);
            ui.add_space(10.0);
            crate::gui::style::row(ui, |ui| {
                if style::primary(ui, "", "Save").clicked() {
                    done = Some(true);
                }
                if style::secondary(ui, "", "Cancel").clicked() {
                    done = Some(false);
                }
            });
        });
        if let Some(save) = done {
            if save {
                self.meta.set_comment(id, text);
                let _ = self.meta.save();
                let (id, text) = (id.clone(), text.clone());
                if let Some(i) = self.programs.iter().position(|p| p.id == id) {
                    self.programs[i].comment = (!text.trim().is_empty()).then(|| text.trim().to_string());
                    self.haystacks[i] = self.programs[i].haystack();
                }
                cx.info(tr("Comment saved"));
            }
            self.comment_edit = None;
        }
    }

    fn history_ui(&mut self, ui: &mut Ui, p: Palette) {
        style::card(ui, |ui| {
            if self.history.is_empty() {
                style::empty_state(ui, icon::HISTORY, "Nothing uninstalled yet", "Programs you remove with CleanCore are listed here.");
                return;
            }
            crate::gui::style::row_top(ui, |ui| {
                ui.allocate_ui_with_layout(egui::vec2(380.0, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    egui::ScrollArea::vertical().id_salt("hist").show(ui, |ui| {
                        let mut last_day = String::new();
                        for (i, e) in self.history.iter().enumerate() {
                            let day = e.time.format("%A, %d %B %Y").to_string();
                            if day != last_day {
                                ui.add_space(6.0);
                                ui.add(style::Label::new(style::rich(&day).size(12.0).color(p.muted)));
                                last_day = day;
                            }
                            let sel = self.history_sel == Some(i);
                            let text = crate::tf!("{}  {}   ·  {}  ·  {} removed", e.time.format("%H:%M"), e.program, tr(e.operation.label()), e.removed.len());
                            if ui.add_sized([ui.available_width(), 30.0], egui::Button::selectable(sel, text)).clicked() {
                                self.history_sel = Some(i);
                            }
                        }
                    });
                });
                ui.separator();
                crate::gui::style::column(ui, |ui| match self.history_sel.and_then(|i| self.history.get(i)) {
                    Some(e) => {
                        ui.add(style::Label::new(style::strong(crate::tf!("{} {}", e.program, e.version)).size(17.0)));
                        ui.add(style::Label::new(style::muted(ui, crate::tf!("{} · {}", tr(e.operation.label()), e.time.format("%d %b %Y, %H:%M")))));
                        if let Some(b) = &e.backup_id {
                            ui.add(style::Label::new(style::muted(ui, crate::tf!("Backup {b} (restore it from Backups)"))));
                        }
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().id_salt("hist-detail").show(ui, |ui| {
                            for r in &e.removed {
                                crate::gui::style::row(ui, |ui| {
                                    ui.add(style::Label::new(style::rich(icon::CHECK).color(p.success)));
                                    ui.add(style::Label::new(style::rich(r).monospace().size(12.0)).truncate());
                                });
                            }
                            for (l, why) in &e.failed {
                                crate::gui::style::row(ui, |ui| {
                                    ui.add(style::Label::new(style::rich(icon::X_CIRCLE).color(p.danger)));
                                    ui.add(style::Label::new(style::rich(crate::tf!("{l}: {why}")).monospace().size(12.0)).truncate());
                                });
                            }
                        });
                        if style::ghost(ui, icon::COPY, "Copy as text").clicked() {
                            ui.ctx().copy_text(uninst::history_text(e));
                        }
                    }
                    None => {
                        ui.add(style::Label::new(style::muted(ui, tr("Select an entry to see what was removed."))));
                    }
                });
            });
        });
    }

    // ------------------------------------------------------------ wizard

    /// Returns false when the wizard is closed.
    fn wizard_page(&mut self, ui: &mut Ui, cx: &mut Cx, w: &mut Wizard) -> bool {
        let p = Palette::get(ui.ctx());

        // Poll background work
        if let Some(r) = take(&mut w.run) {
            match r {
                Ok((notes, run)) => {
                    w.run_info = notes;
                    match run {
                        Some(r) => w.run_info.push(match r.exit_code {
                            Some(0) => crate::tf!("The uninstaller finished successfully ({})", r.command),
                            Some(c) => crate::tf!("The uninstaller ended with exit code {c} ({})", r.command),
                            None => crate::tf!("The uninstaller ended without an exit code ({})", r.command),
                        }),
                        None => w.run_info.push("No built-in uninstaller: CleanCore removes the program's files and leftovers itself.".into()),
                    }
                }
                Err(e) => w.run_info.push(crate::tf!("The built-in uninstaller could not run: {e}")),
            }
            w.step = Step::ChooseMode;
            // Settings › "Only run the built-in uninstaller": record it and finish.
            if cx.settings.uninstall_builtin_only && w.op == Operation::Uninstall && w.program.as_ref().is_some_and(|p| p.uninstall_string.is_some()) {
                let (s, name, ver) = (cx.settings.clone(), w.label.clone(), w.program.as_ref().map(|p| p.version.clone()).unwrap_or_default());
                w.removing = Some(Pending::spawn(ui.ctx(), "Finishing", move || uninst::remove(&s, &name, &ver, Operation::Uninstall, &[]).map_err(|e| crate::tf!("{e:#}"))));
                w.step = Step::Removing;
            }
        }
        if let Some(found) = take(&mut w.scan) {
            w.checked = vec![cx.settings.leftovers_checked_by_default; found.len()];
            w.leftovers = found;
            w.step = Step::Leftovers;
        }
        if let Some(r) = take(&mut w.removing) {
            match r {
                Ok(rep) => {
                    if let Some(id) = &w.log_id {
                        if cx.settings.delete_trace_after_uninstall && rep.failed.is_empty() && !w.advanced_done {
                            let _ = crate::tracer::delete(id);
                        }
                    }
                    cx.info(crate::tf!("{}: removed {} leftover(s)", w.label, rep.cleaned));
                    w.summary = Some((rep.cleaned, rep.backup_id, rep.failed));
                }
                Err(e) => {
                    cx.error(crate::tf!("Removing leftovers failed: {e}"));
                    w.summary = Some((0, String::new(), vec![("leftovers".into(), e)]));
                }
            }
            w.step = Step::Done;
        }

        // Header: what is being removed, and where we are.
        let mut close = false;
        let subtitle = match w.op {
            Operation::TracedUninstall => "Undo exactly what the installation changed",
            _ => "Run the program's uninstaller, then remove what it leaves behind",
        };
        style::page_header(ui, w.op.label(), subtitle, |ui| {
            if matches!(w.step, Step::ChooseMode | Step::Leftovers) && style::secondary(ui, icon::X, "Cancel").clicked() {
                close = true;
            }
        });
        if close {
            // Cancelled: the uninstall is not to be continued, so forget it (else the programs
            // page offers to finish it).
            if uninst::pending().is_some_and(|pend| pend.label == w.label) {
                uninst::clear_pending();
            }
            return false;
        }
        style::hero(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                match &w.program {
                    Some(prog) => program_tile(ui, &self.app_icons, &self.meta, prog, 56.0, p.accent),
                    None => {
                        style::logo(ui, icon::RECORD, 56.0);
                    }
                }
                ui.add_space(6.0);
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add(style::Label::new(style::strong(&w.label).size(20.0)));
                    ui.add(style::Label::new(style::muted(ui, program_facts(w.program.as_ref(), w.leftovers.len(), w.op))));
                    if let Some(prog) = &w.program {
                        crate::gui::style::row(ui, |ui| {
                            style::badge(ui, prog.kind.label(), p.accent);
                            if let Some(l) = &prog.install_location {
                                ui.add(style::Label::new(style::rich(crate::tf!("{}  {l}", icon::FOLDER)).size(12.0).color(p.faint)).truncate());
                            }
                        });
                    }
                });
            });
            ui.add_space(14.0);
            steps_bar(ui, w.step, p);
        });
        ui.add_space(14.0);

        match w.step {
            Step::Running => {
                let secs = w.run.as_ref().map_or(0.0, |r| r.elapsed());
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        style::progress_ring(ui, egui::Id::unique("uninstall-run"), 84.0, None, icon::TRASH);
                        ui.add_space(10.0);
                        crate::gui::style::column(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.add_space(6.0);
                            ui.add(style::Label::new(style::strong(tr("Running the program's uninstaller")).size(16.0)));
                            ui.add(style::Label::new(style::muted(ui, tr("Follow its window. When it closes, CleanCore looks for what it left behind."))));
                            if let Some(cmd) = w.program.as_ref().and_then(|p| p.uninstall_string.as_ref()) {
                                ui.add(style::Label::new(style::rich(cmd).monospace().size(12.0).color(p.faint)).truncate());
                            }
                            ui.add(style::Label::new(style::rich(crate::tf!("{secs:.0}s")).size(12.0).color(p.faint)));
                        });
                    });
                    ui.add_space(8.0);
                    // Some uninstallers hand off to a copy of themselves: let the user move on.
                    crate::gui::style::row(ui, |ui| {
                        if style::secondary(ui, crate::gui::style::icon_forward(), "It's done, continue").clicked() {
                            w.run = None;
                            w.run_info.push("Continued before the uninstaller reported back.".into());
                            w.step = Step::ChooseMode;
                        }
                        ui.add(style::Label::new(style::muted(ui, tr("Use this if the uninstaller's window is gone but this screen is still waiting.")).size(12.0)));
                    });
                });
            }
            Step::ChooseMode => {
                if !w.run_info.is_empty() {
                    style::card(ui, |ui| {
                        style::section_title(ui, "What happened so far");
                        for line in &w.run_info {
                            report_line(ui, line, p);
                        }
                    });
                    ui.add_space(14.0);
                }
                style::eyebrow(ui, "How deep to look for leftovers");
                let gap = 12.0;
                let cw = ((ui.available_width() - gap * 2.0) / 3.0).max(200.0);
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (m, ic, title, desc) in [
                        (ScanMode::Safe, icon::SHIELD_CHECK, "Safe", "The usual places in settings and on disk. Fastest."),
                        (ScanMode::Moderate, icon::GAUGE, "Moderate", "Safe, plus everything that points to the program. Recommended."),
                        (ScanMode::Advanced, icon::ROCKET, "Advanced", "Moderate, plus a deep name search. Review the results."),
                    ] {
                        if choice_card(ui, cw, w.mode == m, ic, title, desc, p).clicked() {
                            w.mode = m;
                        }
                    }
                });
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    style::toggle(ui, &mut w.all_users, "Scan all user accounts");
                    ui.add(style::Label::new(tr("Also look in other user accounts")));
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::primary(ui, icon::SEARCH, "Scan for leftovers").clicked() {
                            let (s, t, m, all) = (cx.settings.clone(), w.target.clone(), w.mode, w.all_users);
                            w.scan = Some(Pending::spawn(ui.ctx(), "Scanning for leftovers", move || uninst::scan(&s, &t, m, all)));
                            w.step = Step::Scanning;
                        }
                    });
                });
            }
            Step::Scanning => {
                let secs = w.scan.as_ref().map_or(0.0, |r| r.elapsed());
                let mode = match w.mode {
                    ScanMode::Safe => tr("Safe"),
                    ScanMode::Moderate => tr("Moderate"),
                    ScanMode::Advanced => tr("Advanced"),
                };
                busy_card(ui, icon::SEARCH, "Looking for leftovers…", &crate::tf!("{mode} scan · {secs:.0}s"), p);
            }
            Step::Leftovers => self.leftovers_review(ui, cx, w, p),
            Step::Removing => {
                let n = w.checked.iter().filter(|c| **c).count();
                let secs = w.removing.as_ref().map_or(0.0, |r| r.elapsed());
                busy_card(ui, icon::BROOM, &crate::tf!("Backing up and removing {n} item(s)…"), &crate::tf!("Everything is backed up first · {secs:.0}s"), p);
            }
            Step::Done => {
                let mut back = false;
                if let Some((n, backup, failed)) = &w.summary {
                    style::card(ui, |ui| {
                        crate::gui::style::row(ui, |ui| {
                            let (glyph, col) = if failed.is_empty() { (icon::CHECK_CIRCLE, p.success) } else { (icon::WARNING, p.warning) };
                            let (rect, _) = ui.allocate_exact_size(Vec2::splat(52.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 26.0, col.gamma_multiply(0.16));
                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(glyph), egui::FontId::new(26.0, style::icon_fill()), col);
                            ui.add_space(6.0);
                            crate::gui::style::column(ui, |ui| {
                                ui.add_space(4.0);
                                ui.add(style::Label::new(style::strong(crate::tf!("{} is uninstalled", w.label)).size(18.0)));
                                let text = if failed.is_empty() { tr("Nothing was left behind.") } else { tr("Most of it is gone; a few items could not be removed.") };
                                ui.add(style::Label::new(style::muted(ui, text)));
                            });
                            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                                back = style::primary(ui, crate::gui::style::icon_back(), "Back to programs").clicked();
                            });
                        });
                        ui.add_space(14.0);
                        let tw = ((ui.available_width() - 20.0) / 3.0).max(140.0);
                        crate::gui::style::row(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            style::stat_tile(ui, tw, icon::BROOM, &n.to_string(), "Leftovers removed", p.success);
                            style::stat_tile(ui, tw, icon::X_CIRCLE, &failed.len().to_string(), "Could not be removed", if failed.is_empty() { p.muted } else { p.danger });
                            let b = if backup.is_empty() { tr("None").to_string() } else { backup.clone() };
                            style::stat_tile(ui, tw, icon::ARCHIVE, &b, "Backup (restore it in Backups)", p.accent);
                        });
                        if !failed.is_empty() {
                            ui.add_space(12.0);
                            ui.add(style::Label::new(style::rich(crate::tf!("{} item(s) could not be removed", failed.len())).color(p.danger).family(style::semibold())));
                            for (l, why) in failed.iter().take(12) {
                                crate::gui::style::row(ui, |ui| {
                                    ui.add(style::Label::new(style::rich(icon::X).color(p.danger)));
                                    crate::gui::style::column(ui, |ui| {
                                        ui.add(style::Label::new(style::rich(l).monospace().size(12.0)).truncate());
                                        ui.add(style::Label::new(style::rich(why).size(12.0).color(p.muted)));
                                    });
                                });
                            }
                            if failed.len() > 12 {
                                ui.add(style::Label::new(style::muted(ui, crate::tf!("…and {} more", failed.len() - 12)).size(12.0)));
                            }
                        }
                    });
                    if w.op == Operation::TracedUninstall && !w.advanced_done {
                        ui.add_space(10.0);
                        style::card(ui, |ui| {
                            crate::gui::style::row(ui, |ui| {
                                crate::gui::style::column(ui, |ui| {
                                    ui.add(style::Label::new(style::strong(tr("Advanced uninstall"))));
                                    ui.add(style::Label::new(style::muted(ui, tr("Look for related data the log didn't capture, such as settings the program created later."))));
                                });
                                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                                    if style::secondary(ui, icon::SEARCH, "Scan for related leftovers").clicked() {
                                        w.advanced_done = true;
                                        w.mode = ScanMode::Moderate;
                                        w.step = Step::ChooseMode;
                                    }
                                });
                            });
                        });
                    }
                }
                if back {
                    return false;
                }
            }
        }
        true
    }

    /// Leftovers found after an uninstall: counts, a filter, and the items grouped by where
    /// they live, each with what kind of thing it is.
    fn leftovers_review(&mut self, ui: &mut Ui, cx: &mut Cx, w: &mut Wizard, p: Palette) {
        let n_sel = w.checked.iter().filter(|c| **c).count();
        let finish = |w: &mut Wizard, ctx: &egui::Context, cx: &Cx, chosen: Vec<Issue>| {
            let (s, label, version, op) = (cx.settings.clone(), w.label.clone(), w.program.as_ref().map(|p| p.version.clone()).unwrap_or_default(), w.op);
            let what = if chosen.is_empty() { tr("Saving") } else { tr("Removing leftovers") };
            w.removing = Some(Pending::spawn(ctx, what, move || uninst::remove(&s, &label, &version, op, &chosen).map_err(|e| crate::tf!("{e:#}"))));
            w.step = Step::Removing;
        };
        if w.leftovers.is_empty() {
            style::card(ui, |ui| {
                style::empty_state(ui, icon::CHECK_CIRCLE, "No leftovers found", "The program removed itself cleanly.");
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if style::primary(ui, icon::CHECK, "Finish").clicked() {
                        finish(w, ui.ctx(), cx, Vec::new());
                    }
                });
                ui.add_space(12.0);
            });
            return;
        }
        let reg_cat = "leftover_registry";
        let n_reg = w.leftovers.iter().filter(|i| i.category == reg_cat).count();
        let n_files = w.leftovers.len() - n_reg;
        let tw = ((ui.available_width() - 30.0) / 4.0).max(130.0);
        crate::gui::style::row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            style::stat_tile(ui, tw, icon::SEARCH, &w.leftovers.len().to_string(), "Leftovers found", p.accent);
            style::stat_tile(ui, tw, icon::CHECK_CIRCLE, &n_sel.to_string(), "Selected to delete", if n_sel > 0 { p.success } else { p.muted });
            style::stat_tile(ui, tw, icon::TREE, &n_reg.to_string(), if cfg!(windows) { tr("Registry entries") } else { tr("Settings entries") }, p.accent);
            style::stat_tile(ui, tw, icon::FOLDER, &n_files.to_string(), "Files and folders", p.accent);
        });
        ui.add_space(12.0);
        crate::gui::style::row(ui, |ui| {
            style::search_field(ui, &mut w.filter, "Filter leftovers", 220.0);
            if style::ghost(ui, icon::CHECK, "Select all").clicked() {
                w.checked.iter_mut().for_each(|c| *c = true);
            }
            if style::ghost(ui, icon::X, "Select none").clicked() {
                w.checked.iter_mut().for_each(|c| *c = false);
            }
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if ui.add_enabled_ui(n_sel > 0, |ui| style::primary(ui, icon::TRASH, &crate::tf!("Delete {n_sel} item(s)"))).inner.clicked() {
                    let chosen: Vec<Issue> = w.leftovers.iter().zip(&w.checked).filter(|(_, c)| **c).map(|(i, _)| i.clone()).collect();
                    finish(w, ui.ctx(), cx, chosen);
                }
                ui.add(style::Label::new(style::muted(ui, tr("Backed up before deleting")).size(12.0)));
            });
        });
        ui.add_space(8.0);
        let q = w.filter.to_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let reg_label = if cfg!(windows) { tr("Registry") } else { tr("Settings") };
            for (cat, label, ic) in [(reg_cat, reg_label, icon::TREE), ("leftover_files", "Files and folders", icon::FOLDER)] {
                let idx: Vec<usize> = (0..w.leftovers.len())
                    .filter(|i| w.leftovers[*i].category == cat)
                    .filter(|i| q.is_empty() || w.leftovers[*i].location.to_lowercase().contains(&q) || w.leftovers[*i].detail.to_lowercase().contains(&q))
                    .collect();
                if idx.is_empty() {
                    continue;
                }
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        let mut all = idx.iter().all(|i| w.checked[*i]);
                        if ui.checkbox(&mut all, style::vis("")).on_hover_text(style::vis(tr("Select the whole group"))).changed() {
                            idx.iter().for_each(|i| w.checked[*i] = all);
                        }
                        style::icon_tile(ui, ic, p.accent, 28.0);
                        ui.add(style::Label::new(style::strong(label)));
                        let sel = idx.iter().filter(|i| w.checked[**i]).count();
                        style::badge(ui, &crate::tf!("{sel} / {}", idx.len()), p.accent);
                    });
                    ui.add_space(6.0);
                    for (n, i) in idx.into_iter().enumerate() {
                        if n > 0 {
                            style::divider(ui);
                        }
                        let (kind_ic, kind) = fix_kind(&w.leftovers[i].fix);
                        crate::gui::style::row(ui, |ui| {
                            ui.checkbox(&mut w.checked[i], style::vis(""));
                            ui.add(style::Label::new(style::rich(kind_ic).size(16.0).color(if w.checked[i] { p.accent } else { p.faint })));
                            crate::gui::style::column(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.set_width((ui.available_width() - 90.0).max(120.0));
                                let loc = &w.leftovers[i].location;
                                ui.add(style::Label::new(style::rich(loc).monospace().size(12.5)).truncate()).on_hover_text(style::vis(loc));
                                let d = &w.leftovers[i].detail;
                                if !d.is_empty() {
                                    let col = if d.contains("needs administrator") { p.warning } else { p.muted };
                                    ui.add(style::Label::new(style::rich(d).size(12.0).color(col)).truncate());
                                }
                            });
                            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| style::badge(ui, kind, p.muted));
                        });
                    }
                });
                ui.add_space(10.0);
            }
        });
    }

    // ------------------------------------------------------------ queue

    fn queue_page(&mut self, ui: &mut Ui, cx: &mut Cx, q: &mut Queue) -> bool {
        let p = Palette::get(ui.ctx());
        if let Some(notes) = take(&mut q.prepared) {
            q.notes = notes;
        }
        let secs = q.job.as_ref().map_or(0.0, |j| j.elapsed());
        if let Some(r) = take(&mut q.job) {
            if let Some(item) = q.items.iter_mut().find(|it| matches!(it.state, QState::Running)) {
                item.secs = secs;
                item.state = match r {
                    Ok(rep) => QState::Done(rep),
                    Err(e) => QState::Failed(e),
                };
            }
        }
        if q.prepared.is_none() && q.job.is_none() {
            if let Some(item) = q.items.iter_mut().find(|it| matches!(it.state, QState::Waiting)) {
                item.state = QState::Running;
                let (s, prog) = (cx.settings.clone(), item.prog.clone());
                q.job = Some(Pending::spawn(ui.ctx(), crate::tf!("Uninstalling {}", prog.name), move || uninst::quick_uninstall(&s, &prog).map_err(|e| crate::tf!("{e:#}"))));
            } else if q.took.is_none() {
                q.took = Some(q.started.elapsed().as_secs_f32());
                let failed = q.items.iter().filter(|it| matches!(it.state, QState::Failed(_))).count();
                if failed == 0 {
                    cx.info(crate::tf!("Quick uninstall finished: {} program(s)", q.items.len()));
                } else {
                    cx.error(crate::tf!("Quick uninstall finished: {failed} of {} program(s) failed", q.items.len()));
                }
            }
        }

        let total = q.items.len();
        let ok = q.items.iter().filter(|it| matches!(it.state, QState::Done(_))).count();
        let failed = q.items.iter().filter(|it| matches!(it.state, QState::Failed(_))).count();
        let waiting = q.items.iter().filter(|it| matches!(it.state, QState::Waiting)).count();
        let removed: usize = q.items.iter().filter_map(|it| if let QState::Done(r) = &it.state { Some(r.leftovers_removed) } else { None }).sum();
        let done = q.took.is_some();
        let mode = match cx.settings.uninstall_scan_mode {
            ScanMode::Safe => tr("Safe"),
            ScanMode::Moderate => tr("Moderate"),
            ScanMode::Advanced => tr("Advanced"),
        };

        let mut close = false;
        style::page_header(ui, "Quick Uninstall", &crate::tf!("{total} program(s) · silent uninstallers, then a {mode} leftover scan · leftovers removed automatically"), |ui| {
            if done && style::primary(ui, crate::gui::style::icon_back(), "Back to programs").clicked() {
                close = true;
            }
        });
        if close {
            self.checked.clear();
            return false;
        }

        // Overview
        let elapsed = q.took.unwrap_or_else(|| q.started.elapsed().as_secs_f32());
        style::hero(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                let frac = if total > 0 { (ok + failed) as f32 / total as f32 } else { 1.0 };
                style::progress_ring(ui, egui::Id::unique(("quick-uninstall", q.started)), 96.0, Some(frac), if done { icon::CHECK } else { icon::LIGHTNING });
                ui.add_space(12.0);
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add_space(6.0);
                    let title = if q.prepared.is_some() {
                        "Preparing: restore point and backup…".to_string()
                    } else if done {
                        if failed == 0 {
                            crate::tf!("All {total} program(s) uninstalled")
                        } else {
                            crate::tf!("Finished with {failed} problem(s)")
                        }
                    } else {
                        let current = q.items.iter().find(|it| matches!(it.state, QState::Running)).map(|it| it.prog.name.as_str()).unwrap_or("");
                        crate::tf!("Uninstalling {} of {total}: {current}", ok + failed + 1)
                    };
                    ui.add(style::Label::new(style::strong(title).size(19.0)));
                    crate::gui::style::row(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        stat_inline(ui, icon::CHECK_CIRCLE, &crate::tf!("{ok} done"), p.success);
                        stat_inline(ui, icon::X_CIRCLE, &crate::tf!("{failed} failed"), if failed > 0 { p.danger } else { p.muted });
                        stat_inline(ui, icon::CLOCK, &crate::tf!("{waiting} waiting"), p.muted);
                        stat_inline(ui, icon::BROOM, &crate::tf!("{removed} leftovers removed"), p.accent);
                        stat_inline(ui, icon::HISTORY, &format_secs(elapsed), p.muted);
                    });
                    let w = ui.available_width().min(560.0);
                    if q.prepared.is_some() {
                        style::busy_bar(ui, w);
                    } else {
                        style::progress_bar(ui, egui::Id::unique(("quick-uninstall-bar", q.started)), w, if total > 0 { (ok + failed) as f32 / total as f32 } else { 1.0 });
                    }
                });
            });
        });
        ui.add_space(12.0);
        if !q.notes.is_empty() {
            style::card(ui, |ui| {
                style::section_title(ui, "Before starting");
                for n in &q.notes {
                    report_line(ui, n, p);
                }
            });
            ui.add_space(12.0);
        }

        // One row per program
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            style::card(ui, |ui| {
                for (i, it) in q.items.iter().enumerate() {
                    if i > 0 {
                        style::divider(ui);
                    }
                    crate::gui::style::row(ui, |ui| {
                        ui.set_min_height(52.0);
                        // Status mark
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
                        match &it.state {
                            QState::Waiting => {
                                ui.painter().circle_stroke(rect.center(), 11.0, egui::Stroke::new(1.5, p.border));
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    style::vstr((i + 1).to_string()),
                                    egui::FontId::new(11.5, style::semibold()),
                                    p.muted,
                                );
                            }
                            QState::Running => {
                                ui.put(rect, egui::Spinner::new().size(20.0).color(p.accent));
                            }
                            QState::Done(r) => {
                                let col = if r.still_installed || r.leftovers_failed > 0 { p.warning } else { p.success };
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::CHECK_CIRCLE), egui::FontId::new(22.0, style::icon_fill()), col);
                            }
                            QState::Failed(_) => {
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(icon::X_CIRCLE), egui::FontId::new(22.0, style::icon_fill()), p.danger);
                            }
                        }
                        program_tile(ui, &self.app_icons, &self.meta, &it.prog, 36.0, p.accent);
                        crate::gui::style::column(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.set_width((ui.available_width() - 150.0).max(160.0));
                            crate::gui::style::row(ui, |ui| {
                                ui.add(style::Label::new(style::strong(&it.prog.name)));
                                let facts = [it.prog.version.as_str(), it.prog.publisher.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
                                ui.add(style::Label::new(style::rich(facts).size(12.0).color(p.faint)).truncate());
                            });
                            match &it.state {
                                QState::Waiting => {
                                    ui.add(style::Label::new(style::rich(tr("Waiting its turn")).size(12.5).color(p.muted)));
                                }
                                QState::Running => {
                                    ui.add(style::Label::new(style::rich(crate::tf!("Running the silent uninstaller, then scanning · {secs:.0}s")).size(12.5).color(p.accent)));
                                    style::busy_bar(ui, ui.available_width().min(320.0));
                                }
                                QState::Done(r) => {
                                    let mut parts =
                                        vec![if r.scanned { crate::tf!("{} leftover(s) removed", r.leftovers_removed) } else { "Leftover scan off in settings".to_string() }];
                                    if r.leftovers_failed > 0 {
                                        parts.push(crate::tf!("{} could not be removed", r.leftovers_failed));
                                    }
                                    if let Some(c) = r.exit_code.filter(|c| *c != 0) {
                                        parts.push(crate::tf!("uninstaller exit code {c}"));
                                    }
                                    ui.add(style::Label::new(style::rich(parts.join(" · ")).size(12.5).color(p.muted)));
                                    if r.still_installed {
                                        ui.add(style::Label::new(
                                            style::rich(crate::tf!("{}  Still appears to be installed: try a normal Uninstall", icon::WARNING)).size(12.5).color(p.warning),
                                        ));
                                    }
                                }
                                QState::Failed(e) => {
                                    ui.add(style::Label::new(style::rich(e).size(12.5).color(p.danger)).wrap());
                                }
                            }
                        });
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            let (text, col) = match &it.state {
                                QState::Waiting => ("Waiting", p.muted),
                                QState::Running => ("Uninstalling", p.accent),
                                QState::Done(r) if r.still_installed => ("Check", p.warning),
                                QState::Done(_) => ("Removed", p.success),
                                QState::Failed(_) => ("Failed", p.danger),
                            };
                            style::badge(ui, text, col);
                            if it.secs > 0.0 {
                                ui.add(style::Label::new(style::rich(format_secs(it.secs)).size(12.0).color(p.faint)));
                            }
                        });
                    });
                }
            });
        });
        true
    }

    // ------------------------------------------------------------ forced

    fn forced_page(&mut self, ui: &mut Ui, cx: &mut Cx, f: &mut Forced) -> bool {
        let p = Palette::get(ui.ctx());
        if style::page_header_back(ui, "Forced Uninstall", "For programs that aren't listed, were partly removed or have a broken uninstaller", |_| {}) {
            return false;
        }
        style::card(ui, |ui| {
            egui::Grid::new("forced-grid").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
                ui.add(style::Label::new(style::muted(ui, tr("Program name"))));
                style::text_input(ui, &mut f.names, "Exact name; several separated by ;", 520.0);
                ui.end_row();
                ui.add(style::Label::new(style::muted(ui, tr("Folder or main file"))));
                crate::gui::style::row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    style::text_input(ui, &mut f.paths, "Pick below or type; several separated by ;", 520.0 - 2.0 * (style::CONTROL_H + 6.0));
                    if let Some(p) = style::browse_button(ui, "forced-paths", style::Pick::Any, f.paths.split(';').next_back().unwrap_or("")) {
                        if f.paths.trim().is_empty() {
                            f.paths = p;
                        } else if !f.paths.split(';').any(|x| x.trim() == p) {
                            f.paths = crate::tf!("{}; {p}", f.paths.trim().trim_end_matches(';'));
                        }
                    }
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(6).show(ui, |ui| {
                ui.set_height(180.0);
                ui.set_width(ui.available_width());
                f.browser.show(ui, "forced-browser");
            });
            if let Some(sel) = f.browser.selected.take() {
                if f.paths.trim().is_empty() {
                    f.paths = sel;
                } else if !f.paths.split(';').any(|x| x.trim() == sel) {
                    f.paths = crate::tf!("{}; {sel}", f.paths.trim());
                }
            }
            ui.add_space(10.0);
            let names: Vec<String> = f.names.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            let paths: Vec<String> = f.paths.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            let ready = !names.is_empty() || !paths.is_empty();
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), style::CONTROL_H), crate::gui::style::far_layout(egui::Align::Center), |ui| {
                // Built right to left: Next, then Find uninstallers.
                let next_clicked = ui.add_enabled_ui(ready, |ui| style::primary(ui, crate::gui::style::icon_forward(), "Next")).inner.clicked();
                if ui.add_enabled_ui(ready, |ui| style::secondary(ui, icon::SEARCH, "Find uninstallers")).inner.clicked() {
                    let toks = Target { names: names.clone(), paths: paths.clone(), ..Default::default() }.tokens();
                    f.found = Some(
                        self.programs
                            .iter()
                            .filter(|pr| {
                                toks.iter().any(|t| Target::from_program(pr).tokens().contains(t))
                                    || pr.install_location.as_deref().is_some_and(|l| paths.iter().any(|x| x.to_lowercase().starts_with(&l.to_lowercase())))
                            })
                            .cloned()
                            .collect(),
                    );
                    f.use_uninstaller = None;
                    f.logs = crate::tracer::matching(&names);
                    f.log_files = vec![None; f.logs.len()];
                    for d in crate::tracer::db_matching(cx.settings, &names) {
                        if !f.logs.iter().any(|l| l.id == d.log.id) {
                            f.logs.push(d.log);
                            f.log_files.push(Some(d.file));
                        }
                    }
                    f.use_log = None;
                }
                if next_clicked {
                    let label = if names.is_empty() { paths.join(", ") } else { names.join(", ") };
                    let mut import_failed = false;
                    let db_file = f.use_log.and_then(|i| f.log_files.get(i).cloned().flatten());
                    let log = match (f.use_log.and_then(|i| f.logs.get(i)).cloned(), db_file) {
                        (Some(_), Some(file)) => match crate::tracer::import(&file) {
                            Ok(l) => Some(l),
                            Err(e) => {
                                cx.error(crate::tf!("{e:#}"));
                                import_failed = true;
                                None
                            }
                        },
                        (l, None) => l,
                        (None, Some(_)) => None,
                    };
                    if import_failed {
                    } else if let Some(log) = log {
                        self.start_log(ui.ctx(), cx, log, true);
                    } else if let Some(prog) = f.use_uninstaller.and_then(|i| f.found.as_ref().and_then(|v| v.get(i))).cloned() {
                        self.start_wizard(ui.ctx(), cx, prog, RunMode::Normal);
                        if let Screen::Wizard(w) = &mut self.screen {
                            w.op = Operation::ForcedUninstall;
                            w.target.names.extend(names.clone());
                            w.target.paths.extend(paths.clone());
                        }
                    } else {
                        let settings = cx.settings.clone();
                        let l2 = label.clone();
                        self.screen = Screen::Wizard(Box::new(Wizard {
                            program: None,
                            label: label.clone(),
                            target: Target { names, paths, ..Default::default() },
                            op: Operation::ForcedUninstall,
                            step: Step::Running,
                            run: Some(Pending::spawn(ui.ctx(), "Preparing", move || Ok((uninst::prepare(&settings, &l2), None)))),
                            run_info: Vec::new(),
                            mode: ScanMode::Advanced,
                            all_users: false,
                            scan: None,
                            leftovers: Vec::new(),
                            checked: Vec::new(),
                            removing: None,
                            summary: None,
                            log_id: None,
                            advanced_done: false,
                            filter: String::new(),
                        }));
                    }
                }
            });
        });
        if !matches!(self.screen, Screen::List) {
            return true; // switched to the wizard; caller keeps the new screen
        }
        if let Some(found) = &f.found {
            ui.add_space(10.0);
            style::card(ui, |ui| {
                ui.add(style::Label::new(style::strong(crate::tf!("Found {} program(s) with an uninstaller", found.len()))));
                if found.is_empty() {
                    ui.add(style::Label::new(style::muted(ui, tr("None: Next scans for leftovers by name and path."))));
                }
                let mut none = f.use_uninstaller.is_none();
                if ui.radio(none, style::vis("Don't run an uninstaller, only remove leftovers")).clicked() {
                    none = true;
                    f.use_uninstaller = None;
                }
                let _ = none;
                for (i, prog) in found.iter().enumerate() {
                    if ui.radio(f.use_uninstaller == Some(i), style::vis(crate::tf!("Run the uninstaller of {} {}", prog.name, prog.version))).clicked() {
                        f.use_uninstaller = Some(i);
                    }
                }
                if !f.logs.is_empty() {
                    ui.add_space(8.0);
                    ui.add(style::Label::new(style::strong(crate::tf!("Found {} trace log(s)", f.logs.len()))));
                    for (i, l) in f.logs.iter().enumerate() {
                        let from = if f.log_files.get(i).is_some_and(|x| x.is_some()) { tr(" from the logs database") } else { "" };
                        if ui.radio(f.use_log == Some(i), style::vis(crate::tf!("Use the log \"{}\"{from} ({})", l.name, l.created.format("%d %b %Y")))).clicked() {
                            f.use_log = Some(i);
                            f.use_uninstaller = None;
                        }
                    }
                }
            });
        }
        true
    }
}

/// Groups manager (Revo: Groups Manager): create, rename, reorder, delete.
#[derive(Default)]
struct GroupsDialog {
    new_name: String,
    /// Group being renamed and the text typed so far.
    rename: Option<(String, String)>,
}

impl UninstallerView {
    fn groups_dialog(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some(d) = &mut self.groups_dlg else {
            return;
        };
        let p = Palette::get(ctx);
        let mut close = false;
        let mut changed = false;
        let mut err: Option<String> = None;
        egui::Modal::new(egui::Id::unique("groups-manager")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.add(style::Label::new(style::strong(tr("Groups")).size(18.0)));
            ui.add(style::Label::new(style::muted(ui, tr("Programs are added to a group from their right-click menu.")).size(12.5)));
            ui.add_space(8.0);
            let groups = self.meta.groups.clone();
            for (i, (g, ids)) in groups.iter().enumerate() {
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::rich(icon::TAG).color(p.accent)));
                    match &mut d.rename {
                        Some((old, text)) if old == g => {
                            let r = style::text_input(ui, text, "Name", 200.0);
                            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if style::secondary(ui, icon::CHECK, "Save").clicked() || enter {
                                match self.meta.rename_group(g, text) {
                                    Ok(()) => {
                                        if self.group.as_deref() == Some(g.as_str()) {
                                            self.group = Some(text.trim().to_string());
                                        }
                                        changed = true;
                                    }
                                    Err(e) => err = Some(crate::tf!("{e:#}")),
                                }
                                d.rename = None;
                            }
                        }
                        _ => {
                            ui.add(style::Label::new(g));
                            ui.add(style::Label::new(style::muted(ui, crate::tf!("{} program(s)", ids.len())).size(12.5)));
                        }
                    }
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::icon_button(ui, icon::TRASH, &crate::tf!("Delete group {g}")).clicked() {
                            let _ = self.meta.delete_group(g);
                            if self.group.as_deref() == Some(g.as_str()) {
                                self.group = None;
                            }
                            changed = true;
                        }
                        if style::icon_button(ui, icon::CHAT, &crate::tf!("Rename group {g}")).clicked() {
                            d.rename = Some((g.clone(), g.clone()));
                        }
                        ui.add_enabled_ui(i + 1 < groups.len(), |ui| {
                            if style::icon_button(ui, icon::ARROW_DOWN, "Move down").clicked() {
                                self.meta.move_group(g, 1);
                                changed = true;
                            }
                        });
                        ui.add_enabled_ui(i > 0, |ui| {
                            if style::icon_button(ui, icon::ARROW_UP, "Move up").clicked() {
                                self.meta.move_group(g, -1);
                                changed = true;
                            }
                        });
                    });
                });
            }
            ui.add_space(6.0);
            crate::gui::style::row(ui, |ui| {
                let r = style::text_input(ui, &mut d.new_name, "New group", 260.0);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if style::secondary(ui, icon::PLUS, "Create").clicked() || enter {
                    match self.meta.create_group(&d.new_name) {
                        Ok(()) => {
                            d.new_name.clear();
                            changed = true;
                        }
                        Err(e) => err = Some(crate::tf!("{e:#}")),
                    }
                }
            });
            ui.add_space(10.0);
            if style::primary(ui, "", "Done").clicked() {
                close = true;
            }
        });
        if changed {
            if let Err(e) = self.meta.save() {
                cx.error(crate::tf!("{e:#}"));
            }
        }
        if let Some(e) = err {
            cx.error(e);
        }
        if close {
            self.groups_dlg = None;
        }
    }
}

enum RowAction {
    Uninstall(Program, RunMode),
    Quick(Program),
    Open(String),
    RegKey(String),
    Modify(String),
    Comment(String, String),
    Group(String, String),
    Ungroup(String),
    Icon(String, Option<String>),
    RunApp(Program),
    TerminateApp(Program),
    #[cfg(windows)]
    ResetStore,
    #[cfg(windows)]
    AllUsers(Program),
    #[cfg(windows)]
    Reset(Program),
}

/// The right-click menu of a program. Uninstalling is offered only for a ticked program.
fn context_menu(ui: &mut Ui, prog: &Program, ticked: bool, meta: &Meta, new_group: &mut String) -> Option<RowAction> {
    let mut out = None;
    let mut item = |ui: &mut Ui, ic: &str, label: &str, a: RowAction| {
        if style::ghost(ui, ic, label).clicked() {
            out = Some(a);
            ui.close();
        }
    };
    if ticked {
        item(ui, icon::TRASH, "Uninstall", RowAction::Uninstall(prog.clone(), RunMode::Normal));
        item(ui, icon::LIGHTNING, "Quick Uninstall", RowAction::Quick(prog.clone()));
        if prog.msi_product_code().is_some() && prog.kind != ProgramKind::WindowsApp {
            item(ui, icon::WRENCH, "Forced MSI Uninstall", RowAction::Uninstall(prog.clone(), RunMode::ForcedMsi));
        }
        #[cfg(windows)]
        if prog.kind == ProgramKind::WindowsApp {
            item(ui, icon::USERS, "Uninstall for all users", RowAction::AllUsers(prog.clone()));
        }
    }
    #[cfg(windows)]
    if prog.kind == ProgramKind::WindowsApp {
        item(ui, icon::UNDO, "Reset the app", RowAction::Reset(prog.clone()));
    }
    if prog.kind.is_store_app() {
        ui.separator();
        item(ui, icon::PLAY, "Run", RowAction::RunApp(prog.clone()));
        item(ui, icon::X_CIRCLE, "Terminate", RowAction::TerminateApp(prog.clone()));
        #[cfg(windows)]
        item(ui, icon::STOREFRONT, "Reset Microsoft Store", RowAction::ResetStore);
    }
    if let Some(m) = &prog.modify_path {
        item(ui, icon::WRENCH, "Change / Repair", RowAction::Modify(m.clone()));
    }
    ui.separator();
    if let Some(l) = &prog.install_location {
        item(ui, icon::FOLDER_OPEN, "Install location", RowAction::Open(l.clone()));
    }
    if let Some(k) = &prog.registry_key {
        item(ui, icon::TREE, "Open registry key", RowAction::RegKey(k.clone()));
    }
    let q: String = prog.name.chars().map(|c| if c.is_alphanumeric() { c.to_string() } else { "+".into() }).collect();
    item(ui, icon::GLOBE, "Search online", RowAction::Open(crate::tf!("https://duckduckgo.com/?q={q}")));
    for (label, link) in [("About link", &prog.website), ("Help link", &prog.help_link), ("Update link", &prog.update_link)] {
        if let Some(u) = link {
            item(ui, icon::LINK, label, RowAction::Open(u.clone()));
        }
    }
    ui.separator();
    item(ui, icon::CHAT, "Add / edit comment", RowAction::Comment(prog.id.clone(), prog.comment.clone().unwrap_or_default()));
    ui.menu_button(style::vis(crate::tf!("{}  Add to group", icon::TAG)), |ui| {
        for (g, _) in &meta.groups {
            if ui.button(style::vis(g)).clicked() {
                out = Some(RowAction::Group(prog.id.clone(), g.clone()));
                ui.close();
            }
        }
        ui.separator();
        crate::gui::style::row(ui, |ui| {
            style::text_input(ui, new_group, "New group", 140.0);
            if ui.button(style::vis(tr("Add"))).clicked() && !new_group.trim().is_empty() {
                out = Some(RowAction::Group(prog.id.clone(), new_group.trim().to_string()));
                new_group.clear();
                ui.close();
            }
        });
    });
    if meta.group_of(&prog.id).is_some() && style::ghost(ui, icon::X, "Remove from group").clicked() {
        out = Some(RowAction::Ungroup(prog.id.clone()));
        ui.close();
    }
    ui.menu_button(style::vis(crate::tf!("{}  Change icon", icon::PALETTE)), |ui| {
        if let Some(choice) = style::icon_picker(ui, meta.icons.get(&prog.id).map(String::as_str)) {
            out = Some(RowAction::Icon(prog.id.clone(), choice));
            ui.close();
        }
    });
    out
}

/// A program's own icon when it has one (and the user didn't pick another), else a glyph.
fn program_icon(ui: &mut Ui, icons: &super::appicons::IconCache, meta: &Meta, p: &Program, size: f32, glyph_color: Color32) {
    match icons.get(&p.id).filter(|_| !meta.icons.contains_key(&p.id)) {
        Some(t) => {
            ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::splat(size)));
        }
        None => {
            ui.add(style::Label::new(style::rich(prog_icon(meta, p)).size(size * 0.9).color(glyph_color)));
        }
    }
}

/// Large version for headers: the real icon as is, or the glyph on a soft tile.
fn program_tile(ui: &mut Ui, icons: &super::appicons::IconCache, meta: &Meta, p: &Program, size: f32, accent: Color32) {
    match icons.get(&p.id).filter(|_| !meta.icons.contains_key(&p.id)) {
        Some(t) => {
            ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::splat(size)));
        }
        None => style::icon_tile(ui, prog_icon(meta, p), accent, size),
    }
}

/// The icon a program shows: the user's choice, or one for its kind.
fn prog_icon(meta: &Meta, p: &Program) -> &'static str {
    meta.icons.get(&p.id).and_then(|k| style::icon_for_key(k)).unwrap_or_else(|| kind_icon(p.kind))
}

fn kind_icon(k: ProgramKind) -> &'static str {
    match k {
        ProgramKind::Win32 => icon::APP_WINDOW,
        ProgramKind::Msi | ProgramKind::Deb | ProgramKind::Rpm | ProgramKind::Pacman => icon::PACKAGE,
        ProgramKind::WindowsApp => icon::STOREFRONT,
        ProgramKind::Flatpak | ProgramKind::Snap => icon::CUBE,
        ProgramKind::AppImage => icon::FILE,
        ProgramKind::Manual => icon::FOLDER,
    }
}

fn steps_bar(ui: &mut Ui, step: Step, p: Palette) {
    let idx = match step {
        Step::Running => 0,
        Step::ChooseMode | Step::Scanning => 1,
        Step::Leftovers | Step::Removing => 2,
        Step::Done => 3,
    };
    crate::gui::style::row(ui, |ui| {
        for (i, label) in ["Uninstaller", "Scan", "Leftovers", "Summary"].iter().enumerate() {
            let (bg, fg) = if i < idx {
                (p.success, Color32::WHITE)
            } else if i == idx {
                (p.accent, Color32::WHITE)
            } else {
                (p.surface_alt, p.muted)
            };
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 12.0, bg);
            let mark = if i < idx { icon::CHECK.to_string() } else { (i + 1).to_string() };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, style::vstr(mark), egui::FontId::new(12.5, style::semibold()), fg);
            ui.add(style::Label::new(style::rich(tr(label)).color(if i <= idx { p.text } else { p.muted })));
            if i < 3 {
                let (r, _) = ui.allocate_exact_size(Vec2::new(36.0, 2.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, if i < idx { p.success } else { p.border });
            }
        }
    });
}

fn choice_card(ui: &mut Ui, width: f32, selected: bool, ic: &str, title: &str, desc: &str, p: Palette) -> egui::Response {
    let (title, desc) = (tr(title), tr(desc));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 96.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, title));
    style::tile_frame(ui, rect, &resp, selected);
    let mr = style::Mirror::new(rect);
    let g = style::text_galley(ui, desc, egui::FontId::proportional(12.5), p.muted, rect.width() - 32.0);
    let painter = ui.painter();
    painter.text(
        mr.pos(16.0, rect.top() + 22.0),
        mr.start(egui::Align::Center),
        style::vstr(ic),
        egui::FontId::new(18.0, if selected { style::icon_fill() } else { egui::FontFamily::Proportional }),
        p.accent,
    );
    painter.text(mr.pos(42.0, rect.top() + 22.0), mr.start(egui::Align::Center), style::vstr(title), egui::FontId::new(15.0, style::semibold()), p.text);
    painter.galley(egui::pos2(mr.x(16.0), rect.top() + 42.0), g, p.muted);
    resp
}

/// Version, publisher, size and install date of a program, or a log's size.
fn program_facts(prog: Option<&Program>, items: usize, op: Operation) -> String {
    let Some(p) = prog else {
        return if op == Operation::TracedUninstall { crate::tf!("Traced installation · {items} recorded change(s)") } else { String::new() };
    };
    let mut v: Vec<String> = Vec::new();
    if !p.version.is_empty() {
        v.push(crate::tf!("Version {}", p.version));
    }
    if !p.publisher.is_empty() {
        v.push(p.publisher.clone());
    }
    if let Some(s) = p.size {
        v.push(crate::backup::human_size(s));
    }
    if let Some(d) = p.install_date {
        v.push(crate::tf!("Installed {}", d.format("%d %b %Y")));
    }
    v.join(" · ")
}

/// One line of a progress report, with an icon that says whether it went well.
fn report_line(ui: &mut Ui, line: &str, p: Palette) {
    let l = line.to_lowercase();
    let (ic, col) = if l.contains("fail") || l.contains("could not") || l.contains("error") {
        (icon::WARNING, p.warning)
    } else if l.contains("exit code") || l.contains("without an exit code") || l.contains("continued before") {
        (icon::INFO, p.accent)
    } else {
        (icon::CHECK_CIRCLE, p.success)
    };
    crate::gui::style::row_top(ui, |ui| {
        ui.add(style::Label::new(style::rich(ic).color(col)));
        ui.add(style::Label::new(style::rich(line).size(13.0)).wrap());
    });
}

/// A card that shows work in progress: spinning ring, title and a status line.
fn busy_card(ui: &mut Ui, glyph: &str, title: &str, status: &str, p: Palette) {
    style::card(ui, |ui| {
        crate::gui::style::row(ui, |ui| {
            style::progress_ring(ui, egui::Id::unique(("busy", title)), 84.0, None, glyph);
            ui.add_space(10.0);
            crate::gui::style::column(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.add_space(14.0);
                ui.add(style::Label::new(style::strong(title).size(16.0)));
                ui.add(style::Label::new(style::rich(status).color(p.muted)));
                style::busy_bar(ui, ui.available_width().min(420.0));
            });
        });
    });
}

/// Icon and coloured text in a row of figures.
fn stat_inline(ui: &mut Ui, ic: &str, text: &str, col: Color32) {
    ui.add(style::Label::new(style::rich(crate::tf!("{ic}  {text}")).size(13.0).color(col)));
}

fn format_secs(s: f32) -> String {
    let s = s.round() as u64;
    if s >= 60 {
        crate::tf!("{}m {:02}s", s / 60, s % 60)
    } else {
        crate::tf!("{s}s")
    }
}

/// What kind of thing a leftover is: icon and a one-word label.
fn fix_kind(f: &crate::model::Fix) -> (&'static str, &'static str) {
    use crate::model::Fix;
    match f {
        Fix::DeleteRegKey { .. } => (icon::TREE, "Key"),
        Fix::DeleteRegValue { .. } => (icon::KEY, "Value"),
        Fix::DeleteFile { .. } => (icon::FILE, "File"),
        Fix::DeleteEmptyDir { .. } | Fix::DeleteDirTree { .. } => (icon::FOLDER, "Folder"),
        Fix::WriteFile { .. } | Fix::WriteSetting { .. } => (icon::UNDO, "Restore"),
        Fix::DconfResetDir { .. } | Fix::DconfResetKey { .. } => (icon::SLIDERS, "Setting"),
        Fix::RemoveXbelBookmark { .. } | Fix::RemoveMimeHandler { .. } | Fix::EditJson { .. } => (icon::FILE_TEXT, "Entry"),
        Fix::PurgePackage { .. } => (icon::PACKAGE, "Package"),
    }
}
