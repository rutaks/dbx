//! The navigator (slice 21): go to a table or view by typing part of its name.
//!
//! **Scope** is one database: the active tab's, else the one under the
//! sidebar cursor, else nothing (the overlay says to give the tab one).
//!
//! **Results** come from two places. At once, on every keystroke: the names
//! the sidebar's tree already holds, matched as completion matches them
//! (exact, prefix, initials, anywhere; shorter first). After `PAUSE` without
//! typing: `Core::complete` over every schema of the database, whose answer
//! adds the names the tree does not hold, after the ones already shown, so
//! the highlighted row never moves under the user. A search no longer wanted
//! (the text changed, the overlay closed) is abandoned, and its reply, if it
//! lands, changes nothing.
//!
//! **From a result:** `enter` reveals the table in the sidebar (expanding
//! the connection, database and schema above it, loading each level as it
//! goes, then putting the cursor on it and focusing the sidebar); `ctrl+o`
//! opens its rows in a browse tab.
//!
//! Everything shown is database-supplied text and goes through the painter.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::complete_match::{match_name, Tier};
use super::keys::{Action, KeyCombo};
use super::overlay::{Overlay, OverlayAction};
use super::requests::Target;
use super::text_field::{Edit, TextField};
use super::tree::{Children, NodeId, RowKind};
use super::{App, Command, CoreCall, Effects, Mode, PaneId};
use crate::core::{
    display_safe, CandidateKind, Completion, CompletionRequest, CoreError, DatabasePath, SchemaPath, SearchKind,
    TableKind, TablePath,
};

/// How long typing must pause before the database is searched.
pub const PAUSE: Duration = Duration::from_millis(200);

/// The most names asked of the database, and the most shown from the tree.
pub const LIMIT: usize = 200;

/// Rows one notch of the wheel moves the highlight.
const WHEEL: isize = 3;

/// A table or view the navigator can go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavItem {
    /// Exactly as stored: what the sidebar and the browse tab are told. Empty
    /// where the connection has no schema level.
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    /// It came from the tree, not from the database's answer.
    pub loaded: bool,
}

impl NavItem {
    /// `schema.name`, or just the name where there are no schemas.
    pub fn label(&self) -> String {
        if self.schema.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    /// What the row says the thing is.
    pub fn kind_label(&self) -> &'static str {
        self.kind.label().unwrap_or("table")
    }
}

/// Where the database search is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// Nothing is asked for: no text, or the text has not paused yet.
    Idle,
    /// Sent, not answered.
    Asking,
    /// Answered: `more` says the answer stopped at its limit.
    Done {
        more: bool,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct NavigatorOverlay {
    pub search: TextField,
    /// The database searched, and how to say where it is (`connection /
    /// database`); `None`: there is none to search.
    pub scope: Option<(DatabasePath, String)>,
    /// The key that gives the tab a database, for the message that says so.
    pub use_key: String,
    pub items: Vec<NavItem>,
    pub selected: usize,
    /// The first row shown and how many fit: set by the view.
    pub top: usize,
    pub rows: usize,
    pub state: Search,
    /// When the database search is due, after typing paused.
    pub due: Option<Instant>,
}

impl NavigatorOverlay {
    pub fn selected_item(&self) -> Option<&NavItem> {
        self.items.get(self.selected)
    }
}

/// A reveal in progress: the table, the level whose listing it is waiting
/// for (only that listing's reply goes on with it), and the levels it has
/// asked for, once per time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reveal {
    pub path: TablePath,
    pub waiting: Option<NodeId>,
    asked: Vec<NodeId>,
}

/// What was typed, as the schema it names (before a dot) and the name.
fn split_typed(typed: &str) -> (Option<&str>, &str) {
    match typed.trim().split_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, typed.trim()),
    }
}

