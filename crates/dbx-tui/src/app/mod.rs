//! The pure part of the client: state, `update(event) -> effects`, and
//! `view(frame)`. No tokio, no terminal I/O, no dbx types.

mod bar;
mod browse;
mod confirm;
mod connection_ui;
mod edit_keys;
pub mod finish;
mod resize;
pub mod complete;
pub mod dump;
pub mod complete_context;
pub mod complete_insert;
pub mod complete_match;
pub mod complete_popup;
pub mod complete_scope;
pub mod config;
pub mod edit_ops;
pub mod editor;
pub mod export;
pub mod form;
pub mod format;
pub mod grid;
pub mod highlight;
pub mod help;
pub mod history;
pub mod keys;
pub mod layout;
pub mod marks;
pub mod navigator;
pub mod overlay;
pub mod pointer;
#[cfg(feature = "test-support")]
pub mod probe;
pub mod query;
pub mod requests;
pub mod settings;
pub mod sidebar;
pub mod syntax;
pub mod tab;
pub mod template;
pub mod text_field;
pub mod theme;
pub mod tree;
pub mod viewer;
pub(crate) mod view;

#[cfg(test)]
mod welcome_tests;
#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod dump_tests;
#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod finish_tests;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod help_tests;
#[cfg(test)]
mod navigator_tests;
#[cfg(test)]
mod editor_keys_tests;
#[cfg(test)]
mod format_tests;
#[cfg(test)]
mod marks_tests;
#[cfg(test)]
mod template_tests;
#[cfg(test)]
mod complete_tests;
#[cfg(test)]
mod complete_scope_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod confirm_tests;
#[cfg(test)]
mod connections_tests;
#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod mouse_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod syntax_tests;
#[cfg(test)]
mod tabs_tests;
#[cfg(test)]
mod tab_bar_tests;
#[cfg(test)]
mod browse_tests;
#[cfg(test)]
mod capabilities_tests;
#[cfg(test)]
mod sqlite_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tree_tests;

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;

use crate::core::{
    Connection, ConnectionDraft, ConnectionId, CoreError, DatabasePath, ResultSet, RunId, RunRequest, SessionId, TabId, DEFAULT_ROW_CAP,
};
use keys::{Action, KeyCombo, Keybinds};
use layout::{HintHit, HitMap, LayoutPrefs};
use pointer::{ClickKey, Clicks, Drag};
use overlay::{ErrorOverlay, FieldHit, Overlay, OverlayAction, OverlayButton};
use requests::{RequestId, Requests, Settled, Target};
use query::{Notice, QueryTab};
use viewer::Scroll;
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
    /// The resize mode (prefix, then `R`): the keys move the dividers until
    /// `esc` or `enter`.
    Resize,
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
    /// The query tabs, in the order they were opened.
    pub tabs: Vec<QueryTab>,
    /// Runs of tabs since removed that were let go of before they ended: each
    /// may still be running until its stale reply says it ended.
    pub orphaned: Vec<(TabId, query::Abandoned)>,
    pub requests: Requests,
    /// What the completion popup has asked the Core for and kept.
    pub completions: complete::CompletionCache,
    pub settings: settings::Settings,
    /// The id the next run gets.
    pub next_run: u64,
    /// The id the next query tab gets. Never reused: a session is named by
    /// its tab, and a late close of a removed tab's session must not reach a
    /// new one.
    pub next_tab: u32,
    /// The most rows a run fetches.
    pub row_cap: usize,
    /// The serial of the next save to the history.
    pub next_save: u64,
    /// The dump in flight (slice 20): one at a time.
    pub dump: Option<dump::Dumping>,
    /// The id the next dump gets.
    pub next_dump: u64,
    /// Quitting waits for the cancelled dump's final progress until then.
    pub quit_after_dump: Option<Instant>,
}

impl Model {
    /// Tab `id`, while it exists.
    pub fn tab(&self, id: TabId) -> Option<&QueryTab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut QueryTab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// The active tab, given its id (`Ui.tab.id`), which is always in the
    /// model. Taking the id keeps the borrow to `model`, apart from `ui`.
    pub fn active(&self, id: TabId) -> &QueryTab {
        self.tab(id).expect("the active tab is in the model")
    }

    pub fn active_mut(&mut self, id: TabId) -> &mut QueryTab {
        self.tab_mut(id).expect("the active tab is in the model")
    }
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

/// What the terminal can do, asked once at startup by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// The terminal speaks the kitty keyboard protocol, so Cmd, Ctrl+Enter and
    /// the other keys a legacy terminal cannot tell apart arrive as they are.
    /// Where it is false the help overlay marks those keys as unavailable.
    pub keyboard_protocol: bool,
}

