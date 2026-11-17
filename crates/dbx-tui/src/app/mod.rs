//! The pure part of the client: state, `update(event) -> effects`, and
//! `view(frame)`. No tokio, no terminal I/O, no dbx types.

pub mod keys;
pub mod layout;
pub mod overlay;
pub mod requests;
pub mod theme;
mod view;

#[cfg(test)]
mod connections_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;

use crate::core::{Connection, CoreError, RunId};
use keys::{Action, KeyCombo, Keybinds};
use layout::HitMap;
use overlay::{ErrorOverlay, Overlay, OverlayAction, OverlayButton};
use requests::{RequestId, Requests, Target};
use theme::Palette;

/// One of the fixed regions of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Sidebar,
    Editor,
    Results,
}

impl PaneId {
    pub fn name(self) -> &'static str {
        match self {
            PaneId::Sidebar => "SIDEBAR",
            PaneId::Editor => "EDITOR",
            PaneId::Results => "RESULTS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// The prefix was pressed; the next key is a command. Ends at `expires`.
    Prefix { expires: Instant },
}

/// Something that is loading, loaded or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(CoreError),
}

/// What the Core told us.
#[derive(Debug)]
pub struct Model {
    pub connections: Loadable<Vec<Connection>>,
    pub requests: Requests,
}

/// Rows one notch of the mouse wheel scrolls.
const WHEEL_ROWS: isize = 3;

/// The sidebar's cursor and how far its list is scrolled, in rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SidebarUi {
    pub cursor: usize,
    /// The first row on screen. `view` keeps the cursor inside the window.
    pub scroll: usize,
}

/// Presentation only.
#[derive(Debug)]
pub struct Ui {
    pub focus: PaneId,
    pub zoomed: bool,
    pub mode: Mode,
    pub sidebar: SidebarUi,
    pub prefix_timeout: Duration,
    pub theme: Palette,
    pub keys: Keybinds,
    /// At most one overlay; while it is open it takes all input.
    pub overlay: Option<Overlay>,
    /// Filled by `view`.
    pub hits: HitMap,
    /// The open overlay's buttons as last drawn. Filled by `view`.
    pub overlay_buttons: Vec<OverlayButton>,
}

#[derive(Debug)]
pub struct App {
    pub model: Model,
    pub ui: Ui,
}

/// What the app asks of the Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreCall {
    Connections,
}

/// The reply to a `CoreCall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreReply {
    Connections(Result<Vec<Connection>, CoreError>),
}

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(u16, u16),
    Core(RequestId, CoreReply),
    Timer,
}

/// What the app asks the shell to do (glossary: not an Action).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Core(RequestId, CoreCall),
    Cancel(RunId),
    Quit,
}

