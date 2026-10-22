//! Lazily loaded registry-key / folder browser.

use super::style;
use crate::platform::{browse_children, browse_roots, BrowseNode};
use eframe::egui::{self, collapsing_header::CollapsingState, Ui};
use std::collections::HashMap;

#[derive(Default)]
pub struct Browser {
    children: HashMap<String, Vec<BrowseNode>>,
    pub selected: Option<String>,
}

impl Browser {
    pub fn show(&mut self, ui: &mut Ui, id_salt: &str) {
        egui::ScrollArea::both().id_salt(id_salt).auto_shrink([false, false]).show(ui, |ui| {
            for root in browse_roots() {
                self.node(ui, id_salt, &root);
            }
        });
    }

    /// Forget cached children (e.g. after a backup restore created keys).
    pub fn reload(&mut self) {
        self.children.clear();
    }

    fn node(&mut self, ui: &mut Ui, salt: &str, n: &BrowseNode) {
        let is_sel = self.selected.as_deref() == Some(n.path.as_str());
        if n.leaf {
            if ui.selectable_label(is_sel, style::vis(&n.name)).on_hover_text(style::vis(&n.path)).clicked() {
                self.selected = Some(n.path.clone());
            }
            return;
        }
        let id = ui.make_persistent_id((salt, &n.path));
        let state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let selected = &mut self.selected;
        let header = state.show_header(ui, |ui| {
            if ui.selectable_label(is_sel, style::vis(&n.name)).on_hover_text(style::vis(&n.path)).clicked() {
                *selected = Some(n.path.clone());
            }
        });
        header.body(|ui| {
            let kids = self.children.entry(n.path.clone()).or_insert_with(|| browse_children(&n.path)).clone();
            if kids.is_empty() {
                ui.add(style::Label::new(style::rich("(empty)").weak()));
            }
            for k in &kids {
                self.node(ui, salt, k);
            }
        });
    }
}
