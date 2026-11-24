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

    /// Opens another query tab, with no target and an empty editor, and
    /// leaves the active one active. (No key yet: slice 15b adds the tab bar.)
    pub fn open_tab(&mut self) -> TabId {
        let id = TabId(self.model.next_tab);
        self.model.next_tab += 1;
        self.model.tabs.push(QueryTab::new(id));
        self.ui.parked.push(TabUi::new(id));
        id
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

    /// Removes tab `id` (no key yet: a test helper until slice 15b). Its run
    /// is cancelled and then its session closed, as when it is given another
    /// target (design section 17); every reply still on its way for it is
    /// forgotten, so it changes nothing and repaints nothing. Removing the
    /// active tab makes another active, opening an empty one if it was the
    /// last.
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
            let next = match self.model.tabs.iter().map(|t| t.id).find(|t| *t != id) {
                Some(next) => next,
                None => self.open_tab(),
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
