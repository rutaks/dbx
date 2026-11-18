//! The thin outer layer: owns the terminal and tokio, turns the app's
//! commands into Core calls, and feeds results back as events. One
//! `select!` over the terminal's events, the reply channel and the app's
//! next deadline; it draws only when an update asks for a repaint.

pub mod prefs;
mod terminal;

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::app::requests::RequestId;
use crate::app::tree::{Listing, NodeId};
use crate::app::{App, Command, CoreCall, CoreReply, Effects, Event};
use crate::core::{ConnectionId, Core, SessionId};

pub use terminal::TerminalGuard;

/// The Core calls in flight, by request, so the app can abandon one: aborting
/// the task drops the Core's future, which is what stops a slow listing.
#[derive(Clone)]
pub struct Tasks {
    /// Where `Command::CopyToClipboard` text goes: the terminal, in the
    /// client; nowhere, unless a test gives one.
    clipboard: ClipboardSink,
    calls: Arc<Mutex<HashMap<RequestId, AbortHandle>>>,
    /// Session closes still running, for quitting to wait on.
    closing: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// The run in flight in each session, with its connection: true once it
    /// has ended (or its task is gone).
    runs: Arc<Mutex<HashMap<SessionId, RunEntry>>>,
    /// Where `Command::SavePrefs` writes; nowhere unless something is given.
    prefs: Option<std::path::PathBuf>,
}

/// What puts text on the clipboard.
pub type ClipboardSink = Arc<dyn Fn(&str) + Send + Sync>;

impl Default for Tasks {
    fn default() -> Self {
        Tasks { clipboard: Arc::new(|_| {}), calls: Default::default(), closing: Default::default(), runs: Default::default(), prefs: None }
    }
}

impl Tasks {
    /// The same tasks, copying through `sink`.
    pub fn with_clipboard(mut self, sink: ClipboardSink) -> Self {
        self.clipboard = sink;
        self
    }

    /// The same tasks, saving the layout preferences to `path`.
    pub fn with_prefs(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.prefs = path;
        self
    }
}

#[derive(Clone)]
struct RunEntry {
    connection: ConnectionId,
    ended: tokio::sync::watch::Receiver<bool>,
}

/// Waits until a run has ended, or `bound` has passed. True if it ended.
async fn run_ended(mut done: tokio::sync::watch::Receiver<bool>, bound: std::time::Duration) -> bool {
    tokio::time::timeout(bound, async move {
        // A dropped sender (the task was aborted) means it ended too.
        while !*done.borrow_and_update() {
            if done.changed().await.is_err() {
                break;
            }
        }
    })
    .await
    .is_ok()
}

impl Tasks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RequestId, AbortHandle>> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many calls are in flight.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn abandon(&self, id: RequestId) {
        if let Some(handle) = self.lock().remove(&id) {
            handle.abort();
        }
    }
}

/// How long quitting waits for the query tabs' sessions to close.
pub const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// How long a session close waits for the tab's run to end first. dbx sends
/// Postgres's cancel request from inside the run, using the session's cancel
/// context, which closing the session removes: a close that went first would
/// leave the statement running on the server. A cancel lands in tens of
/// milliseconds on the fixture and took 1.0 s in the spike; 2 s covers a slow
/// network without making quitting feel stuck.
pub const RUN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The most quitting waits, in all, for runs to end and sessions to close. It
/// returns as soon as they have.
pub const QUIT_WAIT: std::time::Duration = std::time::Duration::from_millis(2_500);

/// Waits until every run and session close started so far has ended, or
/// `bound` has passed. True if they all ended.
pub async fn finish_closing(tasks: &Tasks, bound: std::time::Duration) -> bool {
    let closes: Vec<_> = std::mem::take(&mut *tasks.closing.lock().unwrap_or_else(|p| p.into_inner()));
    let runs: Vec<_> = tasks.runs.lock().unwrap_or_else(|p| p.into_inner()).values().map(|r| r.ended.clone()).collect();
    let all = async {
        let ended = futures::future::join_all(runs.into_iter().map(|r| run_ended(r, bound))).await;
        futures::future::join_all(closes).await;
        ended.into_iter().all(|e| e)
    };
    tokio::time::timeout(bound, all).await.unwrap_or(false)
}

