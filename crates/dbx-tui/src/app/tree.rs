//! The schema tree's data: which node is which (`NodeId`), what the Core said
//! about each (`SchemaCache`), and the flat list of visible rows built from
//! both (`flatten`). No drawing and no I/O.
//!
//! The tree is connection, database, schema, table or view, column. A node is
//! addressed by its path (`NodeId`), never by position, so a reply can find its
//! node however the list has moved since the call was made.

use std::collections::{HashMap, HashSet};

use crate::core::{
    ColumnInfo, Connection, ConnectionId, CoreError, DatabasePath, DbKind, SchemaPath, Table, TableKind, TablePath,
};

/// One node of the tree that can have children, addressed by its path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeId {
    Connection(ConnectionId),
    Database(DatabasePath),
    Schema(SchemaPath),
    Table(TablePath),
}

impl NodeId {
    /// The connection this node is under.
    pub fn connection(&self) -> &ConnectionId {
        match self {
            NodeId::Connection(id) => id,
            NodeId::Database(d) => &d.connection,
            NodeId::Schema(s) => &s.database.connection,
            NodeId::Table(t) => &t.schema.database.connection,
        }
    }

    /// The database this node is in. A connection is in none (the default
    /// database is the connection's business, not the tree's).
    pub fn database(&self) -> Option<&str> {
        match self {
            NodeId::Connection(_) => None,
            NodeId::Database(d) => Some(&d.database),
            NodeId::Schema(s) => Some(&s.database.database),
            NodeId::Table(t) => Some(&t.schema.database.database),
        }
    }

    pub fn parent(&self) -> Option<NodeId> {
        match self {
            NodeId::Connection(_) => None,
            NodeId::Database(d) => Some(NodeId::Connection(d.connection.clone())),
            NodeId::Schema(s) => Some(NodeId::Database(s.database.clone())),
            NodeId::Table(t) => Some(NodeId::Schema(t.schema.clone())),
        }
    }

    /// Whether `self` is `ancestor` or somewhere below it. Compares paths
    /// without building any, so it is cheap to ask of every cached node.
    pub fn is_at_or_below(&self, ancestor: &NodeId) -> bool {
        match ancestor {
            NodeId::Connection(c) => self.connection() == c,
            NodeId::Database(d) => match self {
                NodeId::Database(x) => x == d,
                NodeId::Schema(s) => &s.database == d,
                NodeId::Table(t) => &t.schema.database == d,
                NodeId::Connection(_) => false,
            },
            NodeId::Schema(s) => match self {
                NodeId::Schema(x) => x == s,
                NodeId::Table(t) => &t.schema == s,
                _ => false,
            },
            NodeId::Table(t) => matches!(self, NodeId::Table(x) if x == t),
        }
    }

    /// Whether `self` is somewhere below `ancestor` (and not `ancestor` itself).
    pub fn is_below(&self, ancestor: &NodeId) -> bool {
        self != ancestor && self.is_at_or_below(ancestor)
    }

    /// The database this node is, or is in: `None` for a connection.
    pub fn database_path(&self) -> Option<&DatabasePath> {
        match self {
            NodeId::Connection(_) => None,
            NodeId::Database(d) => Some(d),
            NodeId::Schema(s) => Some(&s.database),
            NodeId::Table(t) => Some(&t.schema.database),
        }
    }
}

/// What a listing of one node's children came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Databases(Vec<String>),
    Schemas(Vec<String>),
    Tables(Vec<Table>),
    Columns(Vec<ColumnInfo>),
}

impl Listing {
    /// Whether this is the kind of listing `node` has.
    pub fn fits(&self, node: &NodeId) -> bool {
        matches!(
            (node, self),
            (NodeId::Connection(_), Listing::Databases(_))
                | (NodeId::Database(_), Listing::Schemas(_))
                | (NodeId::Schema(_), Listing::Tables(_))
                | (NodeId::Table(_), Listing::Columns(_))
        )
    }
}