/// The nodes above a table, from the connection down.
fn reveal_chain(path: &TablePath) -> Vec<NodeId> {
    let db = &path.schema.database;
    let mut chain = vec![NodeId::Connection(db.connection.clone())];
    if !db.database.is_empty() {
        chain.push(NodeId::Database(db.clone()));
    }
    if !path.schema.schema.is_empty() {
        chain.push(NodeId::Schema(path.schema.clone()));
    }
    chain
}

impl App {
    /// The database the navigator looks in: the active tab's, else the one
    /// under the sidebar cursor.
    fn navigator_scope(&self) -> Option<DatabasePath> {
        if let Some(target) = &self.tab().target {
            return Some(target.clone());
        }
        let row = self.cursor_row()?;
        match &row.kind {
            RowKind::Connection { default_database, .. } => {
                let connection = row.node.connection().clone();
                let c = self.connection(&connection)?;
                if !c.capabilities.databases {
                    Some(DatabasePath { connection, database: String::new() })
                } else {
                    default_database.clone().map(|database| DatabasePath { connection, database })
                }
            }
            _ => row.node.database_path().cloned(),
        }
    }

    pub(super) fn open_navigator(&mut self, fx: &mut Effects) {
        self.close_completion(fx);
        // A search of an earlier navigator, whose overlay went some other way,
        // is not this one's.
        self.forget_navigator_requests(fx);
        self.ui.mode = Mode::Normal;
        let scope = self.navigator_scope().map(|at| {
            let name =
                self.connection(&at.connection).map(|c| c.name.clone()).unwrap_or_else(|| at.connection.0.clone());
            let place = if at.database.is_empty() { name } else { format!("{name} / {}", at.database) };
            (at, place)
        });
        let use_key = self.ui.keys.key_for(Action::UseTarget).unwrap_or_default();
        self.ui.overlay = Some(Overlay::Navigator(Box::new(NavigatorOverlay {
            search: TextField::new(),
            scope,
            use_key,
            items: Vec::new(),
            selected: 0,
            top: 0,
            rows: 0,
            state: Search::Idle,
            due: None,
        })));
        fx.repaint = true;
    }

    /// The overlay is closed (or being left): a search on its way is not wanted.
    fn forget_navigator_requests(&mut self, fx: &mut Effects) {
        let dropped = self.model.requests.forget(&Target::Navigate);
        fx.commands.extend(dropped.into_iter().map(Command::Abandon));
    }

    pub(super) fn close_navigator(&mut self, fx: &mut Effects) {
        self.ui.overlay = None;
        self.forget_navigator_requests(fx);
        fx.repaint = true;
    }

    /// Closing any overlay by a button: a navigator's search is let go of.
    pub(super) fn forget_navigator_if_closing(&mut self, fx: &mut Effects) {
        if matches!(self.ui.overlay, Some(Overlay::Navigator(_))) {
            self.forget_navigator_requests(fx);
        }
    }

    pub(super) fn navigator_housekeeping(&mut self, now: Instant, fx: &mut Effects) {
        let due = matches!(&self.ui.overlay, Some(Overlay::Navigator(n)) if n.due.is_some_and(|d| now >= d));
        if due {
            self.search_database(fx);
        }
    }

    pub(super) fn navigator_deadline(&self) -> Option<Instant> {
        match &self.ui.overlay {
            Some(Overlay::Navigator(n)) => n.due,
            _ => None,
        }
    }

    /// Tables and views the tree holds that the typed text matches, best
    /// first: exact, prefix, initials, anywhere; shorter names before longer.
    fn loaded_matches(&self, at: &DatabasePath, typed: &str) -> Vec<NavItem> {
        let (schema, pattern) = split_typed(typed);
        if typed.trim().is_empty() {
            return Vec::new();
        }
        let wanted = schema.map(str::to_lowercase);
        let mut found: Vec<(Tier, usize, NavItem)> = Vec::new();
        for (schema, table) in self.model.schema.tables_in(at) {
            if wanted.as_ref().is_some_and(|w| !schema.to_lowercase().starts_with(w.as_str())) {
                continue;
            }
            if let Some(m) = match_name(pattern, &table.name, false) {
                found.push((
                    m.tier,
                    table.name.chars().count(),
                    NavItem { schema, name: table.name, kind: table.kind, loaded: true },
                ));
            }
        }
        found.sort_by(|a, b| (a.0, a.1, &a.2.schema, &a.2.name).cmp(&(b.0, b.1, &b.2.schema, &b.2.name)));
        found.into_iter().map(|(_, _, item)| item).take(LIMIT).collect()
    }

