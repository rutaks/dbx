//! Confirmations (slice 17): the adapter will not run a statement that writes
//! to production, or a dangerous one, without the user's yes. It answers
//! `CoreErrorKind::NeedsConfirmation` with an `Assessment`; the app shows it
//! in a red `Confirm` box, and a yes sends the same run again carrying the
//! assessment's `Confirmation`, which the adapter accepts only for exactly
//! that statement, connection, database and session.
//!
//! The decision is never the app's: every path that runs a statement goes
//! through `Core::run`, so every path is gated (runs, the chooser, load more,
//! browse pages and counts).

use std::time::Instant;

use super::overlay::{ConfirmRun, Overlay, RunPurpose};
use super::query::{Count, Fetch, LoadMore, RunState};
use super::requests::Target;
use super::{App, Command, CoreCall, Effects, Mode};
use crate::core::{CoreError, Cursor, RunId, RunRequest, TabId};

impl App {
    /// The run of tab `tab` came back "needs confirmation": nothing ran. The
    /// tab is left as it was before the run, and the user is asked; a tab not
    /// on screen is told when it is next shown.
    pub(super) fn ask_to_run(&mut self, tab: TabId, error: CoreError, more: Option<LoadMore>, fetch: Option<Fetch>, fx: &mut Effects) {
        let row_cap = match (&more, &fetch, self.model.tab(tab).and_then(|t| t.browse.as_ref())) {
            (Some(m), _, _) => m.cap,
            (None, Some(Fetch::Count), _) => 1,
            (None, Some(Fetch::Page { .. }), Some(b)) => b.page_size,
            _ => self.model.row_cap,
        };
        if let (Some(Fetch::Count), Some(b)) = (&fetch, self.model.tab_mut(tab).and_then(|t| t.browse.as_mut())) {
            b.count = Count::NotAsked;
        }
        let purpose = match (more, fetch) {
            (Some(m), _) => RunPurpose::LoadMore(m),
            (None, Some(f)) => RunPurpose::Browse(f),
            (None, None) => RunPurpose::Plain,
        };
        let Some(assessment) = error.assessment else { return };
        if !self.is_active(tab) {
            if let Some(ui) = self.tab_ui_mut(tab) {
                ui.held_notice = Some("Not run: the statement needs confirming. Run it again here to see why.".into());
            }
            fx.repaint = true;
            return;
        }
        self.close_completion(fx);
        self.ui.overlay = Some(Overlay::ConfirmRun(Box::new(ConfirmRun { tab, assessment: *assessment, purpose, row_cap, scroll: 0, opened: self.ui.now })));
        self.ui.mode = Mode::Normal;
        fx.repaint = true;
    }

    /// `y`: the same run again, carrying the yes. It runs the confirmed text,
    /// whatever is in the editor by now, in the tab's session as it is now: if
    /// that is not the one confirmed, the adapter asks again.
    pub(super) fn confirm_run(&mut self, now: Instant, fx: &mut Effects) {
        if let Some(Overlay::ConfirmRun(ask)) = &self.ui.overlay {
            if ask.opened.is_some_and(|opened| now < opened + super::overlay::CONFIRM_GRACE) {
                return;
            }
        }
        let Some(Overlay::ConfirmRun(ask)) = self.ui.overlay.take() else { return };
        fx.repaint = true;
        let ConfirmRun { tab: id, assessment, purpose, row_cap, .. } = *ask;
        let Some(tab) = self.model.tab(id) else { return };
        if tab.run.in_flight().is_some() {
            return self.notify("Not run: a statement is still running in this tab.".into(), true, fx);
        }
        let Some(target) = tab.target.clone() else {
            return self.notify("Not run: this tab has no database now.".into(), true, fx);
        };
        let statement = assessment.confirmation.statement.clone();
        let lines: Vec<&str> = statement.split('\n').collect();
        let end = Cursor { row: lines.len() - 1, col: lines.last().map_or(0, |l| l.chars().count()) };
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            session: tab.session(),
            buffer: statement.clone(),
            cursor: Cursor::default(),
            selection: Some((Cursor::default(), end)),
            row_cap,
            confirm_dangerous: self.model.settings.confirm_dangerous,
            confirmation: Some(assessment.confirmation),
        };
        let run = RunId(self.model.next_run);
        self.model.next_run += 1;
        let request_id = self.model.requests.begin(Target::Run(id));
        let tab = self.model.active_mut(id);
        tab.run = RunState::Running { run, request: request_id, started: now };
        match purpose {
            RunPurpose::Plain => {
                tab.run_mentions_search_path = statement.to_lowercase().contains("search_path");
                // The statement the user said yes to has an outcome now.
                tab.history = true;
            }
            RunPurpose::LoadMore(m) => {
                tab.loading_more = Some(m);
                tab.history = false;
            }
            RunPurpose::Browse(fetch) => {
                tab.history = false;
                if let Some(b) = tab.browse.as_mut() {
                    if fetch == Fetch::Count {
                        b.count = Count::Asking;
                    }
                    b.fetch = Some(fetch);
                    b.failed = None;
                    b.count_cancelled = false;
                }
            }
        }
        fx.commands.push(Command::Core(request_id, CoreCall::Run(run, request)));
    }

    /// `esc`: nothing runs, and the tab is as it was.
    pub(super) fn decline_run(&mut self, fx: &mut Effects) {
        self.ui.overlay = None;
        self.notify("Not run.".into(), false, fx);
    }

    /// Whether tab `id` runs on a database marked production (its connection,
    /// or the database in the connection's list): the `PROD` badge.
    pub(super) fn tab_is_production(&self, id: TabId) -> bool {
        let Some(at) = self.model.tab(id).and_then(|t| t.target.as_ref()) else { return false };
        self.connection(&at.connection).is_some_and(|c| c.is_production_database(&at.database))
    }
}
