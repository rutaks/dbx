//! The sidebar's tree: what keys, clicks and the Core's replies do to it.
//!
//! The visible rows are a flat list rebuilt from the connection list, the
//! schema cache and the set of expanded nodes, and only when one of those
//! changes (`touch_rows`). The cursor is an index into that list, and also
//! remembers *which* row it is on (`cursor_key`), so a reply that inserts rows
//! above it does not move it onto another row.

use std::collections::HashSet;
use std::time::Instant;

use super::keys::Action;
use super::pointer::{self, ClickKey, Drag};
use super::requests::Target;
use super::tree::{self, Children, Health, Level, Listing, NodeId, Row, RowKey, RowKind};
use super::view;
use super::{App, Command, CoreCall, Effects, Loadable, Mode, SidebarNote};
use crate::core::{Connection, ConnectionId, CoreError, CoreErrorKind};

/// The sidebar's cursor, scroll position, expanded nodes and visible rows.
#[derive(Debug)]
pub struct SidebarUi {
    /// Index of the cursor row in `rows`.
    pub cursor: usize,
    /// The first row on screen. `view` keeps the cursor inside the window.
    pub scroll: usize,
    /// The nodes the user has opened.
    pub expanded: HashSet<NodeId>,
    /// The visible rows, as last built.
    pub rows: Vec<Row>,
    /// Which row the cursor is on, to find it again after a rebuild.
    pub cursor_key: Option<RowKey>,
    /// `rows` is out of date.
    pub dirty: bool,
    /// The sidebar width the rows were built for (error text is wrapped to it).
    pub built_width: u16,
    /// The retry key the rows name under a failure, as built.
    pub built_retry_key: Option<String>,
    /// How many times `rows` has been rebuilt: tests watch that it is only when
    /// the tree changes.
    pub rebuilds: usize,
}

impl Default for SidebarUi {
    fn default() -> Self {
        SidebarUi {
            cursor: 0,
            scroll: 0,
            expanded: HashSet::new(),
            rows: Vec::new(),
            cursor_key: None,
            dirty: true,
            built_width: 0,
            built_retry_key: None,
            rebuilds: 0,
        }
    }
}

/// Columns a row uses before its label: the margin, then two per level.
pub const MARGIN: u16 = 1;

impl App {
    /// The tree changed: rebuild the rows before they are next read.
    pub(super) fn touch_rows(&mut self) {
        self.ui.sidebar.dirty = true;
    }

    /// Rebuilds the visible rows if the tree (or the sidebar's width) changed,
    /// and keeps the cursor on the row it was on.
    pub fn ensure_rows(&mut self) {
        let width = match self.ui.hits.sidebar.width {
            0 => 26,
            w => w,
        };
        let retry_key = self.ui.keys.key_for(Action::Refresh);
        let sb = &mut self.ui.sidebar;
        if sb.dirty || sb.built_width != width || sb.built_retry_key != retry_key {
            // The rows are replaced: a press on one of the old ones pairs with nothing.
            self.ui.clicks.forget();
            let sb = &mut self.ui.sidebar;
            let before = sb.cursor_key.clone().or_else(|| sb.rows.get(sb.cursor).map(Row::key));
            let connections: &[Connection] = match &self.model.connections {
                Loadable::Ready(list) => list,
                _ => &[],
            };
            let rows =
                tree::flatten(connections, &self.model.schema, &sb.expanded, width as usize, retry_key.as_deref(), view::wrap);
            let found = before.as_ref().and_then(|key| {
                if let Some(at) = rows.iter().position(|r| &r.key() == key) {
                    return Some(at);
                }
                // The row is gone: the nearest ancestor still shown, not
                // whatever row now sits at the same position.
                let mut node = Some(key.node.clone());
                while let Some(n) = node {
                    if let Some(at) = rows.iter().position(|r| r.is_node() && r.node == n) {
                        return Some(at);
                    }
                    node = n.parent();
                }
                None
            });
            sb.cursor = found.unwrap_or(sb.cursor).min(rows.len().saturating_sub(1));
            sb.cursor_key = rows.get(sb.cursor).map(Row::key);
            sb.rows = rows;
            sb.dirty = false;
            sb.built_width = width;
            sb.built_retry_key = retry_key;
            sb.rebuilds += 1;
        }
    }

    /// The row under the cursor.
    pub(super) fn cursor_row(&self) -> Option<&Row> {
        self.ui.sidebar.rows.get(self.ui.sidebar.cursor)
    }