    /// The text changed: the tree's names at once, the database's after the
    /// pause, and what was asked of it before is not wanted any more.
    fn navigator_typed(&mut self, fx: &mut Effects) {
        let now = self.ui.now.unwrap_or_else(Instant::now);
        let Some(Overlay::Navigator(n)) = &self.ui.overlay else { return };
        let (Some((at, _)), typed) = (n.scope.clone(), n.search.text().to_string()) else { return };
        let items = self.loaded_matches(&at, &typed);
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
        n.items = items;
        n.selected = 0;
        n.top = 0;
        n.state = Search::Idle;
        n.due = (!typed.trim().is_empty()).then_some(now + PAUSE);
        self.forget_navigator_requests(fx);
        fx.repaint = true;
    }

    /// Asks the database for the names that start with what is typed, in
    /// every schema it holds.
    fn search_database(&mut self, fx: &mut Effects) {
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
        n.due = None;
        let Some((at, _)) = n.scope.clone() else { return };
        let (_, pattern) = split_typed(n.search.text());
        let prefix = pattern.to_string();
        if n.search.text().trim().is_empty() {
            return;
        }
        n.state = Search::Asking;
        let id = self.model.requests.begin(Target::Navigate);
        let request =
            CompletionRequest { at, schema: None, kind: SearchKind::Relation, prefix, limit: LIMIT, global: true };
        fx.commands.push(Command::Core(id, CoreCall::Complete(request)));
        fx.repaint = true;
    }