/// Presentation only.
#[derive(Debug)]
pub struct Ui {
    pub caps: Capabilities,
    /// What the active query tab shows: its editor, popup, template, marks,
    /// find bar and grid (`tab::TabUi`).
    pub tab: tab::TabUi,
    /// The other tabs' `TabUi`, as they were left.
    pub parked: Vec<tab::TabUi>,
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
    /// The tab bar's tabs as last drawn. Filled by `view`.
    pub tab_hits: Vec<layout::TabHit>,
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
    /// What every tab's editor shares of the Core's syntax answers: the
    /// statement finder and the type names of each kind of database.
    pub syntax: syntax::SyntaxState,
    pub notice: Option<Notice>,
    /// The config file read at startup: where it is and what was wrong in it.
    pub config: config::Report,
    /// The `now` of the last update: what the view measures elapsed time by.
    pub now: Option<Instant>,
    /// Where the user put the dividers; `None` is the automatic size. The
    /// shell reads it from a file at the start and writes it back
    /// (`Command::SavePrefs`).
    pub layout: LayoutPrefs,
    /// The layout changed since it was last sent to be saved.
    pub layout_unsaved: bool,
    /// A drag the left button is in the middle of.
    pub drag: Option<Drag>,
    /// The last press, for double-clicks.
    pub clicks: Clicks,
    /// Where an export goes by default and what time it is stamped with
    /// (given by the shell; the app does no I/O).
    pub export_env: export::ExportEnv,
    /// The serial of the next export's request, so a reply is told from an
    /// earlier export's.
    pub next_export: u64,
    /// How the last dump ended (its words, and whether it failed), for the
    /// progress overlay once it is over.
    pub dump_ended: Option<(String, bool)>,
    /// The table the navigator is taking the sidebar to, while the levels
    /// above it load (slice 21).
    pub reveal: Option<navigator::Reveal>,
    /// Which store is in use and where (given by the shell), for the welcome
    /// screen. None when it could not be worked out.
    pub store: Option<crate::core::StoreInfo>,
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
    /// The children of a node, and what they are (the level the connection's
    /// shape puts there).
    List(NodeId, tree::Level),
    /// Run a statement in a query tab.
    Run(RunId, RunRequest),
    /// A prefix search of names for the completion popup.
    Complete(crate::core::CompletionRequest),
    /// The schema unqualified names resolve in on a database.
    CurrentSchema(DatabasePath),
    /// The columns of a table, for completion: kept in the completion cache,
    /// not in the sidebar's tree.
    TableColumns(crate::core::TablePath),
    /// The foreign keys of a table, for completion.
    ForeignKeys(crate::core::TablePath),
    /// What dbx's analysis says a statement refers to.
    AnalyzeReferences(crate::core::DbKind, String),
    /// Search the history (the overlay's search, or its next page).
    History(crate::core::HistoryQuery),
    /// Save one run to the history. Nothing waits for it.
    SaveHistory(crate::core::NewHistoryEntry),
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
    Completed(Result<crate::core::Completion, CoreError>),
    CurrentSchema(Result<Option<String>, CoreError>),
    TableColumns(Result<Vec<crate::core::ColumnInfo>, CoreError>),
    ForeignKeys(Result<Vec<crate::core::ForeignKey>, CoreError>),
    /// `None`: the statement does not parse.
    References(Result<Option<crate::core::References>, CoreError>),
    History(Result<crate::core::HistoryPage, CoreError>),
    HistorySaved(Result<(), CoreError>),
    /// The write of an export, from the shell (not the Core).
    Exported(Result<crate::core::Exported, crate::core::ExportError>),
    /// One report of a dump's progress. Many come for one request; they do
    /// not settle it.
    DumpProgress(crate::core::DumpProgress),
    /// A dump has ended (or was refused), with what the shell found at the
    /// path. This one settles the request.
    Dumped(Result<crate::core::Dumped, crate::core::DumpFailure>),
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
    /// Put this text on the user's clipboard. Fire and forget. The text is
    /// cell data: the shell must carry it so the terminal cannot read it as
    /// anything but the clipboard's content (OSC 52 base64).
    CopyToClipboard(String),
    /// Write a result to a file, off the event loop (slice 19). The answer
    /// comes back as `CoreReply::Exported` for this request, and only then
    /// does the app say anything was written.
    Export(RequestId, crate::core::ExportJob),
    /// Dump a database or a schema to a file (slice 20). The shell checks the
    /// path before dbx is called, sends progress back as
    /// `CoreReply::DumpProgress` for this request, and the end as
    /// `CoreReply::Dumped`.
    Dump(RequestId, crate::core::DumpId, crate::core::DumpRequest),
    /// Ask a dump to stop. Fire and forget: its final progress says it did.
    CancelDump(crate::core::DumpId),
    /// Write the layout preferences beside the store. Fire and forget; a
    /// failure leaves the defaults for the next run, nothing more.
    SavePrefs(LayoutPrefs),
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
    /// The query tab's connection, once the connection list has loaded.
    pub fn active_connection(&self) -> Option<&Connection> {
        let target = self.tab().target.as_ref()?;
        self.connection(&target.connection)
    }

