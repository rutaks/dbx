//! The query tab: where its statements run (its target, set explicitly from
//! the sidebar), the run in flight, and what the last run gave.
//!
//! A tab owns a database session (dbx's `client_session_id` names the tab and
//! a serial, `SessionId`, design section 10). When it closes:
//!
//! - **The tab is pointed at another database or connection:** the old
//!   session is closed (`Command::CloseSession`), and the tab's next run opens
//!   a new one with the next serial, so a close that lands late can never
//!   end it. A run still in flight is cancelled first (`Command::Cancel`); the
//!   shell closes the session once that run has ended, or after a moment.
//! - **The user stops waiting for a cancel that has not landed** (a second
//!   cancel, confirmed): the same, without a new target.
//! - **Quit:** the same, for the tab's target; the shell gives those closes a
//!   moment to finish before the terminal is restored.
//! - **The connection is removed or edited:** dbx itself drops every pool of
//!   the connection, sessions included. A run on it is cancelled first, and
//!   the shell holds the removal until that run has ended.
//! - **Never on a sidebar collapse:** `Core::release` removes only a
//!   database's shared pool, keyed without the session, so collapsing the
//!   tab's database in the tree cannot end the tab's session or a run on it.

use std::time::{Duration, Instant};

use crate::core::{CoreError, CoreErrorKind, DatabasePath, DbKind, ResultSet, RunId, SessionId, TabId, TablePath};


/// One query tab: what the Core said about it. What it shows is `tab::TabUi`.
#[derive(Debug, Clone)]
pub struct QueryTab {
    pub id: TabId,
    /// Where its statements run. Set only from the sidebar's "use" key, never
    /// by moving the sidebar's cursor.
    pub target: Option<DatabasePath>,
    /// The serial of the tab's current database session (see `SessionId`).
    pub session: u32,
    pub run: RunState,
    /// What the last run that finished gave.
    pub outcome: Option<Outcome>,
    /// Runs the tab let go of before they ended (it stopped waiting, or was
    /// given another target or lost its connection mid-cancel): each may
    /// still be running on the server until its stale reply says it ended.
    pub abandoned: Vec<Abandoned>,
    /// The session serial the outcome's rows were fetched in: "load more"
    /// re-runs a statement, and in another session the `SET` values and
    /// temporary tables it relied on are gone.
    pub result_session: u32,
    /// The run in flight, if it is "load more" of the shown result.
    pub loading_more: Option<LoadMore>,
    /// The run in flight, or the last one, had `search_path` in its text.
    pub run_mentions_search_path: bool,
    /// Set while the tab pages through one table (opened from the sidebar);
    /// editing the text and running it clears it.
    pub browse: Option<Browse>,
    /// The run in flight is a statement of the user's (typed, chosen, or run
    /// from the history), whose outcome goes to the history. Load more and
    /// the browse pages and counts are not.
    pub history: bool,
}

/// A tab that pages through one table: the select it holds is generated, one
/// page at a time, with a stable order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    pub table: TablePath,
    /// The kind of database, to build the next page's SQL.
    pub kind: DbKind,
    /// The columns the pages are ordered by: the primary key. Empty when the
    /// table has none, and rows may move between pages.
    pub order_by: Vec<String>,
    /// Rows to a page: the row cap the tab opened with.
    pub page_size: usize,
    /// How many rows come before the page on screen.
    pub offset: usize,
    /// The text of the editor that the page on screen was read with; the tab
    /// leaves browse mode when it is run changed.
    pub sql: String,
    pub count: Count,
    /// The run in flight, if it is one of the browse's: a page or the count.
    pub fetch: Option<Fetch>,
    /// The last page asked for failed or timed out: what to say about it. The
    /// page on screen is the earlier run's until another run starts.
    pub failed: Option<String>,
    /// The run just cancelled was the count: the page on screen is still the
    /// latest page, not an earlier run's.
    pub count_cancelled: bool,
}

/// What is known of the number of rows in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// Not asked: it costs the server a `count(*)`, so it is on request.
    NotAsked,
    /// Asked, no answer yet.
    Asking,
    Known(u64),
}

/// A browse run in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// A page, starting at row `offset`, with the select that reads it.
    Page { offset: usize, sql: String },
    Count,
}

/// A "load more" run in flight: the result it replaces stays on screen,
/// navigable, until it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadMore {
    /// The row cap it runs with.
    pub cap: usize,
}

/// A run the tab let go of before it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandoned {
    /// The run's request, whose (stale) reply says it has ended.
    pub request: RequestId,
    /// `connection / database`, where it may still be running.
    pub on: String,
}

