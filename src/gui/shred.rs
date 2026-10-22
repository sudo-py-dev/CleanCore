//! Tools › Unrecoverable delete (files and folders, dropped or typed) and Evidence remover
//! (free space of a drive): overwrite passes, a clear warning, progress with pause / resume /
//! cancel.

use super::style::{self, icon, Palette};
use super::{Cx, Pending};
use crate::i18n::tr;
use crate::shred::{self, Control};
use eframe::egui::{self, Ui};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A running overwrite job.
struct Job<T> {
    pending: Pending<Result<T, String>>,
    ctl: Arc<Control>,
}

fn passes_picker(ui: &mut Ui, cx: &mut Cx) {
    crate::gui::style::row(ui, |ui| {
        ui.add(style::Label::new(tr("Overwrite passes")));
        for (n, tip) in [(1, "Fast; enough for modern drives"), (3, "Zeros, ones, random (default)"), (7, "Seven passes; slow")] {
            if style::segment(ui, cx.settings.shred_passes == n, &n.to_string()).on_hover_text(style::vis(tr(tip))).clicked() {
                cx.settings.shred_passes = n;
                cx.save_settings();
            }
        }
    });
}

fn ssd_note(ui: &mut Ui, p: Palette) {
    egui::Frame::new().fill(p.warning.gamma_multiply(if p.dark { 0.18 } else { 0.10 })).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        crate::gui::style::row_top(ui, |ui| {
            ui.add(style::Label::new(style::rich(icon::INFO).size(17.0).color(p.warning)));
            ui.add(
                style::Label::new(style::rich(tr("On SSDs, flash drives and copy-on-write filesystems (btrfs, ZFS) the drive may keep old copies of data in places software can't reach. Overwriting is most reliable on hard disks; full-disk encryption protects SSDs best.")).size(12.5).color(p.text))
                    .wrap(),
            );
        });
    });
}

/// Progress bar with Pause / Resume / Cancel.
fn job_progress(ui: &mut Ui, ctl: &Control, label: &str, p: Palette) {
    style::card(ui, |ui| {
        let paused = ctl.pause.load(Ordering::Relaxed);
        crate::gui::style::row(ui, |ui| {
            if !paused {
                ui.spinner();
            }
            let pass = ctl.pass.load(Ordering::Relaxed);
            ui.add(style::Label::new(style::strong(if paused {
                crate::tf!("{label}: paused")
            } else if pass > 0 {
                crate::tf!("{label}: pass {pass}")
            } else {
                label.to_string()
            })));
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                if style::secondary(ui, icon::X, "Cancel").clicked() {
                    ctl.cancel.store(true, Ordering::Relaxed);
                }
                if paused {
                    if style::secondary(ui, icon::PLAY, "Resume").clicked() {
                        ctl.pause.store(false, Ordering::Relaxed);
                    }
                } else if style::secondary(ui, icon::PAUSE, "Pause").clicked() {
                    ctl.pause.store(true, Ordering::Relaxed);
                }
            });
        });
        ui.add_space(6.0);
        ui.add(egui::ProgressBar::new(ctl.fraction()).fill(p.accent).desired_height(10.0));
        ui.add_space(4.0);
        let cur = ctl.current.lock().map(|c| c.clone()).unwrap_or_default();
        ui.add(style::Label::new(
            style::muted(ui, crate::tf!("{} of {}", super::human(ctl.done.load(Ordering::Relaxed)), super::human(ctl.total.load(Ordering::Relaxed)))).size(12.5),
        ));
        ui.add(style::Label::new(style::rich(cur).size(11.5).color(p.faint)).truncate());
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
}

// ---------------------------------------------------------------- Unrecoverable Delete

/// A path queued for shredding, with what it is and its size measured once when added.
struct Item {
    path: PathBuf,
    dir: bool,
    size: u64,
}

pub struct ShredView {
    items: Vec<Item>,
    new_path: String,
    confirm: bool,
    job: Option<Job<shred::ShredReport>>,
}

impl ShredView {
    pub fn new() -> Self {
        Self { items: Vec::new(), new_path: String::new(), confirm: false, job: None }
    }

