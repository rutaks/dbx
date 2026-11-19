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

use crate::core::{CoreError, CoreErrorKind, DatabasePath, ResultSet, RunId, SessionId, TabId};


/// One query tab. The tab bar arrives in slice 15; there is one tab until then.
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
        QueryTab { id, target: None, session: 1, run: RunState::Idle, outcome: None, abandoned: Vec::new(), result_session: 1, loading_more: None, run_mentions_search_path: false }
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
    /// The tab statements run in: the only one, until tabs arrive.
    pub(super) fn tab(&self) -> &QueryTab {
        &self.model.tabs[0]
    }

    fn tab_mut(&mut self) -> &mut QueryTab {
        &mut self.model.tabs[0]
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
        })
    }

    /// Gives the tab the database under the sidebar's cursor: the one the row
    /// is in, or for a connection row the database it was saved with.
    pub(super) fn use_target(&mut self, fx: &mut Effects) {
        let Some(row) = self.cursor_row().cloned() else { return };
        let at = match (&row.kind, row.node.database_path()) {
            (_, Some(at)) => at.clone(),
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

    /// Points the tab somewhere else (or nowhere). A run in flight is
    /// cancelled and its reply made stale; the old session is closed.
    fn retarget(&mut self, to: Option<DatabasePath>, fx: &mut Effects) {
        if self.tab().target == to {
            return;
        }
        self.end_tab_session(fx);
        self.close_completion(fx);
        let tab = self.tab_mut();
        // A new session: the old one's close may land after the next run has
        // started, and must not end it (`SessionId`).
        if tab.target.is_some() {
            tab.session += 1;
        }
        tab.target = to;
        tab.outcome = None;
        tab.loading_more = None;
        // Nothing of the old target's runs is shown for the new one.
        tab.run = RunState::Idle;
        fx.repaint = true;
    }

    /// Lets go of the tab's run, if one is in flight: cancelled (again, if
    /// it was being cancelled: a cancel that never reached dbx is not lost)
    /// and its reply made stale. A question about it goes too.
    fn abandon_run(&mut self, fx: &mut Effects) {
        let id = self.tab().id;
        if let Some((run, request, _)) = self.tab().run.in_flight() {
            fx.commands.push(Command::Cancel(run));
            self.model.requests.supersede(Target::Run(id));
            self.remember_abandoned(request);
            self.tab_mut().run = RunState::Idle;
            self.tab_mut().loading_more = None;
            if matches!(self.ui.overlay, Some(Overlay::ConfirmDropSession(_))) {
                self.ui.overlay = None;
            }
            fx.repaint = true;
        }
    }

    /// Notes that the tab let go of run `request` before it ended.
    fn remember_abandoned(&mut self, request: RequestId) {
        let on = self.target_label().unwrap_or_else(|| "the server".into());
        self.tab_mut().abandoned.push(Abandoned { request, on });
    }

    /// The places where a run the tab let go of may still be running.
    pub fn abandoned_on(&self) -> Vec<String> {
        self.tab().abandoned.iter().map(|a| a.on.clone()).collect()
    }

    /// Cancels the tab's run, if any, and closes its session, if any. The
    /// shell closes the session only once the run has ended (or a moment has
    /// passed), so the cancel can still reach the server.
    fn end_tab_session(&mut self, fx: &mut Effects) {
        self.abandon_run(fx);
        if let Some(at) = self.tab().target.clone() {
            fx.commands.push(Command::CloseSession(self.tab().session(), at));
        }
    }

    /// Quit: end the tab's session first.
    pub(super) fn quit(&mut self, fx: &mut Effects) {
        self.end_tab_session(fx);
        self.save_layout(fx);
        fx.commands.push(Command::Quit);
    }

    /// A connection was removed: a tab pointed at it points nowhere now.
    pub(super) fn connection_removed(&mut self, id: &ConnectionId, fx: &mut Effects) {
        if self.tab().target.as_ref().is_some_and(|t| &t.connection == id) {
            self.abandon_run(fx);
            self.close_completion(fx);
            // dbx dropped the connection's pools, sessions with them.
            let t = self.tab_mut();
            t.session += 1;
            t.run = RunState::Idle;
            t.target = None;
            t.outcome = None;
            t.loading_more = None;
            fx.repaint = true;
        }
    }

    /// Runs the selection, or the statement under the cursor, in the tab.
    pub(super) fn run_statement(&mut self, now: Instant, fx: &mut Effects) {
        self.run_range(None, now, fx)
    }

    /// Whether a run can start now; when it cannot, the notice says why.
    pub(super) fn can_run(&mut self, fx: &mut Effects) -> bool {
        let use_key = self.ui.keys.key_for(Action::UseTarget).unwrap_or_default();
        if self.tab().target.is_none() {
            let text = format!("This tab has no database yet: in the sidebar, press {use_key} on one.");
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
        if self.ui.editor.text().trim().is_empty() {
            self.notify("Nothing to run: type a statement first.".into(), false, fx);
            return false;
        }
        true
    }

    /// Runs `range` of the buffer, or the selection, or else the statement
    /// under the cursor.
    pub(super) fn run_range(&mut self, range: Option<(crate::core::Cursor, crate::core::Cursor)>, now: Instant, fx: &mut Effects) {
        if !self.can_run(fx) {
            return;
        }
        let Some(target) = self.tab().target.clone() else { return };
        let tab = self.tab().id;
        let run = crate::core::RunId(self.model.next_run);
        self.model.next_run += 1;
        let id = self.model.requests.begin(Target::Run(tab));
        self.tab_mut().run_mentions_search_path = self.ui.editor.text().to_lowercase().contains("search_path");
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            session: self.tab().session(),
            buffer: self.ui.editor.text(),
            cursor: self.ui.editor.cursor(),
            selection: range.or_else(|| self.ui.editor.selection()),
            row_cap: self.model.row_cap,
        };
        self.tab_mut().run = RunState::Running { run, request: id, started: now };
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
                self.tab_mut().loading_more = None;
                fx.repaint = true;
            }
            RunState::Cancelling { .. } => {
                let target = self.target_label().unwrap_or_default();
                self.ui.overlay = Some(Overlay::ConfirmDropSession(ConfirmDropSession { target, stopped: false }));
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
        self.ui.overlay = None;
        fx.repaint = true;
        let RunState::Cancelling { started, request, .. } = self.tab().run else { return };
        let Some(at) = self.tab().target.clone() else { return };
        fx.commands.push(Command::CloseSession(self.tab().session(), at));
        let after = self.ui.now.map_or(Duration::ZERO, |now| now.saturating_duration_since(started));
        self.remember_abandoned(request);
        // A notice that said to wait for this run no longer holds.
        self.ui.notice = None;
        let tab = self.tab_mut();
        tab.session += 1;
        tab.run = RunState::Cancelled { after, statement: None, how: CancelEnd::Abandoned };
    }

    /// A stale reply to a run. If it is the run being cancelled, the run has
    /// ended: the tab is `Cancelled`, and what the reply carried is dropped.
    /// Any other stale reply changes nothing and repaints nothing.
    pub(super) fn on_ran_after_cancel(&mut self, id: RequestId, result: Result<ResultSet, CoreError>, fx: &mut Effects) {
        self.after_run_forget_current_schema(fx);
        // A run the tab let go of has ended: it is no longer running anywhere.
        let before = self.tab().abandoned.len();
        self.tab_mut().abandoned.retain(|a| a.request != id);
        if self.tab().abandoned.len() != before {
            fx.repaint = true;
            return;
        }
        let RunState::Cancelling { request, started, .. } = self.tab().run else { return };
        if request != id {
            return;
        }
        let after = self.ui.now.map_or(Duration::ZERO, |now| now.saturating_duration_since(started));
        let (statement, how) = match result {
            Ok(set) => (Some(set.ran), CancelEnd::FinishedFirst),
            Err(e) if e.kind == CoreErrorKind::Cancelled => (e.statement, CancelEnd::Stopped),
            Err(e) => {
                let named = match e.kind {
                    CoreErrorKind::Connect => format!("could not reach the server: {}", e.message),
                    CoreErrorKind::Timeout => format!("timed out: {}", e.message),
                    _ => e.message,
                };
                (e.statement, CancelEnd::Failed(named))
            }
        };
        self.tab_mut().run = RunState::Cancelled { after, statement, how };
        // The run ended on its own: the question of stopping waiting is moot.
        // The box stays, saying so, so a key meant for it cannot land in the
        // editor; any key it takes only closes it.
        if let Some(Overlay::ConfirmDropSession(ask)) = &mut self.ui.overlay {
            ask.stopped = true;
        }
        fx.repaint = true;
    }

    /// The reply to the tab's current run.
    pub(super) fn on_ran(&mut self, result: Result<crate::core::ResultSet, CoreError>, fx: &mut Effects) {
        self.after_run_forget_current_schema(fx);
        let now = self.ui.now;
        let tab = self.tab_mut();
        let elapsed = match (now, tab.run.in_flight()) {
            (Some(now), Some((_, _, started))) => now.saturating_duration_since(started),
            _ => Duration::ZERO,
        };
        let more = tab.loading_more.take();
        tab.run = RunState::Idle;
        match (result, more) {
            // "Load more" landed: the cursor stays on its row and column.
            (Ok(set), Some(_)) => self.more_landed(set, fx),
            // It failed or was cut short: the rows on screen are still the
            // result, and the user is told.
            (Err(error), Some(_)) if error.kind != CoreErrorKind::Cancelled => {
                let what = if error.kind == CoreErrorKind::Timeout { "timed out" } else { "failed" };
                self.notify(format!("Load more {what}: {}", error.message), true, fx);
            }
            (Err(_), Some(_)) => {
                let tab = self.tab_mut();
                tab.run = RunState::Cancelled { after: elapsed, statement: None, how: CancelEnd::Stopped };
            }
            (Ok(set), None) => {
                let tab = self.tab_mut();
                tab.result_session = tab.session;
                tab.outcome = Some(Outcome::Rows(set));
                // A new result: back to the corner, nothing measured, and a
                // press on a cell of the old one pairs with nothing.
                self.ui.grid.reset();
                self.ui.clicks.forget();
            }
            // Cancelled by something other than this tab (it would have made
            // the reply stale): the run is over, and gave nothing to show.
            (Err(error), None) if error.kind == CoreErrorKind::Cancelled => {
                tab.run = RunState::Cancelled { after: elapsed, statement: error.statement, how: CancelEnd::Stopped };
            }
            (Err(error), None) if error.kind == CoreErrorKind::Timeout => {
                tab.outcome = Some(Outcome::TimedOut { error, elapsed });
                self.ui.clicks.forget();
            }
            (Err(error), None) => {
                tab.outcome = Some(Outcome::Failed { error, elapsed });
                self.ui.clicks.forget();
            }
        }
    }

    /// The rows of a "load more" arrived. They replace the result; the cursor
    /// stays on the same row and column, and is brought back inside the new
    /// result (with a notice) if the data changed and it is now shorter.
    fn more_landed(&mut self, set: ResultSet, fx: &mut Effects) {
        let (rows, cols) = (set.rows.len(), set.columns.len());
        // Load more only appends rows: what was on screen should be the start
        // of what came back. Compared once, here, not per frame.
        let changed = match super::grid::shown(&self.model.tabs[0].outcome) {
            Some(old) => old.columns != set.columns || rows < old.rows.len() || old.rows[..] != set.rows[..old.rows.len()],
            None => false,
        };
        let g = &mut self.ui.grid;
        let row_moved = g.row >= rows.max(1);
        g.row = g.row.min(rows.saturating_sub(1));
        g.col = g.col.min(cols.saturating_sub(1));
        // The rows changed under the column widths and the window.
        g.widths.clear();
        let loaded = rows;
        let tab = self.tab_mut();
        tab.result_session = tab.session;
        tab.outcome = Some(Outcome::Rows(set));
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

    /// When the time shown next changes: the notice's expiry, and while a run
    /// is in flight, the next whole second of its elapsed time.
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

    /// Time passed: a notice may have expired, the running clock moved on.
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
        if let RunState::Cancelling { asked, stuck, .. } = &mut self.tab_mut().run {
            if !*stuck && now >= *asked + CANCEL_BOUND {
                *stuck = true;
                fx.repaint = true;
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

    /// What the editor's title says the tab runs on: `name / database`.
    pub(super) fn target_label(&self) -> Option<String> {
        let at = self.tab().target.as_ref()?;
        let name = match &self.model.connections {
            Loadable::Ready(list) => list.iter().find(|c| c.id == at.connection).map(|c| c.name.clone()),
            _ => None,
        }
        .unwrap_or_else(|| at.connection.0.clone());
        Some(format!("{name} / {}", at.database))
    }
}