    /// The kind of database the query tab's connection is: what dbx is asked
    /// about the editor's text for (type names, statement ranges, parsing).
    pub fn active_kind(&self) -> Option<crate::core::DbKind> {
        self.active_connection().map(|c| c.kind.clone())
    }

    /// The SQL dialect the query tab's connection speaks: what the editor
    /// colours, formats and quotes for.
    pub fn active_dialect(&self) -> Option<crate::core::Dialect> {
        self.active_connection().map(|c| c.capabilities.dialect)
    }

    /// Whether names, columns and analysis come from the Core for the query
    /// tab's connection: the client can browse it and the Core answers the
    /// searches completion asks (`Capabilities::completes`). The dialect alone
    /// says how to colour, quote and format, not this.
    pub fn core_names(&self) -> bool {
        self.active_connection().is_some_and(|c| c.capabilities.browse && c.capabilities.completes)
    }

    /// The schema unqualified names resolve in for the database `db`: the
    /// connection type's fixed one (`main` for SQLite), else what the Core
    /// said when it was asked (`Need::CurrentSchema`).
    pub(super) fn current_schema_of(&self, db: &DatabasePath) -> Option<String> {
        let fixed = self.connection(&db.connection).is_some_and(|c| c.capabilities.fixed_schema);
        if fixed {
            return Some(crate::core::FIXED_SCHEMA.to_string());
        }
        self.model.completions.current_schema.get(db).cloned()
    }

    pub fn new() -> Self {
        App {
            model: Model {
                connections: Loadable::Loading,
                schema: SchemaCache::default(),
                tabs: vec![QueryTab::new(TabId(1))],
                orphaned: Vec::new(),
                requests: Requests::default(),
                completions: complete::CompletionCache::default(),
                settings: settings::Settings::default(),
                next_run: 1,
                next_tab: 2,
                row_cap: DEFAULT_ROW_CAP,
                next_save: 1,
                dump: None,
                next_dump: 1,
                quit_after_dump: None,
            },
            ui: Ui {
                caps: Capabilities::default(),
                tab: tab::TabUi::new(TabId(1)),
                parked: Vec::new(),
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
                tab_hits: Vec::new(),
                overlay_buttons: Vec::new(),
                overlay_fields: Vec::new(),
                next_form_serial: 1,
                reload_anchor: None,
                note: None,
                syntax: syntax::SyntaxState::default(),
                notice: None,
                config: config::Report::default(),
                now: None,
                layout: LayoutPrefs::default(),
                layout_unsaved: false,
                drag: None,
                clicks: Clicks::default(),
                export_env: export::ExportEnv::default(),
                store: None,
                next_export: 1,
                dump_ended: None,
                reveal: None,
            },
        }
    }

