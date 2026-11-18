//! Moving the dividers: by dragging them, or by the keys of the resize mode.
//! Both end up in `Ui.layout`, the proportions `layout` turns into
//! rectangles on every frame and the shell keeps in a file.

use super::keys::Action;
use super::layout::{HitMap, LayoutPrefs};
use super::{App, Command, Effects, Mode};

impl App {
    /// The terminal as last drawn, less the mode bar: where the dividers move.
    fn main_size(&self) -> Option<(u16, u16)> {
        let area = self.ui.hits.area;
        (!area.is_empty() && !self.ui.zoomed).then(|| (area.width, area.height - 1))
    }

    /// Sends the layout to be saved, if it changed since the last time.
    pub(super) fn save_layout(&mut self, fx: &mut Effects) {
        if self.ui.layout_unsaved {
            self.ui.layout_unsaved = false;
            fx.commands.push(Command::SavePrefs(self.ui.layout));
        }
    }

    /// Makes the sidebar (with its separator) `total` columns wide, as far as
    /// the layout lets it (neither it nor the rest may become too narrow).
    pub(super) fn set_sidebar_total(&mut self, total: i32, fx: &mut Effects) {
        let Some((width, _)) = self.main_size() else { return };
        let before = HitMap::sidebar_total(width, &self.ui.layout);
        let permille = HitMap::sidebar_permille(width, total.clamp(0, i32::from(width)) as u16);
        let next = LayoutPrefs { sidebar: Some(permille), editor: self.ui.layout.editor };
        if HitMap::sidebar_total(width, &next) != before {
            self.ui.layout = next;
            self.ui.layout_unsaved = true;
            fx.repaint = true;
        }
    }

    /// Makes the editor `rows` high, as far as the layout lets it.
    pub(super) fn set_editor_height(&mut self, rows: i32, fx: &mut Effects) {
        let Some((_, height)) = self.main_size() else { return };
        let before = HitMap::editor_height(height, &self.ui.layout);
        let permille = HitMap::editor_permille(height, rows.clamp(0, i32::from(height)) as u16);
        let next = LayoutPrefs { sidebar: self.ui.layout.sidebar, editor: Some(permille) };
        if HitMap::editor_height(height, &next) != before {
            self.ui.layout = next;
            self.ui.layout_unsaved = true;
            fx.repaint = true;
        }
    }

    /// Puts a divider back to the automatic size (a double-click on it).
    pub(super) fn reset_divider(&mut self, sidebar: bool, fx: &mut Effects) {
        let slot = if sidebar { &mut self.ui.layout.sidebar } else { &mut self.ui.layout.editor };
        if slot.take().is_some() {
            self.ui.layout_unsaved = true;
            fx.repaint = true;
            self.save_layout(fx);
        }
    }

    /// What a key of the resize mode (or a click on its hint) does: a divider
    /// moves one column or row, or the mode ends.
    pub(super) fn perform_resize(&mut self, action: Action, fx: &mut Effects) {
        let sidebar_now = |app: &Self| app.main_size().map(|(w, _)| i32::from(HitMap::sidebar_total(w, &app.ui.layout)));
        let editor_now = |app: &Self| app.main_size().map(|(_, h)| i32::from(HitMap::editor_height(h, &app.ui.layout)));
        match action {
            Action::ResizeLeft | Action::ResizeRight => {
                let step = if action == Action::ResizeRight { 1 } else { -1 };
                if let Some(now) = sidebar_now(self) {
                    self.set_sidebar_total(now + step, fx);
                }
            }
            Action::ResizeUp | Action::ResizeDown => {
                let step = if action == Action::ResizeDown { 1 } else { -1 };
                if let Some(now) = editor_now(self) {
                    self.set_editor_height(now + step, fx);
                }
            }
            Action::ResizeDone => {
                self.ui.mode = Mode::Normal;
                fx.repaint = true;
                self.save_layout(fx);
            }
            _ => {}
        }
    }
}
