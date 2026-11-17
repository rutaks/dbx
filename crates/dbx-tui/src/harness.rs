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
}

impl Harness {
    /// Starts the app against `fake` and runs the first round of commands.
    pub fn start(fake: FakeCore) -> Self {
        let fake = Arc::new(fake);
        let core: Arc<dyn Core> = fake.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let mut h = Harness { app: App::new(), fake, core, tx, rx, tasks: Tasks::default(), quit: false };
        let fx = h.app.start();
        h.run(fx);
        h
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
            let fx = self.app.update(Event::Core(id, reply), base());
            self.run(fx);
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        }
    }

    /// Like `press`, and returns the commands the keys produced.
    pub fn press_fx(&mut self, keys: &str) -> Vec<crate::app::Command> {
        let fx = crate::app::testing::press(&mut self.app, keys);
        let commands = fx.commands.clone();
        self.run(fx);
        commands
    }

    /// Types `text` one character at a time, as a person would.
    pub fn type_text(&mut self, text: &str) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for c in text.chars() {
            let fx = self.app.update(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)), base());
            self.run(fx);
        }
    }

    /// Sends keys (see `testing::press`) and runs what they ask for.
    pub fn press(&mut self, keys: &str) -> bool {
        let fx = crate::app::testing::press(&mut self.app, keys);
        let repaint = fx.repaint;
        self.run(fx);
        repaint
    }
}