impl QueryTab {
    /// The session the tab's statements run in now.
    pub fn session(&self) -> SessionId {
        SessionId { tab: self.id, serial: self.session }
    }

    pub fn new(id: TabId) -> Self {
        QueryTab {
            id,
            target: None,
            session: 1,
            run: RunState::Idle,
            outcome: None,
            abandoned: Vec::new(),
            result_session: 1,
            loading_more: None,
            run_mentions_search_path: false,
            browse: None,
            history: false,
        }
    }

    /// The run in flight is no longer the tab's to wait for (cancelled, let
    /// go of, or the tab moved): nothing it was loading is pending.
    pub fn drop_pending(&mut self) {
        self.loading_more = None;
        if let Some(b) = &mut self.browse {
            let counting = b.fetch.take() == Some(Fetch::Count);
            b.count_cancelled = counting;
            if counting {
                b.count = Count::NotAsked;
            }
        }
    }
}

/// Where the tab's run is. A run that ends with a result (done, failed,
/// timed out) leaves the tab `Idle` with that `Outcome`; a cancelled run
/// leaves it `Cancelled`, and the outcome before it stays, marked as the
/// earlier run's.
///
/// **How a cancel ends (design section 11, as settled in slice 06).**
/// Cancelling bumps the run's generation, so whatever the run replies is
/// stale and never lands: no rows, no error. But the reply still says the run
/// is over, and that matters: the tab's session is one server connection, so
/// until the run has ended the statement is still running there, and a new
/// run would queue behind it. So the tab is `Cancelling` from the moment of
/// the cancel until the run's (stale) reply arrives, and only then
/// `Cancelled`; the user is never told a statement stopped while it may
/// still be running. If the reply has not come within `CANCEL_BOUND`, the
/// tab says the statement is still running on the server (`stuck`), and a
/// second cancel offers to drop the session instead. A stale reply that does
/// not belong to the run being cancelled (one superseded by a new target,
/// say) changes nothing and repaints nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Idle,
    /// A run is in flight: `request` is the Core call that carries it.
    Running { run: RunId, request: RequestId, started: Instant },
    /// The user cancelled the run; it has not ended yet.
    Cancelling {
        run: RunId,
        request: RequestId,
        started: Instant,
        /// When the cancel was asked for.
        asked: Instant,
        /// `CANCEL_BOUND` passed without the run ending: the statement is
        /// still running on the server.
        stuck: bool,
    },
    /// The last run was cancelled, and has ended.
    Cancelled {
        /// From the start of the run to its end.
        after: Duration,
        /// The statement, when the run's last word named it.
        statement: Option<String>,
        how: CancelEnd,
    },
}

impl RunState {
    /// The run in flight, running or being cancelled: its id, its request
    /// and when it started.
    pub fn in_flight(&self) -> Option<(RunId, RequestId, Instant)> {
        match *self {
            RunState::Running { run, request, started } | RunState::Cancelling { run, request, started, .. } => {
                Some((run, request, started))
            }
            _ => None,
        }
    }
}

/// How a cancelled run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelEnd {
    /// It stopped (dbx said cancelled, or it failed some other way).
    Stopped,
    /// It finished before the cancel reached the server: whatever it changed
    /// stands. Its result is not shown: what a cancelled run gave never is.
    FinishedFirst,
    /// It ended with an error other than the cancel (the connection was
    /// lost, say): whether the statement stopped is not known.
    Failed(String),
    /// The user stopped waiting for it (a second cancel): the tab moved to a
    /// new session and the old one is closed once dbx lets go of the run.
    /// Whether the server stopped the statement is not known.
    Abandoned,
    /// It was stopped after it reached the server, and the Core says what it
    /// did there is not known (`CoreError::outcome_unknown`): a run of more
    /// than one statement, whose earlier statements may have run, or of one
    /// not proven read-only. Saved to the history as such.
    OutcomeUnknown,
}

/// How long a cancel may take before the tab says the statement is still
/// running on the server. dbx sends Postgres's cancel request from inside the
/// run and returns once it has been sent (or its attempt, up to 5 s, failed);
/// on the fixture that took a few milliseconds, in the spike 1.0 s. The same
/// bound as the shell's wait for a run before closing its session
/// (`shell::RUN_WAIT`).
pub const CANCEL_BOUND: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rows(ResultSet),
    /// The run failed; `error.statement` is what ran, `elapsed` is measured
    /// by the app from the start of the run to the reply.
    Failed { error: CoreError, elapsed: Duration },
    /// The run was stopped by a time limit: the connection's query timeout,
    /// which `error.message` names. Not a failure of the SQL.
    TimedOut { error: CoreError, elapsed: Duration },
}

