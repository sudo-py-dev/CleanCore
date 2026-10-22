//! Auto Clean / Auto Backup: one page that scrolls, with a card per scheduled task (when it
//! runs in plain words, its next and last run, Run now / Edit / Delete) and, above them, the
//! editor while a task is being created or changed. Labels sit above their controls, so the
//! form reads the same left to right and right to left.

use super::style::{self, icon, Palette};
use super::{take, tree::Browser, Cx, Pending};
use crate::i18n::tr;
use crate::model::ScanMode;
use crate::scheduler::{self, Action, MonthDay, Schedule, Task};
use chrono::{NaiveTime, Weekday};
use eframe::egui::{self, Ui};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Clean,
    Backup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Every {
    Daily,
    Weekly,
    Monthly,
}

struct Form {
    id: Option<String>,
    name: String,
    note: String,
    every: Every,
    weekday: Weekday,
    last_day: bool,
    month_day: u32,
    hour: u32,
    minute: u32,
    mode: ScanMode,
    all_users: bool,
    whole: bool,
    key: String,
}

impl Form {
    fn new(kind: Kind) -> Self {
        Self {
            id: None,
            name: match kind {
                Kind::Clean => "Weekly clean".into(),
                Kind::Backup => "Daily backup".into(),
            },
            note: String::new(),
            every: if kind == Kind::Clean { Every::Weekly } else { Every::Daily },
            weekday: Weekday::Sun,
            last_day: false,
            month_day: 1,
            hour: 3,
            minute: 0,
            mode: ScanMode::Safe,
            all_users: false,
            whole: true,
            key: String::new(),
        }
    }

    fn from_task(t: &Task) -> Self {
        let mut f = Self::new(if matches!(t.action, Action::AutoClean { .. }) { Kind::Clean } else { Kind::Backup });
        f.id = Some(t.id.clone());
        f.name = t.name.clone();
        f.note = t.note.clone();
        let time = match t.schedule {
            Schedule::Daily { time } => {
                f.every = Every::Daily;
                time
            }
            Schedule::Weekly { day, time } => {
                f.every = Every::Weekly;
                f.weekday = day;
                time
            }
            Schedule::Monthly { day, time } => {
                f.every = Every::Monthly;
                match day {
                    MonthDay::Day(d) => f.month_day = d,
                    MonthDay::Last => f.last_day = true,
                }
                time
            }
        };
        use chrono::Timelike;
        (f.hour, f.minute) = (time.hour(), time.minute());
        match &t.action {
            Action::AutoClean { mode, all_users } => (f.mode, f.all_users) = (*mode, *all_users),
            Action::AutoBackup { key } => {
                f.whole = key.is_none();
                f.key = key.clone().unwrap_or_default();
            }
            // Managed on Tools › Microsoft slop, not here.
            Action::RemoveSlop { .. } | Action::KeepAway => {}
        }
        f
    }

    fn schedule(&self) -> Schedule {
        let time = NaiveTime::from_hms_opt(self.hour, self.minute, 0).unwrap();
        match self.every {
            Every::Daily => Schedule::Daily { time },
            Every::Weekly => Schedule::Weekly { day: self.weekday, time },
            Every::Monthly => Schedule::Monthly { day: if self.last_day { MonthDay::Last } else { MonthDay::Day(self.month_day) }, time },
        }
    }

    fn action(&self, kind: Kind) -> Result<Action, String> {
        Ok(match kind {
            Kind::Clean => Action::AutoClean { mode: self.mode, all_users: self.all_users },
            Kind::Backup if self.whole => Action::AutoBackup { key: None },
            Kind::Backup if self.key.trim().is_empty() => return Err("Please select a key to back up.".into()),
            Kind::Backup => Action::AutoBackup { key: Some(self.key.trim().to_string()) },
        })
    }
}

pub struct TaskPanel {
    kind: Kind,
    running: Option<Pending<Result<String, String>>>,
    /// The task `running` runs.
    running_id: Option<String>,
    tasks: Vec<Task>,
    /// What the system scheduler says about each task, read on refresh (it starts a process).
    status: HashMap<String, String>,
    /// The task being created or changed; `None` shows only the list.
    form: Option<Form>,
    browser: Browser,
    /// Delete was asked for this task: confirm first.
    confirm_delete: Option<String>,
}

impl TaskPanel {
    pub fn new(kind: Kind) -> Self {
        Self { kind, running: None, running_id: None, tasks: Vec::new(), status: HashMap::new(), form: None, browser: Browser::default(), confirm_delete: None }
    }

    pub fn refresh(&mut self, cx: &mut Cx) {
        match scheduler::load_tasks() {
            Ok(t) => self.tasks = t,
            Err(e) => cx.error(crate::tf!("Reading scheduled tasks failed: {e:#}")),
        }
        self.status = self.mine().map(|t| (t.id.clone(), crate::platform::sched_status(&t.id))).collect();
    }