    /// The connection under the cursor, when the cursor is on a connection row.
    pub(super) fn selected_connection(&self) -> Option<&Connection> {
        let row = self.cursor_row()?;
        let RowKind::Connection { .. } = row.kind else { return None };
        match &self.model.connections {
            Loadable::Ready(list) => list.iter().find(|c| NodeId::Connection(c.id.clone()) == row.node),
            _ => None,
        }
    }

    /// The saved connection `id` names, if it is still in the list.
    pub(super) fn connection(&self, id: &ConnectionId) -> Option<&Connection> {
        match &self.model.connections {
            Loadable::Ready(list) => list.iter().find(|c| &c.id == id),
            _ => None,
        }
    }

    /// Puts the cursor on row `index` (clamped). True if it moved.
    pub(super) fn set_cursor(&mut self, index: usize) -> bool {
        let sb = &mut self.ui.sidebar;
        let index = index.min(sb.rows.len().saturating_sub(1));
        sb.cursor_key = sb.rows.get(index).map(Row::key);
        let moved = index != sb.cursor;
        sb.cursor = index;
        moved
    }

    // ---- keys ------------------------------------------------------------------

    /// The sidebar takes its keys directly: it accepts no text. Which key does
    /// what comes from the live keybindings.
    pub(super) fn on_sidebar_key(&mut self, combo: super::keys::KeyCombo, fx: &mut Effects) {
        // The user took over: a reveal on its way no longer moves the cursor.
        self.cancel_reveal();
        if self.ui.note.take().is_some() {
            fx.repaint = true;
        }
        let Some(action) = self.ui.keys.lookup_sidebar(&combo) else { return };
        self.perform_sidebar(action, fx);
    }

    pub(super) fn perform_sidebar(&mut self, action: Action, fx: &mut Effects) {
        self.ensure_rows();
        let (cursor, last) = (self.ui.sidebar.cursor, self.ui.sidebar.rows.len().saturating_sub(1));
        let page = (self.ui.hits.sidebar_list.height as usize).saturating_sub(1).max(1);
        let target = match action {
            Action::MoveDown => Some((cursor + 1).min(last)),
            Action::MoveUp => Some(cursor.saturating_sub(1)),
            Action::PageDown => Some((cursor + page).min(last)),
            Action::PageUp => Some(cursor.saturating_sub(page)),
            Action::First => Some(0),
            Action::Last => Some(last),
            _ => None,
        };
        if let Some(target) = target {
            if self.set_cursor(target) {
                fx.repaint = true;
            }
            return;
        }
        match action {
            Action::Expand => self.expand_at_cursor(fx),
            Action::Collapse => self.collapse_at_cursor(fx),
            Action::Refresh => self.refresh_at_cursor(fx),
            Action::EditConnection => self.open_edit_form(fx),
            Action::RemoveConnection => self.open_remove_confirm(fx),
            Action::UseTarget => self.use_target(fx),
            Action::OpenTable => self.open_table_at_cursor(fx),
            Action::Dump => self.open_dump(fx),
            _ => {}
        }
    }

    fn expand_at_cursor(&mut self, fx: &mut Effects) {
        let Some(row) = self.cursor_row().cloned() else { return };
        if !row.is_node() {
            return;
        }
        if !row.expanded {
            return self.expand(row.node, fx);
        }
        // Already open: go in, if there is anything to go to.
        let next = self.ui.sidebar.cursor + 1;
        if self.ui.sidebar.rows.get(next).is_some_and(|r| r.depth > row.depth) && self.set_cursor(next) {
            fx.repaint = true;
        }
    }

    fn collapse_at_cursor(&mut self, fx: &mut Effects) {
        let Some(row) = self.cursor_row().cloned() else { return };
        if row.is_node() && row.expanded {
            return self.collapse(&row.node, fx);
        }
        // Not open (or not a node): the parent is where `h` goes.
        let parent = if row.is_node() { row.node.parent() } else { Some(row.node.clone()) };
        let at = parent.and_then(|p| self.ui.sidebar.rows.iter().position(|r| r.is_node() && r.node == p));
        if let Some(at) = at {
            if self.set_cursor(at) {
                fx.repaint = true;
            }
        }
    }

    fn refresh_at_cursor(&mut self, fx: &mut Effects) {
        let Some(row) = self.cursor_row() else { return };
        let node = row.node.clone();
        // One listing at a time: a refresh while it loads would only repeat it.
        if self.model.schema.state(&node).is_working() {
            return;
        }
        if !self.ui.sidebar.expanded.contains(&node) {
            // Closed: nothing is on screen to refresh. Drop what it held, so
            // that opening it asks again, and ask nothing now.
            return self.drop_cache(&node, fx);
        }
        self.load(node, fx);
    }