/// The state of one node's children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Children {
    NotLoaded,
    Loading,
    Loaded(Listing),
    /// Asked again while the old listing is still held and still drawn: what
    /// is below it can be found, abandoned and pruned like any loaded node.
    Refreshing(Listing),
    Failed(CoreError),
}

impl Children {
    /// A listing is on its way (first time or again).
    pub fn is_working(&self) -> bool {
        matches!(self, Children::Loading | Children::Refreshing(_))
    }

    /// The listing held, if any (loaded, or kept while refreshing).
    pub fn listing(&self) -> Option<&Listing> {
        match self {
            Children::Loaded(l) | Children::Refreshing(l) => Some(l),
            _ => None,
        }
    }
}

/// Whether a call on a connection has worked, as the state glyph shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// The last call on it succeeded.
    Ok,
    /// The last call on it failed.
    Failed,
}

/// What the Core told us about the tree. A node with no entry is not loaded.
#[derive(Debug, Default)]
pub struct SchemaCache {
    nodes: HashMap<NodeId, Children>,
    health: HashMap<ConnectionId, Health>,
    /// Counts changes to what is loaded: what the semantic colours key by.
    version: u64,
}

static NOT_LOADED: Children = Children::NotLoaded;

impl SchemaCache {
    pub fn state(&self, node: &NodeId) -> &Children {
        self.nodes.get(node).unwrap_or(&NOT_LOADED)
    }

    /// Changes whenever a listing is stored or dropped.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn set(&mut self, node: NodeId, state: Children) {
        self.version += 1;
        match state {
            Children::NotLoaded => self.nodes.remove(&node),
            state => self.nodes.insert(node, state),
        };
    }

    pub fn health(&self, connection: &ConnectionId) -> Option<Health> {
        self.health.get(connection).copied()
    }

    pub fn set_health(&mut self, connection: &ConnectionId, health: Health) {
        self.health.insert(connection.clone(), health);
    }

    /// Every connection the cache holds something for.
    pub fn connections(&self) -> HashSet<ConnectionId> {
        self.nodes.keys().map(|n| n.connection().clone()).chain(self.health.keys().cloned()).collect()
    }

    /// How many nodes hold an entry (loading, loaded or failed).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The children of `node` that are nodes themselves, from its listing.
    pub fn child_nodes(&self, node: &NodeId) -> Vec<NodeId> {
        match (node, self.state(node).listing()) {
            (NodeId::Connection(c), Some(Listing::Databases(names))) => names
                .iter()
                .map(|n| NodeId::Database(DatabasePath { connection: c.clone(), database: n.clone() }))
                .collect(),
            (NodeId::Database(d), Some(Listing::Schemas(names))) => {
                names.iter().map(|n| NodeId::Schema(SchemaPath { database: d.clone(), schema: n.clone() })).collect()
            }
            (NodeId::Schema(s), Some(Listing::Tables(tables))) => tables
                .iter()
                .map(|t| NodeId::Table(TablePath { schema: s.clone(), table: t.name.clone() }))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Every cached node below `node`, at any depth.
    pub fn descendants(&self, node: &NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = self.child_nodes(node);
        while let Some(next) = stack.pop() {
            stack.extend(self.child_nodes(&next));
            out.push(next);
        }
        out
    }

    /// Forgets everything below `node` that its listing no longer names (a
    /// refresh found a table gone), and returns what it dropped.
    pub fn prune_missing_below(&mut self, node: &NodeId) -> Vec<NodeId> {
        let named: HashSet<NodeId> = self.descendants(node).into_iter().collect();
        let gone: Vec<NodeId> = self.nodes.keys().filter(|n| n.is_below(node) && !named.contains(*n)).cloned().collect();
        for n in &gone {
            self.nodes.remove(n);
        }
        self.version += 1;
        gone
    }

    /// Forgets every cached node at or below `node` (`node` too), and returns
    /// them. One pass over the cache, whatever is below.
    pub fn remove_at_or_below(&mut self, node: &NodeId) -> Vec<NodeId> {
        let gone: Vec<NodeId> = self.nodes.keys().filter(|n| n.is_at_or_below(node)).cloned().collect();
        for n in &gone {
            self.nodes.remove(n);
        }
        self.version += 1;
        gone
    }

    /// Every database that has anything cached at or below `node`: the ones
    /// the Core has opened server connections for.
    pub fn databases_at_or_below(&self, node: &NodeId) -> Vec<DatabasePath> {
        let mut out: Vec<DatabasePath> =
            self.nodes.keys().filter(|n| n.is_at_or_below(node)).filter_map(|n| n.database_path().cloned()).collect();
        out.sort_by(|a, b| a.database.cmp(&b.database));
        out.dedup();
        out
    }

    /// Whether any listing at or below `node` is on its way.
    pub fn any_working_at_or_below(&self, node: &NodeId) -> bool {
        self.nodes.iter().any(|(n, s)| s.is_working() && n.is_at_or_below(node))
    }

    /// Forgets every node and the health of `connection`.
    pub fn remove_connection(&mut self, connection: &ConnectionId) {
        self.version += 1;
        self.nodes.retain(|n, _| n.connection() != connection);
        self.health.remove(connection);
    }
}

// ---- rows -------------------------------------------------------------------

/// What a visible row is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Connection { name: String, kind: String, default_database: Option<String> },
    Database(String),
    Schema(String),
    Table { name: String, kind: TableKind },
    Column(ColumnInfo),
    /// A line of text under a node: loading, an error, or "empty".
    Note { text: String, error: bool },
}

