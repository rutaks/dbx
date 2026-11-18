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
pub use fake::{Call, FakeCore, Scripted, SyntaxCalls};

// ---- identifiers -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub String);

/// One execution of one statement (glossary: Run).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

/// A query tab. Each tab owns a database session (design section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// One database session of a query tab: dbx's `client_session_id`. A tab
/// starts a new session, with the next `serial`, whenever it is given another
/// target or its session is dropped. A session is therefore never reopened
/// once its close has been asked for, and a close that lands late (it waits
/// for the tab's run to end first) cannot end a session opened after it: a
/// tab given database A, then B, then A again runs on A in a session other
/// than the one being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId {
    pub tab: TabId,
    pub serial: u32,
}

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
    /// The kind as a person writes it.
    pub fn display_name(&self) -> &str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL",
            DbKind::Sqlite => "SQLite",
            DbKind::Other(name) => name,
        }
    }

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
    pub username: String,
    /// TLS on or off, as the form shows it.
    pub tls: bool,
    /// dbx marks the connection as production: writes need confirming.
    pub production: bool,
    /// The connection's own query timeout in seconds, when it sets one: the
    /// longest a run may take before it is cancelled. `None` is no limit.
    pub query_timeout: Option<u64>,
}

/// `s` made safe to draw: control characters (which a terminal would act on,
/// as escape sequences) become `�`, tabs and line breaks become spaces, and
/// the bidirectional overrides that reorder what is shown are dropped. Names,
/// hosts and server messages come from other programs and other people.
pub fn display_safe(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// The connection list in the order the sidebar shows it: by name without
/// regard to case, then by id.
pub fn sort_connections(list: &mut [Connection]) {
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
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
    /// `None` keeps the stored password of the connection `id` names (and is
    /// an empty password for a new one). The form never loads the stored one.
    pub password: Option<String>,
    pub tls: bool,
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
            .field("tls", &self.tls)
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
    MaterializedView,
    /// A table whose rows live in another server.
    Foreign,
}

impl TableKind {
    /// What the tree says next to a name that is not a plain table, or `None`
    /// for a plain table (a table is the default and needs no label).
    pub fn label(self) -> Option<&'static str> {
        match self {
            TableKind::Table => None,
            TableKind::View => Some("view"),
            TableKind::MaterializedView => Some("matview"),
            TableKind::Foreign => Some("foreign"),
        }
    }
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
    /// The tab's session the statement runs in.
    pub session: SessionId,
    pub buffer: String,
    pub cursor: Cursor,
    pub selection: Option<(Cursor, Cursor)>,
    pub row_cap: usize,
}

/// How far into `buffer` the position `at` is, in UTF-16 code units: the unit
/// dbx's statement finder counts in. Rows are split at `\n`; a position past
/// the end of its row or of the buffer is clamped to that end.
pub fn utf16_offset(buffer: &str, at: Cursor) -> usize {
    let byte = byte_offset(buffer, at);
    buffer[..byte].encode_utf16().count()
}

/// The text between two positions of `buffer`, in either order.
pub fn text_between(buffer: &str, a: Cursor, b: Cursor) -> String {
    let (from, to) = if (a.row, a.col) <= (b.row, b.col) { (a, b) } else { (b, a) };
    buffer[byte_offset(buffer, from)..byte_offset(buffer, to)].to_string()
}

/// The byte index of a position (row, then character within the row),
/// clamped to the end of its row and of the buffer.
fn byte_offset(buffer: &str, at: Cursor) -> usize {
    let mut start = 0;
    for (row, line) in buffer.split('\n').enumerate() {
        if row == at.row {
            let within = line.char_indices().nth(at.col).map_or(line.len(), |(i, _)| i);
            return start + within;
        }
        start += line.len() + 1;
    }
    buffer.len()
}

/// Where the statement `text` (what dbx's statement finder returned for
/// `cursor`) is in `buffer`: its first and last position, the end exclusive.
/// dbx returns only the text, so it is searched for. When it occurs once that
/// is the place; when it occurs several times the one the caret is in or at
/// the edge of; when there is no such copy, or none at all, `None`.
pub fn locate_statement(buffer: &str, cursor: Cursor, text: &str) -> Option<(Cursor, Cursor)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Every place the text occurs (copies may overlap).
    let mut found: Vec<usize> = Vec::new();
    let mut from = 0;
    while let Some(i) = buffer[from..].find(text) {
        found.push(from + i);
        from += i + buffer[from + i..].chars().next().map_or(1, char::len_utf8);
    }
    let at = byte_offset(buffer, cursor);
    let start = match found.as_slice() {
        [] => return None,
        [only] => *only,
        many => {
            let mut holding = many.iter().filter(|&&s| s <= at && at <= s + text.len());
            match (holding.next(), holding.next()) {
                (Some(&s), None) => s,
                _ => return None,
            }
        }
    };
    Some((cursor_at(buffer, start), cursor_at(buffer, start + text.len())))
}