#[derive(Debug, Default)]
pub struct Effects {
    pub repaint: bool,
    pub commands: Vec<Command>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            model: Model { connections: Loadable::Loading, requests: Requests::default() },
            ui: Ui {
                focus: PaneId::Sidebar,
                zoomed: false,
                mode: Mode::Normal,
                sidebar: SidebarUi::default(),
                prefix_timeout: Duration::from_secs(3),
                theme: Palette::default_theme(),
                keys: Keybinds::default(),
                overlay: None,
                hits: HitMap::default(),
                overlay_buttons: Vec::new(),
            },
        }
    }

    /// Asks for what the first screen needs. Calling it again reloads, and
    /// the earlier request's reply becomes stale.
    pub fn start(&mut self) -> Effects {
        let id = self.model.requests.begin(Target::Connections);
        self.model.connections = Loadable::Loading;
        Effects { repaint: true, commands: vec![Command::Core(id, CoreCall::Connections)] }
    }

    pub fn update(&mut self, event: Event, now: Instant) -> Effects {
        let mut fx = Effects::default();
        self.expire_prefix(now, &mut fx);
        // Anything but a key press ends prefix mode, so a click or a paste
        // cannot be followed by a stray command key. Plain pointer movement
        // does not count: terminals report it constantly.
        let abandons_prefix = match &event {
            Event::Mouse(m) => m.kind != MouseEventKind::Moved,
            Event::Paste(_) => true,
            _ => false,
        };
        if abandons_prefix && matches!(self.ui.mode, Mode::Prefix { .. }) {
            self.ui.mode = Mode::Normal;
            fx.repaint = true;
        }
        match event {
            // Routing order: overlay, then prefix mode, then the focused pane.
            Event::Key(key) if self.ui.overlay.is_some() => self.on_overlay_key(key, &mut fx),
            Event::Mouse(mouse) if self.ui.overlay.is_some() => self.on_overlay_mouse(mouse, &mut fx),
            Event::Paste(_) if self.ui.overlay.is_some() => {}
            Event::Key(key) => self.on_key(key, now, &mut fx),
            Event::Mouse(mouse) => self.on_mouse(mouse, &mut fx),
            Event::Resize(..) => fx.repaint = true,
            Event::Core(id, reply) => self.on_reply(id, reply, &mut fx),
            Event::Paste(_) | Event::Timer => {}
        }
        fx
    }

    /// When the shell should next wake with `Event::Timer`.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.ui.mode {
            Mode::Prefix { expires } => Some(expires),
            Mode::Normal => None,
        }
    }

    pub fn view(&mut self, frame: &mut Frame) {
        view::draw(self, frame);
    }

    fn expire_prefix(&mut self, now: Instant, fx: &mut Effects) {
        if let Mode::Prefix { expires } = self.ui.mode {
            if now >= expires {
                self.ui.mode = Mode::Normal;
                fx.repaint = true;
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant, fx: &mut Effects) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let combo = KeyCombo::from_event(&key);
        match self.ui.mode {
            Mode::Normal => {
                if combo == self.ui.keys.prefix {
                    self.ui.mode = Mode::Prefix { expires: now + self.ui.prefix_timeout };
                    fx.repaint = true;
                } else if self.ui.focus == PaneId::Sidebar {
                    self.on_sidebar_key(combo, fx);
                }
            }
            Mode::Prefix { .. } => {
                self.ui.mode = Mode::Normal;
                fx.repaint = true;
                if let Some(action) = self.ui.keys.lookup(&combo) {
                    self.perform(action, fx);
                }
            }
        }
    }

    /// The sidebar takes navigation keys directly: it accepts no text.
    fn on_sidebar_key(&mut self, combo: KeyCombo, fx: &mut Effects) {
        let Loadable::Ready(list) = &self.model.connections else { return };
        let last = list.len().saturating_sub(1);
        let cursor = &mut self.ui.sidebar.cursor;
        let moved = match combo.code {
            KeyCode::Char('j') | KeyCode::Down if combo.mods.is_empty() => (*cursor + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up if combo.mods.is_empty() => cursor.saturating_sub(1),
            _ => return,
        };
        if moved != *cursor {
            *cursor = moved;
            fx.repaint = true;
        }
    }

    fn perform(&mut self, action: Action, fx: &mut Effects) {
        use PaneId::*;
        let target = match (action, self.ui.focus) {
            (Action::FocusRight, Sidebar) => Some(Editor),
            (Action::FocusLeft, Editor | Results) => Some(Sidebar),
            (Action::FocusDown, Editor) => Some(Results),
            (Action::FocusUp, Results) => Some(Editor),
            (Action::Zoom, _) => {
                self.ui.zoomed = !self.ui.zoomed;
                None
            }
            (Action::Quit, _) => {
                fx.commands.push(Command::Quit);
                None
            }
            _ => None,
        };
        if let Some(pane) = target {
            self.ui.focus = pane;
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        let at = (mouse.column, mouse.row).into();
        match mouse.kind {
            // The wheel scrolls what is under the pointer, focused or not.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if self.ui.hits.sidebar.contains(at) => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                self.scroll_sidebar(if down { WHEEL_ROWS } else { -WHEEL_ROWS }, fx);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pane) = self.ui.hits.pane_at(mouse.column, mouse.row) {
                    if pane != self.ui.focus {
                        self.ui.focus = pane;
                        fx.repaint = true;
                    }
                }
                let list = self.ui.hits.sidebar_list;
                if list.contains(at) {
                    let row = self.ui.sidebar.scroll + (mouse.row - list.y) as usize;
                    if row != self.ui.sidebar.cursor {
                        self.ui.sidebar.cursor = row;
                        fx.repaint = true;
                    }
                }
            }
            _ => {}
        }
    }

    /// Moves the sidebar's window by `rows`, and the cursor with it when it
    /// would leave the window, so the cursor row always stays on screen.
    fn scroll_sidebar(&mut self, rows: isize, fx: &mut Effects) {
        let Loadable::Ready(list) = &self.model.connections else { return };
        let room = self.ui.hits.sidebar_list.height.max(1) as usize;
        let sb = &mut self.ui.sidebar;
        let max = list.len().saturating_sub(room);
        let scroll = sb.scroll.saturating_add_signed(rows).min(max);
        let cursor = sb.cursor.clamp(scroll, scroll + room - 1).min(list.len().saturating_sub(1));
        if (scroll, cursor) != (sb.scroll, sb.cursor) {
            (sb.scroll, sb.cursor) = (scroll, cursor);
            fx.repaint = true;
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent, fx: &mut Effects) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let combo = KeyCombo::from_event(&key);
        let Some(overlay) = &self.ui.overlay else { return };
        let action = overlay.buttons().iter().find(|b| KeyCombo::parse(b.key) == Some(combo)).map(|b| b.action);
        if let Some(action) = action {
            self.overlay_action(action, fx);
        }
    }

    fn on_overlay_mouse(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let hit = self.ui.overlay_buttons.iter().find(|b| b.rect.contains((mouse.column, mouse.row).into()));
        if let Some(action) = hit.map(|b| b.action) {
            self.overlay_action(action, fx);
        }
    }

    fn overlay_action(&mut self, action: OverlayAction, fx: &mut Effects) {
        match action {
            OverlayAction::Retry => {
                self.ui.overlay = None;
                let again = self.start();
                fx.commands.extend(again.commands);
                fx.repaint = true;
            }
            OverlayAction::Quit => fx.commands.push(Command::Quit),
        }
    }

    fn on_reply(&mut self, id: RequestId, reply: CoreReply, fx: &mut Effects) {
        // A stale reply is dropped without touching state or repainting.
        let Some(target) = self.model.requests.finish(id) else { return };
        let (Target::Connections, CoreReply::Connections(result)) = (target, reply) else { return };
        self.model.connections = match result {
            Ok(list) => {
                let sidebar = &mut self.ui.sidebar;
                sidebar.cursor = sidebar.cursor.min(list.len().saturating_sub(1));
                Loadable::Ready(list)
            }
            Err(e) => {
                // A store or key failure blocks everything: it gets the overlay.
                if let Some(overlay) = ErrorOverlay::for_startup(&e) {
                    self.ui.overlay = Some(Overlay::Error(overlay));
                    self.ui.mode = Mode::Normal;
                }
                Loadable::Failed(e)
            }
        };
        fx.repaint = true;
    }
}
