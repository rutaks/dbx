//! What one query tab shows and is in the middle of: its editor (text, caret,
//! selection, undo), the colours and statement range drawn from it, the
//! completion popup, a live template, the semantic marks, the find bar and
//! the grid's cursor and window. The Core's side of the tab (its target,
//! session and run) is `query::QueryTab`, in the model.
//!
//! The active tab's value is `Ui.tab`; the others wait in `Ui.parked`. Making
//! another tab active swaps the two, so every key, paste, click, frame and
//! deadline acts on `Ui.tab` and cannot reach a tab that is not shown. A reply
//! for a parked tab is applied to its parked value (`App::tab_ui_mut`).

use super::query::QueryTab;
use super::requests::Target;
use super::{bar, complete, editor, grid, marks, syntax, template, App, Command, Effects};
use crate::core::TabId;

#[derive(Debug)]
pub struct TabUi {
    pub id: TabId,
    /// The SQL editor.
    pub editor: editor::Editor,
    /// What the editor draws from: colours and the statement range, kept
    /// between frames for this tab's text.
    pub syntax: syntax::TabSyntax,
    /// The completion popup.
    pub complete: complete::CompleteUi,
    /// The live template being filled in, if any.
    pub template: template::TemplateUi,
    /// What the loaded schema says about the names in the buffer.
    pub marks: marks::Marks,
    /// The find or go-to-line bar on the editor's last row, while open.
    pub bar: Option<bar::Bar>,
    /// What was last searched for: where the find bar starts with no selection.
    pub last_find: String,
    /// The results pane's cell cursor and window.
    pub grid: grid::GridUi,
    /// What failed while the tab was not on screen, said as a failure notice
    /// when it is shown again.
    pub held_notice: Option<String>,
}

impl TabUi {
    pub fn new(id: TabId) -> Self {
        TabUi {
            id,
            editor: editor::Editor::new(),
            syntax: syntax::TabSyntax::default(),
            complete: complete::CompleteUi::default(),
            template: template::TemplateUi::default(),
            marks: marks::Marks::default(),
            bar: None,
            last_find: String::new(),
            grid: grid::GridUi::default(),
            held_notice: None,
        }
    }
}

impl App {
    /// The tab keys, pastes, clicks and frames act on.
    pub fn active_tab(&self) -> TabId {
        self.ui.tab.id
    }

    /// What tab `id` shows, on screen or parked.
    pub(super) fn tab_ui_mut(&mut self, id: TabId) -> Option<&mut TabUi> {
        if self.ui.tab.id == id {
            return Some(&mut self.ui.tab);
        }
        self.ui.parked.iter_mut().find(|t| t.id == id)
    }

    /// Keeps `text` on tab `id` to be said as a failure notice when it is next
    /// shown; on screen already, says it now.
    pub(super) fn notify_tab(&mut self, id: TabId, text: String, fx: &mut Effects) {
        if self.is_active(id) {
            return self.notify(text, true, fx);
        }
        if let Some(ui) = self.tab_ui_mut(id) {
            ui.held_notice = Some(text);
        }
    }

    /// Opens another query tab, on the active tab's target, in a session of
    /// its own (serial 1), with an empty editor, and leaves the active one
    /// active.
    pub fn open_tab(&mut self) -> TabId {
        let id = TabId(self.model.next_tab);
        self.model.next_tab += 1;
        let mut tab = QueryTab::new(id);
        tab.target = self.model.tab(self.ui.tab.id).and_then(|t| t.target.clone());
        self.model.tabs.push(tab);
        self.ui.parked.push(TabUi::new(id));
        id
    }

    /// A new tab, made active, with the editor focused to type in.
    pub(super) fn new_tab(&mut self, fx: &mut Effects) {
        let id = self.open_tab();
        let switched = self.activate_tab(id);
        fx.commands.extend(switched.commands);
        self.ui.focus = super::PaneId::Editor;
        fx.repaint = true;
    }

    /// Where tab `id` is in the bar, counted from 0.
    pub(super) fn tab_index(&self, id: TabId) -> Option<usize> {
        self.model.tabs.iter().position(|t| t.id == id)
    }

    /// Makes the tab `step` places along the bar active, wrapping.
    pub(super) fn cycle_tab(&mut self, step: isize, fx: &mut Effects) {
        let n = self.model.tabs.len() as isize;
        let Some(at) = self.tab_index(self.ui.tab.id) else { return };
        let to = self.model.tabs[(at as isize + step).rem_euclid(n) as usize].id;
        self.switch_to(to, fx);
    }

    /// Makes the `n`th tab of the bar (from 1) active, if there is one.
    pub(super) fn go_to_tab(&mut self, n: u8, fx: &mut Effects) {
        if let Some(to) = self.model.tabs.get(usize::from(n).wrapping_sub(1)).map(|t| t.id) {
            self.switch_to(to, fx);
        }
    }

    /// `activate_tab`, merged into `fx`; the active tab again changes nothing.
    pub(super) fn switch_to(&mut self, id: TabId, fx: &mut Effects) {
        let switched = self.activate_tab(id);
        fx.repaint |= switched.repaint;
        fx.commands.extend(switched.commands);
    }