/// The position of byte `index` of `buffer`: its row, and the character
/// within the row.
fn cursor_at(buffer: &str, index: usize) -> Cursor {
    let before = &buffer[..index];
    let row = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    Cursor { row, col: before[line_start..].chars().count() }
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
    /// What the server said while running it (notices, warnings), one line
    /// each, as `SEVERITY: message`.
    pub messages: Vec<String>,
    /// Rows changed, for a statement that returns no result set (`columns`
    /// is empty).
    pub affected: u64,
    /// The statement text that actually ran.
    pub ran: String,
    /// Whether running `ran` again is safe. "Load more" re-runs the
    /// statement, so it needs this.
    pub rerun: Rerun,
}

/// What is known about running a statement a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rerun {
    /// A `SELECT`, `WITH ... SELECT`, `VALUES` or `TABLE` that dbx proved
    /// read-only (`prove_read_only_for_database`): no function in it can
    /// change data.
    Proven,
    /// Read-only as far as dbx's denylist can see, but not proven: it calls a
    /// function (`select f()`, `select pg_notify(...)`) that may change data.
    /// Repeating it needs the user's say-so.
    ReadOnlyUnproven,
    /// A write, or anything that is not a query. Never repeated.
    Write,
}

/// The rows a run fetches unless told otherwise (design section 5).
pub const DEFAULT_ROW_CAP: usize = 1_000;

/// The most rows "load more" will fetch. dbx builds the whole result in
/// memory as JSON values before we turn it into text, so every row costs
/// twice for a moment; 100,000 rows keeps that to tens of megabytes for a
/// normal table and is already more than anyone reads in a grid. Past it the
/// answer is a narrower query.
pub const MAX_ROW_CAP: usize = 100_000;

/// The most cells (rows times columns) "load more" will hold. Measured on the
/// million-row fixture (3 columns), a result costs about 120 bytes per cell
/// while it is held, so 2,000,000 cells is about 240 MB at the very most, and
/// a 120-column result stops near 16,000 rows where a 3-column one reaches
/// `MAX_ROW_CAP`.
pub const MAX_CELLS: usize = 2_000_000;

/// The most rows "load more" fetches for a result of `columns` columns.
pub fn row_ceiling(columns: usize) -> usize {
    (MAX_CELLS / columns.max(1)).clamp(DEFAULT_ROW_CAP, MAX_ROW_CAP)
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

/// Where in the statement that ran the server found an error: 1-based line
/// and column, counted in characters of that statement (not of the buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlPosition {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub kind: CoreErrorKind,
    pub message: String,
    /// For a failed run, the statement that was sent: dbx leaves the SQL out
    /// of its error text, so the error view shows it next to the message.
    pub statement: Option<String>,
    /// For a SQL error, where the server says it is, when it says (native
    /// Postgres does).
    pub position: Option<SqlPosition>,
}

impl CoreError {
    pub fn new(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), statement: None, position: None }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

// ---- the seam --------------------------------------------------------------

/// What the editor asks of dbx that needs no I/O: answered at once from data
/// compiled into the client, so the view can ask while it draws. The app
/// caches the answers (types once per kind of database, the statement range
/// once per text and caret), so nothing here is asked per frame.
pub trait Syntax: Send + Sync {
    /// The names of the data types of a kind of database
    /// (`list_dialect_type_names`).
    fn type_names(&self, kind: &DbKind) -> Vec<String>;

    /// Where in `buffer` the statement a run would send is, with the caret at
    /// `cursor` and nothing selected. It is the statement `Core::run` picks
    /// (the same function), located in the buffer; `None` when there is no
    /// statement, or when its text cannot be placed unambiguously (the same
    /// text more than once and the caret in none of the copies, say): then no
    /// outline is drawn rather than a wrong one.
    fn statement_range(&self, kind: &DbKind, buffer: &str, cursor: Cursor) -> Option<(Cursor, Cursor)>;
}

/// Everything the client asks of dbx (design sections 2 and 10).
#[async_trait]
pub trait Core: Syntax {
    async fn connections(&self) -> Result<Vec<Connection>, CoreError>;
    async fn save_connection(&self, draft: &ConnectionDraft) -> Result<Connection, CoreError>;
    async fn remove_connection(&self, id: &ConnectionId) -> Result<(), CoreError>;
    async fn test_connection(&self, draft: &ConnectionDraft) -> Result<(), CoreError>;