/// One visible row of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub depth: u16,
    pub kind: RowKind,
    /// The node this row is, or belongs to (a column's table, a note's node).
    pub node: NodeId,
    pub expanded: bool,
}

impl Row {
    /// Whether the row is its node (and not something under it).
    pub fn is_node(&self) -> bool {
        matches!(self.kind, RowKind::Connection { .. } | RowKind::Database(_) | RowKind::Schema(_) | RowKind::Table { .. })
    }

    /// The row's name for the cursor to find again after a rebuild.
    pub fn key(&self) -> RowKey {
        let slot = match &self.kind {
            RowKind::Column(c) => Slot::Column(c.name.clone()),
            RowKind::Note { text, .. } => Slot::Note(text.clone()),
            _ => Slot::Node,
        };
        RowKey { node: self.node.clone(), slot }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Node,
    Column(String),
    Note(String),
}

/// Where a row is, independent of its position: what the cursor remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowKey {
    pub node: NodeId,
    pub slot: Slot,
}

/// The most lines an error takes under its node.
const ERROR_LINES: usize = 8;

/// Builds the visible rows: every connection, and under each expanded one its
/// loaded children, down as far as the expanded nodes go. `width` is the
/// sidebar's, so a long error can be wrapped under its node.
pub fn flatten(
    connections: &[Connection],
    cache: &SchemaCache,
    expanded: &HashSet<NodeId>,
    width: usize,
    retry_key: Option<&str>,
    wrap: impl Fn(&str, usize) -> Vec<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut ctx = Flatten { cache, expanded, width, retry_key, wrap: &wrap, rows: &mut rows };
    for c in connections {
        let node = NodeId::Connection(c.id.clone());
        let open = expanded.contains(&node);
        ctx.rows.push(Row {
            depth: 0,
            kind: RowKind::Connection { name: c.name.clone(), kind: c.kind.label().to_string(), default_database: c.database.clone() },
            node: node.clone(),
            expanded: open,
        });
        if open && c.kind != DbKind::Postgres {
            // Nothing is asked of dbx for these: it would answer "no schemas".
            let text = format!("Browsing {} is not supported yet.", c.kind.display_name());
            ctx.wrapped_note(&node, 1, &text, false);
        } else if open {
            ctx.children(&node, 1);
        }
    }
    rows
}