    /// Opens a node, and asks the Core for its children unless they are known
    /// or already being fetched.
    pub(super) fn expand(&mut self, node: NodeId, fx: &mut Effects) {
        self.ui.sidebar.expanded.insert(node.clone());
        if matches!(self.model.schema.state(&node), Children::NotLoaded | Children::Failed(_)) {
            self.load(node, fx);
        }
        self.touch_rows();
        fx.repaint = true;
    }

    /// What `node`'s children are, by the shape of its connection: a level
    /// the connection does not have is skipped. A connection not (yet) in the
    /// list is taken to be shaped like PostgreSQL.
    fn level_below(&self, node: &NodeId) -> Level {
        let (databases, schemas) = self.connection(node.connection()).map_or((true, true), |c| (c.capabilities.databases, c.capabilities.schemas));
        match node {
            NodeId::Connection(_) if databases => Level::Databases,
            NodeId::Connection(_) | NodeId::Database(_) if schemas => Level::Schemas,
            NodeId::Connection(_) | NodeId::Database(_) | NodeId::Schema(_) => Level::Tables,
            NodeId::Table(_) => Level::Columns,
        }
    }

    /// Asks the Core for `node`'s children, replacing any listing in flight.
    /// A node that already has a listing keeps it (and keeps drawing it)
    /// until the new one arrives.
    fn load(&mut self, node: NodeId, fx: &mut Effects) {
        // dbx has nothing to say about these kinds yet; the tree says so itself.
        if self.browsing_unsupported(&node) {
            return;
        }
        // A refresh makes what completion kept for this part of the tree stale.
        if self.model.schema.state(&node).listing().is_some() {
            self.invalidate_completions(&node, fx);
        }
        let target = Target::Children(node.clone());
        for old in self.model.requests.forget(&target) {
            fx.commands.push(Command::Abandon(old));
        }
        let id = self.model.requests.begin(target);
        let next = match self.model.schema.state(&node).listing() {
            Some(listing) => Children::Refreshing(listing.clone()),
            None => Children::Loading,
        };
        self.model.schema.set(node.clone(), next);
        let level = self.level_below(&node);
        fx.commands.push(Command::Core(id, CoreCall::List(node, level)));
        self.touch_rows();
        fx.repaint = true;
    }

    /// Whether `node` is a connection of a kind the tree cannot browse yet.
    fn browsing_unsupported(&self, node: &NodeId) -> bool {
        matches!(node, NodeId::Connection(c) if self.connection(c).is_some_and(|k| !k.capabilities.browse))
    }

    /// Lets go of everything at or below `node` that is in flight, and the
    /// requests' bookkeeping with it.
    fn abandon_at_or_below(&mut self, node: &NodeId, fx: &mut Effects) {
        let gone = self.model.requests.forget_where(|t| matches!(t, Target::Children(n) if n.is_at_or_below(node)));
        fx.commands.extend(gone.into_iter().map(Command::Abandon));
    }

    /// Asks the Core to drop the server connections of every database at or
    /// below `node` that the tree opened. `keep_default` spares the one the
    /// connection was saved with: it is where the connection starts, and what
    /// its row makes active.
    fn release_below(&mut self, node: &NodeId, keep_default: bool, fx: &mut Effects) {
        let default = match (keep_default, self.connection(node.connection())) {
            (true, Some(c)) => c.database.clone(),
            _ => None,
        };
        for at in self.model.schema.databases_at_or_below(node) {
            if default.as_deref() != Some(at.database.as_str()) {
                fx.commands.push(Command::Release(at));
            }
        }
    }

    /// Forgets what `node` and everything below it hold: the cache, the
    /// opened state of what is below, the requests, the server connections.
    /// `node` itself keeps its opened state.
    fn drop_cache(&mut self, node: &NodeId, fx: &mut Effects) {
        self.abandon_at_or_below(node, fx);
        self.release_below(node, true, fx);
        self.invalidate_completions(node, fx);
        self.model.schema.remove_at_or_below(node);
        self.ui.sidebar.expanded.retain(|n| !n.is_below(node));
        self.touch_rows();
        fx.repaint = true;
    }

