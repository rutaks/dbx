//! Opening a table from the sidebar and paging through it (slice 16).
//!
//! A browse tab is an ordinary query tab that holds a generated select: the
//! adapter builds one page of it (`Syntax::table_page_sql`: `LIMIT`/`OFFSET`,
//! ordered by the primary key), and each page is a run of that text in the
//! tab's own session. Paging and the count are runs too: refused while the
//! tab runs, cancelled with `ctrl+c`, stale by the usual rules. Editing the
//! text and running it ends the mode (`leave_browse_if_edited`).

use std::time::Instant;

use super::query::{Browse, Count, Fetch, Outcome, RunState};
use super::requests::Target;
use super::tree::{Listing, NodeId, RowKind};
use super::{grid, App, Command, CoreCall, Effects, PaneId};
use crate::core::{ColumnInfo, CoreError, CoreErrorKind, Cursor, ResultSet, RunId, RunRequest, TabId, TablePath};

/// Which way a page key goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    Next,
    Prev,
}

impl App {
    /// The table the sidebar's cursor is on, if it is on one.
    fn cursor_table(&self) -> Option<TablePath> {
        let row = self.cursor_row()?;
        match (&row.kind, &row.node) {
            (RowKind::Table { .. }, NodeId::Table(path)) => Some(path.clone()),
            _ => None,
        }
    }

    /// Whether `o` would open the row under the cursor: a table or view on a
    /// connection that browses and runs statements. The hint is shown only
    /// then.
    pub(super) fn cursor_can_browse(&self) -> bool {
        self.cursor_table()
            .and_then(|t| self.connection(&t.schema.database.connection).map(|c| c.capabilities.browse && c.capabilities.query))
            .unwrap_or(false)
    }

    /// `o` in the sidebar: opens the table under the cursor in a tab of its
    /// own, or goes to the tab that already browses it.
    pub(super) fn open_table_at_cursor(&mut self, fx: &mut Effects) {
        let Some(path) = self.cursor_table() else { return };
        self.open_table_path(path, fx);
    }

    /// Opens the table `path` names in a tab of its own (the sidebar's `o`,
    /// and the navigator's `ctrl+o`).
    pub(super) fn open_table_path(&mut self, path: TablePath, fx: &mut Effects) {
        let Some(connection) = self.connection(&path.schema.database.connection) else { return };
        if !connection.capabilities.browse {
            return;
        }
        if !connection.capabilities.query {
            let text = format!("{} cannot be queried here: dbx does not run statements on {}.", connection.name, connection.kind.display_name());
            self.say(text, true);
            fx.repaint = true;
            return;
        }
        if self.show_browse_tab(&path, fx) {
            return;
        }
        // The primary key comes from the columns the tree holds for the
        // table; if it has not opened the table yet, they are asked for.
        let columns = match self.model.schema.state(&NodeId::Table(path.clone())).listing() {
            Some(Listing::Columns(columns)) => Some(columns.clone()),
            _ => None,
        };
        match columns {
            Some(columns) => self.open_table(path, columns, fx),
            None => {
                let id = self.model.requests.begin(Target::OpenTable(path.clone()));
                fx.commands.push(Command::Core(id, CoreCall::TableColumns(path)));
            }
        }
    }

    /// The columns asked for by `open_table_at_cursor` arrived.
    pub(super) fn on_open_table_columns(&mut self, path: TablePath, result: Result<Vec<ColumnInfo>, CoreError>, fx: &mut Effects) {
        match result {
            Ok(columns) => self.open_table(path, columns, fx),
            Err(e) => {
                let text = format!("Could not open {}: {}", path.table, e.message);
                self.notify(text, true, fx);
            }
        }
    }

    /// Makes the tab that browses `path` active, if there is one.
    fn show_browse_tab(&mut self, path: &TablePath, fx: &mut Effects) -> bool {
        let Some(id) = self.model.tabs.iter().find(|t| t.browse.as_ref().is_some_and(|b| &b.table == path)).map(|t| t.id) else {
            return false;
        };
        self.switch_to(id, fx);
        self.ui.focus = PaneId::Results;
        fx.repaint = true;
        true
    }