/// The state glyph a tab shows in the bar: yellow while its run is in
/// flight (running or being cancelled), red after a failure or a timeout,
/// none when idle or done.
pub fn glyph(tab: &QueryTab) -> Option<super::theme::Role> {
    use super::theme::Role;
    if tab.run.in_flight().is_some() {
        return Some(Role::Yellow);
    }
    match (&tab.run, &tab.outcome) {
        (RunState::Cancelled { how: CancelEnd::Failed(_), .. }, _) => Some(Role::Red),
        (RunState::Cancelled { .. }, _) => None,
        (_, Some(Outcome::Failed { .. } | Outcome::TimedOut { .. })) => Some(Role::Red),
        _ => None,
    }
}

/// A short message in a corner that expires on its own and takes no input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Something was refused or failed (red), rather than said (yellow).
    pub failure: bool,
    pub expires: Instant,
}

/// How long a notice stays.
pub const NOTICE_FOR: Duration = Duration::from_secs(4);

use super::keys::Action;
use super::requests::{RequestId, Target};
use super::tree::RowKind;
use super::overlay::{ConfirmDropSession, Overlay};
use super::{App, Command, CoreCall, Effects, Loadable, Mode};
use crate::core::{ConnectionId, RunRequest};

impl App {
    /// The active tab: the one keys, the editor and the results pane act on.
    pub(super) fn tab(&self) -> &QueryTab {
        self.model.active(self.ui.tab.id)
    }

    fn tab_mut(&mut self) -> &mut QueryTab {
        self.model.active_mut(self.ui.tab.id)
    }

    /// Whether `tab` is the one on screen.
    pub(super) fn is_active(&self, tab: TabId) -> bool {
        self.ui.tab.id == tab
    }

    /// Shows a notice until `NOTICE_FOR` from now.
    pub(super) fn notify(&mut self, text: String, failure: bool, fx: &mut Effects) {
        let now = self.ui.now.unwrap_or_else(Instant::now);
        self.ui.notice = Some(Notice { text, failure, expires: now + NOTICE_FOR });
        fx.repaint = true;
    }

    /// Whether `use` would give the tab a database from the cursor row.
    pub(super) fn cursor_can_be_used(&self) -> bool {
        self.cursor_row().is_some_and(|row| {
            row.node.database_path().is_some()
                || matches!(&row.kind, RowKind::Connection { default_database: Some(_), .. })
                || (matches!(&row.kind, RowKind::Connection { .. })
                    && self.connection(row.node.connection()).is_some_and(|c| !c.capabilities.databases))
        })
    }

    /// Gives the tab the database under the sidebar's cursor: the one the row
    /// is in, or for a connection row the database it was saved with.
    pub(super) fn use_target(&mut self, fx: &mut Effects) {
        let Some(row) = self.cursor_row().cloned() else { return };
        let at = match (&row.kind, row.node.database_path()) {
            (_, Some(at)) => at.clone(),
            // A connection with no database level (a SQLite file) is itself
            // what a tab runs on, under the empty database name.
            (RowKind::Connection { .. }, None) if self.connection(row.node.connection()).is_some_and(|c| !c.capabilities.databases) => {
                DatabasePath { connection: row.node.connection().clone(), database: String::new() }
            }
            (RowKind::Connection { name, default_database: Some(db), .. }, None) => {
                let _ = name;
                DatabasePath { connection: row.node.connection().clone(), database: db.clone() }
            }
            (RowKind::Connection { name, .. }, None) => {
                let key = self.ui.keys.key_for(Action::Expand).unwrap_or_default();
                let use_key = self.ui.keys.key_for(Action::UseTarget).unwrap_or_default();
                self.say(format!("{name} has no database of its own: open it with {key} and press {use_key} on one."), false);
                fx.repaint = true;
                return;
            }
            _ => return,
        };
        self.retarget(Some(at), fx);
    }

