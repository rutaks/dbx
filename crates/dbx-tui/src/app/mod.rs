//! The pure part of the client: state, `update(event) -> effects`, and
//! `view(frame)`. No tokio, no terminal I/O, no dbx types.

mod connection_ui;
pub mod editor;
pub mod form;
pub mod keys;
pub mod layout;
pub mod overlay;
pub mod query;
pub mod requests;
pub mod sidebar;
pub mod text_field;
pub mod theme;
pub mod tree;
pub(crate) mod view;

#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod connections_tests;
#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tree_tests;

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;

use crate::core::{
    Connection, ConnectionDraft, ConnectionId, CoreError, DatabasePath, ResultSet, RunId, RunRequest, SessionId, TabId, DEFAULT_ROW_CAP,
};
use keys::{Action, KeyCombo, Keybinds};
use layout::{HintHit, HitMap};
use overlay::{ErrorOverlay, FieldHit, Overlay, OverlayAction, OverlayButton};
use requests::{RequestId, Requests, Settled, Target};
use query::{Notice, QueryTab};
use sidebar::SidebarUi;
use theme::Palette;
use tree::{Listing, NodeId, RowKey, SchemaCache, Slot};

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
    /// What the Core told us about each connection's databases, schemas,
    /// tables and columns.
    pub schema: SchemaCache,
    /// The query tabs. One until the tab bar arrives (slice 15).
    pub tabs: Vec<QueryTab>,
    pub requests: Requests,
    /// The id the next run gets.
    pub next_run: u64,
    /// The most rows a run fetches.
    pub row_cap: usize,
}

/// `ctrl+c`: never a key for the focused pane.
const CTRL_C: KeyCombo = KeyCombo { code: KeyCode::Char('c'), mods: crossterm::event::KeyModifiers::CONTROL };

/// Rows one notch of the mouse wheel scrolls.
const WHEEL_ROWS: isize = 3;

/// A short message at the foot of the sidebar, for what does not deserve an
/// overlay or must not disturb one that is open. Cleared by the next key the
/// sidebar takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarNote {
    pub text: String,
    /// Something the user asked for failed (red), rather than a hint (yellow).
    pub failure: bool,
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
    /// The mode bar's hints as last drawn, each with what a click on it does.
    /// Filled by `view`.
    pub mode_hints: Vec<HintHit>,
    /// The open overlay's buttons as last drawn. Filled by `view`.
    pub overlay_buttons: Vec<OverlayButton>,
    /// The open form's rows as last drawn, for clicks. Filled by `view`.
    pub overlay_fields: Vec<FieldHit>,
    /// The serial the next form gets.
    pub next_form_serial: u64,
    /// The connection the cursor was on when a reload began, so it can be put
    /// back on the same one (by id) when the new list arrives.
    pub reload_anchor: Option<ConnectionId>,
    pub note: Option<SidebarNote>,
    /// The SQL editor of the query tab.
    pub editor: editor::Editor,
    pub notice: Option<Notice>,
    /// The `now` of the last update: what the view measures elapsed time by.
    pub now: Option<Instant>,
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
    /// Try a connection without saving it. Carries the password the form holds.
    TestConnection(ConnectionDraft),
    SaveConnection(ConnectionDraft),
    RemoveConnection(ConnectionId),
    /// List one tree node's children: databases of a connection, schemas of a
    /// database, tables of a schema, columns of a table.
    List(NodeId),
    /// Run a statement in a query tab.
    Run(RunId, RunRequest),
}

