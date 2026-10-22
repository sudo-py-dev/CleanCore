//! Auto Clean / Auto Backup task editor: saved tasks on the left, the selected task's
//! settings on the right (like the original's split window).

use super::style::{self, icon, Palette};
use super::{take, tree::Browser, Cx, Pending};
use crate::i18n::tr;
use crate::model::ScanMode;
use crate::scheduler::{self, Action, MonthDay, Schedule, Task};
use chrono::{NaiveTime, Weekday};
use eframe::egui::{self, Ui};

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
    tasks: Vec<Task>,
    form: Form,
    browser: Browser,
}

impl TaskPanel {
    pub fn new(kind: Kind) -> Self {
        Self { kind, running: None, tasks: Vec::new(), form: Form::new(kind), browser: Browser::default() }
    }

    pub fn refresh(&mut self, cx: &mut Cx) {
        match scheduler::load_tasks() {
            Ok(t) => self.tasks = t,
            Err(e) => cx.error(crate::tf!("Reading scheduled tasks failed: {e:#}")),
        }
    }

    fn mine(&self) -> impl Iterator<Item = &Task> {
        let kind = self.kind;
        self.tasks.iter().filter(move |t| matches!((&t.action, kind), (Action::AutoClean { .. }, Kind::Clean) | (Action::AutoBackup { .. }, Kind::Backup)))
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        let mine: Vec<Task> = self.mine().cloned().collect();
        crate::gui::style::row_top(ui, |ui| {
            // Saved tasks
            ui.allocate_ui_with_layout(egui::vec2(290.0, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                style::card(ui, |ui| {
                    crate::gui::style::row(ui, |ui| {
                        ui.add(style::Label::new(style::strong(tr(match self.kind {
                            Kind::Clean => "Scheduled cleans",
                            Kind::Backup => "Scheduled backups",
                        }))));
                        ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                            if style::icon_button(ui, icon::PLUS, "New task").clicked() {
                                self.form = Form::new(self.kind);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    if mine.is_empty() {
                        ui.add(style::Label::new(style::muted(ui, tr("No tasks yet. Fill in the form and save."))));
                    }
                    for t in &mine {
                        let sel = self.form.id.as_deref() == Some(t.id.as_str());
                        if task_row(ui, t, sel, p).clicked() {
                            self.form = Form::from_task(t);
                        }
                    }
                });
            });
            ui.add_space(6.0);
            // Form
            crate::gui::style::column(ui, |ui| {
                egui::ScrollArea::vertical().id_salt(("form", self.kind == Kind::Clean)).show(ui, |ui| style::card(ui, |ui| self.form_ui(ui, cx)));
            });
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
                cx.info(crate::tf!("Deleted task {id}"));
                self.form = Form::new(self.kind);
            }
            Err(e) => cx.error(crate::tf!("Deleting task failed: {e:#}")),
        }
        self.refresh(cx);
    }

    fn form_ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        ui.add(style::Label::new(style::strong(if self.form.id.is_some() { tr("Edit task") } else { tr("New task") }).size(16.0)));
        ui.add_space(8.0);
        let f = &mut self.form;
        egui::Grid::new("task-form").num_columns(2).spacing([20.0, 12.0]).show(ui, |ui| {
            ui.add(style::Label::new(style::muted(ui, tr("Name"))));
            style::text_input(ui, &mut f.name, "", 320.0);
            ui.end_row();
            ui.add(style::Label::new(style::muted(ui, tr("Note"))));
            style::text_input(ui, &mut f.note, "Optional", 320.0);
            ui.end_row();
            ui.add(style::Label::new(style::muted(ui, tr("Repeat"))));
            crate::gui::style::row(ui, |ui| {
                for (e, l) in [(Every::Daily, "Daily"), (Every::Weekly, "Weekly"), (Every::Monthly, "Monthly")] {
                    if style::segment(ui, f.every == e, l).clicked() {
                        f.every = e;
                    }
                }
            });
            ui.end_row();
            match f.every {
                Every::Weekly => {
                    ui.add(style::Label::new(style::muted(ui, tr("On"))));
                    crate::gui::style::row(ui, |ui| {
                        for d in [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun] {
                            if style::segment(ui, f.weekday == d, &d.to_string()).clicked() {
                                f.weekday = d;
                            }
                        }
                    });
                    ui.end_row();
                }
                Every::Monthly => {
                    ui.add(style::Label::new(style::muted(ui, tr("Day"))));
                    crate::gui::style::row(ui, |ui| {
                        ui.add_enabled(!f.last_day, egui::DragValue::new(&mut f.month_day).range(1..=28));
                        ui.checkbox(&mut f.last_day, style::vis(tr("Last day of the month")));
                    });
                    ui.end_row();
                }
                Every::Daily => {}
            }
            ui.add(style::Label::new(style::muted(ui, tr("At"))));
            crate::gui::style::row(ui, |ui| {
                ui.add(egui::DragValue::new(&mut f.hour).range(0..=23).custom_formatter(|v, _| crate::tf!("{v:02.0}")));
                ui.add(style::Label::new(":"));
                ui.add(egui::DragValue::new(&mut f.minute).range(0..=59).custom_formatter(|v, _| crate::tf!("{v:02.0}")));
            });
            ui.end_row();
            if self.kind == Kind::Clean {
                ui.add(style::Label::new(style::muted(ui, tr("Scan"))));
                crate::gui::style::row(ui, |ui| {
                    for (m, l) in [(ScanMode::Safe, "Safe"), (ScanMode::Moderate, "Moderate"), (ScanMode::Advanced, "Advanced")] {
                        if style::segment(ui, f.mode == m, l).clicked() {
                            f.mode = m;
                        }
                    }
                });
                ui.end_row();
                ui.add(style::Label::new(style::muted(ui, tr("Accounts"))));
                crate::gui::style::row(ui, |ui| {
                    if style::segment(ui, !f.all_users, "Current account").clicked() {
                        f.all_users = false;
                    }
                    if style::segment(ui, f.all_users, "All accounts").clicked() {
                        f.all_users = true;
                    }
                });
                ui.end_row();
            } else {
                ui.add(style::Label::new(style::muted(ui, tr("Back up"))));
                crate::gui::style::row(ui, |ui| {
                    let everything = if cfg!(windows) { tr("Whole registry") } else { tr("Whole config store") };
                    if style::segment(ui, f.whole, everything).clicked() {
                        f.whole = true;
                    }
                    if style::segment(ui, !f.whole, "One key / folder").clicked() {
                        f.whole = false;
                    }
                });
                ui.end_row();
                if !f.whole {
                    ui.add(style::Label::new(style::muted(ui, tr("Location"))));
                    style::key_or_path_input(ui, "schedule-key", &mut f.key, "Pick below or type a path", 420.0, style::Pick::Any);
                    ui.end_row();
                }
            }
        });
        if self.kind == Kind::Backup && !self.form.whole {
            ui.add_space(6.0);
            egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(8).show(ui, |ui| {
                ui.set_height(170.0);
                self.browser.show(ui, "backup-key-browser");
            });
            if let Some(sel) = self.browser.selected.take() {
                self.form.key = sel;
            }
        }

        ui.add_space(12.0);
        style::divider(ui);
        ui.add_space(10.0);
        let now = chrono::Local::now();
        let sched = self.form.schedule();
        crate::gui::style::row(ui, |ui| {
            ui.add(style::Label::new(style::rich(icon::CLOCK).color(p.muted)));
            ui.add(style::Label::new(style::muted(ui, crate::tf!("Next run {}", scheduler::next_run(&sched, now).format("%a %d %b %Y, %H:%M")))));
        });
        if let Some(id) = &self.form.id {
            if let Some(t) = self.tasks.iter().find(|t| &t.id == id) {
                crate::gui::style::row(ui, |ui| {
                    ui.add(style::Label::new(style::rich(icon::CALENDAR).color(p.muted)));
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("System scheduler: {}", crate::platform::sched_status(id)))));
                });
                if let Some(r) = &t.last_run {
                    crate::gui::style::row(ui, |ui| {
                        let (ic, col) = if r.ok { (icon::CHECK_CIRCLE, p.success) } else { (icon::WARNING, p.danger) };
                        ui.add(style::Label::new(style::rich(ic).color(col)));
                        ui.add(style::Label::new(style::rich(crate::tf!("Last run {} — {}", r.time.format("%d %b %H:%M"), r.summary)).color(if r.ok {
                            p.muted
                        } else {
                            p.danger
                        })));
                    });
                }
            }
        }
        ui.add_space(12.0);
        crate::gui::style::row(ui, |ui| {
            if style::primary(ui, icon::FLOPPY, "Save").clicked() {
                self.save(cx);
            }
            if let Some(id) = self.form.id.clone() {
                let idle = self.running.is_none();
                if ui.add_enabled_ui(idle, |ui| style::secondary(ui, icon::PLAY, "Run now")).inner.clicked() {
                    let settings = cx.settings.clone();
                    let task_id = id.clone();
                    self.running = Some(Pending::spawn(ui.ctx(), crate::tf!("Running {id}"), move || scheduler::execute(&settings, &task_id).map_err(|e| crate::tf!("{e:#}"))));
                }
                if style::danger(ui, icon::TRASH, "Delete").clicked() {
                    self.delete(&id, cx);
                }
            }
        });
        if let Some(pend) = &self.running {
            pend.show_progress(ui);
        }
        if let Some(r) = take(&mut self.running) {
            match r {
                Ok(s) => cx.info(crate::tf!("Task ran: {s}")),
                Err(e) => cx.error(crate::tf!("Task failed: {e}")),
            }
            self.refresh(cx);
        }
    }

    fn save(&mut self, cx: &mut Cx) {
        let f = &self.form;
        if f.name.trim().is_empty() {
            cx.error(tr("Please enter a name."));
            return;
        }
        let action = match f.action(self.kind) {
            Ok(a) => a,
            Err(e) => return cx.error(e),
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
                anyhow::bail!("A task with the same parameters already exists!");
            }
            scheduler::register(&task)?;
            let id = task.id.clone();
            match existing {
                Some(i) => all[i] = task,
                None => all.push(task),
            }
            scheduler::save_tasks(&all)?;
            Ok(id)
        })();
        match r {
            Ok(id) => {
                cx.info(crate::tf!("Saved task {id}"));
                self.form.id = Some(id);
            }
            Err(e) => cx.error(crate::tf!("Saving task failed: {e:#}")),
        }
        self.refresh(cx);
    }
}

fn task_row(ui: &mut Ui, t: &Task, selected: bool, p: Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &t.name));
    let bg = if selected {
        p.accent_soft
    } else if resp.hovered() {
        p.surface_alt
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 8.0, bg);
    let ic = if matches!(t.action, Action::AutoClean { .. }) { icon::BROOM } else { icon::ARCHIVE };
    let mr = crate::gui::style::Mirror::new(rect);
    ui.painter().text(mr.pos(12.0, rect.center().y), mr.start(egui::Align::Center), style::vstr(ic), egui::FontId::proportional(18.0), if selected { p.accent } else { p.muted });
    ui.painter().text(
        mr.pos(40.0, rect.top() + 16.0),
        mr.start(egui::Align::Center),
        style::vstr(&t.name),
        egui::FontId::new(14.0, style::semibold()),
        if selected { p.accent } else { p.text },
    );
    let sub = match &t.last_run {
        Some(r) if !r.ok => crate::tf!("{} · last run failed", t.schedule),
        _ => t.schedule.to_string(),
    };
    ui.painter().text(mr.pos(40.0, rect.top() + 35.0), mr.start(egui::Align::Center), style::vstr(sub), egui::FontId::proportional(12.5), p.muted);
    resp
}
