//! Layer 2 with a Core behind it: an `App` wired to a `FakeCore` through the
//! shell's own command runner, so scripted replies (and their delays) reach the
//! app exactly as they do in the client. Time is tokio's, so tests that pause
//! it decide when a slow reply lands.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::app::requests::RequestId;
use crate::app::testing::base;
use crate::app::{App, CoreReply, Effects, Event};
use crate::core::{Core, FakeCore};
use crate::shell::{execute, Tasks};

pub struct Harness {
    pub app: App,
    pub fake: Arc<FakeCore>,
    core: Arc<dyn Core>,
    tx: mpsc::UnboundedSender<(RequestId, CoreReply)>,
    rx: mpsc::UnboundedReceiver<(RequestId, CoreReply)>,
    pub tasks: Tasks,
    pub quit: bool,
    /// How many of the replies `settle` fed the app asked for a repaint.
    pub repaints: usize,
    /// tokio's time when the harness started: the app's clock is `base()`
    /// plus tokio time since then, so a test that pauses and advances tokio
    /// time moves the app's clock with it.
    t0: tokio::time::Instant,
    /// Time the test has let pass without waiting for it (`advance`).
    skew: std::time::Duration,
}

impl Harness {
    /// Starts the app against `fake` and runs the first round of commands.
    pub fn start(fake: FakeCore) -> Self {
        let fake = Arc::new(fake);
        let core: Arc<dyn Core> = fake.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let syntax: Arc<dyn crate::core::Syntax> = fake.clone();
        let mut h = Harness { app: App::new(), fake, core, tx, rx, tasks: Tasks::default(), quit: false, repaints: 0, t0: tokio::time::Instant::now(), skew: std::time::Duration::ZERO };
        h.app.ui.syntax.set_handle(syntax);
        let fx = h.app.start();
        h.run(fx);
        h
    }

    /// The app's clock now.
    pub fn now(&self) -> std::time::Instant {
        base() + (tokio::time::Instant::now() - self.t0) + self.skew
    }

    /// Lets `by` pass on the app's clock without waiting for it, and wakes
    /// the app as the shell's timer would at its next deadline. Replies of
    /// Core calls are on tokio's time and are not moved by this.
    pub fn advance(&mut self, by: std::time::Duration) {
        self.skew += by;
        self.tick();
    }

    /// Lets the app's clock reach now, as the shell's timer would: an
    /// `Event::Timer` at the current time.
    pub fn tick(&mut self) -> bool {
        let now = self.now();
        let fx = self.app.update(Event::Timer, now);
        let repaint = fx.repaint;
        self.run(fx);
        repaint
    }

    /// Runs the commands in `fx`, as the shell does.
    pub fn run(&mut self, fx: Effects) {
        self.quit |= execute(fx.commands, &self.core, &self.tx, &self.tasks);
    }

    /// Lets spawned Core calls finish that are ready now, and feeds every reply
    /// that has arrived to the app. Does not advance time.
    pub async fn settle(&mut self) {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        while let Ok((id, reply)) = self.rx.try_recv() {
            let now = self.now();
            let fx = self.app.update(Event::Core(id, reply), now);
            self.repaints += usize::from(fx.repaint);
            self.run(fx);
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        }
    }

    /// Like `press`, and returns the commands the keys produced.
    pub fn press_fx(&mut self, keys: &str) -> Vec<crate::app::Command> {
        let now = self.now();
        let fx = crate::app::testing::press_at(&mut self.app, keys, now);
        let commands = fx.commands.clone();
        self.run(fx);
        commands
    }

    /// Types `text` one character at a time, as a person would.
    pub fn type_text(&mut self, text: &str) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for c in text.chars() {
            let now = self.now();
            let fx = self.app.update(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)), now);
            self.run(fx);
        }
    }

    /// Sends keys (see `testing::press`) and runs what they ask for.
    pub fn press(&mut self, keys: &str) -> bool {
        let now = self.now();
        let fx = crate::app::testing::press_at(&mut self.app, keys, now);
        let repaint = fx.repaint;
        self.run(fx);
        repaint
    }
}