    fn mine(&self) -> impl Iterator<Item = &Task> {
        let kind = self.kind;
        self.tasks.iter().filter(move |t| matches!((&t.action, kind), (Action::AutoClean { .. }, Kind::Clean) | (Action::AutoBackup { .. }, Kind::Backup)))
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        if let Some(r) = take(&mut self.running) {
            self.running_id = None;
            match r {
                Ok(s) => cx.info(crate::tf!("Task ran: {s}")),
                Err(e) => cx.error(crate::tf!("Task failed: {e}")),
            }
            self.refresh(cx);
        }
        let mine: Vec<Task> = self.mine().cloned().collect();
        let clean = self.kind == Kind::Clean;
        egui::ScrollArea::vertical().id_salt(("scheduled", clean)).auto_shrink([false, false]).show(ui, |ui| {
            if self.form.is_some() {
                self.editor(ui, cx, p);
                ui.add_space(18.0);
            }
            crate::gui::style::row(ui, |ui| {
                ui.add(style::Label::new(style::strong(tr(if clean { "Scheduled cleans" } else { "Scheduled backups" })).size(16.0)));
                if !mine.is_empty() {
                    style::badge(ui, &mine.len().to_string(), p.accent);
                }
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    if self.form.is_none() && style::primary(ui, icon::PLUS, "New schedule").clicked() {
                        self.form = Some(Form::new(self.kind));
                    }
                });
            });
            ui.add_space(10.0);
            if mine.is_empty() {
                style::card(ui, |ui| {
                    if clean {
                        style::empty_state(
                            ui,
                            icon::CALENDAR,
                            "No scheduled cleans yet",
                            "CleanCore can scan and clean on its own every day, week or month. Every clean is backed up first.",
                        );
                    } else {
                        style::empty_state(ui, icon::CALENDAR, "No scheduled backups yet", "CleanCore can make backups on its own every day, week or month.");
                    }
                    ui.add_space(10.0);
                });
            }
            for t in &mine {
                self.task_card(ui, cx, t, p);
                ui.add_space(10.0);
            }
        });
    }

    /// One scheduled task: what and when, its next and last run, and what can be done with it.
    fn task_card(&mut self, ui: &mut Ui, cx: &mut Cx, t: &Task, p: Palette) {
        let editing = self.form.as_ref().and_then(|f| f.id.as_deref()) == Some(t.id.as_str());
        style::card(ui, |ui| {
            crate::gui::style::row(ui, |ui| {
                style::icon_tile(ui, if matches!(t.action, Action::AutoClean { .. }) { icon::BROOM } else { icon::ARCHIVE }, p.accent, 40.0);
                ui.add_space(4.0);
                crate::gui::style::column(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    // Room for the buttons on the far side.
                    ui.set_width((ui.available_width() - 290.0).max(160.0));
                    crate::gui::style::row(ui, |ui| {
                        ui.add(style::Label::new(style::strong(&t.name).size(15.0)).truncate());
                        if editing {
                            style::badge(ui, "Editing", p.accent);
                        }
                    });
                    ui.add(style::Label::new(style::rich(scheduler::describe(&t.schedule)).size(13.0)).truncate());
                    ui.add(style::Label::new(style::muted(ui, what(&t.action)).size(12.5)).truncate());
                });
                ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                    if style::ghost(ui, icon::TRASH, "Delete").clicked() {
                        self.confirm_delete = Some(t.id.clone());
                    }
                    if style::ghost(ui, icon::SLIDERS, "Edit").clicked() {
                        self.form = Some(Form::from_task(t));
                    }
                    if ui.add_enabled_ui(self.running.is_none(), |ui| style::secondary(ui, icon::PLAY, "Run now")).inner.clicked() {
                        let (settings, id) = (cx.settings.clone(), t.id.clone());
                        self.running_id = Some(id.clone());
                        self.running =
                            Some(Pending::spawn(ui.ctx(), crate::tf!("Running {}", t.name), move || scheduler::execute(&settings, &id).map_err(|e| crate::tf!("{e:#}"))));
                    }
                });
            });
            ui.add_space(8.0);
            style::divider(ui);
            ui.add_space(8.0);
            let next = scheduler::next_run(&t.schedule, chrono::Local::now());
            status_line(ui, icon::CLOCK, p.muted, &crate::tf!("Next run {}", next.format("%a %d %b %Y, %H:%M")), p.muted);
            match &t.last_run {
                Some(r) => {
                    let (ic, col) = if r.ok { (icon::CHECK_CIRCLE, p.success) } else { (icon::WARNING, p.danger) };
                    status_line(ui, ic, col, &crate::tf!("Last run {}: {}", r.time.format("%d %b %H:%M"), r.summary), if r.ok { p.muted } else { p.danger });
                }
                None => status_line(ui, icon::INFO, p.muted, tr("Hasn't run yet"), p.muted),
            }
            if self.status.get(&t.id).is_some_and(|s| s == scheduler::NOT_REGISTERED) {
                status_line(
                    ui,
                    icon::WARNING,
                    p.warning,
                    tr("The system scheduler doesn't have this task, so it won't run on its own. Edit it and save to set it up again."),
                    p.warning,
                );
            }
            if self.running_id.as_deref() == Some(t.id.as_str()) {
                if let Some(pend) = &self.running {
                    ui.add_space(6.0);
                    pend.show_progress(ui);
                }
            }
            if self.confirm_delete.as_deref() == Some(t.id.as_str()) {
                ui.add_space(8.0);
                crate::gui::style::row_wrapped(ui, |ui| {
                    ui.add(style::Label::new(style::rich(crate::tf!("Delete \"{}\"? It stops running on its schedule.", t.name)).color(p.danger)));
                    if style::danger(ui, icon::TRASH, "Delete").clicked() {
                        self.confirm_delete = None;
                        self.delete(&t.id, cx);
                    }
                    if style::ghost(ui, icon::X, "Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                });
            }
        });
    }

    fn delete(&mut self, id: &str, cx: &mut Cx) {
        let r = scheduler::unregister(id).and_then(|_| {
            let mut all = scheduler::load_tasks()?;
            all.retain(|t| t.id != id);
            scheduler::save_tasks(&all)
        });
        match r {
            Ok(()) => {
                cx.info(tr("Schedule deleted"));
                if self.form.as_ref().and_then(|f| f.id.as_deref()) == Some(id) {
                    self.form = None;
                }
            }
            Err(e) => cx.error(crate::tf!("Deleting task failed: {e:#}")),
        }
        self.refresh(cx);
    }

    /// Creating or changing a task.
    fn editor(&mut self, ui: &mut Ui, cx: &mut Cx, p: Palette) {
        let kind = self.kind;
        let mut save = false;
        let mut cancel = false;
        style::card(ui, |ui| {
            let Some(f) = self.form.as_mut() else { return };
            ui.add(style::Label::new(style::strong(tr(if f.id.is_some() { "Edit schedule" } else { "New schedule" })).size(17.0)));
            ui.add_space(12.0);
            field(ui, "Name", |ui| {
                style::text_input(ui, &mut f.name, "", ui.available_width().min(360.0));
            });
            field(ui, "Repeat", |ui| {
                for (e, l) in [(Every::Daily, "Daily"), (Every::Weekly, "Weekly"), (Every::Monthly, "Monthly")] {
                    if style::segment(ui, f.every == e, l).clicked() {
                        f.every = e;
                    }
                }
            });
            match f.every {
                Every::Weekly => field(ui, "On", |ui| {
                    for (d, l) in [
                        (Weekday::Mon, "Mon"),
                        (Weekday::Tue, "Tue"),
                        (Weekday::Wed, "Wed"),
                        (Weekday::Thu, "Thu"),
                        (Weekday::Fri, "Fri"),
                        (Weekday::Sat, "Sat"),
                        (Weekday::Sun, "Sun"),
                    ] {
                        if style::segment(ui, f.weekday == d, l).clicked() {
                            f.weekday = d;
                        }
                    }
                }),
                Every::Monthly => field(ui, "Day", |ui| {
                    ui.add_enabled(!f.last_day, egui::DragValue::new(&mut f.month_day).range(1..=28));
                    ui.checkbox(&mut f.last_day, style::vis(tr("Last day of the month")));
                }),
                Every::Daily => {}
            }
            field(ui, "At", |ui| {
                // A time reads left to right in every language (egui's `horizontal` follows the
                // page's direction).
                ui.allocate_ui_with_layout(egui::vec2(120.0, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::DragValue::new(&mut f.hour).range(0..=23).custom_formatter(|v, _| crate::tf!("{v:02.0}")));
                    ui.add(style::Label::new(":"));
                    ui.add(egui::DragValue::new(&mut f.minute).range(0..=59).custom_formatter(|v, _| crate::tf!("{v:02.0}")));
                });
            });
            if kind == Kind::Clean {
                field(ui, "Scan", |ui| {
                    for (m, l) in [(ScanMode::Safe, "Safe"), (ScanMode::Moderate, "Moderate"), (ScanMode::Advanced, "Advanced")] {
                        if style::segment(ui, f.mode == m, l).clicked() {
                            f.mode = m;
                        }
                    }
                });
                field(ui, "Accounts", |ui| {
                    if style::segment(ui, !f.all_users, "Current account").clicked() {
                        f.all_users = false;
                    }
                    if style::segment(ui, f.all_users, "All accounts").clicked() {
                        f.all_users = true;
                    }
                });
            } else {
                field(ui, "Back up", |ui| {
                    let everything = if cfg!(windows) { tr("Whole registry") } else { tr("Whole config store") };
                    if style::segment(ui, f.whole, everything).clicked() {
                        f.whole = true;
                    }
                    if style::segment(ui, !f.whole, "One key / folder").clicked() {
                        f.whole = false;
                    }
                });
                if !f.whole {
                    field(ui, "Location", |ui| {
                        style::key_or_path_input(ui, "schedule-key", &mut f.key, "Pick below or type a path", ui.available_width().min(460.0), style::Pick::Any);
                    });
                    egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(8).show(ui, |ui| {
                        ui.set_height(170.0);
                        self.browser.show(ui, "backup-key-browser");
                    });
                    if let Some(sel) = self.browser.selected.take() {
                        f.key = sel;
                    }
                    ui.add_space(10.0);
                }
            }
            field(ui, "Note", |ui| {
                style::text_input(ui, &mut f.note, "Optional", ui.available_width().min(360.0));
            });
            style::divider(ui);
            ui.add_space(10.0);
            let sched = f.schedule();
            status_line(ui, icon::CALENDAR, p.accent, &scheduler::describe(&sched), p.text);
            status_line(ui, icon::CLOCK, p.muted, &crate::tf!("Next run {}", scheduler::next_run(&sched, chrono::Local::now()).format("%a %d %b %Y, %H:%M")), p.muted);
            ui.add_space(12.0);
            crate::gui::style::row(ui, |ui| {
                save = style::primary(ui, icon::FLOPPY, "Save").clicked();
                cancel = style::ghost(ui, icon::X, "Cancel").clicked();
            });
        });
        if save {
            self.save(cx);
        }
        if cancel {
            self.form = None;
        }
    }

    fn save(&mut self, cx: &mut Cx) {
        let Some(f) = &self.form else { return };
        if f.name.trim().is_empty() {
            cx.error(tr("Please enter a name."));
            return;
        }
        let action = match f.action(self.kind) {
            Ok(a) => a,
            Err(e) => return cx.error(tr(&e)),
        };
        let r = (|| -> anyhow::Result<String> {
            let mut all = scheduler::load_tasks()?;
            let existing = f.id.as_ref().and_then(|id| all.iter().position(|t| &t.id == id));
            let task = Task {
                id: match &f.id {
                    Some(id) => id.clone(),
                    None => scheduler::make_id(&f.name, &all),
                },
                name: f.name.trim().to_string(),
                note: f.note.clone(),
                schedule: f.schedule(),
                action,
                created: existing.map(|i| all[i].created).unwrap_or_else(chrono::Local::now),
                last_run: existing.and_then(|i| all[i].last_run.clone()),
            };
            if all.iter().any(|t| t.id != task.id && t.same_params(&task)) {
                anyhow::bail!("{}", tr("A task with the same parameters already exists!"));
            }
            scheduler::register(&task)?;
            let name = task.name.clone();
            match existing {
                Some(i) => all[i] = task,
                None => all.push(task),
            }
            scheduler::save_tasks(&all)?;
            Ok(name)
        })();
        match r {
            Ok(name) => {
                cx.info(crate::tf!("Saved \"{name}\""));
                self.form = None;
            }
            Err(e) => cx.error(crate::tf!("Saving task failed: {e:#}")),
        }
        self.refresh(cx);
    }
}