    fn add(&mut self, p: PathBuf, cx: &mut Cx) {
        match std::fs::symlink_metadata(&p) {
            Err(_) => cx.error(crate::tf!("{} doesn't exist", p.display())),
            Ok(_) if self.items.iter().any(|i| i.path == p) => {}
            Ok(m) => self.items.push(Item { dir: m.is_dir(), size: crate::backup::dir_size(&p), path: p }),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        if let Some(job) = &self.job {
            if let Some(r) = job.pending.poll() {
                match r {
                    Ok(r) if r.failed.is_empty() => cx.info(crate::tf!("Shredded {} file(s) and {} folder(s), {}", r.files, r.folders, super::human(r.bytes))),
                    Ok(r) => cx.error(crate::tf!("Shredded {} file(s); {} item(s) failed: {}", r.files, r.failed.len(), r.failed[0].1)),
                    Err(e) => cx.error(e),
                }
                self.job = None;
                self.items.retain(|i| std::fs::symlink_metadata(&i.path).is_ok());
            } else {
                job_progress(ui, &job.ctl, "Shredding", p);
                return;
            }
        }
        // Files dropped onto the window are added.
        for f in ui.ctx().input(|i| i.raw.dropped_files.clone()) {
            let path = f.path();
            if !path.as_os_str().is_empty() {
                self.add(path.to_path_buf(), cx);
            }
        }

        crate::gui::style::row(ui, |ui| {
            passes_picker(ui, cx);
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!self.items.is_empty(), |ui| {
                    if style::danger(ui, icon::TRASH, &crate::tf!("Destroy {} item(s)", self.items.len())).clicked() {
                        self.confirm = true;
                    }
                });
            });
        });
        ui.add_space(10.0);
        style::card(ui, |ui| {
            style::section_title(ui, "Files and folders to destroy");
            ui.add(style::Label::new(style::muted(ui, tr("Drop files or folders on this window, or type a path.")).size(12.5)));
            ui.add_space(6.0);
            let mut remove = None;
            if self.items.is_empty() {
                ui.add(style::Label::new(style::rich(tr("Nothing added yet")).color(p.faint)));
            }
            for (i, item) in self.items.iter().enumerate() {
                crate::gui::style::row(ui, |ui| {
                    let ic = if item.dir { icon::FOLDER } else { icon::FILE };
                    ui.add(style::Label::new(style::rich(ic).color(p.accent)));
                    ui.add(style::Label::new(style::rich(item.path.display().to_string()).monospace().size(12.5)).truncate());
                    ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                        if style::icon_button(ui, icon::X, "Remove from the list").clicked() {
                            remove = Some(i);
                        }
                        ui.add(style::Label::new(style::muted(ui, super::human(item.size))));
                    });
                });
            }
            if let Some(i) = remove {
                self.items.remove(i);
            }
            ui.add_space(6.0);
            crate::gui::style::row(ui, |ui| {
                let r = style::text_input(ui, &mut self.new_path, if cfg!(windows) { r"C:\Users\me\secret.docx" } else { "/home/me/secret.pdf" }, 460.0);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(p) = style::browse_button(ui, "shred-add", style::Pick::Any, &self.new_path) {
                    self.add(PathBuf::from(p), cx);
                }
                if (style::secondary(ui, icon::PLUS, "Add").clicked() || enter) && !self.new_path.trim().is_empty() {
                    let path = PathBuf::from(crate::pathutil::expand_env(self.new_path.trim()));
                    self.add(path, cx);
                    self.new_path.clear();
                }
            });
        });
        ui.add_space(10.0);
        ssd_note(ui, p);

        if self.confirm {
            let mut close = false;
            egui::Modal::new(egui::Id::unique("shred-confirm")).show(ui.ctx(), |ui| {
                ui.set_width(440.0);
                ui.add(style::Label::new(style::strong(crate::tf!("Destroy {} item(s)?", self.items.len())).size(18.0)));
                ui.add_space(6.0);
                ui.add(style::Label::new(style::muted(
                    ui,
                    crate::tf!(
                        "They are overwritten {} time(s) and deleted. Nothing is backed up and they can't be recovered: not from the Trash, not by CleanCore.",
                        cx.settings.shred_passes
                    ),
                )));
                ui.add_space(12.0);
                crate::gui::style::row(ui, |ui| {
                    if style::danger(ui, icon::TRASH, "Destroy").clicked() {
                        let ctl = Arc::new(Control::default());
                        let (c, items, passes) = (ctl.clone(), self.items.iter().map(|i| i.path.clone()).collect::<Vec<_>>(), cx.settings.shred_passes);
                        self.job = Some(Job { pending: Pending::spawn(ui.ctx(), "Shredding", move || shred::shred(&items, passes, &c).map_err(|e| crate::tf!("{e:#}"))), ctl });
                        close = true;
                    }
                    if style::secondary(ui, "", "Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = false;
            }
        }
    }
}

// ---------------------------------------------------------------- Evidence Remover

pub struct EvidenceView {
    drives: Vec<crate::platform::Drive>,
    loaded: bool,
    selected: Option<usize>,
    confirm: bool,
    job: Option<Job<u64>>,
}

impl EvidenceView {
    pub fn new() -> Self {
        Self { drives: Vec::new(), loaded: false, selected: None, confirm: false, job: None }
    }

    pub fn refresh(&mut self) {
        if self.job.is_none() {
            self.loaded = false;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let p = Palette::get(ui.ctx());
        if !self.loaded {
            self.drives = crate::platform::drives();
            self.loaded = true;
        }
        if let Some(job) = &self.job {
            if let Some(r) = job.pending.poll() {
                match r {
                    Ok(n) => cx.info(crate::tf!("Free space wiped: wrote {} of overwrite data", super::human(n))),
                    Err(e) => cx.error(e),
                }
                self.job = None;
                self.loaded = false;
            } else {
                job_progress(ui, &job.ctl, "Wiping free space", p);
                return;
            }
        }

        let drive = self.selected.and_then(|i| self.drives.get(i)).cloned();
        crate::gui::style::row(ui, |ui| {
            passes_picker(ui, cx);
            ui.with_layout(crate::gui::style::far_layout(egui::Align::Center), |ui| {
                ui.add_enabled_ui(drive.is_some(), |ui| {
                    if style::primary(ui, icon::DRIVES, "Wipe free space").on_hover_text(style::vis(tr("Pick a drive first"))).clicked() {
                        self.confirm = true;
                    }
                });
            });
        });
        ui.add_space(6.0);
        ui.add(style::Label::new(style::muted(
            ui,
            tr("Deleted files stay on the drive until something overwrites them. This fills the drive's free space with random data, then deletes it again. Pick a drive:"),
        )));
        ui.add_space(10.0);
        crate::gui::style::row_wrapped(ui, |ui| {
            for (i, d) in self.drives.iter().enumerate() {
                let sel = self.selected == Some(i);
                let (rect, r) = ui.allocate_exact_size(egui::vec2(250.0, 92.0), egui::Sense::click());
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, sel, &d.label));
                style::tile_frame(ui, rect, &r, sel);
                let painter = ui.painter();
                let mr = crate::gui::style::Mirror::new(rect);
                let top = rect.top() + 16.0;
                painter.text(mr.pos(16.0, top), mr.start(egui::Align::Min), style::vstr(icon::DRIVES), egui::FontId::proportional(22.0), if sel { p.accent } else { p.muted });
                painter.text(mr.pos(50.0, top), mr.start(egui::Align::Min), style::vstr(&d.label), egui::FontId::new(14.5, style::semibold()), p.text);
                painter.text(
                    mr.pos(50.0, top + 20.0),
                    mr.start(egui::Align::Min),
                    style::vstr(crate::tf!("{} · {}", d.path.display(), d.fs)),
                    egui::FontId::proportional(12.0),
                    p.muted,
                );
                let bar = egui::Rect::from_min_size(rect.left_bottom() + egui::vec2(16.0, -26.0), egui::vec2(rect.width() - 32.0, 6.0));
                painter.rect_filled(bar, 3.0, p.surface_alt);
                let used = if d.total > 0 { 1.0 - d.free as f32 / d.total as f32 } else { 0.0 };
                // The used part fills from the start edge.
                painter.rect_filled(crate::gui::style::Mirror::new(bar).rect(0.0, bar.top(), egui::vec2(bar.width() * used, bar.height())), 3.0, p.accent);
                painter.text(
                    mr.pos(16.0, bar.bottom() + 4.0),
                    mr.start(egui::Align::Min),
                    style::vstr(crate::tf!("{} free of {}", super::human(d.free), super::human(d.total))),
                    egui::FontId::proportional(11.5),
                    p.muted,
                );
                if r.clicked() {
                    self.selected = Some(i);
                }
            }
        });
        ui.add_space(12.0);
        ssd_note(ui, p);
        let drive = self.selected.and_then(|i| self.drives.get(i)).cloned();

        if self.confirm {
            let mut close = false;
            if let Some(d) = &drive {
                egui::Modal::new(egui::Id::unique("wipe-confirm")).show(ui.ctx(), |ui| {
                    ui.set_width(460.0);
                    ui.add(style::Label::new(style::strong(crate::tf!("Wipe the free space of {}?", d.label)).size(18.0)));
                    ui.add_space(6.0);
                    let passes = cx.settings.shred_passes.min(7);
                    ui.add(style::Label::new(style::muted(ui, crate::tf!("{} of random data is written {passes} time(s). This can take a long time, and the drive is briefly full: programs saving files meanwhile may fail. Your files are not touched.", super::human(d.free)))));
                    ui.add_space(12.0);
                    crate::gui::style::row(ui, |ui| {
                        if style::primary(ui, icon::DRIVES, "Wipe").clicked() {
                            let ctl = Arc::new(Control::default());
                            let (c, dir) = (ctl.clone(), d.path.clone());
                            self.job = Some(Job { pending: Pending::spawn(ui.ctx(), "Wiping", move || shred::wipe_free_space(&dir, passes, None, &c).map_err(|e| crate::tf!("{e:#}"))), ctl });
                            close = true;
                        }
                        if style::secondary(ui, "", "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            } else {
                close = true;
            }
            if close {
                self.confirm = false;
            }
        }
    }
}
