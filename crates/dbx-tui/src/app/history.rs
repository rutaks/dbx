//! History (slice 18): saving what ran, and the overlay that finds it again.
//!
//! The store is the dbx desktop app's: a statement this client runs shows in
//! the desktop's history, and the desktop's show here. Everything read from it
//! was written by another program and is drawn only through the painter.
//!
//! **What is saved:** a statement of the user's that reached the database and
//! has an outcome: it returned rows or changed some, failed in the database,
//! or timed out (the desktop records every finished run, with its error when
//! it failed). Not saved: a run that waits at a confirmation (nothing ran),
//! load more and browse pages and counts (the statement is already there, or
//! the client wrote it), a run the Core refused before sending anything or
//! could not deliver (no statement in the error, or a connection error), and
//! a run the user cancelled that then stopped (nothing finished; whether the
//! server ran any of it is not known). A cancelled run that finished first
//! is saved: it ran, and what it changed stands.
//!
//! **The overlay:** a search field, the entries newest first, and the
//! highlighted statement in full. Typing searches after `PAUSE`; the end of
//! the list fetches the next page. `enter` puts the statement in the editor,
//! `f5` runs it through the normal run path (so the confirmation gate of
//! slice 17 applies).

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::clean_paste;
use super::keys::{Action, KeyCombo};
use super::overlay::{Overlay, OverlayAction};
use super::requests::Target;
use super::text_field::{Edit, TextField};
use super::{App, Command, CoreCall, Effects, Mode, PaneId};
use crate::core::{
    display_safe, ConnectionId, CoreError, CoreErrorKind, HistoryCursor, HistoryEntry, HistoryPage, HistoryQuery, NewHistoryEntry, ResultSet, TabId,
};

/// Entries to a page.
pub const PAGE: usize = 50;

/// How long typing must pause before the search is sent.
pub const PAUSE: Duration = Duration::from_millis(200);

/// Rows one notch of the wheel moves the highlight.
const WHEEL: isize = 3;

/// Where a search is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    Searching,
    Ready,
    Failed(String),
}

/// The overlay's state.
#[derive(Debug, Clone)]
pub struct HistoryOverlay {
    /// The tab it was opened in: `enter` and `f5` act on it.
    pub tab: TabId,
    pub search: TextField,
    /// The tab's connection (id and name) when it has one: what "this
    /// connection" means. `None`: the tab has no database yet, so every
    /// connection's entries are shown.
    pub here: Option<(ConnectionId, String)>,
    /// Every connection's entries rather than `here`'s.
    pub all: bool,
    pub entries: Vec<HistoryEntry>,
    pub next: Option<HistoryCursor>,
    /// How many entries match, all pages together.
    pub total: usize,
    pub load: Load,
    /// The search that made `entries` (what the next page continues).
    pub searched: HistoryQuery,
    /// The next page is on its way.
    pub more_loading: bool,
    /// The next page failed: said under the list until the highlight moves.
    pub more_failed: Option<String>,
    pub selected: usize,
    /// The first row shown and how many fit: set by the view.
    pub top: usize,
    pub rows: usize,
    /// The first line of the preview shown, and how many lines fit.
    pub preview_scroll: usize,
    pub preview_rows: usize,
    /// When the search is due, after typing paused.
    pub due: Option<Instant>,
}

impl HistoryOverlay {
    pub fn selected_entry(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.selected)
    }

    /// What the scope toggle can flip: only a tab with a connection has a
    /// "this connection" to go back to.
    pub fn can_toggle(&self) -> bool {
        self.here.is_some()
    }

    /// The list on screen is not the answer to what the field and the scope
    /// say: a search is due or on its way.
    pub fn pending(&self) -> bool {
        self.due.is_some() || self.load == Load::Searching
    }

    /// The statement as it is shown, inserted and run: control characters
    /// replaced and bidirectional overrides dropped.
    pub fn selected_sql(&self) -> Option<String> {
        self.selected_entry().map(|e| clean_paste(&e.sql))
    }
}

/// What a finished run gives the history, before it is given a place.
pub(super) struct Saved {
    sql: String,
    elapsed: Duration,
    success: bool,
    error: Option<String>,
    affected: Option<u64>,
}

