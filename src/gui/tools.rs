//! Tools page: Junk Files, Duplicate Finder, History Cleaner, Unrecoverable Delete, Evidence Remover and
//! System Tools, as tabs.

use super::style::{self, icon};
use super::Cx;
use crate::i18n::tr;
use eframe::egui::Ui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Junk,
    Dupes,
    History,
    Shred,
    Evidence,
    System,
    Slop,
}

pub struct ToolsView {
    tab: Tab,
    junk: super::junk::JunkView,
    dupes: super::dupes::DupesView,
    history: super::history::HistoryView,
    shred: super::shred::ShredView,
    evidence: super::shred::EvidenceView,
    system: super::systools::SystemToolsView,
    slop: super::slop::SlopView,
}

impl ToolsView {
    pub fn new() -> Self {
        Self {
            tab: Tab::Junk,
            junk: super::junk::JunkView::new(),
            dupes: super::dupes::DupesView::new(),
            history: super::history::HistoryView::new(),
            shred: super::shred::ShredView::new(),
            evidence: super::shred::EvidenceView::new(),
            system: super::systools::SystemToolsView::new(),
            slop: super::slop::SlopView::new(),
        }
    }

    pub fn refresh(&mut self) {
        self.history.refresh();
        self.evidence.refresh();
        self.slop.refresh();
    }

    pub fn main(&mut self, ui: &mut Ui, cx: &mut Cx) {
        let (title, subtitle) = match self.tab {
            Tab::Junk => ("Junk Files", "Temporary files, editor backups and crash dumps left on your drives"),
            Tab::Dupes => ("Duplicate Finder", "Identical files in your folders, and deleting the extra copies"),
            Tab::History => ("History Cleaner", "Browser history and caches, recent files and other traces"),
            Tab::Shred => ("Unrecoverable Delete", "Overwrite files and folders so they can't be recovered"),
            Tab::Evidence => ("Evidence Remover", "Overwrite free space so already-deleted files can't be recovered"),
            Tab::System => (if cfg!(windows) { tr("Windows Tools") } else { tr("System Tools") }, "The operating system's own utilities in one place"),
            Tab::Slop => ("Microsoft Slop Cleaner", "Remove preinstalled apps and turn off Copilot, ads and telemetry"),
        };
        style::page_header(ui, title, subtitle, |_| {});
        let mut items = vec![
            (Tab::Junk, icon::BROOM, "Junk files", None),
            (Tab::Dupes, icon::COPY, "Duplicates", None),
            (Tab::History, icon::HISTORY, "History", None),
            (Tab::Shred, icon::SHIELD_WARNING, "Unrecoverable delete", None),
            (Tab::Evidence, icon::DRIVES, "Evidence remover", None),
            (Tab::System, icon::WRENCH, if cfg!(windows) { tr("Windows tools") } else { tr("System tools") }, None),
        ];
        // Only Windows has Microsoft slop to clean.
        if cfg!(windows) {
            items.push((Tab::Slop, icon::SPARKLE, "Microsoft slop", None));
        }
        style::tabs(ui, &mut self.tab, &items);
        match self.tab {
            Tab::Junk => self.junk.ui(ui, cx),
            Tab::Dupes => self.dupes.ui(ui, cx),
            Tab::History => self.history.ui(ui, cx),
            Tab::Shred => self.shred.ui(ui, cx),
            Tab::Evidence => self.evidence.ui(ui, cx),
            Tab::System => self.system.ui(ui, cx),
            Tab::Slop => self.slop.ui(ui, cx),
        }
    }
}