    /// Points the active tab somewhere else (or nowhere). A run in flight is
    /// cancelled and its reply made stale; the old session is closed.
    fn retarget(&mut self, to: Option<DatabasePath>, fx: &mut Effects) {
        if self.tab().target == to {
            return;
        }
        self.end_session_of(self.ui.tab.id, fx);
        self.close_completion(fx);
        let tab = self.tab_mut();
        // A new session: the old one's close may land after the next run has
        // started, and must not end it (`SessionId`).
        if tab.target.is_some() {
            tab.session += 1;
        }
        tab.target = to;
        tab.outcome = None;
        tab.drop_pending();
        // The new target has not got the table the tab was paging.
        tab.browse = None;
        // Nothing of the old target's runs is shown for the new one.
        tab.run = RunState::Idle;
        fx.repaint = true;
    }

    /// Lets go of tab `id`'s run, if one is in flight: cancelled (again, if
    /// it was being cancelled: a cancel that never reached dbx is not lost)
    /// and its reply made stale. A question about it goes too.
    fn abandon_run_of(&mut self, id: TabId, fx: &mut Effects) {
        let Some((run, request, _)) = self.model.tab(id).and_then(|t| t.run.in_flight()) else { return };
        fx.commands.push(Command::Cancel(run));
        self.model.requests.supersede(Target::Run(id));
        self.remember_abandoned(id, request);
        if let Some(tab) = self.model.tab_mut(id) {
            tab.run = RunState::Idle;
            tab.drop_pending();
        }
        if self.is_active(id) {
            if matches!(self.ui.overlay, Some(Overlay::ConfirmDropSession(_))) {
                self.ui.overlay = None;
            }
            fx.repaint = true;
        }
    }

    /// Notes that tab `id` let go of run `request` before it ended.
    fn remember_abandoned(&mut self, id: TabId, request: RequestId) {
        let on = self.target_label_of(id).unwrap_or_else(|| "the server".into());
        if let Some(tab) = self.model.tab_mut(id) {
            tab.abandoned.push(Abandoned { request, on });
        }
    }

    /// The places where a run some tab let go of may still be running.
    pub fn abandoned_on(&self) -> Vec<String> {
        let own = self.model.tabs.iter().flat_map(|t| t.abandoned.iter().map(|a| a.on.clone()));
        own.chain(self.model.orphaned.iter().map(|(_, a)| a.on.clone())).collect()
    }

    /// Cancels tab `id`'s run, if any, and closes its session, if any. The
    /// shell closes the session only once the run has ended (or a moment has
    /// passed), so the cancel can still reach the server.
    pub(super) fn end_session_of(&mut self, id: TabId, fx: &mut Effects) {
        self.abandon_run_of(id, fx);
        if let Some(tab) = self.model.tab(id) {
            if let Some(at) = tab.target.clone() {
                fx.commands.push(Command::CloseSession(tab.session(), at));
            }
        }
    }

    /// Quit: end every tab's session first.
    pub(super) fn quit(&mut self, fx: &mut Effects) {
        self.cancel_dump_for_quit(fx);
        let ids: Vec<TabId> = self.model.tabs.iter().map(|t| t.id).collect();
        for id in ids {
            self.end_session_of(id, fx);
        }
        self.save_layout(fx);
        fx.commands.push(Command::Quit);
    }

    /// A connection was removed: every tab pointed at it points nowhere now.
    pub(super) fn connection_removed(&mut self, id: &ConnectionId, fx: &mut Effects) {
        let on: Vec<TabId> = self.model.tabs.iter().filter(|t| t.target.as_ref().is_some_and(|t| &t.connection == id)).map(|t| t.id).collect();
        for tab in on {
            self.abandon_run_of(tab, fx);
            if self.is_active(tab) {
                self.close_completion(fx);
                fx.repaint = true;
            }
            // dbx dropped the connection's pools, sessions with them.
            if let Some(t) = self.model.tab_mut(tab) {
                t.session += 1;
                t.run = RunState::Idle;
                t.target = None;
                t.outcome = None;
                t.drop_pending();
                t.browse = None;
            }
        }
    }

    /// Runs the selection, or the statement under the cursor, in the tab.
    pub(super) fn run_statement(&mut self, now: Instant, fx: &mut Effects) {
        self.run_range(None, now, fx)
    }

    /// Whether a run can start now; when it cannot, the notice says why.
    pub(super) fn can_run(&mut self, fx: &mut Effects) -> bool {
        if !self.can_start(fx) {
            return false;
        }
        if self.ui.tab.editor.text().trim().is_empty() {
            self.notify("Nothing to run: type a statement first.".into(), false, fx);
            return false;
        }
        true
    }

