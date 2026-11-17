//! The Core seam: everything the client asks of dbx, as one collaborator.
//!
//! Only `dbx.rs` (the `DbxCore` adapter) may name a dbx type. Everything else
//! speaks the types defined here. `FakeCore` is the test double.

mod dbx;
mod fake;

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;

pub use dbx::{DbxCore, KeySource, StoreOptions};
#[cfg(feature = "test-support")]
pub use dbx::check_seed_dir;
pub use fake::{Call, FakeCore, Scripted};

// ---- identifiers -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub String);

/// One execution of one statement (glossary: Run).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

/// A query tab. Each tab owns a database session (design section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

// ---- connections -----------------------------------------------------------

/// The kind of database server a connection reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbKind {
    Postgres,
    MySql,
    Sqlite,
    /// Any other kind dbx supports, by dbx's name for it.
    Other(String),
}

impl DbKind {
    /// The short name the sidebar shows.
    pub fn label(&self) -> &str {
        match self {
            DbKind::Postgres => "postgres",
            DbKind::MySql => "mysql",
            DbKind::Sqlite => "sqlite",
            DbKind::Other(name) => name,
        }
    }
}

/// A saved, named way to reach one database server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: ConnectionId,
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: Option<u16>,
    /// The database the connection opens by default, if it names one.
    pub database: Option<String>,
    /// dbx marks the connection as production: writes need confirming.
    pub production: bool,
}

/// What the connection form edits. `id` is `None` for a new connection.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionDraft {
    pub id: Option<ConnectionId>,
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: String,
    pub password: Option<String>,
    pub production: bool,
}

impl fmt::Debug for ConnectionDraft {
    // The password never reaches a log or a failed assertion.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionDraft")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("production", &self.production)
            .finish()
    }
}

// ---- schema ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DatabasePath {
    pub connection: ConnectionId,
    pub database: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemaPath {
    pub database: DatabasePath,
    pub schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TablePath {
    pub schema: SchemaPath,
    pub table: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
}

/// A column of a table, as the schema tree shows it. Not to be confused with
/// `Column`, which describes a column of a result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub db_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
}

// ---- runs and results ------------------------------------------------------

/// A position in the editor buffer, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// Run what the user is pointing at: the selection if there is one, otherwise
/// the statement under the cursor. The Core picks the statement (design
/// section 11), so the request carries the whole buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub connection: ConnectionId,
    pub database: Option<String>,
    pub tab: TabId,
    pub buffer: String,
    pub cursor: Cursor,
    pub selection: Option<(Cursor, Cursor)>,
    pub row_cap: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Integer,
    Decimal,
    Float,
    Bool,
    Text,
    Temporal,
    Json,
    Binary,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub db_type: String,
    pub kind: CellKind,
}

/// NULL, or the exact text the database gave. Numbers stay text so nothing
/// above the seam can lose precision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Null,
    Value(String),
}

/// The rows and columns a run returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// The row cap cut the result short. There is no total count.
    pub truncated: bool,
    pub elapsed: Duration,
    pub messages: Vec<String>,
    /// The statement text that actually ran.
    pub ran: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryQuery {
    pub text: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub connection: ConnectionId,
    pub sql: String,
    pub at_unix: i64,
}

// ---- errors ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreErrorKind {
    Cancelled,
    Timeout,
    Connect,
    Sql,
    Store,
    Key,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub kind: CoreErrorKind,
    pub message: String,
}

impl CoreError {
    pub fn new(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

// ---- the seam --------------------------------------------------------------

/// Everything the client asks of dbx (design sections 2 and 10).
#[async_trait]
pub trait Core: Send + Sync {
    async fn connections(&self) -> Result<Vec<Connection>, CoreError>;
    async fn save_connection(&self, draft: &ConnectionDraft) -> Result<Connection, CoreError>;
    async fn remove_connection(&self, id: &ConnectionId) -> Result<(), CoreError>;
    async fn test_connection(&self, draft: &ConnectionDraft) -> Result<(), CoreError>;

    async fn databases(&self, id: &ConnectionId) -> Result<Vec<String>, CoreError>;
    async fn schemas(&self, at: &DatabasePath) -> Result<Vec<String>, CoreError>;
    async fn tables(&self, at: &SchemaPath) -> Result<Vec<Table>, CoreError>;
    async fn columns(&self, at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError>;

    /// Runs until done, failed, or `cancel(run)` is called. After a cancel it
    /// returns `CoreErrorKind::Cancelled` promptly.
    async fn run(&self, run: RunId, request: RunRequest) -> Result<ResultSet, CoreError>;
    fn cancel(&self, run: RunId);
    /// Ends the database session a query tab owns.
    async fn close_session(&self, tab: TabId) -> Result<(), CoreError>;

    async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, CoreError>;
}
