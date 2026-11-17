//! The thin outer layer: owns the terminal and tokio, turns the app's
//! commands into Core calls, and feeds results back as events. One
//! `select!` over the terminal's events, the reply channel and the app's
//! next deadline; it draws only when an update asks for a repaint.

mod terminal;

use std::io;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

use crate::app::requests::RequestId;
use crate::app::{App, Command, CoreCall, CoreReply, Effects, Event};
use crate::core::Core;

pub use terminal::TerminalGuard;

/// Runs the client until the user quits. The terminal is restored on return.
pub async fn run(core: Arc<dyn Core>) -> io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<(RequestId, CoreReply)>();
    let mut events = EventStream::new();
    let mut app = App::new();
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut interrupt = signal(SignalKind::interrupt())?;

    let mut effects = app.start();
    loop {
        if execute(effects.commands, &core, &tx) {
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
fn execute(
    commands: Vec<Command>,
    core: &Arc<dyn Core>,
    tx: &mpsc::UnboundedSender<(RequestId, CoreReply)>,
) -> bool {
    let mut quit = false;
    for command in commands {
        match command {
            Command::Quit => quit = true,
            Command::Cancel(run) => core.cancel(run),
            Command::Core(id, call) => {
                let (core, tx) = (core.clone(), tx.clone());
                tokio::spawn(async move {
                    let reply = match call {
                        CoreCall::Connections => CoreReply::Connections(core.connections().await),
                    };
                    let _ = tx.send((id, reply));
                });
            }
        }
    }
    quit
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
        let quit = execute(vec![Command::Cancel(RunId(1)), Command::Quit, Command::Cancel(RunId(2))], &core, &tx);
        assert!(quit);
        assert_eq!(fake.calls(), vec![Call::Cancel(RunId(1)), Call::Cancel(RunId(2))]);
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