    async fn databases(&self, id: &ConnectionId) -> Result<Vec<String>, CoreError>;
    async fn schemas(&self, at: &DatabasePath) -> Result<Vec<String>, CoreError>;
    async fn tables(&self, at: &SchemaPath) -> Result<Vec<Table>, CoreError>;
    async fn columns(&self, at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError>;
    /// Lets go of the server connections the schema calls opened on one
    /// database (the tree collapsed it), so browsing many databases does not
    /// hold a slot on the server for each. Nothing is lost: the next call on
    /// that database opens what it needs again.
    async fn release(&self, at: &DatabasePath) -> Result<(), CoreError>;

    /// Runs until done, failed, or `cancel(run)` is called. After a cancel it
    /// returns `CoreErrorKind::Cancelled` promptly.
    async fn run(&self, run: RunId, request: RunRequest) -> Result<ResultSet, CoreError>;
    fn cancel(&self, run: RunId);
    /// Ends the database session `session` holds on database `at`, if there
    /// is one: its `SET`s, temporary tables and open transaction go with it.
    /// Another session of the same tab is not touched.
    async fn close_session(&self, session: SessionId, at: &DatabasePath) -> Result<(), CoreError>;

    async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(row: usize, col: usize) -> Cursor {
        Cursor { row, col }
    }

    #[test]
    fn a_cursor_after_a_multi_byte_character_counts_in_utf16_units() {
        // `é` is one UTF-16 unit (two UTF-8 bytes), `😀` two (four bytes).
        let buffer = "select 'é😀';\nselect 2;";
        assert_eq!(utf16_offset(buffer, at(0, 0)), 0);
        assert_eq!(utf16_offset(buffer, at(0, 9)), 9, "after `é`");
        assert_eq!(utf16_offset(buffer, at(0, 10)), 11, "after `😀`, which takes two units");
        assert_eq!(utf16_offset(buffer, at(0, 12)), 13, "end of the first row");
        assert_eq!(utf16_offset(buffer, at(1, 0)), 14, "the line break counts as one");
        assert_eq!(utf16_offset(buffer, at(1, 3)), 17);
        assert_eq!(utf16_offset(buffer, at(1, 99)), 23, "clamped to the row's end");
        assert_eq!(utf16_offset(buffer, at(9, 0)), 23, "clamped to the buffer's end");
    }

    #[test]
    fn the_text_between_two_positions_spans_rows_and_takes_either_order() {
        let buffer = "select 1;\nselect 'é😀' as x;\nselect 3;";
        assert_eq!(text_between(buffer, at(1, 0), at(1, 11)), "select 'é😀'");
        assert_eq!(text_between(buffer, at(1, 11), at(1, 0)), "select 'é😀'", "reversed");
        assert_eq!(text_between(buffer, at(0, 7), at(2, 6)), "1;\nselect 'é😀' as x;\nselect");
        assert_eq!(text_between(buffer, at(2, 0), at(2, 99)), "select 3;", "clamped");
        assert_eq!(text_between(buffer, at(1, 2), at(1, 2)), "");
    }

    fn range(a: (usize, usize), b: (usize, usize)) -> Option<(Cursor, Cursor)> {
        Some((at(a.0, a.1), at(b.0, b.1)))
    }

    #[test]
    fn a_statement_that_occurs_once_is_found_wherever_the_caret_is() {
        let buffer = "select 1;\nselect 2\n  from t;\nselect 3;";
        for cursor in [at(1, 0), at(2, 3), at(0, 4), at(3, 9)] {
            assert_eq!(locate_statement(buffer, cursor, "select 2\n  from t"), range((1, 0), (2, 8)), "{cursor:?}");
        }
    }

    #[test]
    fn the_text_dbx_returns_is_trimmed_before_it_is_searched_for() {
        let buffer = "select 1;\n\n  select 2;";
        assert_eq!(locate_statement(buffer, at(2, 3), "\n  select 2;\n"), range((2, 2), (2, 11)));
        assert_eq!(locate_statement(buffer, at(2, 3), "   "), None);
        assert_eq!(locate_statement(buffer, at(2, 3), ""), None);
    }

    #[test]
    fn positions_are_characters_even_after_multi_byte_text() {
        let buffer = "select 'é😀', 'x';\nselect 2;";
        assert_eq!(locate_statement(buffer, at(1, 2), "select 2;"), range((1, 0), (1, 9)));
        assert_eq!(locate_statement(buffer, at(0, 3), "select 'é😀', 'x';"), range((0, 0), (0, 17)));
    }

    #[test]
    fn a_statement_that_occurs_twice_is_the_copy_the_caret_is_in() {
        let buffer = "select 1;\nselect 1;\nselect 2;";
        assert_eq!(locate_statement(buffer, at(0, 3), "select 1;"), range((0, 0), (0, 9)));
        assert_eq!(locate_statement(buffer, at(1, 3), "select 1;"), range((1, 0), (1, 9)));
        assert_eq!(locate_statement(buffer, at(1, 9), "select 1;"), range((1, 0), (1, 9)), "at its end");
    }

    #[test]
    fn a_statement_that_occurs_twice_with_the_caret_in_neither_is_not_placed() {
        let buffer = "select 1;\n\nselect 1;";
        assert_eq!(locate_statement(buffer, at(1, 0), "select 1;"), None, "on the blank line between them");
        // A copy inside another statement does not make a text ambiguous for a caret in the longer one.
        let buffer = "select 1 from t;\nselect 1";
        assert_eq!(locate_statement(buffer, at(1, 2), "select 1"), range((1, 0), (1, 8)));
        assert_eq!(locate_statement(buffer, at(0, 2), "select 1"), range((0, 0), (0, 8)));
    }

    #[test]
    fn text_that_is_not_in_the_buffer_is_not_placed() {
        assert_eq!(locate_statement("select 1;", at(0, 0), "select 2;"), None);
        assert_eq!(locate_statement("", at(0, 0), "select 2;"), None);
    }
}