    /// `can_run` for text that is not the editor's (a statement from the
    /// history): everything but the editor needing text.
    fn can_start(&mut self, fx: &mut Effects) -> bool {
        let use_key = self.ui.keys.key_for(Action::UseTarget).unwrap_or_default();
        if self.tab().target.is_none() {
            let text = format!("This tab has no database yet: in the sidebar, press {use_key} on one.");
            self.notify(text, true, fx);
            return false;
        }
        if let Some(c) = self.active_connection().filter(|c| !c.capabilities.query) {
            let text = format!("{} cannot be queried here: dbx does not run statements on {}.", c.name, c.kind.display_name());
            self.notify(text, true, fx);
            return false;
        }
        // The session is one connection: a new run would only queue behind
        // the old one, cancelled or not, so it waits until that has ended.
        match self.tab().run {
            RunState::Running { .. } => {
                self.notify("A statement is still running in this tab.".into(), true, fx);
                return false;
            }
            RunState::Cancelling { .. } => {
                let key = self.ui.keys.key_for(Action::CancelDirect).unwrap_or_default();
                let text = format!("The cancelled statement has not stopped yet: run again once it has, or press {key} to stop waiting.");
                self.notify(text, true, fx);
                return false;
            }
            _ => {}
        }
        true
    }

    /// Runs `range` of the buffer, or the selection, or else the statement
    /// under the cursor.
    pub(super) fn run_range(&mut self, range: Option<(crate::core::Cursor, crate::core::Cursor)>, now: Instant, fx: &mut Effects) {
        if !self.can_run(fx) {
            return;
        }
        // Text that was edited, or a part of it, is a query of the user's now.
        self.leave_browse_if_edited(range.is_none() && self.ui.tab.editor.selection().is_none());
        let buffer = self.ui.tab.editor.text();
        let (cursor, selection) = (self.ui.tab.editor.cursor(), range.or_else(|| self.ui.tab.editor.selection()));
        self.begin_run(buffer, cursor, selection, now, fx);
    }

    /// Runs `sql`, all of it and only it, in the active tab: a statement from
    /// the history. The editor is left as it is. The same path as any run,
    /// so the Core's confirmation gate applies.
    pub(super) fn run_sql(&mut self, sql: &str, now: Instant, fx: &mut Effects) {
        if !self.can_start(fx) {
            return;
        }
        // Not the table the tab was paging any more.
        self.leave_browse_if_edited(false);
        let lines: Vec<&str> = sql.split('\n').collect();
        let end = crate::core::Cursor { row: lines.len() - 1, col: lines.last().map_or(0, |l| l.chars().count()) };
        self.begin_run(sql.to_string(), crate::core::Cursor::default(), Some((crate::core::Cursor::default(), end)), now, fx);
    }