    /// How the bar and a question name tab `id`: `tab 2 (local pg / pagila)`.
    pub(super) fn tab_name(&self, id: TabId) -> String {
        let n = self.tab_index(id).map_or(0, |i| i + 1);
        match self.target_label_of(id) {
            Some(label) => format!("tab {n} ({label})"),
            None => format!("tab {n}"),
        }
    }

    /// Closes tab `id`: at once if it is empty and idle, otherwise after a
    /// question naming it and saying what goes with it.
    pub(super) fn request_close_tab(&mut self, id: TabId, fx: &mut Effects) {
        let Some(tab) = self.model.tab(id) else { return };
        let running = tab.run.in_flight().is_some();
        let text = self.tab_ui_mut(id).is_some_and(|ui| !ui.editor.text().is_empty());
        if running || text {
            let name = self.tab_name(id);
            self.close_completion(fx);
            self.ui.overlay = Some(super::overlay::Overlay::ConfirmCloseTab(super::overlay::ConfirmCloseTab { tab: id, name, text, running }));
            self.ui.mode = super::Mode::Normal;
            fx.repaint = true;
            return;
        }
        self.close_tab(id, fx);
    }

    /// The close-tab question was answered yes.
    pub(super) fn confirm_close_tab(&mut self, fx: &mut Effects) {
        let asked = match self.ui.overlay.take() {
            Some(super::overlay::Overlay::ConfirmCloseTab(ask)) => ask.tab,
            other => {
                self.ui.overlay = other;
                return;
            }
        };
        fx.repaint = true;
        self.close_tab(asked, fx);
    }

    /// `remove_tab`, merged into `fx`.
    pub(super) fn close_tab(&mut self, id: TabId, fx: &mut Effects) {
        let closed = self.remove_tab(id);
        fx.repaint = true;
        fx.commands.extend(closed.commands);
    }

    /// Makes tab `id` the active one. The tab left closes its completion
    /// popup, leaves template mode and forgets a `*` expansion that was
    /// waiting, keeping its text; everything else of it (text, caret,
    /// selection, undo, find text, grid) waits as it was.
    pub fn activate_tab(&mut self, id: TabId) -> Effects {
        let mut fx = Effects::default();
        let Some(i) = self.ui.parked.iter().position(|t| t.id == id) else { return fx };
        self.close_completion(&mut fx);
        self.leave_template(&mut fx);
        self.ui.tab.complete.forget_pending();
        // A drag or a double-click begun in the tab left ends with it.
        self.ui.drag = None;
        self.ui.clicks.forget();
        // A question or a viewer about the tab left does not follow the user.
        self.ui.overlay = None;
        std::mem::swap(&mut self.ui.tab, &mut self.ui.parked[i]);
        if let Some(text) = self.ui.tab.held_notice.take() {
            self.notify(text, true, &mut fx);
        }
        fx.repaint = true;
        fx
    }

    /// Removes tab `id`, without asking. Its run is cancelled and then its
    /// session closed, as when it is given another target (design section
    /// 17); every reply still on its way for it is forgotten, so it changes
    /// nothing and repaints nothing. Removing the active tab makes the one to
    /// its left active (to its right, for the first), opening an empty one on
    /// the same target if it was the last.
    pub fn remove_tab(&mut self, id: TabId) -> Effects {
        let mut fx = Effects::default();
        if self.model.tab(id).is_none() {
            return fx;
        }
        if self.ui.tab.id == id {
            // A question about the tab's run or result goes with it.
            if matches!(self.ui.overlay, Some(super::overlay::Overlay::ConfirmDropSession(_) | super::overlay::Overlay::ConfirmRerun(_))) {
                self.ui.overlay = None;
            }
            let at = self.tab_index(id).unwrap_or(0);
            let next = match (at.checked_sub(1), self.model.tabs.get(at + 1)) {
                (Some(left), _) => self.model.tabs[left].id,
                (None, Some(right)) => right.id,
                (None, None) => self.open_tab(),
            };
            let switched = self.activate_tab(next);
            fx.repaint |= switched.repaint;
            fx.commands.extend(switched.commands);
        }
        self.end_session_of(id, &mut fx);
        // What the tab let go of may still be running: the model keeps it,
        // and the stale reply (not forgotten) settles it.
        let left = self.model.tab_mut(id).map(|t| std::mem::take(&mut t.abandoned)).unwrap_or_default();
        if left.is_empty() {
            self.model.requests.forget(&Target::Run(id));
        }
        self.model.orphaned.extend(left.into_iter().map(|a| (id, a)));
        self.model.requests.forget_where(|t| matches!(t, Target::Complete(tab, _) if *tab == id));
        let asked: Vec<_> = self.model.completions.inflight.keys().filter(|(tab, _)| *tab == id).cloned().collect();
        for key in asked {
            if let Some((request, _)) = self.model.completions.inflight.remove(&key) {
                fx.commands.push(Command::Abandon(request));
            }
        }
        self.model.tabs.retain(|t| t.id != id);
        self.ui.parked.retain(|t| t.id != id);
        fx
    }
}