/// What a run's outcome saves, or `None` when there is nothing to claim: it
/// did not reach the database, it waits for a yes, or it was cancelled.
pub(super) fn saved_of(result: &Result<ResultSet, CoreError>, measured: Duration) -> Option<Saved> {
    match result {
        Ok(set) => Some(Saved {
            sql: set.ran.clone(),
            elapsed: set.elapsed,
            success: true,
            error: None,
            affected: set.columns.is_empty().then_some(set.affected),
        }),
        Err(e) if matches!(e.kind, CoreErrorKind::Cancelled | CoreErrorKind::NeedsConfirmation | CoreErrorKind::Connect | CoreErrorKind::Key | CoreErrorKind::Store) => {
            None
        }
        // The statement is in the error only when it was sent.
        Err(e) => e.statement.clone().filter(|s| !s.trim().is_empty()).map(|sql| Saved {
            sql,
            elapsed: measured,
            success: false,
            // Control characters out, line breaks kept: what is stored is
            // read back by the desktop, not drawn here.
            error: Some(clean_paste(e.message.trim())).filter(|m| !m.is_empty()),
            affected: None,
        }),
    }
}

impl App {
    // ---- saving -------------------------------------------------------------------

    /// A run of tab `tab` has an outcome that goes to the history (`saved_of`
    /// said so: a statement of the user's that reached the database). One
    /// command, fire and forget.
    pub(super) fn save_run(&mut self, tab: TabId, saved: Saved, fx: &mut Effects) {
        let Some(target) = self.model.tab(tab).and_then(|t| t.target.clone()) else { return };
        let connection_name = self.connection(&target.connection).map(|c| c.name.clone()).unwrap_or_else(|| target.connection.0.clone());
        let entry = NewHistoryEntry {
            connection: target.connection,
            connection_name,
            database: target.database,
            sql: saved.sql,
            elapsed: saved.elapsed,
            success: saved.success,
            error: saved.error,
            affected: saved.affected,
        };
        let serial = self.model.next_save;
        self.model.next_save += 1;
        let id = self.model.requests.begin(Target::SaveHistory(serial));
        fx.commands.push(Command::Core(id, CoreCall::SaveHistory(entry)));
    }

    /// The save's answer: only a failure is said, once.
    pub(super) fn on_history_saved(&mut self, _serial: u64, result: Result<(), CoreError>, fx: &mut Effects) {
        if let Err(e) = result {
            let why = display_safe(e.message.trim());
            self.notify(format!("The statement ran, but it was not saved to the history: {why}"), true, fx);
        }
    }

    // ---- the overlay ---------------------------------------------------------------

    pub(super) fn open_history(&mut self, fx: &mut Effects) {
        self.close_completion(fx);
        self.ui.mode = Mode::Normal;
        let here = self.tab().target.as_ref().map(|t| {
            let name = self.connection(&t.connection).map(|c| c.name.clone()).unwrap_or_else(|| t.connection.0.clone());
            (t.connection.clone(), name)
        });
        self.ui.overlay = Some(Overlay::History(Box::new(HistoryOverlay {
            tab: self.ui.tab.id,
            search: TextField::new(),
            all: here.is_none(),
            here,
            entries: Vec::new(),
            next: None,
            total: 0,
            load: Load::Searching,
            searched: HistoryQuery::default(),
            more_loading: false,
            more_failed: None,
            selected: 0,
            top: 0,
            rows: 0,
            preview_scroll: 0,
            preview_rows: 0,
            due: None,
        })));
        self.search_history(fx);
        fx.repaint = true;
    }

    /// Sends the search the field and the scope say, now. Anything earlier
    /// (a search, a next page) is stale from here on.
    fn search_history(&mut self, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        let query = HistoryQuery {
            search_text: h.search.text().to_string(),
            connection: if h.all { None } else { h.here.as_ref().map(|(id, _)| id.clone()) },
            cursor: None,
            limit: PAGE,
        };
        h.searched = query.clone();
        h.due = None;
        h.load = Load::Searching;
        h.more_loading = false;
        h.more_failed = None;
        self.model.requests.supersede(Target::HistoryMore);
        let id = self.model.requests.begin(Target::History);
        fx.commands.push(Command::Core(id, CoreCall::History(query)));
        fx.repaint = true;
    }