    /// Closes a node. What it had loaded stays cached; a listing still in
    /// flight at or under it is abandoned (it would only fill a node nobody
    /// sees, and a refresh gives back the listing it was replacing), the
    /// request table lets go of everything below, and a database that closes
    /// lets go of its server connections.
    pub(super) fn collapse(&mut self, node: &NodeId, fx: &mut Effects) {
        if matches!(node, NodeId::Database(_) | NodeId::Connection(_)) {
            self.release_below(node, true, fx);
        }
        self.abandon_at_or_below(node, fx);
        let mut affected = self.model.schema.descendants(node);
        affected.push(node.clone());
        for n in affected {
            match self.model.schema.state(&n) {
                Children::Loading => {
                    self.model.schema.set(n.clone(), Children::NotLoaded);
                    if &n != node {
                        self.ui.sidebar.expanded.remove(&n);
                    }
                }
                Children::Refreshing(listing) => {
                    let listing = listing.clone();
                    self.model.schema.set(n, Children::Loaded(listing));
                }
                _ => {}
            }
        }
        self.ui.sidebar.expanded.remove(node);
        self.touch_rows();
        fx.repaint = true;
    }

    /// Toggles a node open or closed (a double-click, a click on its arrow).
    fn toggle(&mut self, row: usize, fx: &mut Effects) {
        let Some(r) = self.ui.sidebar.rows.get(row).cloned() else { return };
        if !r.is_node() {
            return;
        }
        if r.expanded {
            self.collapse(&r.node, fx);
        } else {
            self.expand(r.node, fx);
        }
    }

    // ---- replies and removals ---------------------------------------------------

    /// What a listing came back with, for a request that is still current.
    /// False when it was dropped and nothing changed (nothing to repaint).
    pub(super) fn on_listed(&mut self, node: NodeId, result: Result<Listing, CoreError>, fx: &mut Effects) -> bool {
        // Not a node of a connection we still have: nothing to put it in.
        if self.connection(node.connection()).is_none() {
            return false;
        }
        let connection = node.connection().clone();
        match result {
            Ok(listing) if listing.level() == self.level_below(&node) => {
                self.model.schema.set(node.clone(), Children::Loaded(listing));
                // A refresh can drop children; what was cached below them goes too.
                for gone in self.model.schema.prune_missing_below(&node) {
                    self.ui.sidebar.expanded.remove(&gone);
                    for old in self.model.requests.forget(&Target::Children(gone.clone())) {
                        fx.commands.push(Command::Abandon(old));
                    }
                    if let NodeId::Database(at) = gone {
                        fx.commands.push(Command::Release(at));
                    }
                }
                self.model.schema.set_health(&connection, Health::Ok);
            }
            // Not the level this node lists any more: nothing to show, and it
            // must not stay loading.
            Ok(_) => {
                self.model.schema.set(node.clone(), Children::NotLoaded);
                self.ui.sidebar.expanded.remove(&node);
            }
            // Abandoned on purpose: the node is simply not loaded again.
            Err(e) if e.kind == CoreErrorKind::Cancelled => {
                let next = match self.model.schema.state(&node).listing() {
                    Some(listing) => Children::Loaded(listing.clone()),
                    None => Children::NotLoaded,
                };
                if matches!(next, Children::NotLoaded) {
                    self.ui.sidebar.expanded.remove(&node);
                }
                self.model.schema.set(node.clone(), next);
            }
            Err(e) => {
                // What it showed is no longer shown, so what was cached below
                // it goes: nothing can reach it to close it.
                self.drop_cache(&node, fx);
                self.model.schema.set(node.clone(), Children::Failed(e));
                self.model.schema.set_health(&connection, Health::Failed);
            }
        }
        self.touch_rows();
        true
    }

    /// A connection is gone, or was edited so that what was loaded from it may
    /// no longer be true: forget its tree, and stop waiting for its listings.
    pub(super) fn forget_connection(&mut self, id: &ConnectionId, fx: &mut Effects) {
        let node = NodeId::Connection(id.clone());
        if self.ui.reveal.as_ref().is_some_and(|r| &r.path.schema.database.connection == id) {
            self.ui.reveal = None;
        }
        self.release_below(&node, false, fx);
        self.invalidate_completions(&node, fx);
        self.model.schema.remove_connection(id);
        self.ui.sidebar.expanded.retain(|n| n.connection() != id);
        for old in self.model.requests.forget_where(|t| matches!(t, Target::Children(n) if n.connection() == id)) {
            fx.commands.push(Command::Abandon(old));
        }
        self.touch_rows();
        fx.repaint = true;
    }

    // ---- the mouse ----------------------------------------------------------------