/// What to tell the user once the terminal is restored, when quitting did not
/// see every run end and every session close: where a statement may still be
/// running. `None` when everything ended.
pub fn quit_note(ended: bool, places: &[String]) -> Option<String> {
    if ended {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    for place in places {
        if !seen.contains(&place.as_str()) {
            seen.push(place);
        }
    }
    let on = match seen.as_slice() {
        [] => "the server".to_string(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    Some(format!("A statement may still be running on {on}."))
}

/// Stack size for the runtime's threads. dbx's metadata and export futures
/// overflow tokio's 2 MiB default; dbx's own heads use 16 MiB
/// (`dbx_core::scheduled_backup::worker_runtime`).
const THREAD_STACK: usize = 16 * 1024 * 1024;

/// The client's tokio runtime: multi-threaded, with a 16 MiB stack per thread.
pub fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(THREAD_STACK).build()
}

/// Installs rustls's process-wide crypto provider, which dbx's TLS needs.
/// Call once at startup; a second call changes nothing.
pub fn install_crypto_provider() {
    // Err only means a provider is already installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Runs the client until the user quits. The terminal is restored on return.
/// Returns a line for the user when quitting could not see a statement end
/// (`quit_note`), to be printed once the terminal is theirs again.
///
/// `prefs` is the layout preferences file (beside the store): read at the
/// start, written when the layout changes. Without one the layout is the
/// default and nothing is written.
pub async fn run(core: Arc<dyn Core>, prefs: Option<std::path::PathBuf>) -> io::Result<Option<String>> {
    let mut guard = TerminalGuard::enter()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<(RequestId, CoreReply)>();
    let mut events = EventStream::new();
    let mut app = App::new();
    let syntax: Arc<dyn crate::core::Syntax> = core.clone();
    app.ui.syntax.set_handle(syntax);
    app.ui.theme = crate::app::theme::Palette::from_setting(std::env::var("DBX_TUI_THEME").ok().as_deref());
    if let Some(path) = &prefs {
        app.set_layout(self::prefs::load(path));
    }
    let tasks = Tasks::default().with_clipboard(Arc::new(terminal::copy_to_terminal)).with_prefs(prefs);
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut interrupt = signal(SignalKind::interrupt())?;

    let mut effects = app.start();
    loop {
        if execute(effects.commands, &core, &tx, &tasks) {
            // A tab's session ends cleanly if the server answers quickly;
            // otherwise the process ending closes its socket all the same.
            let ended = finish_closing(&tasks, QUIT_WAIT).await;
            return Ok(quit_note(ended, &app.abandoned_on()));
        }
        if effects.repaint {
            guard.terminal.draw(|frame| app.view(frame))?;
        }

        let deadline = app.next_deadline().map(tokio::time::Instant::from_std);
        let event = tokio::select! {
            biased;
            next = events.next() => match next {
                Some(Ok(term)) => from_terminal(term),
                Some(Err(e)) => return Err(e),
                None => return Ok(None),
            },
            // Being killed is a way to quit, the same way as the keys: runs are
            // cancelled and sessions closed, then the guard restores the terminal.
            _ = terminate.recv() => { effects = app.quit_requested(); continue; }
            _ = hangup.recv() => { effects = app.quit_requested(); continue; }
            _ = interrupt.recv() => { effects = app.quit_requested(); continue; }
            Some((id, reply)) = rx.recv() => Some(Event::Core(id, reply)),
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => Some(Event::Timer),
        };
        effects = match event {
            Some(event) => app.update(event, Instant::now()),
            None => Effects::default(),
        };
    }
}

/// Starts everything the app asked for, then reports whether it asked to
/// quit. Quit does not cut the list short: a Cancel next to it still runs.
pub(crate) fn execute(
    commands: Vec<Command>,
    core: &Arc<dyn Core>,
    tx: &mpsc::UnboundedSender<(RequestId, CoreReply)>,
    tasks: &Tasks,
) -> bool {
    let mut quit = false;
    for command in commands {
        match command {
            Command::Quit => quit = true,
            Command::CopyToClipboard(text) => (tasks.clipboard)(&text),
            // A write that fails leaves the defaults for the next run: nothing
            // is worth telling anyone about a layout.
            Command::SavePrefs(layout) => {
                if let Some(path) = &tasks.prefs {
                    let _ = prefs::save(path, &layout);
                }
            }
            Command::Cancel(run) => core.cancel(run),
            Command::Abandon(id) => tasks.abandon(id),
            Command::Release(at) => {
                let core = core.clone();
                // Nothing waits for it, and a failure leaves only an idle
                // connection the server will time out.
                tokio::spawn(async move {
                    let _ = core.release(&at).await;
                });
            }
            Command::CloseSession(session, at) => {
                let core = core.clone();
                let run = tasks.runs.lock().unwrap_or_else(|p| p.into_inner()).get(&session).map(|r| r.ended.clone());
                // Nothing waits for it but quitting: a failure leaves an idle
                // session the server ends when the process does. The tab's
                // run ends first, so its cancel can still reach the server.
                let task = tokio::spawn(async move {
                    if let Some(run) = run {
                        run_ended(run, RUN_WAIT).await;
                    }
                    let _ = core.close_session(session, &at).await;
                });
                let mut closing = tasks.closing.lock().unwrap_or_else(|p| p.into_inner());
                closing.retain(|t| !t.is_finished());
                closing.push(task);
            }
            Command::Core(id, call) => {
                let (core, tx, done) = (core.clone(), tx.clone(), tasks.clone());
                // A run tells whoever waits on its tab when it has ended.
                let ended = match &call {
                    CoreCall::Run(_, request) => {
                        let (ended, watch) = tokio::sync::watch::channel(false);
                        let mut runs = tasks.runs.lock().unwrap_or_else(|p| p.into_inner());
                        runs.retain(|_, r| !*r.ended.borrow());
                        runs.insert(request.session, RunEntry { connection: request.connection.clone(), ended: watch });
                        Some(ended)
                    }
                    _ => None,
                };
                // Removing a connection drops dbx's pools for it, and with them
                // the context a cancel reaches the server through: a run on it
                // (the app has cancelled it) ends first, or RUN_WAIT passes.
                let wait_for: Vec<_> = match &call {
                    CoreCall::RemoveConnection(id) => tasks
                        .runs
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .values()
                        .filter(|r| &r.connection == id)
                        .map(|r| r.ended.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                // Held while spawning, so a call that finishes at once cannot
                // forget itself before it has been remembered.
                let mut running = tasks.lock();
                let task = tokio::spawn(async move {
                    futures::future::join_all(wait_for.into_iter().map(|r| run_ended(r, RUN_WAIT))).await;
                    let reply = match call {
                        CoreCall::Connections => CoreReply::Connections(core.connections().await),
                        CoreCall::TestConnection(draft) => CoreReply::Tested(core.test_connection(&draft).await),
                        CoreCall::SaveConnection(draft) => CoreReply::Saved(core.save_connection(&draft).await),
                        CoreCall::RemoveConnection(id) => CoreReply::Removed(core.remove_connection(&id).await),
                        CoreCall::List(node) => CoreReply::Listed(list(core.as_ref(), node).await),
                        CoreCall::Run(run, request) => CoreReply::Ran(core.run(run, request).await),
                        CoreCall::Complete(request) => CoreReply::Completed(core.complete(&request).await),
                        CoreCall::CurrentSchema(at) => CoreReply::CurrentSchema(core.current_schema(&at).await),
                        CoreCall::TableColumns(at) => CoreReply::TableColumns(core.columns(&at).await),
                        CoreCall::ForeignKeys(at) => CoreReply::ForeignKeys(core.foreign_keys(&at).await),
                        CoreCall::AnalyzeReferences(kind, sql) => CoreReply::References(core.analyze_references(&kind, &sql).await),
                    };
                    done.lock().remove(&id);
                    if let Some(ended) = ended {
                        let _ = ended.send(true);
                    }
                    let _ = tx.send((id, reply));
                });
                running.insert(id, task.abort_handle());
            }
        }
    }
    quit
}

/// The Core call that lists `node`'s children.
async fn list(core: &dyn Core, node: NodeId) -> Result<Listing, crate::core::CoreError> {
    match node {
        NodeId::Connection(id) => core.databases(&id).await.map(Listing::Databases),
        NodeId::Database(at) => core.schemas(&at).await.map(Listing::Schemas),
        NodeId::Schema(at) => core.tables(&at).await.map(Listing::Tables),
        NodeId::Table(at) => core.columns(&at).await.map(Listing::Columns),
    }
}

/// Terminal events the app understands. Key releases are dropped.
pub fn from_terminal(event: TermEvent) -> Option<Event> {
    match event {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => Some(Event::Key(key)),
        TermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
        TermEvent::Paste(text) => Some(Event::Paste(text)),
        TermEvent::Resize(w, h) => Some(Event::Resize(w, h)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;
    use crate::core::{Call, FakeCore, RunId, Scripted};

    #[tokio::test]
    async fn quit_does_not_swallow_the_commands_around_it() {
        let fake = Arc::new(FakeCore::new());
        let core: Arc<dyn Core> = fake.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let quit = execute(vec![Command::Cancel(RunId(1)), Command::Quit, Command::Cancel(RunId(2))], &core, &tx, &Tasks::default());
        assert!(quit);
        assert_eq!(fake.calls(), vec![Call::Cancel(RunId(1)), Call::Cancel(RunId(2))]);
    }

    #[tokio::test]
    async fn a_copy_command_reaches_the_clipboard_sink_with_the_exact_text_and_touches_no_core_call() {
        let fake = Arc::new(FakeCore::new());
        let core: Arc<dyn Core> = fake.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        let tasks = Tasks::default().with_clipboard(Arc::new(move |text: &str| sink.lock().unwrap().push(text.to_string())));
        let quit = execute(vec![Command::CopyToClipboard("a\tb\u{1b}[0m".into()), Command::CopyToClipboard(String::new())], &core, &tx, &tasks);
        assert!(!quit);
        assert_eq!(*seen.lock().unwrap(), vec!["a\tb\u{1b}[0m".to_string(), String::new()]);
        assert!(fake.calls().is_empty() && rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_release_command_reaches_the_core_and_waits_for_nothing() {
        use crate::core::{ConnectionId, DatabasePath};
        let fake = Arc::new(FakeCore::new());
        let core: Arc<dyn Core> = fake.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let at = DatabasePath { connection: ConnectionId("c".into()), database: "edge".into() };
        let quit = execute(vec![Command::Release(at.clone())], &core, &tx, &Tasks::default());
        assert!(!quit);
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(fake.calls(), vec![Call::Release(at)]);
        assert!(rx.try_recv().is_err(), "no reply comes back");
    }

    #[tokio::test(start_paused = true)]
    async fn an_abandoned_request_is_aborted_so_its_reply_never_arrives_and_a_finished_one_forgets_itself() {
        use crate::core::{ConnectionId, Scripted};
        use std::time::Duration;
        let fake = Arc::new(FakeCore::new());
        fake.script_databases(Scripted::ok(vec!["slow".into()]).after(Duration::from_secs(5)));
        fake.script_databases(Scripted::ok(vec!["quick".into()]));
        let core: Arc<dyn Core> = fake.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        let list = |n: u64| Command::Core(RequestId(n), CoreCall::List(NodeId::Connection(ConnectionId("c".into()))));

        execute(vec![list(1)], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        assert_eq!(tasks.len(), 1, "in flight");
        execute(vec![Command::Abandon(RequestId(1))], &core, &tx, &tasks);
        assert!(tasks.is_empty(), "abandoning forgets it");
        tokio::time::advance(Duration::from_secs(10)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(rx.try_recv().is_err(), "the aborted call sent no reply");

        execute(vec![list(2)], &core, &tx, &tasks);
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let (id, reply) = rx.try_recv().expect("the second call replied");
        assert_eq!(id, RequestId(2));
        assert!(matches!(reply, CoreReply::Listed(Ok(Listing::Databases(v))) if v == vec!["quick".to_string()]));
        assert!(tasks.is_empty(), "a finished call forgets itself");
        execute(vec![Command::Abandon(RequestId(99))], &core, &tx, &tasks); // unknown: nothing happens
    }

    fn session() -> SessionId {
        SessionId { tab: crate::core::TabId(1), serial: 1 }
    }

    fn edge() -> crate::core::DatabasePath {
        crate::core::DatabasePath { connection: crate::core::ConnectionId("c".into()), database: "edge".into() }
    }

    #[tokio::test(start_paused = true)]
    async fn quitting_waits_for_session_closes_but_never_longer_than_the_bound() {
        use crate::core::Scripted;
        use std::time::Duration;
        let fake = Arc::new(FakeCore::new());
        fake.script_close_session(Scripted::ok(()).after(Duration::from_millis(100)));
        let core: Arc<dyn Core> = fake.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        let quit = execute(vec![Command::CloseSession(session(), edge()), Command::Quit], &core, &tx, &tasks);
        assert!(quit);
        let started = tokio::time::Instant::now();
        assert!(finish_closing(&tasks, QUIT_WAIT).await, "the close finished");
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(fake.calls(), vec![Call::CloseSession(session(), edge())]);

        // A close that hangs does not hold the quit beyond the bound.
        fake.script_close_session(Scripted::ok(()).after(Duration::from_secs(60)));
        execute(vec![Command::CloseSession(session(), edge())], &core, &tx, &tasks);
        let started = tokio::time::Instant::now();
        assert!(!finish_closing(&tasks, QUIT_WAIT).await, "gave up");
        assert_eq!(started.elapsed(), QUIT_WAIT);
    }

    fn run_call(n: u64) -> Command {
        let request = crate::core::RunRequest {
            connection: crate::core::ConnectionId("c".into()),
            database: Some("edge".into()),
            session: session(),
            buffer: "select pg_sleep(20)".into(),
            cursor: crate::core::Cursor::default(),
            selection: None,
            row_cap: 10,
        };
        Command::Core(RequestId(n), CoreCall::Run(RunId(n), request))
    }

    #[tokio::test(start_paused = true)]
    async fn a_tabs_session_is_closed_only_after_its_run_has_finished_so_the_cancel_can_reach_the_server() {
        use crate::core::Scripted;
        use std::time::Duration;
        let fake = Arc::new(FakeCore::new());
        // A run that ignores nothing but takes a second to wind down.
        fake.script_run(Scripted::ok(crate::core::ResultSet {
            columns: vec![], rows: vec![], truncated: false, elapsed: Duration::ZERO, messages: vec![], affected: 0, ran: String::new(), rerun: crate::core::Rerun::Proven,
        }).after(Duration::from_secs(1)));
        fake.script_close_session(Scripted::ok(()));
        let core: Arc<dyn Core> = fake.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        execute(vec![run_call(1)], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        let quit = execute(vec![Command::CloseSession(session(), edge()), Command::Quit], &core, &tx, &tasks);
        assert!(quit);
        tokio::time::advance(Duration::from_millis(900)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(fake.calls(), vec![Call::Run(RunId(1))], "no close while the run is still going");
        assert!(finish_closing(&tasks, QUIT_WAIT).await);
        assert_eq!(fake.calls(), vec![Call::Run(RunId(1)), Call::CloseSession(session(), edge())]);
    }

    #[tokio::test(start_paused = true)]
    async fn quitting_waits_for_a_cancelled_run_to_end_but_not_forever() {
        use std::time::Duration;
        let fake = Arc::new(FakeCore::new());
        // A run that would take a minute and is never cancelled: quitting gives up.
        fake.script_run(Scripted::err(crate::core::CoreErrorKind::Other, "slow").after(Duration::from_secs(60)));
        let core: Arc<dyn Core> = fake.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        execute(vec![run_call(1), Command::Quit], &core, &tx, &tasks);
        let started = tokio::time::Instant::now();
        assert!(!finish_closing(&tasks, QUIT_WAIT).await);
        assert_eq!(started.elapsed(), QUIT_WAIT);
        // A cancelled run ends at once, and quitting does not wait out the bound.
        let fake = Arc::new(FakeCore::new());
        fake.script_run(Scripted::err(crate::core::CoreErrorKind::Other, "slow").after(Duration::from_secs(60)));
        let core: Arc<dyn Core> = fake.clone();
        let tasks = Tasks::default();
        execute(vec![run_call(2)], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        execute(vec![Command::Cancel(RunId(2)), Command::Quit], &core, &tx, &tasks);
        let started = tokio::time::Instant::now();
        assert!(finish_closing(&tasks, QUIT_WAIT).await);
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[tokio::test(start_paused = true)]
    async fn removing_a_connection_waits_for_a_run_on_it_to_end_but_not_for_runs_elsewhere_and_not_forever() {
        use crate::core::ConnectionId;
        use std::time::Duration;
        let remove = |n: u64, c: &str| Command::Core(RequestId(n), CoreCall::RemoveConnection(ConnectionId(c.into())));
        let removes = |fake: &FakeCore| fake.calls().into_iter().filter(|c| matches!(c, Call::RemoveConnection(_))).count();
        let fake = Arc::new(FakeCore::new());
        // A run on connection `c` that takes a second to wind down after its cancel.
        fake.script_run(Scripted::err(crate::core::CoreErrorKind::Cancelled, "c").after(Duration::from_secs(1)).deaf());
        for _ in 0..3 {
            fake.script_remove_connection(Scripted::ok(()));
        }
        let core: Arc<dyn Core> = fake.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        execute(vec![run_call(1)], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        execute(vec![Command::Cancel(RunId(1)), remove(2, "other")], &core, &tx, &tasks);
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(removes(&fake), 1, "another connection is removed at once");
        execute(vec![remove(3, "c")], &core, &tx, &tasks);
        tokio::time::advance(Duration::from_millis(900)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(removes(&fake), 1, "not while the run on it is still going");
        tokio::time::advance(Duration::from_millis(200)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(removes(&fake), 2, "once it has ended");

        // A run that never ends holds the removal for RUN_WAIT, no longer.
        fake.script_run(Scripted::err(crate::core::CoreErrorKind::Other, "never").after(Duration::from_secs(600)).deaf());
        execute(vec![run_call(4)], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        execute(vec![remove(5, "c")], &core, &tx, &tasks);
        tokio::task::yield_now().await;
        tokio::time::advance(RUN_WAIT - Duration::from_millis(10)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(removes(&fake), 2, "still held");
        tokio::time::advance(Duration::from_millis(10)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(removes(&fake), 3);
    }

    #[test]
    fn quitting_with_work_unfinished_names_where_a_statement_may_still_be_running() {
        assert_eq!(quit_note(true, &["local pg / pagila".into()]), None, "everything ended");
        assert_eq!(quit_note(false, &["local pg / pagila".into()]).as_deref(), Some("A statement may still be running on local pg / pagila."));
        assert_eq!(
            quit_note(false, &["a / x".into(), "b / y".into(), "a / x".into()]).as_deref(),
            Some("A statement may still be running on a / x and b / y.")
        );
        assert_eq!(quit_note(false, &[]).as_deref(), Some("A statement may still be running on the server."));
    }

    #[tokio::test]
    async fn quitting_mid_run_leaves_the_app_naming_where_the_run_was() {
        let mut app = App::new();
        app.model.tabs[0].target = Some(crate::core::DatabasePath { connection: crate::core::ConnectionId("c".into()), database: "edge".into() });
        let request = app.model.requests.begin(crate::app::requests::Target::Run(crate::core::TabId(1)));
        app.model.tabs[0].run = crate::app::query::RunState::Running { run: RunId(1), request, started: std::time::Instant::now() };
        app.quit_requested();
        assert_eq!(app.abandoned_on(), vec!["c / edge".to_string()]);
    }

    #[test]
    fn the_runtime_gives_worker_threads_a_16_mib_stack() {
        // dbx's metadata futures overflow tokio's default 2 MiB stack. A task
        // that needs 8 MiB of stack runs here; on a default runtime it would
        // crash this test binary.
        let rt = runtime().expect("runtime");
        let used = rt.block_on(async {
            tokio::spawn(async {
                let big = std::hint::black_box([7u8; 8 << 20]);
                big.iter().map(|&b| b as usize).sum::<usize>()
            })
            .await
            .expect("task")
        });
        assert_eq!(used, 7 * (8 << 20));
    }

    #[test]
    fn the_crypto_provider_is_installed_once_and_a_second_install_is_harmless() {
        install_crypto_provider();
        install_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    #[test]
    fn key_releases_never_reach_the_app() {
        let key = |kind| KeyEvent { code: KeyCode::Char('g'), modifiers: KeyModifiers::CONTROL, kind, state: KeyEventState::NONE };
        assert!(from_terminal(TermEvent::Key(key(KeyEventKind::Press))).is_some());
        assert!(from_terminal(TermEvent::Key(key(KeyEventKind::Repeat))).is_some());
        assert!(from_terminal(TermEvent::Key(key(KeyEventKind::Release))).is_none());
        assert!(matches!(from_terminal(TermEvent::Resize(80, 24)), Some(Event::Resize(80, 24))));
        assert!(from_terminal(TermEvent::FocusGained).is_none());
    }
}