    /// Opens a tab on `path`, ordered by its primary key (the `columns` that
    /// say they are one), holding the select of the first page, and runs it.
    fn open_table(&mut self, path: TablePath, columns: Vec<ColumnInfo>, fx: &mut Effects) {
        if self.show_browse_tab(&path, fx) {
            return;
        }
        let Some(kind) = self.connection(&path.schema.database.connection).map(|c| c.kind.clone()) else { return };
        let order_by: Vec<String> = columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
        let page_size = self.model.row_cap;
        let sql = self.ui.syntax.table_page_sql(&kind, &path, &order_by, page_size, 0);
        let id = self.open_tab();
        if let Some(tab) = self.model.tab_mut(id) {
            tab.target = Some(path.schema.database.clone());
            tab.browse = Some(Browse {
                table: path,
                kind,
                order_by,
                page_size,
                offset: 0,
                sql: sql.clone(),
                count: Count::NotAsked,
                fetch: None,
                failed: None,
                count_cancelled: false,
            });
        }
        self.switch_to(id, fx);
        self.ui.tab.editor.set_text(&sql);
        self.ui.focus = PaneId::Results;
        let now = self.ui.now.unwrap_or_else(Instant::now);
        self.begin_browse_run(id, &sql, Fetch::Page { offset: 0, sql: sql.clone() }, page_size, now, fx);
    }