    /// The database's answer to the search still wanted: its names that the
    /// list does not have are added after it. Whether anything changed.
    pub(super) fn on_navigator_reply(&mut self, result: Result<Completion, CoreError>) -> bool {
        let has_schemas = match &self.ui.overlay {
            Some(Overlay::Navigator(n)) => n
                .scope
                .as_ref()
                .is_none_or(|(at, _)| self.connection(&at.connection).is_none_or(|c| c.capabilities.schemas)),
            _ => return false,
        };
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return false };
        match result {
            Ok(found) => {
                let (schema, _) = split_typed(n.search.text());
                let wanted = schema.map(str::to_lowercase);
                for c in found.candidates {
                    let kind = match c.kind {
                        CandidateKind::Table => TableKind::Table,
                        CandidateKind::View => TableKind::View,
                        CandidateKind::MaterializedView => TableKind::MaterializedView,
                        _ => continue,
                    };
                    // Where schemas exist, a name with none cannot be found again.
                    let Some(schema) = c.schema.or_else(|| (!has_schemas).then(String::new)) else { continue };
                    if wanted.as_ref().is_some_and(|w| !schema.to_lowercase().starts_with(w.as_str())) {
                        continue;
                    }
                    if n.items.iter().any(|i| i.schema == schema && i.name == c.name) || n.items.len() >= 2 * LIMIT {
                        continue;
                    }
                    n.items.push(NavItem { schema, name: c.name, kind, loaded: false });
                }
                n.state = Search::Done { more: found.incomplete };
            }
            Err(e) => n.state = Search::Failed(display_safe(e.message.trim())),
        }
        true
    }

    // ---- input ---------------------------------------------------------------------

    pub(super) fn on_navigator_key(&mut self, key: &KeyEvent, combo: KeyCombo, fx: &mut Effects) {
        let direct = self.ui.keys.lookup_direct(&combo);
        let plain = combo.mods.is_empty();
        let ctrl = combo.mods == KeyModifiers::CONTROL;
        match combo.code {
            KeyCode::Esc if plain => return self.close_navigator(fx),
            _ if direct == Some(Action::NavigatorDirect) => return self.close_navigator(fx),
            KeyCode::Enter if plain => return self.navigator_reveal(fx),
            KeyCode::Char('o') if ctrl => return self.navigator_open(fx),
            KeyCode::Down if plain => return self.navigator_move(1, fx),
            KeyCode::Up if plain => return self.navigator_move(-1, fx),
            KeyCode::Char('n') if ctrl => return self.navigator_move(1, fx),
            KeyCode::Char('p') if ctrl => return self.navigator_move(-1, fx),
            KeyCode::PageDown if plain => return self.navigator_page(1, fx),
            KeyCode::PageUp if plain => return self.navigator_page(-1, fx),
            _ => {}
        }
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
        if n.scope.is_none() {
            return;
        }
        match n.search.handle_key(key) {
            Edit::Changed => self.navigator_typed(fx),
            Edit::Moved => fx.repaint = true,
            Edit::Ignored => {}
        }
    }

    pub(super) fn on_navigator_paste(&mut self, text: &str, fx: &mut Effects) {
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
        if n.scope.is_none() {
            return;
        }
        n.search.insert_str(text);
        self.navigator_typed(fx);
    }

    pub(super) fn on_navigator_wheel(&mut self, down: bool, fx: &mut Effects) {
        self.navigator_move(if down { WHEEL } else { -WHEEL }, fx);
    }

    fn navigator_move(&mut self, by: isize, fx: &mut Effects) {
        let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
        if n.items.is_empty() {
            return;
        }
        let to = n.selected.saturating_add_signed(by).min(n.items.len() - 1);
        if to != n.selected {
            n.selected = to;
            fx.repaint = true;
        }
    }

    fn navigator_page(&mut self, way: isize, fx: &mut Effects) {
        let rows = match &self.ui.overlay {
            Some(Overlay::Navigator(n)) => n.rows.saturating_sub(1).max(1) as isize,
            _ => return,
        };
        self.navigator_move(way * rows, fx);
    }

    pub(super) fn navigator_action(&mut self, action: OverlayAction, fx: &mut Effects) {
        match action {
            OverlayAction::NavigatorReveal => self.navigator_reveal(fx),
            OverlayAction::NavigatorOpen => self.navigator_open(fx),
            OverlayAction::NavigatorPick(i) => {
                let Some(Overlay::Navigator(n)) = &mut self.ui.overlay else { return };
                if i < n.items.len() && n.selected != i {
                    n.selected = i;
                    fx.repaint = true;
                }
            }
            _ => {}
        }
    }

    /// The highlighted name as a table path, closing the overlay.
    fn navigator_take(&mut self, fx: &mut Effects) -> Option<TablePath> {
        let Some(Overlay::Navigator(n)) = &self.ui.overlay else { return None };
        let (at, _) = n.scope.clone()?;
        let item = n.selected_item()?;
        let path =
            TablePath { schema: SchemaPath { database: at, schema: item.schema.clone() }, table: item.name.clone() };
        self.close_navigator(fx);
        Some(path)
    }

    /// `enter`: show the table in the sidebar.
    fn navigator_reveal(&mut self, fx: &mut Effects) {
        if let Some(path) = self.navigator_take(fx) {
            self.start_reveal(path, fx);
        }
    }

    /// `ctrl+o`: browse its rows.
    fn navigator_open(&mut self, fx: &mut Effects) {
        if let Some(path) = self.navigator_take(fx) {
            self.open_table_path(path, fx);
        }
    }

    // ---- revealing -------------------------------------------------------------------

    /// Takes the sidebar to `path`: opens the connection, database and schema
    /// above it, asking the Core for each level not yet loaded, one after the
    /// other as the answers come.
    fn start_reveal(&mut self, path: TablePath, fx: &mut Effects) {
        // A level that failed before is asked again: the user wants it now.
        for node in reveal_chain(&path) {
            if matches!(self.model.schema.state(&node), Children::Failed(_)) {
                self.model.schema.set(node, Children::NotLoaded);
            }
        }
        self.ui.reveal = Some(Reveal { path, waiting: None, asked: Vec::new() });
        self.continue_reveal(fx);
    }

    /// Whether the reveal in progress is waiting for the listing of `node`:
    /// only that listing's reply goes on with it.
    pub(super) fn reveal_waits_for(&self, node: &NodeId) -> bool {
        self.ui.reveal.as_ref().is_some_and(|r| r.waiting.as_ref() == Some(node))
    }

    /// The user took over (a key or the mouse in the sidebar, another pane,
    /// another tab): a reveal on its way is given up.
    pub(super) fn cancel_reveal(&mut self) {
        self.ui.reveal = None;
    }

    /// The listing it waited for arrived: go on with the reveal in progress,
    /// if there is one. A level is asked for at most twice (the first time and
    /// one retry); after that the sidebar stops on it.
    pub(super) fn continue_reveal(&mut self, fx: &mut Effects) {
        let Some(mut reveal) = self.ui.reveal.take() else { return };
        let path = reveal.path.clone();
        for node in reveal_chain(&path) {
            let open = self.ui.sidebar.expanded.contains(&node);
            if !open || matches!(self.model.schema.state(&node), Children::NotLoaded) {
                if reveal.asked.iter().filter(|n| **n == node).count() >= 2 {
                    return self.finish_reveal(Some(node), None, fx);
                }
                reveal.asked.push(node.clone());
                self.expand(node.clone(), fx);
            }
            match self.model.schema.state(&node) {
                Children::Loaded(_) | Children::Refreshing(_) => {}
                Children::Loading => {
                    reveal.waiting = Some(node);
                    self.ui.reveal = Some(reveal);
                    return;
                }
                // Nothing was asked: a kind the tree cannot browse. Stop there, and say so.
                Children::NotLoaded => {
                    let name = self.connection(node.connection()).map(|c| c.name.clone()).unwrap_or_default();
                    let what = display_safe(&path.table);
                    self.say(format!("{name} cannot be browsed yet, so {what} cannot be shown in the sidebar."), true);
                    return self.finish_reveal(Some(node), None, fx);
                }
                // The level that failed is where the sidebar stops, showing why.
                Children::Failed(_) => return self.finish_reveal(Some(node), None, fx),
            }
        }
        self.finish_reveal(None, Some(path), fx);
    }

    /// Puts the cursor on the table (or on `stopped_at`) and the focus in the sidebar.
    fn finish_reveal(&mut self, stopped_at: Option<NodeId>, table: Option<TablePath>, fx: &mut Effects) {
        self.ui.reveal = None;
        self.touch_rows();
        self.ensure_rows();
        self.ui.focus = PaneId::Sidebar;
        let wanted = stopped_at.clone().or_else(|| table.clone().map(NodeId::Table));
        let at = wanted.as_ref().and_then(|w| self.ui.sidebar.rows.iter().position(|r| r.is_node() && &r.node == w));
        match (at, table) {
            (Some(at), _) => {
                self.set_cursor(at);
            }
            // Listed, and not there: say so, on the schema that should hold it.
            (None, Some(table)) => {
                let parent = NodeId::Table(table.clone()).parent();
                if let Some(at) =
                    parent.and_then(|p| self.ui.sidebar.rows.iter().position(|r| r.is_node() && r.node == p))
                {
                    self.set_cursor(at);
                }
                let key = self.ui.keys.key_for(Action::Refresh).unwrap_or_default();
                self.say(
                    format!(
                        "{} is not in the list the sidebar loaded. Reload it with {key}.",
                        display_safe(&table.table)
                    ),
                    true,
                );
            }
            (None, None) => {}
        }
        fx.repaint = true;
    }
}