    /// Moves the sidebar's window by `rows`, and the cursor with it when it
    /// would leave the window, so the cursor row always stays on screen.
    pub(super) fn scroll_sidebar(&mut self, rows: isize, fx: &mut Effects) {
        self.ensure_rows();
        let room = self.ui.hits.sidebar_list.height.max(1) as usize;
        let len = self.ui.sidebar.rows.len();
        let sb = &mut self.ui.sidebar;
        let max = len.saturating_sub(room);
        let scroll = sb.scroll.saturating_add_signed(rows).min(max);
        let cursor = sb.cursor.clamp(scroll, scroll + room - 1).min(len.saturating_sub(1));
        let moved = scroll != sb.scroll;
        sb.scroll = scroll;
        if self.set_cursor(cursor) || moved {
            fx.repaint = true;
        }
    }

    /// A left click in the list at screen row `y`, column `x`.
    pub(super) fn click_sidebar_row(&mut self, x: u16, y: u16, now: Instant, fx: &mut Effects) {
        self.ensure_rows();
        let list = self.ui.hits.sidebar_list;
        let row = self.ui.sidebar.scroll + (y - list.y) as usize;
        let Some(r) = self.ui.sidebar.rows.get(row).cloned() else { return };
        if self.set_cursor(row) {
            fx.repaint = true;
        }
        // The arrow (and the space after it) toggles on one click.
        let arrow = list.x + MARGIN + 2 * r.depth;
        if r.is_node() && x >= arrow && x < arrow + 2 {
            self.ui.clicks.forget();
            return self.toggle(row, fx);
        }
        // A second click on the row, soon after, toggles it.
        if self.ui.clicks.press(now, ClickKey::SidebarRow(row)) {
            self.toggle(row, fx);
        }
    }

    /// A press on the sidebar's scrollbar column at screen row `y`: the list
    /// jumps there (the thumb is centred on the press), or, on the thumb, it is
    /// grabbed where it was pressed. Returns the drag it begins.
    pub(super) fn press_sidebar_bar(&mut self, y: u16, fx: &mut Effects) -> Option<Drag> {
        let bar = self.ui.hits.sidebar_bar;
        let thumb = view::scrollbar_thumb(self.ui.sidebar.rows.len(), bar.height as usize, self.ui.sidebar.scroll)?;
        let (grab, _) = pointer::grab(thumb, y.saturating_sub(bar.y) as usize);
        self.drag_sidebar_bar(y, grab as u16, fx);
        Some(Drag::SidebarBar { grab: grab as u16 })
    }

    /// The pointer is at screen row `y` with the thumb grabbed `grab` rows
    /// below its top: scroll so the thumb is there.
    pub(super) fn drag_sidebar_bar(&mut self, y: u16, grab: u16, fx: &mut Effects) {
        let bar = self.ui.hits.sidebar_bar;
        let (total, room, scroll) = (self.ui.sidebar.rows.len(), bar.height as usize, self.ui.sidebar.scroll);
        let Some((_, len)) = view::scrollbar_thumb(total, room, scroll) else { return };
        let thumb_top = (y.saturating_sub(bar.y) as usize).saturating_sub(grab as usize);
        let to = pointer::scroll_for_thumb(total, room, len, thumb_top);
        self.scroll_sidebar(to as isize - scroll as isize, fx);
    }

    /// A click on a mode-bar hint does what its key does.
    pub(super) fn click_hint(&mut self, action: Action, now: Instant, fx: &mut Effects) {
        match action {
            Action::OpenPrefix => {
                self.ui.mode = Mode::Prefix { expires: now + self.ui.prefix_timeout };
                fx.repaint = true;
            }
            // The mode bar's dump status: the dump's progress.
            Action::Dump if self.model.dump.is_some() => self.open_dump_progress(fx),
            a if a.scope() == super::keys::Scope::Results => self.perform_results(a, now, fx),
            a if a.scope() == super::keys::Scope::Resize => self.perform_resize(a, fx),
            a if a.scope() == super::keys::Scope::Complete => self.perform_complete(a, fx),
            a if a.scope() == super::keys::Scope::Template => self.perform_template(a, fx),
            a if a.scope() != super::keys::Scope::Sidebar => self.perform(a, now, fx),
            a => self.perform_sidebar(a, fx),
        }
    }

    /// Whether the connection under the cursor is one the form can edit.
    pub(super) fn cursor_on_editable(&self) -> bool {
        self.selected_connection().is_some_and(|c| c.capabilities.edit_in_form)
    }

    /// A short note under the sidebar: used by edit for kinds the form does not handle.
    pub(super) fn say(&mut self, text: String, failure: bool) {
        self.ui.note = Some(SidebarNote { text, failure });
    }
}