    /// Starts a run of `sql` (the whole of it) in tab `id`, as the page or the
    /// count `fetch` says.
    fn begin_browse_run(&mut self, id: TabId, sql: &str, fetch: Fetch, cap: usize, now: Instant, fx: &mut Effects) {
        let Some(tab) = self.model.tab(id) else { return };
        let Some(target) = tab.target.clone() else { return };
        let session = tab.session();
        let lines: Vec<&str> = sql.split('\n').collect();
        let end = Cursor { row: lines.len() - 1, col: lines.last().map_or(0, |l| l.chars().count()) };
        let run = RunId(self.model.next_run);
        self.model.next_run += 1;
        let request_id = self.model.requests.begin(Target::Run(id));
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            session,
            buffer: sql.to_string(),
            cursor: Cursor::default(),
            selection: Some((Cursor::default(), end)),
            row_cap: cap,
            confirm_dangerous: self.model.settings.confirm_dangerous,
            confirmation: None,
        };
        let tab = self.model.active_mut(id);
        tab.run = RunState::Running { run, request: request_id, started: now };
        tab.run_mentions_search_path = false;
        // A page or a count is the client's own statement, not the user's.
        tab.history = false;
        if let Some(b) = &mut tab.browse {
            b.fetch = Some(fetch);
            b.failed = None;
            b.count_cancelled = false;
        }
        fx.commands.push(Command::Core(request_id, CoreCall::Run(run, request)));
        fx.repaint = true;
    }

    /// Whether the active tab is a browse tab (the paging keys are only
    /// offered there).
    pub(super) fn is_browsing(&self) -> bool {
        self.tab().browse.is_some()
    }

    /// Whether `]` would show another page now: the page on screen is full.
    pub(super) fn can_page_next(&self) -> bool {
        let tab = self.tab();
        let Some(b) = &tab.browse else { return false };
        tab.run.in_flight().is_none() && grid::shown(&tab.outcome).is_some_and(|set| set.rows.len() >= b.page_size)
    }

    /// Whether `[` would show an earlier page now.
    pub(super) fn can_page_prev(&self) -> bool {
        let tab = self.tab();
        tab.run.in_flight().is_none() && tab.browse.as_ref().is_some_and(|b| b.offset > 0)
    }

    /// Whether `#` would count now.
    pub(super) fn can_count(&self) -> bool {
        let tab = self.tab();
        tab.run.in_flight().is_none() && tab.browse.as_ref().is_some_and(|b| b.count == Count::NotAsked)
    }

    /// `]` and `[`.
    fn page(&mut self, way: Way, now: Instant, fx: &mut Effects) {
        let id = self.ui.tab.id;
        let Some(b) = self.tab().browse.clone() else { return };
        // A run in the way (the first page still loading included) is said so
        // before anything else: what the key would do is not known yet.
        if self.tab().run.in_flight().is_some() {
            return self.notify("A statement is still running in this tab: page once it has ended.".into(), true, fx);
        }
        // Nothing to go to: the key does nothing, not even a notice.
        let offset = match way {
            Way::Next if grid::shown(&self.tab().outcome).is_some_and(|set| set.rows.len() >= b.page_size) => b.offset + b.page_size,
            Way::Prev if b.offset > 0 => b.offset.saturating_sub(b.page_size),
            _ => return,
        };
        if self.ui.tab.editor.text() != b.sql {
            let run_key = self.ui.keys.key_for(super::keys::Action::RunDirect).unwrap_or_default();
            let text = format!("The text in this tab was edited, so it no longer pages {}: {run_key} runs it, or undo the edit to page again.", b.table.table);
            return self.notify(text, true, fx);
        }
        let sql = self.ui.syntax.table_page_sql(&b.kind, &b.table, &b.order_by, b.page_size, offset);
        self.begin_browse_run(id, &sql.clone(), Fetch::Page { offset, sql }, b.page_size, now, fx);
    }

    pub(super) fn next_page(&mut self, now: Instant, fx: &mut Effects) {
        self.page(Way::Next, now, fx)
    }

    pub(super) fn prev_page(&mut self, now: Instant, fx: &mut Effects) {
        self.page(Way::Prev, now, fx)
    }

    /// `#`: counts the rows of the table, once, as a run in the tab's session.
    pub(super) fn count_rows(&mut self, now: Instant, fx: &mut Effects) {
        let id = self.ui.tab.id;
        let Some(b) = self.tab().browse.clone() else { return };
        if b.count != Count::NotAsked {
            return;
        }
        if self.tab().run.in_flight().is_some() {
            return self.notify("A statement is still running in this tab: count once it has ended.".into(), true, fx);
        }
        let sql = self.ui.syntax.table_count_sql(&b.kind, &b.table);
        if let Some(b) = self.model.active_mut(id).browse.as_mut() {
            b.count = Count::Asking;
        }
        self.begin_browse_run(id, &sql, Fetch::Count, 1, now, fx);
    }

    /// Before a run of the editor's text starts: a tab whose text was edited
    /// (or of which only a part runs) is an ordinary query tab from now on.
    pub(super) fn leave_browse_if_edited(&mut self, whole_text: bool) {
        let Some(b) = &self.tab().browse else { return };
        if whole_text && self.ui.tab.editor.text() == b.sql {
            return;
        }
        self.model.active_mut(self.ui.tab.id).browse = None;
    }

    /// The reply to a run that was a page or the count of tab `tab`.
    pub(super) fn browse_landed(&mut self, tab: TabId, fetch: Fetch, result: Result<ResultSet, CoreError>, elapsed: std::time::Duration, fx: &mut Effects) {
        let active = self.is_active(tab);
        let wording = |e: &CoreError| if e.kind == CoreErrorKind::Timeout { "timed out" } else { "failed" };
        match (fetch, result) {
            (Fetch::Count, Ok(set)) => {
                let counted = set.rows.first().and_then(|r| r.first()).and_then(|c| match c {
                    crate::core::Cell::Value(v) => v.trim().parse::<u64>().ok(),
                    crate::core::Cell::Null => None,
                });
                if let Some(b) = self.model.tab_mut(tab).and_then(|t| t.browse.as_mut()) {
                    b.count = counted.map_or(Count::NotAsked, Count::Known);
                }
                if counted.is_none() {
                    self.notify_tab(tab, "The count did not come back as a number, so no total is shown.".into(), fx);
                }
            }
            (Fetch::Count, Err(e)) => {
                if let Some(t) = self.model.tab_mut(tab) {
                    if let Some(b) = t.browse.as_mut() {
                        b.count = Count::NotAsked;
                    }
                    if e.kind == CoreErrorKind::Cancelled {
                        t.run = RunState::Cancelled { after: elapsed, statement: e.statement.clone(), how: super::query::CancelEnd::Stopped };
                        return;
                    }
                }
                self.notify_tab(tab, format!("Counting the rows {}: {}", wording(&e), e.message), fx);
            }
            (Fetch::Page { offset, sql }, Ok(set)) => {
                let mut before = String::new();
                if let Some(t) = self.model.tab_mut(tab) {
                    t.result_session = t.session;
                    t.outcome = Some(Outcome::Rows(set));
                    if let Some(b) = t.browse.as_mut() {
                        before = std::mem::replace(&mut b.sql, sql.clone());
                        b.offset = offset;
                    }
                }
                if let Some(ui) = self.tab_ui_mut(tab) {
                    // The editor follows the page, unless there is typing in it.
                    if ui.editor.text() == before {
                        ui.editor.set_text(&sql);
                    }
                    ui.grid.reset();
                }
                if active {
                    self.ui.clicks.forget();
                }
            }
            (Fetch::Page { .. }, Err(e)) => {
                let Some(t) = self.model.tab_mut(tab) else { return };
                if e.kind == CoreErrorKind::Cancelled {
                    t.run = RunState::Cancelled { after: elapsed, statement: e.statement, how: super::query::CancelEnd::Stopped };
                } else if grid::shown(&t.outcome).is_some() {
                    // The page on screen is still the table's: it stays, as
                    // the earlier run's, with the failure said under it until
                    // another run starts.
                    let text = format!("The page {}: {}", wording(&e), e.message);
                    if let Some(b) = t.browse.as_mut() {
                        b.failed = Some(text.clone());
                    }
                    self.notify_tab(tab, text, fx);
                } else if e.kind == CoreErrorKind::Timeout {
                    t.outcome = Some(Outcome::TimedOut { error: e, elapsed });
                } else {
                    t.outcome = Some(Outcome::Failed { error: e, elapsed });
                }
            }
        }
    }
}
