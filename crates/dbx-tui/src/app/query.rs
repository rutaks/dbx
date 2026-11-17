//! The query tab: where its statements run (its target, set explicitly from
//! the sidebar), the run in flight, and what the last run gave.
//!
//! A tab owns a database session (dbx's `client_session_id` is the tab's id,
//! design section 10). When it closes:
//!
//! - **The tab is pointed at another database or connection:** the old
//!   session is closed at once (`Command::CloseSession` with the old target).
//!   A run still in flight there is cancelled first (`Command::Cancel`), and
//!   its reply, superseded, is dropped.
//! - **Quit:** the same, for the tab's target; the shell gives those closes a
//!   moment to finish before the terminal is restored.
//! - **The connection is removed or edited:** dbx itself drops every pool of
//!   the connection, sessions included.
//! - **Never on a sidebar collapse:** `Core::release` removes only a
//!   database's shared pool, keyed without the session, so collapsing the
//!   tab's database in the tree cannot end the tab's session or a run on it.

use std::time::{Duration, Instant};

use crate::core::{CoreError, DatabasePath, ResultSet, RunId, TabId};


/// One query tab. The tab bar arrives in slice 15; there is one tab until then.
#[derive(Debug, Clone)]
pub struct QueryTab {
    pub id: TabId,
    /// Where its statements run. Set only from the sidebar's "use" key, never
    /// by moving the sidebar's cursor.
    pub target: Option<DatabasePath>,
    pub run: RunState,
    /// What the last run that finished gave.
    pub outcome: Option<Outcome>,
}

impl QueryTab {
    pub fn new(id: TabId) -> Self {
        QueryTab { id, target: None, run: RunState::Idle, outcome: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Idle,
    /// A run is in flight. Its `RunId` is what a cancel names (slice 06).
    Running { run: RunId, started: Instant },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rows(ResultSet),
    /// The run failed; `error.statement` is what ran, `elapsed` is measured
    /// by the app from the start of the run to the reply.
    Failed { error: CoreError, elapsed: Duration },
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
use super::requests::Target;
use super::tree::RowKind;
use super::{App, Command, CoreCall, Effects, Loadable};
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
        let tab = self.tab_mut();
        tab.target = to;
        tab.outcome = None;
        fx.repaint = true;
    }

    /// Cancels the tab's run, if any, and closes its session, if any.
    fn end_tab_session(&mut self, fx: &mut Effects) {
        let id = self.tab().id;
        if let RunState::Running { run, .. } = self.tab().run {
            fx.commands.push(Command::Cancel(run));
            self.model.requests.supersede(Target::Run(id));
            self.tab_mut().run = RunState::Idle;
        }
        if let Some(at) = self.tab().target.clone() {
            fx.commands.push(Command::CloseSession(id, at));
        }
    }

    /// Quit: end the tab's session first.
    pub(super) fn quit(&mut self, fx: &mut Effects) {
        self.end_tab_session(fx);
        fx.commands.push(Command::Quit);
    }

    /// A connection was removed: a tab pointed at it points nowhere now.
    pub(super) fn connection_removed(&mut self, id: &ConnectionId, fx: &mut Effects) {
        if self.tab().target.as_ref().is_some_and(|t| &t.connection == id) {
            let tab = self.tab().id;
            if let RunState::Running { run, .. } = self.tab().run {
                fx.commands.push(Command::Cancel(run));
                self.model.requests.supersede(Target::Run(tab));
            }
            // dbx dropped the connection's pools, sessions with them.
            let t = self.tab_mut();
            t.run = RunState::Idle;
            t.target = None;
            t.outcome = None;
            fx.repaint = true;
        }
    }

    /// Runs the selection, or the statement under the cursor, in the tab.
    pub(super) fn run_statement(&mut self, now: Instant, fx: &mut Effects) {
        let use_key = self.ui.keys.key_for(Action::UseTarget).unwrap_or_default();
        let Some(target) = self.tab().target.clone() else {
            let text = format!("This tab has no database yet: in the sidebar, press {use_key} on one.");
            return self.notify(text, true, fx);
        };
        if matches!(self.tab().run, RunState::Running { .. }) {
            return self.notify("A statement is still running in this tab.".into(), true, fx);
        }
        if self.ui.editor.text().trim().is_empty() {
            return self.notify("Nothing to run: type a statement first.".into(), false, fx);
        }
        let tab = self.tab().id;
        let run = crate::core::RunId(self.model.next_run);
        self.model.next_run += 1;
        let id = self.model.requests.begin(Target::Run(tab));
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            tab,
            buffer: self.ui.editor.text(),
            cursor: self.ui.editor.cursor(),
            selection: self.ui.editor.selection(),
            row_cap: self.model.row_cap,
        };
        self.tab_mut().run = RunState::Running { run, started: now };
        fx.commands.push(Command::Core(id, CoreCall::Run(run, request)));
        fx.repaint = true;
    }

    /// The reply to the tab's current run.
    pub(super) fn on_ran(&mut self, result: Result<crate::core::ResultSet, CoreError>) {
        let now = self.ui.now;
        let tab = self.tab_mut();
        let started = match tab.run {
            RunState::Running { started, .. } => Some(started),
            RunState::Idle => None,
        };
        tab.run = RunState::Idle;
        tab.outcome = Some(match result {
            Ok(set) => Outcome::Rows(set),
            Err(error) => {
                let elapsed = match (now, started) {
                    (Some(now), Some(started)) => now.saturating_duration_since(started),
                    _ => Duration::ZERO,
                };
                Outcome::Failed { error, elapsed }
            }
        });
    }

    /// When the time shown next changes: the notice's expiry, and while a run
    /// is in flight, the next whole second of its elapsed time.
    pub(super) fn query_deadline(&self) -> Option<Instant> {
        let notice = self.ui.notice.as_ref().map(|n| n.expires);
        let tick = match (self.tab().run.clone(), self.ui.now) {
            (RunState::Running { started, .. }, Some(now)) => {
                let secs = now.saturating_duration_since(started).as_secs();
                Some(started + Duration::from_secs(secs + 1))
            }
            (RunState::Running { started, .. }, None) => Some(started + Duration::from_secs(1)),
            _ => None,
        };
        [notice, tick].into_iter().flatten().min()
    }

    /// Time passed: a notice may have expired, the running clock moved on.
    pub(super) fn on_time(&mut self, now: Instant, fx: &mut Effects) {
        let last = self.ui.now.replace(now);
        if self.ui.notice.as_ref().is_some_and(|n| now >= n.expires) {
            self.ui.notice = None;
            fx.repaint = true;
        }
        if let (RunState::Running { started, .. }, Some(last)) = (&self.tab().run, last) {
            let shown = |t: Instant| t.saturating_duration_since(*started).as_secs();
            if shown(now) != shown(last) {
                fx.repaint = true;
            }
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