/// A form field: its label above its controls.
fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    crate::gui::style::column(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.add(style::Label::new(style::muted(ui, tr(label)).size(12.5)));
        crate::gui::style::row_wrapped(ui, add);
    });
    ui.add_space(12.0);
}

/// An icon and a line of text.
fn status_line(ui: &mut Ui, glyph: &str, glyph_color: egui::Color32, text: &str, color: egui::Color32) {
    crate::gui::style::row_top(ui, |ui| {
        ui.add(style::Label::new(style::rich(glyph).color(glyph_color)));
        ui.add(style::Label::new(style::rich(text).size(13.0).color(color)).wrap());
    });
}

/// What a task does, in a few words.
fn what(a: &Action) -> String {
    match a {
        Action::AutoClean { mode, all_users } => {
            let depth = match mode {
                ScanMode::Safe => tr("Safe"),
                ScanMode::Moderate => tr("Moderate"),
                ScanMode::Advanced => tr("Advanced"),
            };
            format!("{depth} · {}", tr(if *all_users { "All accounts" } else { "Current account" }))
        }
        Action::AutoBackup { key: None } => tr(if cfg!(windows) { "Whole registry" } else { "Whole config store" }).to_string(),
        Action::AutoBackup { key: Some(k) } => k.clone(),
        Action::RemoveSlop { .. } | Action::KeepAway => String::new(),
    }
}