/// The reply to a `CoreCall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreReply {
    Connections(Result<Vec<Connection>, CoreError>),
    Tested(Result<(), CoreError>),
    Saved(Result<Connection, CoreError>),
    Removed(Result<(), CoreError>),
    Listed(Result<Listing, CoreError>),
    Ran(Result<ResultSet, CoreError>),
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
    /// Stop waiting for a request: its reply will be dropped as stale, so the
    /// shell may abort the call (a slow listing nobody wants any more).
    Abandon(RequestId),
    /// Let the Core drop the server connections it holds for one database
    /// (its node collapsed). Fire and forget: nothing comes back.
    Release(DatabasePath),
    /// End one session of a query tab, on one database, once a run still in
    /// flight in it has ended (or a moment has passed). Fire and forget.
    CloseSession(SessionId, DatabasePath),
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
            model: Model {
                connections: Loadable::Loading,
                schema: SchemaCache::default(),
                tabs: vec![QueryTab::new(TabId(1))],
                requests: Requests::default(),
                next_run: 1,
                row_cap: DEFAULT_ROW_CAP,
            },
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
                mode_hints: Vec::new(),
                overlay_buttons: Vec::new(),
                overlay_fields: Vec::new(),
                next_form_serial: 1,
                reload_anchor: None,
                note: None,
                editor: editor::Editor::new(),
                notice: None,
                now: None,
            },
        }
    }

    /// Asks for what the first screen needs. Calling it again reloads, and
    /// the earlier request's reply becomes stale.
    pub fn start(&mut self) -> Effects {
        let id = self.model.requests.begin(Target::Connections);
        if let Some(row) = self.cursor_row() {
            self.ui.reload_anchor = Some(row.node.connection().clone());
        }
        self.model.connections = Loadable::Loading;
        Effects { repaint: true, commands: vec![Command::Core(id, CoreCall::Connections)] }
    }

    pub fn update(&mut self, event: Event, now: Instant) -> Effects {
        let mut fx = Effects::default();
        self.ensure_rows();
        self.expire_prefix(now, &mut fx);
        self.on_time(now, &mut fx);
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
            Event::Paste(text) if self.ui.overlay.is_some() => self.on_overlay_paste(&text, &mut fx),
            Event::Key(key) => self.on_key(key, now, &mut fx),
            Event::Mouse(mouse) => self.on_mouse(mouse, now, &mut fx),
            Event::Resize(..) => fx.repaint = true,
            Event::Core(id, reply) => self.on_reply(id, reply, &mut fx),
            Event::Paste(text) => self.on_paste(&text, &mut fx),
            Event::Timer => {}
        }
        self.ensure_rows();
        fx
    }

    /// Quitting from outside the keys (a signal).
    pub fn quit_requested(&mut self) -> Effects {
        let mut fx = Effects::default();
        self.quit(&mut fx);
        fx
    }

    /// When the shell should next wake with `Event::Timer`.
    pub fn next_deadline(&self) -> Option<Instant> {
        let prefix = match self.ui.mode {
            Mode::Prefix { expires } => Some(expires),
            Mode::Normal => None,
        };
        [prefix, self.query_deadline()].into_iter().flatten().min()
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
                } else if let Some(action) = self.ui.keys.lookup_direct(&combo) {
                    self.perform(action, now, fx);
                } else if combo == CTRL_C {
                    // Bound to cancel by default (a direct key, above). Even
                    // if rebound, it never reaches the editor, whose own
                    // ctrl+c is a copy into its private buffer.
                } else {
                    match self.ui.focus {
                        PaneId::Sidebar => self.on_sidebar_key(combo, fx),
                        // The editor takes text: every other key is its own.
                        PaneId::Editor => fx.repaint |= self.ui.editor.key(key),
                        PaneId::Results => {}
                    }
                }
            }
            Mode::Prefix { .. } => {
                self.ui.mode = Mode::Normal;
                fx.repaint = true;
                if let Some(action) = self.ui.keys.lookup(&combo) {
                    self.perform(action, now, fx);
                }
            }
        }
    }

    fn on_paste(&mut self, text: &str, fx: &mut Effects) {
        if self.ui.focus == PaneId::Editor {
            match self.ui.editor.paste(text) {
                Ok(changed) => fx.repaint |= changed,
                Err(why) => self.notify(format!("The text was not pasted: {why}."), true, fx),
            }
        }
    }

    fn perform(&mut self, action: Action, now: Instant, fx: &mut Effects) {
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
            (Action::NewConnection, _) => {
                self.open_new_form();
                None
            }
            (Action::Quit, _) => {
                self.quit(fx);
                None
            }
            (Action::Run | Action::RunDirect, _) => {
                self.run_statement(now, fx);
                None
            }
            (Action::Cancel, _) => {
                self.cancel_run(now, fx);
                None
            }
            // ctrl+c cancels only where the run is: the editor and the results.
            (Action::CancelDirect, Editor | Results) => {
                self.cancel_run(now, fx);
                None
            }
            _ => None,
        };
        if let Some(pane) = target {
            self.ui.focus = pane;
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, now: Instant, fx: &mut Effects) {
        let at = (mouse.column, mouse.row).into();
        match mouse.kind {
            // The wheel scrolls what is under the pointer, focused or not.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if self.ui.hits.sidebar.contains(at) => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                self.scroll_sidebar(if down { WHEEL_ROWS } else { -WHEEL_ROWS }, fx);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // A hint in the mode bar does what its key does.
                if let Some(hit) = self.ui.mode_hints.iter().find(|h| h.rect.contains(at)).copied() {
                    return self.click_hint(hit.action, now, fx);
                }
                if let Some(pane) = self.ui.hits.pane_at(mouse.column, mouse.row) {
                    if pane != self.ui.focus {
                        self.ui.focus = pane;
                        fx.repaint = true;
                    }
                }
                if self.ui.hits.sidebar_list.contains(at) {
                    self.click_sidebar_row(mouse.column, mouse.row, now, fx);
                }
            }
            _ => {}
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
            return self.overlay_action(action, fx);
        }
        match overlay {
            Overlay::Form(form) if !form.confirm_discard => self.on_form_key(&key, fx),
            // Backing out one level always works, button or not.
            Overlay::Failure(_) if combo.code == KeyCode::Esc => self.overlay_action(OverlayAction::Dismiss, fx),
            _ => {}
        }
    }

    fn on_overlay_mouse(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let at = (mouse.column, mouse.row).into();
        let hit = self.ui.overlay_buttons.iter().find(|b| b.rect.contains(at));
        if let Some(action) = hit.map(|b| b.action) {
            return self.overlay_action(action, fx);
        }
        // Anything else outside the buttons and the form's rows does nothing:
        // not even to the panes behind the overlay.
        if let Some(hit) = self.ui.overlay_fields.iter().find(|f| f.rect.contains(at)).copied() {
            self.on_form_click(hit, mouse.column - hit.rect.x, fx);
        }
    }

    fn on_overlay_paste(&mut self, text: &str, fx: &mut Effects) {
        if let Some(Overlay::Form(form)) = &mut self.ui.overlay {
            if !form.confirm_discard && form.paste(text) {
                self.form_edited(fx);
            }
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
            OverlayAction::Quit => self.quit(fx),
            OverlayAction::FormSave => self.begin_save(fx),
            OverlayAction::FormTest => self.begin_test(fx),
            OverlayAction::FormClose => self.request_close_form(fx),
            OverlayAction::DiscardYes => self.close_form(fx),
            OverlayAction::DiscardNo => {
                if let Some(Overlay::Form(form)) = &mut self.ui.overlay {
                    form.confirm_discard = false;
                }
                fx.repaint = true;
            }
            OverlayAction::RemoveYes => self.confirm_remove(fx),
            OverlayAction::DropSessionYes => self.drop_session(fx),
            OverlayAction::Dismiss => {
                self.ui.overlay = None;
                fx.repaint = true;
            }
        }
    }

    fn on_reply(&mut self, id: RequestId, reply: CoreReply, fx: &mut Effects) {
        // A stale reply is dropped without touching state or repainting,
        // except that a cancelled run's last word says it has ended.
        let target = match self.model.requests.settle(id) {
            Settled::Current(target) => target,
            Settled::Stale(Target::Run(_)) => {
                if let CoreReply::Ran(result) = reply {
                    self.on_ran_after_cancel(id, result, fx);
                }
                return;
            }
            Settled::Stale(_) | Settled::Unknown => return,
        };
        match (target, reply) {
            (Target::Connections, CoreReply::Connections(result)) => self.on_connections(result, fx),
            (Target::TestConnection(serial), CoreReply::Tested(result)) => self.on_tested(serial, result),
            (Target::SaveConnection(serial), CoreReply::Saved(result)) => self.on_saved(serial, result, fx),
            (Target::RemoveConnection(id), CoreReply::Removed(result)) => self.on_removed(id, result, fx),
            (Target::Run(_), CoreReply::Ran(result)) => self.on_ran(result),
            (Target::Children(node), CoreReply::Listed(result)) => {
                if !self.on_listed(node, result, fx) {
                    return;
                }
            }
            _ => return,
        }
        fx.repaint = true;
    }

    fn on_connections(&mut self, result: Result<Vec<Connection>, CoreError>, fx: &mut Effects) {
        self.model.connections = match result {
            Ok(list) => {
                // The cursor stays on the same connection, wherever it moved to.
                let keep = self.ui.reload_anchor.take().or_else(|| self.cursor_row().map(|r| r.node.connection().clone()));
                if let Some(id) = keep {
                    self.ui.sidebar.cursor_key = Some(RowKey { node: NodeId::Connection(id), slot: Slot::Node });
                }
                // A connection that is no longer saved takes its tree with it.
                for gone in self.model.schema.connections().into_iter().filter(|c| !list.iter().any(|k| &k.id == c)) {
                    self.forget_connection(&gone, fx);
                }
                if let Some(t) = self.tab().target.clone().filter(|t| !list.iter().any(|k| k.id == t.connection)) {
                    self.connection_removed(&t.connection, fx);
                }
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
        self.touch_rows();
    }
}