struct Flatten<'a, W: Fn(&str, usize) -> Vec<String>> {
    cache: &'a SchemaCache,
    expanded: &'a HashSet<NodeId>,
    width: usize,
    /// The key that reloads a node, for the line under a failure.
    retry_key: Option<&'a str>,
    wrap: &'a W,
    rows: &'a mut Vec<Row>,
}

impl<W: Fn(&str, usize) -> Vec<String>> Flatten<'_, W> {
    fn note(&mut self, node: &NodeId, depth: u16, text: &str, error: bool) {
        self.rows.push(Row { depth, kind: RowKind::Note { text: text.to_string(), error }, node: node.clone(), expanded: false });
    }

    /// `text` wrapped to the room under `node`, at most `ERROR_LINES` lines.
    fn wrapped_note(&mut self, node: &NodeId, depth: u16, text: &str, error: bool) {
        // Indent and arrow column, then the text.
        let room = self.width.saturating_sub(2 + 2 * depth as usize + 2).max(8);
        let mut lines = (self.wrap)(text, room);
        if lines.len() > ERROR_LINES {
            lines.truncate(ERROR_LINES);
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }
        for line in lines {
            self.note(node, depth, &line, error);
        }
    }

    fn children(&mut self, node: &NodeId, depth: u16) {
        match self.cache.state(node) {
            Children::NotLoaded => {}
            Children::Loading => self.note(node, depth, "loading…", false),
            Children::Failed(e) => {
                self.wrapped_note(node, depth, &e.message, true);
                // A failure says what to do next.
                if let Some(key) = self.retry_key {
                    self.note(node, depth, &format!("press {key} to retry"), false);
                }
            }
            Children::Loaded(listing) | Children::Refreshing(listing) => self.listed(node, listing, depth),
        }
    }

    fn listed(&mut self, node: &NodeId, listing: &Listing, depth: u16) {
        let (empty, none_found) = match listing {
            Listing::Databases(v) => (v.is_empty(), "(no databases)"),
            Listing::Schemas(v) => (v.is_empty(), "(no schemas)"),
            Listing::Tables(v) => (v.is_empty(), "(empty)"),
            Listing::Columns(v) => (v.is_empty(), "(no columns)"),
        };
        if empty {
            return self.note(node, depth, none_found, false);
        }
        match (node, listing) {
            (NodeId::Connection(c), Listing::Databases(names)) => {
                for name in names {
                    let child = NodeId::Database(DatabasePath { connection: c.clone(), database: name.clone() });
                    self.node_row(child, RowKind::Database(name.clone()), depth);
                }
            }
            (NodeId::Database(d), Listing::Schemas(names)) => {
                for name in names {
                    let child = NodeId::Schema(SchemaPath { database: d.clone(), schema: name.clone() });
                    self.node_row(child, RowKind::Schema(name.clone()), depth);
                }
            }
            (NodeId::Schema(s), Listing::Tables(tables)) => {
                for t in tables {
                    let child = NodeId::Table(TablePath { schema: s.clone(), table: t.name.clone() });
                    self.node_row(child, RowKind::Table { name: t.name.clone(), kind: t.kind }, depth);
                }
            }
            (NodeId::Table(_), Listing::Columns(columns)) => {
                for c in columns {
                    self.rows.push(Row { depth, kind: RowKind::Column(c.clone()), node: node.clone(), expanded: false });
                }
            }
            _ => {}
        }
    }

    fn node_row(&mut self, node: NodeId, kind: RowKind, depth: u16) {
        let open = self.expanded.contains(&node);
        self.rows.push(Row { depth, kind, node: node.clone(), expanded: open });
        if open {
            self.children(&node, depth + 1);
        }
    }
}