    /// Starts with the layout the user last left (read by the shell).
    pub fn set_layout(&mut self, prefs: LayoutPrefs) {
        self.ui.layout = prefs.sane();
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
        let last = self.ui.now;
        self.on_time(now, &mut fx);
        self.dump_on_time(last, now, &mut fx);
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
        self.completion_housekeeping(&mut fx);
        self.template_housekeeping(&mut fx);
        self.refresh_marks(&mut fx);
        self.history_housekeeping(now, &mut fx);
        self.navigator_housekeeping(now, &mut fx);
        self.sync_dump_overlay();
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
            Mode::Normal | Mode::Resize => None,
        };
        [prefix, self.query_deadline(), self.ui.tab.marks.deadline(), self.history_deadline(), self.navigator_deadline(), self.dump_deadline()].into_iter().flatten().min()
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
                } else if let Some(action) = self.direct_action(&combo) {
                    self.perform(action, now, fx);
                } else if combo == CTRL_C {
                    // Bound to cancel by default (a direct key, above). Even
                    // if rebound, it never reaches the editor, whose own
                    // ctrl+c is a copy into its private buffer.
                } else {
                    match self.ui.focus {
                        PaneId::Sidebar => self.on_sidebar_key(combo, fx),
                        // The editor takes text: every other key is its own.
                        PaneId::Editor => self.on_editor_key(key, combo, fx),
                        PaneId::Results => self.on_results_key(combo, now, fx),
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
            // The resize mode takes every key: the ones it binds move a divider
            // or leave it, the rest do nothing.
            Mode::Resize => {
                // Esc leaves whatever the bindings say, so no config can trap the user.
                let esc = KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE);
                let action = if combo == esc { Some(Action::ResizeDone) } else { self.ui.keys.lookup_resize(&combo) };
                if let Some(action) = action {
                    self.perform_resize(action, fx);
                }
            }
        }
    }

    /// The action a direct key does now. `alt+enter` is the intention
    /// actions: it expands a `*` where the caret is on one (`perform` finds
    /// there is nothing to do elsewhere), and is never a run key.
    fn direct_action(&self, combo: &KeyCombo) -> Option<Action> {
        self.ui.keys.lookup_direct(combo)
    }

    fn on_paste(&mut self, text: &str, fx: &mut Effects) {
        if self.bar_open() {
            return self.bar_paste(text, fx);
        }
        if self.ui.focus == PaneId::Editor {
            self.close_completion(fx);
            match self.ui.tab.editor.paste(text) {
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
            (Action::ResizeMode, _) => {
                self.ui.mode = Mode::Resize;
                fx.repaint = true;
                None
            }
            (Action::NewConnection, _) => {
                self.open_new_form();
                None
            }
            (Action::Quit, _) => {
                self.request_quit(fx);
                None
            }
            (Action::Run | Action::RunDirect, _) => {
                self.run_requested(now, fx);
                None
            }
            (Action::Help, _) => {
                self.open_help(fx);
                None
            }
            (Action::History | Action::HistoryDirect, _) => {
                self.open_history(fx);
                None
            }
            (Action::Navigator | Action::NavigatorDirect, _) => {
                self.open_navigator(fx);
                None
            }
            (Action::NewTab, _) => {
                self.new_tab(fx);
                None
            }
            (Action::CloseTab | Action::CloseTabDirect, _) => {
                self.request_close_tab(self.ui.tab.id, fx);
                None
            }
            (Action::NextTab | Action::NextTabDirect, _) => {
                self.cycle_tab(1, fx);
                None
            }
            (Action::PrevTab | Action::PrevTabDirect, _) => {
                self.cycle_tab(-1, fx);
                None
            }
            (Action::GoToTab(n), _) => {
                self.go_to_tab(n, fx);
                None
            }
            (a, _) if a.scope() == keys::Scope::Editor => {
                self.perform_edit(a, now, fx);
                None
            }
            (Action::Cancel, _) => {
                self.cancel_run(now, fx);
                None
            }
            // ctrl+c cancels only where the run is: the editor and the results.
            (Action::ExpandStar, Editor) => {
                self.expand_star(fx);
                None
            }
            (Action::FinishStatementDirect | Action::FinishStatement, Editor) => {
                self.finish_statement(fx);
                None
            }
            (Action::NextUnresolved, Editor) => {
                self.goto_unresolved(true, fx);
                None
            }
            (Action::PrevUnresolved, Editor) => {
                self.goto_unresolved(false, fx);
                None
            }
            // With nothing running, `ctrl+c` in the editor copies a selection.
            (Action::CancelDirect, Editor) if self.tab().run.in_flight().is_none() && self.ui.tab.editor.selection().is_some() => {
                self.copy_selection(false, fx);
                None
            }
            (Action::CancelDirect, Editor | Results) => {
                self.cancel_run(now, fx);
                None
            }
            // The key that opens the completion popup works in the editor.
            (Action::CompleteOpen, Editor) => {
                self.perform_complete(action, fx);
                None
            }
            _ => None,
        };
        if let Some(pane) = target {
            if pane != self.ui.focus {
                self.cancel_reveal();
            }
            self.ui.focus = pane;
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, now: Instant, fx: &mut Effects) {
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                self.on_wheel(mouse, fx)
            }
            MouseEventKind::Down(MouseButton::Left) => self.on_press(mouse, now, fx),
            MouseEventKind::Drag(MouseButton::Left) => self.on_drag(mouse, fx),
            MouseEventKind::Up(MouseButton::Left) => self.on_release(fx),
            _ => {}
        }
    }

    /// The wheel scrolls what is under the pointer, focused or not: the
    /// sidebar's list, the editor's text, the results' rows. With shift, or a
    /// horizontal wheel where the terminal reports one, the results scroll by
    /// columns and the editor sideways.
    fn on_wheel(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        let at = (mouse.column, mouse.row).into();
        let shift = mouse.modifiers.contains(crossterm::event::KeyModifiers::SHIFT);
        let (down, sideways) = match mouse.kind {
            MouseEventKind::ScrollDown => (true, shift),
            MouseEventKind::ScrollUp => (false, shift),
            MouseEventKind::ScrollRight => (true, true),
            _ => (false, true),
        };
        let step = if down { 1 } else { -1 };
        if !sideways && self.wheel_completion(mouse.column, mouse.row, WHEEL_ROWS * step, fx) {
            return;
        }
        if self.ui.hits.sidebar.contains(at) {
            if !sideways {
                self.scroll_sidebar(WHEEL_ROWS * step, fx);
            }
        } else if self.ui.hits.editor.contains(at) {
            let (rows, cols) = if sideways { (0, 4 * step as i16) } else { (WHEEL_ROWS as i16 * step as i16, 0) };
            fx.repaint |= self.ui.tab.editor.scroll(rows, cols);
        } else if self.ui.hits.results.contains(at) {
            if sideways {
                self.scroll_grid(0, step, fx);
            } else {
                self.scroll_grid(WHEEL_ROWS * step, 0, fx);
            }
        }
    }

    /// A left press: a hint, a divider, a scrollbar, or a pane's own content.
    fn on_press(&mut self, mouse: MouseEvent, now: Instant, fx: &mut Effects) {
        let at = (mouse.column, mouse.row).into();
        let (x, y) = (mouse.column, mouse.row);
        self.ui.drag = None;
        // The user took over with the mouse: a reveal on its way is given up.
        self.cancel_reveal();
        // A hint in the mode bar does what its key does.
        if let Some(hit) = self.ui.mode_hints.iter().find(|h| h.rect.contains(at)).copied() {
            self.ui.clicks.forget();
            return self.click_hint(hit.action, now, fx);
        }
        // A tab in the bar: its label makes it active, the `×` closes it.
        if let Some(hit) = self.ui.tab_hits.iter().find(|h| h.rect.contains(at)).copied() {
            self.ui.clicks.forget();
            self.close_completion(fx);
            if hit.close {
                return self.request_close_tab(hit.tab, fx);
            }
            self.switch_to(hit.tab, fx);
            self.ui.focus = PaneId::Editor;
            fx.repaint = true;
            return;
        }
        // The completion popup is drawn over the panes: a press on it is its
        // own, and a press anywhere else closes it and goes on.
        if self.press_completion(x, y, fx) {
            self.ui.clicks.forget();
            return;
        }
        // A divider is nobody's pane: pressing it takes no focus; dragging it
        // resizes, and a double-click puts it back.
        for (vertical, rect) in [(true, self.ui.hits.sidebar_separator), (false, self.ui.hits.divider)] {
            if rect.contains(at) {
                if self.ui.clicks.press(now, ClickKey::Divider(vertical)) {
                    self.reset_divider(vertical, fx);
                } else {
                    self.ui.drag = Some(if vertical { Drag::Sidebar } else { Drag::Split });
                }
                return;
            }
        }
        if let Some(pane) = self.ui.hits.pane_at(x, y) {
            if pane != self.ui.focus {
                self.ui.focus = pane;
                fx.repaint = true;
            }
        }
        if self.ui.hits.sidebar_bar.contains(at) {
            self.ui.clicks.forget();
            self.ui.drag = self.press_sidebar_bar(y, fx);
        } else if self.ui.hits.sidebar_list.contains(at) {
            self.click_sidebar_row(x, y, now, fx);
        } else if self.ui.hits.results.contains(at) {
            if self.ui.tab.grid.bar.contains(at) {
                self.ui.clicks.forget();
                self.ui.drag = self.press_grid_bar(y, fx);
            } else {
                match self.click_grid(x, y, fx) {
                    // The second press on a cell opens it.
                    Some((row, col)) => {
                        if self.ui.clicks.press(now, ClickKey::Cell(row, col)) {
                            self.perform_results(Action::OpenCell, now, fx);
                        }
                    }
                    None => self.ui.clicks.forget(),
                }
            }
        } else if self.ui.hits.editor_text.contains(at) {
            let text = self.ui.hits.editor_text;
            let (col, row) = ((x - text.x) as i32, (y - text.y) as i32);
            let line = self.ui.tab.editor.row_at(col, row);
            let changed = if self.ui.clicks.press(now, ClickKey::Text(line, 0)) {
                self.ui.tab.editor.select_word(col, row)
            } else {
                self.ui.tab.editor.click(col, row)
            };
            fx.repaint |= changed;
            self.ui.drag = Some(Drag::EditorSelect);
        } else {
            self.ui.clicks.forget();
        }
    }

    /// The pointer moved with the left button down.
    fn on_drag(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        let (x, y) = (mouse.column, mouse.row);
        match self.ui.drag {
            Some(Drag::Sidebar) => self.set_sidebar_total(i32::from(x) + 1, fx),
            Some(Drag::Split) => {
                let top = i32::from(self.ui.hits.editor.y);
                self.set_editor_height(i32::from(y) - top, fx)
            }
            Some(Drag::SidebarBar { grab }) => self.drag_sidebar_bar(y, grab, fx),
            Some(Drag::GridBar { grab }) => self.drag_grid_bar(y, grab, fx),
            Some(Drag::EditorSelect) => {
                let text = self.ui.hits.editor_text;
                fx.repaint |= self.ui.tab.editor.drag(i32::from(x) - i32::from(text.x), i32::from(y) - i32::from(text.y));
            }
            None => {}
        }
    }

    /// The left button is up: a drag ends, and a resized layout is saved.
    fn on_release(&mut self, fx: &mut Effects) {
        match self.ui.drag.take() {
            Some(Drag::Sidebar | Drag::Split) => self.save_layout(fx),
            Some(Drag::EditorSelect) => fx.repaint |= self.ui.tab.editor.end_drag(),
            _ => {}
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent, fx: &mut Effects) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let combo = KeyCombo::from_event(&key);
        if matches!(self.ui.overlay, Some(Overlay::CellViewer(_))) {
            return self.on_viewer_key(combo, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::History(_))) {
            return self.on_history_key(&key, combo, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Navigator(_))) {
            return self.on_navigator_key(&key, combo, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Help(_))) {
            return self.on_help_key(&key, combo, fx);
        }
        if self.on_chooser_or_help_key(&combo, fx) {
            return;
        }
        let Some(overlay) = &self.ui.overlay else { return };
        let action = overlay.buttons().iter().find(|b| KeyCombo::parse(b.key) == Some(combo)).map(|b| b.action);
        if let Some(action) = action {
            return self.overlay_action(action, fx);
        }
        match overlay {
            Overlay::Form(form) if !form.confirm_discard => self.on_form_key(&key, fx),
            Overlay::Export(_) => self.on_export_key(&key, fx),
            Overlay::Dump(_) => self.on_dump_key(&key, fx),
            // Backing out one level always works, button or not.
            Overlay::Failure(_) if combo.code == KeyCode::Esc => self.overlay_action(OverlayAction::Dismiss, fx),
            _ => {}
        }
    }

    /// The keys of the chooser and the help that are not buttons: moving.
    /// Returns whether it took the key.
    fn on_chooser_or_help_key(&mut self, combo: &KeyCombo, fx: &mut Effects) -> bool {
        if matches!(self.ui.overlay, Some(Overlay::CopyAs(_))) {
            return self.on_copy_as_key(combo, fx);
        }
        let name = combo.to_string();
        let key = |names: &[&str]| names.contains(&name.as_str());
        match &mut self.ui.overlay {
            Some(Overlay::Chooser(c)) => {
                let last = c.items.len().saturating_sub(1);
                if key(&["down", "j", "ctrl+n"]) {
                    c.selected = (c.selected + 1).min(last);
                } else if key(&["up", "k", "ctrl+p"]) {
                    c.selected = c.selected.saturating_sub(1);
                } else {
                    return false;
                }
                fx.repaint = true;
                true
            }
            // The statement scrolls; the view keeps it in range.
            Some(Overlay::ConfirmRun(c)) => {
                if key(&["down", "j"]) {
                    c.scroll += 1;
                } else if key(&["up", "k"]) {
                    c.scroll = c.scroll.saturating_sub(1);
                } else if key(&["pagedown", "space"]) {
                    c.scroll += 10;
                } else if key(&["pageup"]) {
                    c.scroll = c.scroll.saturating_sub(10);
                } else if key(&["g", "home"]) {
                    c.scroll = 0;
                } else if key(&["G", "end"]) {
                    c.scroll = usize::MAX / 2;
                } else {
                    return false;
                }
                fx.repaint = true;
                true
            }
            _ => false,
        }
    }

    fn on_overlay_mouse(&mut self, mouse: MouseEvent, fx: &mut Effects) {
        // A drag that began before the overlay opened ends here.
        // A divider drag that ends here is saved all the same.
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            self.on_release(fx);
        }
        if let Some(Overlay::ConfirmRun(c)) = &mut self.ui.overlay {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    c.scroll += WHEEL_ROWS as usize;
                    fx.repaint = true;
                    return;
                }
                MouseEventKind::ScrollUp => {
                    c.scroll = c.scroll.saturating_sub(WHEEL_ROWS as usize);
                    fx.repaint = true;
                    return;
                }
                _ => {}
            }
        }
        if let Some(Overlay::Help(h)) = &mut self.ui.overlay {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    h.scroll += WHEEL_ROWS as usize;
                    fx.repaint = true;
                    return;
                }
                MouseEventKind::ScrollUp => {
                    h.scroll = h.scroll.saturating_sub(WHEEL_ROWS as usize);
                    fx.repaint = true;
                    return;
                }
                _ => {}
            }
        }
        if matches!(self.ui.overlay, Some(Overlay::History(_))) {
            match mouse.kind {
                MouseEventKind::ScrollDown => return self.on_history_wheel(true, fx),
                MouseEventKind::ScrollUp => return self.on_history_wheel(false, fx),
                _ => {}
            }
        }
        if matches!(self.ui.overlay, Some(Overlay::Navigator(_))) {
            match mouse.kind {
                MouseEventKind::ScrollDown => return self.on_navigator_wheel(true, fx),
                MouseEventKind::ScrollUp => return self.on_navigator_wheel(false, fx),
                _ => {}
            }
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => return self.scroll_viewer(WHEEL_ROWS, fx),
            MouseEventKind::ScrollUp => return self.scroll_viewer(-WHEEL_ROWS, fx),
            _ => {}
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let at = (mouse.column, mouse.row).into();
        let hit = self.ui.overlay_buttons.iter().find(|b| b.rect.contains(at)).copied();
        if let Some(button) = hit {
            // A click on a text row also puts the cursor where it landed.
            if let OverlayAction::ExportFocus(field) = button.action {
                return self.on_export_click(field, mouse.column - button.rect.x, fx);
            }
            if let OverlayAction::DumpFocus(field) = button.action {
                return self.on_dump_click(field, mouse.column - button.rect.x, fx);
            }
            return self.overlay_action(button.action, fx);
        }
        // Anything else outside the buttons and the form's rows does nothing:
        // not even to the panes behind the overlay.
        if let Some(hit) = self.ui.overlay_fields.iter().find(|f| f.rect.contains(at)).copied() {
            self.on_form_click(hit, mouse.column - hit.rect.x, fx);
        }
    }

    fn on_overlay_paste(&mut self, text: &str, fx: &mut Effects) {
        if matches!(self.ui.overlay, Some(Overlay::History(_))) {
            return self.on_history_paste(text, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Navigator(_))) {
            return self.on_navigator_paste(text, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Help(_))) {
            return self.on_help_paste(text, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Export(_))) {
            return self.on_export_paste(text, fx);
        }
        if matches!(self.ui.overlay, Some(Overlay::Dump(_))) {
            return self.on_dump_paste(text, fx);
        }
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
            OverlayAction::RerunYes => self.confirm_rerun(fx),
            OverlayAction::CloseTabYes => self.confirm_close_tab(fx),
            OverlayAction::ViewerCopy => self.copy_viewer(fx),
            OverlayAction::ViewerToggleRaw => self.toggle_viewer_raw(fx),
            OverlayAction::ViewerLineDown => self.scroll_viewer_by(Scroll::Line(1), fx),
            OverlayAction::ViewerLineUp => self.scroll_viewer_by(Scroll::Line(-1), fx),
            OverlayAction::ViewerPageDown => self.scroll_viewer_by(Scroll::Page(1), fx),
            OverlayAction::ViewerPageUp => self.scroll_viewer_by(Scroll::Page(-1), fx),
            OverlayAction::ViewerTop => self.scroll_viewer_by(Scroll::Top, fx),
            OverlayAction::ViewerBottom => self.scroll_viewer_by(Scroll::Bottom, fx),
            OverlayAction::ChooserRun => {
                let now = self.ui.now.unwrap_or_else(Instant::now);
                self.chooser_run(None, now, fx)
            }
            OverlayAction::ChooserPick(i) => {
                let now = self.ui.now.unwrap_or_else(Instant::now);
                self.chooser_run(Some(i), now, fx)
            }
            OverlayAction::ConfirmRunYes => {
                let now = self.ui.now.unwrap_or_else(Instant::now);
                self.confirm_run(now, fx)
            }
            OverlayAction::ConfirmRunNo => self.decline_run(fx),
            OverlayAction::HistoryInsert | OverlayAction::HistoryRun | OverlayAction::HistoryScope | OverlayAction::HistoryPick(_) => {
                self.history_action(action, fx)
            }
            OverlayAction::NavigatorReveal | OverlayAction::NavigatorOpen | OverlayAction::NavigatorPick(_) => self.navigator_action(action, fx),
            OverlayAction::ExportRun
            | OverlayAction::ExportReplace
            | OverlayAction::ExportKeep
            | OverlayAction::ExportFormat(_)
            | OverlayAction::ExportFocus(_)
            | OverlayAction::ExportNull(_) => self.export_action(action, fx),
            OverlayAction::CopyAsRun | OverlayAction::CopyAsPick(_) => self.copy_as_action(action, fx),
            OverlayAction::DumpRun
            | OverlayAction::DumpReplace
            | OverlayAction::DumpKeep
            | OverlayAction::DumpContent(_)
            | OverlayAction::DumpScope(_)
            | OverlayAction::DumpDrop(_)
            | OverlayAction::DumpFocus(_)
            | OverlayAction::DumpCancel
            | OverlayAction::QuitDumpYes => self.dump_action(action, fx),
            OverlayAction::Dismiss => {
                self.forget_history_if_closing(fx);
                self.forget_navigator_if_closing(fx);
                self.ui.overlay = None;
                fx.repaint = true;
            }
        }
    }

    fn on_reply(&mut self, id: RequestId, reply: CoreReply, fx: &mut Effects) {
        // A dump's progress comes many times for one request: it is looked
        // up, not taken off the table. Stale, it changes nothing.
        let reply = match reply {
            CoreReply::DumpProgress(progress) => return self.on_dump_progress(id, progress, fx),
            reply => reply,
        };
        // A stale reply is dropped without touching state or repainting,
        // except that a cancelled run's last word says it has ended.
        let target = match self.model.requests.settle(id) {
            Settled::Current(target) => target,
            Settled::Stale(Target::Run(tab)) => {
                if let CoreReply::Ran(result) = reply {
                    let glyph = self.model.tab(tab).map(query::glyph);
                    self.on_ran_after_cancel(tab, id, result, fx);
                    // A tab in the bar whose glyph changed is redrawn.
                    if glyph.is_some() && self.model.tab(tab).map(query::glyph) != glyph {
                        fx.repaint = true;
                    }
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
            (Target::Run(tab), CoreReply::Ran(result)) => {
                let glyph = self.model.tab(tab).map(query::glyph);
                self.on_ran_in(tab, result, fx);
                // A tab that is not on screen changed out of sight, but for
                // its glyph in the bar.
                if !self.is_active(tab) && self.model.tab(tab).map(query::glyph) == glyph {
                    return;
                }
            }
            (Target::Complete(tab, need), reply) => {
                // Whether the screen changed is its own business: a reply for
                // a word the caret has left, or for a tab not on screen,
                // repaints nothing.
                if self.on_complete_reply(tab, need, reply, fx) {
                    fx.repaint = true;
                }
                return;
            }
            (Target::OpenTable(path), CoreReply::TableColumns(result)) => self.on_open_table_columns(path, result, fx),
            (Target::History, CoreReply::History(result)) => {
                if !self.on_history_reply(result, false) {
                    return;
                }
            }
            (Target::HistoryMore, CoreReply::History(result)) => {
                if !self.on_history_reply(result, true) {
                    return;
                }
            }
            (Target::Navigate, CoreReply::Completed(result)) => {
                if !self.on_navigator_reply(result) {
                    return;
                }
            }
            (Target::Export(serial), CoreReply::Exported(result)) => return self.on_exported(serial, result, fx),
            (Target::Dump, CoreReply::Dumped(result)) => return self.on_dumped(id, result, fx),
            (Target::SaveHistory(serial), CoreReply::HistorySaved(result)) => {
                self.model.requests.forget(&Target::SaveHistory(serial));
                // Fire and forget: only a failure is worth a word.
                return self.on_history_saved(serial, result, fx);
            }
            (Target::Children(node), CoreReply::Listed(result)) => {
                // Only the listing the reveal waits for goes on with it.
                let waited_for = self.reveal_waits_for(&node);
                if !self.on_listed(node, result, fx) {
                    return;
                }
                if waited_for {
                    self.continue_reveal(fx);
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
                let gone: Vec<ConnectionId> = self
                    .model
                    .tabs
                    .iter()
                    .filter_map(|t| t.target.as_ref().map(|t| t.connection.clone()))
                    .filter(|c| !list.iter().any(|k| &k.id == c))
                    .collect();
                for c in gone {
                    self.connection_removed(&c, fx);
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