    /// The next page of the search on screen, when the highlight is on the
    /// last entry loaded and there is more.
    fn history_more_if_at_end(&mut self, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        let Some(cursor) = h.next.clone() else { return };
        if h.more_loading || h.more_failed.is_some() || h.selected + 1 < h.entries.len() {
            return;
        }
        h.more_loading = true;
        let query = HistoryQuery { cursor: Some(cursor), ..h.searched.clone() };
        let id = self.model.requests.begin(Target::HistoryMore);
        fx.commands.push(Command::Core(id, CoreCall::History(query)));
        fx.repaint = true;
    }

    /// The overlay is closed (or being left): replies still on their way are
    /// not wanted.
    fn forget_history_requests(&mut self, fx: &mut Effects) {
        let mut dropped = self.model.requests.forget(&Target::History);
        dropped.extend(self.model.requests.forget(&Target::HistoryMore));
        fx.commands.extend(dropped.into_iter().map(Command::Abandon));
    }

    pub(super) fn close_history(&mut self, fx: &mut Effects) {
        self.ui.overlay = None;
        self.forget_history_requests(fx);
        fx.repaint = true;
    }

    /// Closing any overlay by a button: a history's replies are let go of.
    pub(super) fn forget_history_if_closing(&mut self, fx: &mut Effects) {
        if matches!(self.ui.overlay, Some(Overlay::History(_))) {
            self.forget_history_requests(fx);
        }
    }

    pub(super) fn history_housekeeping(&mut self, now: Instant, fx: &mut Effects) {
        let due = matches!(&self.ui.overlay, Some(Overlay::History(h)) if h.due.is_some_and(|d| now >= d));
        if due {
            self.search_history(fx);
        }
    }

    pub(super) fn history_deadline(&self) -> Option<Instant> {
        match &self.ui.overlay {
            Some(Overlay::History(h)) => h.due,
            _ => None,
        }
    }

