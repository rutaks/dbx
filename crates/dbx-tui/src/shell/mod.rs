//! The thin outer layer: owns the terminal and tokio, turns the app's
//! commands into Core calls, and feeds results back as events. One
//! `select!` over the terminal's events, the reply channel and the app's
//! next deadline; it draws only when an update asks for a repaint.

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
use crate::core::Core;

pub use terminal::TerminalGuard;

/// The Core calls in flight, by request, so the app can abandon one: aborting
/// the task drops the Core's future, which is what stops a slow listing.
#[derive(Clone, Default)]
pub struct Tasks(Arc<Mutex<HashMap<RequestId, AbortHandle>>>);

impl Tasks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RequestId, AbortHandle>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
pub async fn run(core: Arc<dyn Core>) -> io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<(RequestId, CoreReply)>();
    let mut events = EventStream::new();
    let mut app = App::new();
    let tasks = Tasks::default();
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut interrupt = signal(SignalKind::interrupt())?;

    let mut effects = app.start();
    loop {
        if execute(effects.commands, &core, &tx, &tasks) {
            return Ok(());
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
                None => return Ok(()),
            },
            // Being killed is a way to quit: return so the guard restores the terminal.
            _ = terminate.recv() => return Ok(()),
            _ = hangup.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
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
            Command::Core(id, call) => {
                let (core, tx, done) = (core.clone(), tx.clone(), tasks.clone());
                // Held while spawning, so a call that finishes at once cannot
                // forget itself before it has been remembered.
                let mut running = tasks.lock();
                let task = tokio::spawn(async move {
                    let reply = match call {
                        CoreCall::Connections => CoreReply::Connections(core.connections().await),
                        CoreCall::TestConnection(draft) => CoreReply::Tested(core.test_connection(&draft).await),
                        CoreCall::SaveConnection(draft) => CoreReply::Saved(core.save_connection(&draft).await),
                        CoreCall::RemoveConnection(id) => CoreReply::Removed(core.remove_connection(&id).await),
                        CoreCall::List(node) => CoreReply::Listed(list(core.as_ref(), node).await),
                    };
                    done.lock().remove(&id);
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
    use crate::core::{Call, FakeCore, RunId};

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
