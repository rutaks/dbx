//! Test helpers for layer 2: render an `App`, read the screen as text, ask
//! which palette role drew a cell, send keys and clicks.

use std::sync::OnceLock;
use std::time::Instant;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

use super::keys::KeyCombo;
use super::theme::{Palette, Role};
use super::{App, Command, CoreReply, Effects, Event};

/// A fixed instant all helpers share, so tests can say `base() + 4s`.
pub fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

/// An app that has started and been told the Core has no connections.
pub fn ready_app() -> App {
    let mut app = App::new();
    let fx = app.start();
    let Some(Command::Core(id, _)) = fx.commands.into_iter().next() else {
        panic!("start must request the connection list");
    };
    app.update(Event::Core(id, CoreReply::Connections(Ok(Vec::new()))), base());
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    term.draw(|f| app.view(f)).expect("draw");
    term.backend().buffer().clone()
}

/// The screen as one string per row.
pub fn frame_rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let buf = render(app, w, h);
    (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

/// The screen as one string per row, as a terminal shows it: the cell a wide
/// character covers is skipped, so a row reads as text (`日本語`, not
/// `日 本 語`), and the width of a prefix is its column.
pub fn frame_text(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let buf = render(app, w, h);
    (0..h)
        .map(|y| {
            let mut row = String::new();
            let mut skip = 0;
            for x in 0..w {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buf[(x, y)].symbol();
                skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
                row.push_str(symbol);
            }
            row
        })
        .collect()
}

/// Where `text` first appears on screen.
pub fn find_text(app: &mut App, w: u16, h: u16, text: &str) -> Option<(u16, u16)> {
    frame_rows(app, w, h).iter().enumerate().find_map(|(y, row)| {
        let byte = row.find(text)?;
        Some((row[..byte].chars().count() as u16, y as u16))
    })
}

/// The palette roles behind one cell's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRoles {
    pub fg: Option<Role>,
    pub bg: Option<Role>,
}

/// Renders at the size of the last frame and reports which roles drew the
/// cell at (x, y). Uses the probe palette, in which every role has its own
/// colour, and puts the app's own theme back afterwards.
pub fn role_at(app: &mut App, x: u16, y: u16) -> CellRoles {
    let area = app.ui.hits.area;
    let (w, h) = if area.width == 0 { (100, 30) } else { (area.width, area.height) };
    let probe = Palette::probe();
    let saved = std::mem::replace(&mut app.ui.theme, probe.clone());
    let buf = render(app, w, h);
    app.ui.theme = saved;
    let cell = &buf[(x, y)];
    CellRoles { fg: probe.role_of(cell.fg), bg: probe.role_of(cell.bg) }
}

/// The real colours (not roles) of the cell at (x, y) under the app's own
/// theme, and its modifiers.
pub fn style_at(app: &mut App, x: u16, y: u16) -> (ratatui::style::Color, ratatui::style::Color, ratatui::style::Modifier) {
    let area = app.ui.hits.area;
    let (w, h) = if area.width == 0 { (100, 30) } else { (area.width, area.height) };
    let buf = render(app, w, h);
    let cell = &buf[(x, y)];
    (cell.fg, cell.bg, cell.modifier)
}

/// Sends space-separated keys such as `"ctrl+g q"` at `now`, merging effects.
pub fn press_at(app: &mut App, keys: &str, now: Instant) -> Effects {
    let mut all = Effects::default();
    for key in keys.split_whitespace() {
        let combo = KeyCombo::parse(key).unwrap_or_else(|| panic!("bad key {key:?}"));
        let fx = app.update(Event::Key(KeyEvent::new(combo.code, combo.mods)), now);
        all.repaint |= fx.repaint;
        all.commands.extend(fx.commands);
    }
    all
}

pub fn press(app: &mut App, keys: &str) -> Effects {
    press_at(app, keys, base())
}

/// Left-clicks the middle of `rect`.
pub fn click(app: &mut App, rect: Rect) -> Effects {
    let (column, row) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    let ev = |kind| MouseEvent { kind, column, row, modifiers: crossterm::event::KeyModifiers::NONE };
    let down = app.update(Event::Mouse(ev(MouseEventKind::Down(MouseButton::Left))), base());
    let up = app.update(Event::Mouse(ev(MouseEventKind::Up(MouseButton::Left))), base());
    Effects { repaint: down.repaint || up.repaint, commands: [down.commands, up.commands].concat() }
}

/// Whether the cell at (x, y) is dimmed, as the screen behind an overlay is.
pub fn dimmed_at(app: &mut App, x: u16, y: u16) -> bool {
    let area = app.ui.hits.area;
    let (w, h) = if area.width == 0 { (100, 30) } else { (area.width, area.height) };
    render(app, w, h)[(x, y)].modifier.contains(ratatui::style::Modifier::DIM)
}