    /// The search reply (`more`: the next page's). Whether it changed what is
    /// shown.
    pub(super) fn on_history_reply(&mut self, result: Result<HistoryPage, CoreError>, more: bool) -> bool {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return false };
        match (result, more) {
            (Ok(page), false) => {
                // The highlight stays on the same entry when it is still listed.
                let kept = h.selected_entry().map(|e| e.id.clone()).and_then(|id| page.entries.iter().position(|e| e.id == id));
                if kept.is_none() || kept != Some(h.selected) {
                    h.preview_scroll = 0;
                }
                h.selected = kept.unwrap_or(0);
                h.entries = page.entries;
                h.next = page.next;
                h.total = page.total;
                h.top = 0;
                h.load = Load::Ready;
            }
            (Ok(page), true) => {
                h.entries.extend(page.entries);
                h.next = page.next;
                h.total = page.total;
                h.more_loading = false;
            }
            (Err(e), false) => {
                // What was listed answered another question: it is not kept.
                h.entries.clear();
                h.next = None;
                h.total = 0;
                h.selected = 0;
                h.load = Load::Failed(display_safe(e.message.trim()));
            }
            (Err(e), true) => {
                h.more_loading = false;
                h.more_failed = Some(display_safe(e.message.trim()));
            }
        }
        true
    }

    // ---- input ---------------------------------------------------------------------

    pub(super) fn on_history_key(&mut self, key: &KeyEvent, combo: KeyCombo, fx: &mut Effects) {
        let direct = self.ui.keys.lookup_direct(&combo);
        let plain = combo.mods.is_empty();
        match combo.code {
            KeyCode::Esc if plain => return self.close_history(fx),
            _ if direct == Some(Action::HistoryDirect) => return self.close_history(fx),
            _ if matches!(direct, Some(Action::RunDirect)) => return self.history_run(fx),
            KeyCode::Enter if plain => return self.history_insert(fx),
            KeyCode::Tab if plain => return self.history_toggle_scope(fx),
            KeyCode::Down if plain => return self.history_move(1, fx),
            KeyCode::Up if plain => return self.history_move(-1, fx),
            KeyCode::Char('n') if combo.mods == KeyModifiers::CONTROL => return self.history_move(1, fx),
            KeyCode::Char('p') if combo.mods == KeyModifiers::CONTROL => return self.history_move(-1, fx),
            KeyCode::PageDown if plain => return self.history_page(1, fx),
            KeyCode::PageUp if plain => return self.history_page(-1, fx),
            KeyCode::Down if combo.mods == KeyModifiers::SHIFT => return self.history_scroll_preview(1, fx),
            KeyCode::Up if combo.mods == KeyModifiers::SHIFT => return self.history_scroll_preview(-1, fx),
            _ => {}
        }
        let now = self.ui.now.unwrap_or_else(Instant::now);
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        match h.search.handle_key(key) {
            Edit::Changed => {
                h.due = Some(now + PAUSE);
                fx.repaint = true;
            }
            Edit::Moved => fx.repaint = true,
            Edit::Ignored => {}
        }
    }

    pub(super) fn on_history_paste(&mut self, text: &str, fx: &mut Effects) {
        let now = self.ui.now.unwrap_or_else(Instant::now);
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        h.search.insert_str(text);
        h.due = Some(now + PAUSE);
        fx.repaint = true;
    }

    pub(super) fn on_history_wheel(&mut self, down: bool, fx: &mut Effects) {
        self.history_move(if down { WHEEL } else { -WHEEL }, fx);
    }

    /// Moves the highlight by `by` rows, within the list.
    fn history_move(&mut self, by: isize, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        // Moving is also the way to try the next page again.
        h.more_failed = None;
        if h.entries.is_empty() {
            return;
        }
        let to = h.selected.saturating_add_signed(by).min(h.entries.len() - 1);
        if to != h.selected {
            h.selected = to;
            h.preview_scroll = 0;
            fx.repaint = true;
        }
        self.history_more_if_at_end(fx);
    }

    fn history_page(&mut self, way: isize, fx: &mut Effects) {
        let rows = match &self.ui.overlay {
            Some(Overlay::History(h)) => h.rows.saturating_sub(1).max(1) as isize,
            _ => return,
        };
        self.history_move(way * rows, fx);
    }

    fn history_scroll_preview(&mut self, by: isize, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        // The view keeps it in range.
        let to = h.preview_scroll.saturating_add_signed(by);
        if to != h.preview_scroll {
            h.preview_scroll = to;
            fx.repaint = true;
        }
    }

    pub(super) fn history_action(&mut self, action: OverlayAction, fx: &mut Effects) {
        match action {
            OverlayAction::HistoryInsert => self.history_insert(fx),
            OverlayAction::HistoryRun => self.history_run(fx),
            OverlayAction::HistoryScope => self.history_toggle_scope(fx),
            OverlayAction::HistoryPick(i) => {
                let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
                if i < h.entries.len() {
                    h.selected = i;
                    h.preview_scroll = 0;
                    fx.repaint = true;
                }
                self.history_more_if_at_end(fx);
            }
            _ => {}
        }
    }

    /// `tab`: this connection's entries, or every connection's.
    fn history_toggle_scope(&mut self, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &mut self.ui.overlay else { return };
        if !h.can_toggle() {
            return;
        }
        h.all = !h.all;
        self.search_history(fx);
    }

    /// `enter`: the statement goes into the editor at the caret, on lines of
    /// its own, as one undo step; the overlay closes.
    fn history_insert(&mut self, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &self.ui.overlay else { return };
        if h.pending() {
            return;
        }
        let Some(sql) = h.selected_sql().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { return };
        let tab = h.tab;
        self.close_history(fx);
        if !self.is_active(tab) {
            return;
        }
        let editor = &mut self.ui.tab.editor;
        let caret = editor.cursor();
        let (from, to) = editor.selection().unwrap_or((caret, caret));
        let line = |row: usize| editor.lines().get(row).map(String::as_str).unwrap_or("");
        let before: String = line(from.row).chars().take(from.col).collect();
        let after: String = line(to.row).chars().skip(to.col).collect();
        let mut text = String::new();
        if !before.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&sql);
        if !after.trim().is_empty() {
            text.push('\n');
        }
        match self.ui.tab.editor.paste(&text) {
            Ok(_) => {}
            Err(why) => self.notify(format!("The statement was not inserted: {why}."), true, fx),
        }
        self.ui.focus = PaneId::Editor;
    }

    /// `f5`: the statement runs in the active tab, on the tab's target,
    /// through the normal run path.
    fn history_run(&mut self, fx: &mut Effects) {
        let Some(Overlay::History(h)) = &self.ui.overlay else { return };
        if h.pending() {
            return;
        }
        let Some(sql) = h.selected_sql().filter(|s| !s.trim().is_empty()) else { return };
        self.close_history(fx);
        let now = self.ui.now.unwrap_or_else(Instant::now);
        self.run_sql(&sql, now, fx);
    }
}