    /// Sends the run of `selection` (or the statement at `cursor`) of
    /// `buffer`, which `can_run` has found possible.
    fn begin_run(&mut self, buffer: String, cursor: crate::core::Cursor, selection: Option<(crate::core::Cursor, crate::core::Cursor)>, now: Instant, fx: &mut Effects) {
        let Some(target) = self.tab().target.clone() else { return };
        let tab = self.tab().id;
        let run = crate::core::RunId(self.model.next_run);
        self.model.next_run += 1;
        let id = self.model.requests.begin(Target::Run(tab));
        let mentions = buffer.to_lowercase().contains("search_path");
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            session: self.tab().session(),
            buffer,
            cursor,
            selection,
            row_cap: self.model.row_cap,
            confirm_dangerous: self.model.settings.confirm_dangerous,
            confirmation: None,
        };
        let t = self.tab_mut();
        t.run_mentions_search_path = mentions;
        t.run = RunState::Running { run, request: id, started: now };
        t.history = true;
        if let Some(b) = t.browse.as_mut() {
            b.failed = None;
            b.count_cancelled = false;
        }
        fx.commands.push(Command::Core(id, CoreCall::Run(run, request)));
        fx.repaint = true;
    }

    /// Cancels the tab's run: at once it is `Cancelling` and its reply is
    /// stale (see `RunState`). With nothing in flight, nothing happens at all.
    /// A second cancel, while the first has not landed, asks whether to
    /// drop the tab's session instead.
    pub(super) fn cancel_run(&mut self, now: Instant, fx: &mut Effects) {
        match self.tab().run {
            RunState::Running { run, request, started } => {
                fx.commands.push(Command::Cancel(run));
                let tab = self.tab().id;
                self.model.requests.supersede(Target::Run(tab));
                self.tab_mut().run = RunState::Cancelling { run, request, started, asked: now, stuck: false };
                // The rows on screen are the old result; there is no more coming.
                self.tab_mut().drop_pending();
                fx.repaint = true;
            }
            RunState::Cancelling { .. } => {
                let target = self.target_label().unwrap_or_default();
                self.ui.overlay = Some(Overlay::ConfirmDropSession(ConfirmDropSession { tab: self.ui.tab.id, target, stopped: false }));
                self.ui.mode = Mode::Normal;
                fx.repaint = true;
            }
            _ => {}
        }
    }

    /// The second-cancel question was answered yes: the tab stops waiting
    /// for the run and moves to a new session, so it can run again at once.
    /// The old session is closed the usual way (`Command::CloseSession`, once
    /// dbx has let go of the run, or after a moment).
    ///
    /// Not closed at once, and the run not aborted: dbx keeps a busy
    /// Postgres connection open until its statement ends, whatever happens
    /// to the pool, so an early close would not stop the statement; and
    /// aborting the run would cut short dbx's own cancel attempt, which it
    /// gives up to 5 s per try.
    pub(super) fn drop_session(&mut self, fx: &mut Effects) {
        // The tab the question was asked in, whichever is active now.
        let asked = match &self.ui.overlay {
            Some(Overlay::ConfirmDropSession(ask)) => Some(ask.tab),
            _ => None,
        };
        self.ui.overlay = None;
        fx.repaint = true;
        let id = asked.unwrap_or(self.ui.tab.id);
        let Some(tab) = self.model.tab(id) else { return };
        let RunState::Cancelling { started, request, .. } = tab.run else { return };
        let Some(at) = tab.target.clone() else { return };
        fx.commands.push(Command::CloseSession(tab.session(), at));
        let after = self.ui.now.map_or(Duration::ZERO, |now| now.saturating_duration_since(started));
        self.remember_abandoned(id, request);
        // A notice that said to wait for this run no longer holds.
        if self.is_active(id) {
            self.ui.notice = None;
        }
        let tab = self.model.active_mut(id);
        tab.session += 1;
        tab.run = RunState::Cancelled { after, statement: None, how: CancelEnd::Abandoned };
    }

    /// A stale reply to a run of tab `tab`. If it is the run being cancelled,
    /// the run has ended: the tab is `Cancelled`, and what the reply carried
    /// is dropped. Any other stale reply changes nothing and repaints
    /// nothing; nor does any reply for a tab that is not on screen.
    pub(super) fn on_ran_after_cancel(&mut self, tab: TabId, id: RequestId, result: Result<ResultSet, CoreError>, fx: &mut Effects) {
        if self.model.tab(tab).is_none() {
            // A removed tab's run has ended: it is no longer running anywhere.
            self.model.orphaned.retain(|(_, a)| a.request != id);
            if !self.model.orphaned.iter().any(|(t, _)| *t == tab) {
                self.model.requests.forget(&Target::Run(tab));
            }
            return;
        }
        self.after_run_forget_current_schema(tab, fx);
        let active = self.is_active(tab);
        let now = self.ui.now;
        let Some(t) = self.model.tab_mut(tab) else { return };
        // A run the tab let go of has ended: it is no longer running anywhere.
        let before = t.abandoned.len();
        t.abandoned.retain(|a| a.request != id);
        if t.abandoned.len() != before {
            fx.repaint |= active;
            return;
        }
        let RunState::Cancelling { request, started, .. } = t.run else { return };
        if request != id {
            return;
        }
        let after = now.map_or(Duration::ZERO, |now| now.saturating_duration_since(started));
        // A statement that finished before the cancel landed ran, and what it
        // changed stands: it is saved. One the cancel stopped is not (it
        // never finished, and whether the server ran any of it is unknown).
        let saved = if std::mem::take(&mut t.history) { super::history::saved_of(&result, after) } else { None };
        let (statement, how) = match result {
            Ok(set) => (Some(set.ran), CancelEnd::FinishedFirst),
            Err(e) if e.kind == CoreErrorKind::Cancelled && e.outcome_unknown => (e.statement, CancelEnd::OutcomeUnknown),
            // Nothing ran (it waited for a yes): it is as stopped as can be.
            Err(e) if matches!(e.kind, CoreErrorKind::Cancelled | CoreErrorKind::NeedsConfirmation) => (e.statement, CancelEnd::Stopped),
            Err(e) => {
                let named = match e.kind {
                    CoreErrorKind::Connect => format!("could not reach the server: {}", e.message),
                    CoreErrorKind::Timeout => format!("timed out: {}", e.message),
                    _ => e.message,
                };
                (e.statement, CancelEnd::Failed(named))
            }
        };
        t.run = RunState::Cancelled { after, statement, how };
        if let Some(saved) = saved {
            self.save_run(tab, saved, fx);
        }
        if !active {
            return;
        }
        // The run ended on its own: the question of stopping waiting is moot.
        // The box stays, saying so, so a key meant for it cannot land in the
        // editor; any key it takes only closes it.
        if let Some(Overlay::ConfirmDropSession(ask)) = &mut self.ui.overlay {
            ask.stopped = true;
        }
        fx.repaint = true;
    }

    /// The reply to the active tab's current run.
    #[cfg(test)]
    pub(super) fn on_ran(&mut self, result: Result<crate::core::ResultSet, CoreError>, fx: &mut Effects) {
        self.on_ran_in(self.ui.tab.id, result, fx)
    }

    /// The reply to tab `tab`'s current run. For a tab that is not on
    /// screen, only that tab changes: no notice, no repaint.
    pub(super) fn on_ran_in(&mut self, tab: TabId, result: Result<crate::core::ResultSet, CoreError>, fx: &mut Effects) {
        if self.model.tab(tab).is_none() {
            return;
        }
        self.after_run_forget_current_schema(tab, fx);
        let active = self.is_active(tab);
        let now = self.ui.now;
        let Some(t) = self.model.tab_mut(tab) else { return };
        let elapsed = match (now, t.run.in_flight()) {
            (Some(now), Some((_, _, started))) => now.saturating_duration_since(started),
            _ => Duration::ZERO,
        };
        let more = t.loading_more.take();
        t.run = RunState::Idle;
        // A statement of the user's that has an outcome goes to the history
        // (below, once the tab's own state is settled).
        let saved = if std::mem::take(&mut t.history) { super::history::saved_of(&result, elapsed) } else { None };
        // Nothing ran: it waits for a yes, and the tab is as it was.
        if matches!(&result, Err(e) if e.kind == CoreErrorKind::NeedsConfirmation) {
            let fetch = t.browse.as_mut().and_then(|b| b.fetch.take());
            if let Err(error) = result {
                self.ask_to_run(tab, error, more, fetch, fx);
            }
            return;
        }
        // A page or the count of a browse tab lands by its own rules.
        if let Some(fetch) = t.browse.as_mut().and_then(|b| b.fetch.take()) {
            return self.browse_landed(tab, fetch, result, elapsed, fx);
        }
        match (result, more) {
            // "Load more" landed: the cursor stays on its row and column.
            (Ok(set), Some(_)) => self.more_landed(tab, set, fx),
            // It failed or was cut short: the rows on screen are still the
            // result, and the user is told.
            (Err(error), Some(_)) if error.kind != CoreErrorKind::Cancelled => {
                let what = if error.kind == CoreErrorKind::Timeout { "timed out" } else { "failed" };
                self.notify_tab(tab, format!("Load more {what}: {}", error.message), fx);
            }
            (Err(_), Some(_)) => {
                t.run = RunState::Cancelled { after: elapsed, statement: None, how: CancelEnd::Stopped };
            }
            (Ok(set), None) => {
                t.result_session = t.session;
                t.outcome = Some(Outcome::Rows(set));
                // A new result: back to the corner, nothing measured, and a
                // press on a cell of the old one pairs with nothing.
                if let Some(ui) = self.tab_ui_mut(tab) {
                    ui.grid.reset();
                }
                if active {
                    self.ui.clicks.forget();
                }
            }
            // Cancelled by something other than this tab (it would have made
            // the reply stale): the run is over, and gave nothing to show.
            (Err(error), None) if error.kind == CoreErrorKind::Cancelled => {
                let how = if error.outcome_unknown { CancelEnd::OutcomeUnknown } else { CancelEnd::Stopped };
                t.run = RunState::Cancelled { after: elapsed, statement: error.statement, how };
            }
            (Err(error), None) if error.kind == CoreErrorKind::Timeout => {
                t.outcome = Some(Outcome::TimedOut { error, elapsed });
                if active {
                    self.ui.clicks.forget();
                }
            }
            (Err(error), None) => {
                t.outcome = Some(Outcome::Failed { error, elapsed });
                if active {
                    self.ui.clicks.forget();
                }
            }
        }
        if let Some(saved) = saved {
            self.save_run(tab, saved, fx);
        }
    }

    /// The rows of a "load more" arrived in tab `tab`. They replace the
    /// result; the cursor stays on the same row and column, and is brought
    /// back inside the new result (with a notice, when the tab is on screen)
    /// if the data changed and it is now shorter.
    fn more_landed(&mut self, tab: TabId, set: ResultSet, fx: &mut Effects) {
        let active = self.is_active(tab);
        let (rows, cols) = (set.rows.len(), set.columns.len());
        // Load more only appends rows: what was on screen should be the start
        // of what came back. Compared once, here, not per frame.
        let changed = match self.model.tab(tab).and_then(|t| super::grid::shown(&t.outcome)) {
            Some(old) => old.columns != set.columns || rows < old.rows.len() || old.rows[..] != set.rows[..old.rows.len()],
            None => false,
        };
        let mut row_moved = false;
        if let Some(ui) = self.tab_ui_mut(tab) {
            let g = &mut ui.grid;
            row_moved = g.row >= rows.max(1);
            g.row = g.row.min(rows.saturating_sub(1));
            g.col = g.col.min(cols.saturating_sub(1));
            // The rows changed under the column widths and the window.
            g.widths.clear();
        }
        let loaded = rows;
        if let Some(t) = self.model.tab_mut(tab) {
            t.result_session = t.session;
            t.outcome = Some(Outcome::Rows(set));
        }
        if !active {
            return;
        }
        self.ui.clicks.forget();
        let loaded_text = super::view::thousands(loaded);
        if changed {
            let mut text = "The rows changed since the last run.".to_string();
            if row_moved {
                text.push_str(&format!(" The statement now returns {loaded_text} rows, so the cursor moved to the last."));
            }
            self.notify(text, false, fx);
        } else {
            self.notify(format!("Loaded {loaded_text} rows."), false, fx);
        }
    }

    /// When the time shown next changes: the notice's expiry, and while the
    /// active tab's run is in flight, the next whole second of its elapsed
    /// time. A tab not on screen shows no time, so it wakes nothing.
    pub(super) fn query_deadline(&self) -> Option<Instant> {
        let notice = self.ui.notice.as_ref().map(|n| n.expires);
        let started = self.tab().run.in_flight().map(|(_, _, started)| started);
        let tick = started.map(|started| {
            let secs = self.ui.now.map_or(0, |now| now.saturating_duration_since(started).as_secs());
            started + Duration::from_secs(secs + 1)
        });
        // The moment a cancel that has not landed counts as stuck.
        let bound = match self.tab().run {
            RunState::Cancelling { asked, stuck: false, .. } => Some(asked + CANCEL_BOUND),
            _ => None,
        };
        [notice, tick, bound].into_iter().flatten().min()
    }

    /// Time passed: a notice may have expired, the running clock moved on, a
    /// cancel may have taken too long (in any tab; only the active one
    /// repaints).
    pub(super) fn on_time(&mut self, now: Instant, fx: &mut Effects) {
        let last = self.ui.now.replace(now);
        if self.ui.notice.as_ref().is_some_and(|n| now >= n.expires) {
            self.ui.notice = None;
            fx.repaint = true;
        }
        if let (Some((_, _, started)), Some(last)) = (self.tab().run.in_flight(), last) {
            let shown = |t: Instant| t.saturating_duration_since(started).as_secs();
            if shown(now) != shown(last) {
                fx.repaint = true;
            }
        }
        let active = self.ui.tab.id;
        for tab in &mut self.model.tabs {
            if let RunState::Cancelling { asked, stuck, .. } = &mut tab.run {
                if !*stuck && now >= *asked + CANCEL_BOUND {
                    *stuck = true;
                    fx.repaint |= tab.id == active;
                }
            }
        }
    }

    /// The query timeout of the connection the tab runs on, if it sets one.
    pub(super) fn tab_timeout(&self) -> Option<Duration> {
        let at = self.tab().target.as_ref()?;
        match &self.model.connections {
            Loadable::Ready(list) => list.iter().find(|c| c.id == at.connection)?.query_timeout.map(Duration::from_secs),
            _ => None,
        }
    }

    /// What the editor's title says the active tab runs on: `name / database`.
    pub(super) fn target_label(&self) -> Option<String> {
        self.target_label_of(self.ui.tab.id)
    }

    /// `name / database` for tab `id`.
    pub(super) fn target_label_of(&self, id: TabId) -> Option<String> {
        let at = self.model.tab(id)?.target.as_ref()?;
        let name = match &self.model.connections {
            Loadable::Ready(list) => list.iter().find(|c| c.id == at.connection).map(|c| c.name.clone()),
            _ => None,
        }
        .unwrap_or_else(|| at.connection.0.clone());
        Some(format!("{name} / {}", at.database))
    }
}
