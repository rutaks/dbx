//! A way for the layer 1 ground-truth tests (`tests/ground_truth.rs`) to see a
//! `ResultSet` as the client draws it. Only with the `test-support` feature:
//! the client binary does not contain it.
//!
//! It is a thin wrapper: an `App` whose last run gave `set`, the results pane
//! zoomed to the whole screen, and the screen read back as text. Nothing here
//! is a second implementation of the grid.

use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use super::keys::KeyCombo;
use super::query::Outcome;
use super::{App, Event, PaneId};
use crate::core::ResultSet;

pub struct GridProbe {
    app: App,
}

impl GridProbe {
    /// An app showing `set` in the results pane, which has focus and the
    /// whole screen.
    pub fn new(set: ResultSet) -> Self {
        let mut app = App::new();
        app.model.tabs[0].outcome = Some(Outcome::Rows(set));
        app.ui.focus = PaneId::Results;
        app.ui.zoomed = true;
        GridProbe { app }
    }

    /// Sends space-separated keys such as `"j j enter"`.
    pub fn press(&mut self, keys: &str) {
        for key in keys.split_whitespace() {
            let combo = KeyCombo::parse(key).unwrap_or_else(|| panic!("bad key {key:?}"));
            self.app.update(Event::Key(KeyEvent::new(combo.code, combo.mods)), Instant::now());
        }
    }

    fn render(&mut self, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        term.draw(|f| self.app.view(f)).expect("draw");
        term.backend().buffer().clone()
    }

    /// One screen row as a terminal shows it: the cell a wide character covers
    /// is skipped, so the row reads as text.
    fn row_text(buf: &Buffer, y: u16, x0: u16, x1: u16) -> String {
        let mut row = String::new();
        let mut skip = 0;
        for x in x0..x1 {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buf[(x, y)].symbol();
            skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
            row.push_str(symbol);
        }
        row
    }

    /// The whole screen, one string per row.
    pub fn screen(&mut self, w: u16, h: u16) -> Vec<String> {
        let buf = self.render(w, h);
        (0..h).map(|y| Self::row_text(&buf, y, 0, w)).collect()
    }

    /// The results pane's rows below its title row: the header, then the data
    /// rows from the first one on screen. Each starts at the pane's left edge
    /// (the row-number gutter) and is as wide as the pane.
    pub fn pane(&mut self, w: u16, h: u16) -> Vec<String> {
        let buf = self.render(w, h);
        let r = self.app.ui.hits.results;
        (r.y + 1..r.bottom()).map(|y| Self::row_text(&buf, y, r.x, r.right())).collect()
    }

    /// Whether a cell viewer (or any overlay) is open.
    pub fn overlay_open(&self) -> bool {
        self.app.ui.overlay.is_some()
    }
}
