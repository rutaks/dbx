//! The Core seam: everything the client asks of dbx, as one collaborator.
//!
//! Only `dbx.rs` (the `DbxCore` adapter) may name a dbx type. Everything else
//! speaks the types defined here. `FakeCore` is the test double.

mod dbx;
mod dump;
mod export;
mod fake;
mod sqlite_proof;
mod store;

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;

pub use dbx::{DbxCore, KeySource, StoreOptions};
pub use store::{has_dbx_store, StoreChoice, StoreInfo, StoreKind, StoreSource};
#[cfg(feature = "test-support")]
pub use dbx::check_seed_dir;
#[cfg(feature = "test-support")]
pub use dbx::mysql_statements_read;
pub use dump::{DumpContent, DumpEnd, DumpFailure, DumpId, DumpProgress, DumpRequest, DumpSink, DumpStatus, Dumped, FileLeft};
pub use export::{ExportError, ExportFormat, ExportJob, ExportRequest, Exported};
pub use fake::{Call, Catalogue, FakeCore, Scripted, ScriptedDump, SyntaxCalls};

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

impl Default for DbKind {
    /// The kind a new connection starts as in the form, and the one assumed
    /// for a tab that has no connection yet (statements end at `;` in every
    /// dialect).
    fn default() -> Self {
        DbKind::Postgres
    }
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

/// The schema names resolve in for a connection with `Capabilities::fixed_schema`.
pub const FIXED_SCHEMA: &str = "main";

/// The kinds the connection form may offer, in order; it keeps those whose
/// `Capabilities::edit_in_form` is set. (A function of its own, so that `app`
/// never names a `DbKind` variant.)
pub fn form_kind_candidates() -> Vec<DbKind> {
    vec![DbKind::Postgres, DbKind::MySql, DbKind::Sqlite]
}

/// The SQL dialect a connection speaks, as far as the client chooses between
/// them: the highlighter's quoting, the formatter's case rules and the
/// completion's quoting. `Generic` is every dialect the client has no rules
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    PostgreSql,
    MySql,
    Sqlite,
    #[default]
    Generic,
}

/// What a connection can do and what shape its tree has, from dbx's own
/// description of the connection type (`plugins/connection-types/*.yaml`,
/// bundled in dbx). Only what the client asks today. Everything here is the
/// adapter's to fill in; the app reads it and never asks which database it
/// is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// The tree has a level of databases under the connection. When it has
    /// not, the paths below the connection carry an empty database name and
    /// the Core uses the connection's own.
    pub databases: bool,
    /// The tree has a level of schemas (under each database, or directly
    /// under the connection when `databases` is false). When it has not, the
    /// tables hang straight off the level above and their paths carry an
    /// empty schema name.
    pub schemas: bool,
    /// dbx runs statements on it (`queryExecution`).
    pub query: bool,
    /// The client can browse its metadata: dbx's `metadataBrowse` and an
    /// adapter that implements it for this type.
    pub browse: bool,
    /// dbx can edit rows of its tables (`tableDataEdit`).
    pub edit_data: bool,
    /// dbx can explain a statement (`sqlExplain`).
    pub explain: bool,
    /// The connection form has the fields this type needs (host, port, user,
    /// password, database), so the client can edit it there.
    pub edit_in_form: bool,
    /// The port a new connection of this type starts with.
    pub default_port: Option<u16>,
    pub dialect: Dialect,
    /// dbx keeps one server connection for it (a file database), so nothing
    /// is opened per database.
    pub single_connection: bool,
    /// dbx's dump of this kind is known not to restore whole (checked on
    /// Postgres, whose dump code GaussDB and openGauss share, and on SQLite).
    /// The dump screen and its end say so, with the losses of `dialect`
    /// (README "Dump").
    pub dump_loses: bool,
    /// The database is one file the user names by its path (SQLite): the form
    /// asks for a name and the path, which is saved as the host, and nothing
    /// else.
    pub local_file: bool,
    /// Completion, templates and marks run on it: dbx answers the catalogue
    /// searches they ask for, and the client knows how its names resolve.
    pub completes: bool,
    /// The schema unqualified names resolve in is fixed, `FIXED_SCHEMA`
    /// (`main`, for SQLite), and no query is needed to know it. When false
    /// the app asks the Core (`current_schema`).
    pub fixed_schema: bool,
}

/// A saved, named way to reach one database server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: ConnectionId,
    pub name: String,
    pub kind: DbKind,
    pub capabilities: Capabilities,
    pub host: String,
    pub port: Option<u16>,
    /// The database the connection opens by default, if it names one.
    pub database: Option<String>,
    pub username: String,
    /// TLS on or off, as the form shows it.
    pub tls: bool,
    /// dbx marks the connection as production: writes need confirming.
    pub production: bool,
    /// The databases dbx marks production on this connection
    /// (`production_databases`), as stored.
    pub production_databases: Vec<String>,
    /// The connection's own query timeout in seconds, when it sets one: the
    /// longest a run may take before it is cancelled. `None` is no limit.
    pub query_timeout: Option<u64>,
}

impl Connection {
    /// Whether `database` on this connection is marked production: the whole
    /// connection is, or the database is in its list. Names compare as dbx
    /// compares them (`is_production_database`: trimmed, outer quotes off,
    /// without regard to ASCII case). For the `PROD` badge only: the adapter
    /// decides what needs confirming, from the config it reads at the run.
    pub fn is_production_database(&self, database: &str) -> bool {
        let norm = |s: &str| s.trim().trim_matches(|c| matches!(c, '`' | '"' | '[' | ']')).to_ascii_lowercase();
        self.production
            || (!database.trim().is_empty() && self.production_databases.iter().any(|d| norm(d) == norm(database)))
    }
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
    /// Empty when the connection has no database level
    /// (`Capabilities::databases`): the Core uses the connection's own.
    pub database: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemaPath {
    pub database: DatabasePath,
    /// Empty when the connection has no schema level
    /// (`Capabilities::schemas`).
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

// ---- completion ------------------------------------------------------------

/// What a completion search looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchKind {
    Schema,
    /// Tables, views and materialized views.
    Relation,
    Function,
}

/// A prefix search of one database's names (dbx's
/// `completion_assistant_search_core`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    pub at: DatabasePath,
    /// The schema searched, for relations and functions; the search of
    /// schemas ignores it.
    pub schema: Option<String>,
    pub kind: SearchKind,
    /// Names that start with it, without regard to case; empty is everything.
    pub prefix: String,
    /// The most names returned.
    pub limit: usize,
    /// Relations only, with no `schema`: look in every schema the user may
    /// use, and not only in those the search path makes visible (dbx's
    /// `global_search`). It is how a name that exists in two schemas is found.
    pub global: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateKind {
    Schema,
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
}

/// One name a completion search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Exactly as stored.
    pub name: String,
    pub kind: CandidateKind,
    pub schema: Option<String>,
    /// For a function, what it returns.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub candidates: Vec<Candidate>,
    /// The search stopped at its limit: there may be more names.
    pub incomplete: bool,
}

/// One column pair of a foreign key of a table: the table's own `column`
/// refers to `ref_column` of `ref_table`. A key over several columns is
/// several of these with the same `name`, in column order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub column: String,
    /// The schema of the referenced table, when the database says it.
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_column: String,
}

// ---- what a statement refers to ----------------------------------------------

/// A stretch of SQL text, in characters: `start` is the first character and
/// `end` the one after the last, in the rows and columns of the text that was
/// analysed (the first row is 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Cursor,
    pub end: Cursor,
}

/// A table a statement names (dbx's `SqlTableReference`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableReference {
    pub name: String,
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub span: Span,
    /// The query block it is in; see `References::scopes`.
    pub scope: usize,
}

/// A column a statement names, with the name before its dot when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnReference {
    pub name: String,
    pub qualifier: Option<String>,
    pub span: Span,
    pub scope: usize,
}

/// What dbx's analysis of a statement that parses says it refers to
/// (`analyze_sql_references`). Names are as written; CTE names and subquery
/// aliases are not among `tables`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    pub tables: Vec<TableReference>,
    pub columns: Vec<ColumnReference>,
    /// (id, the id of the block that holds it).
    pub scopes: Vec<(usize, Option<usize>)>,
    /// Qualifiers (`x` of `x.col`) that name no table or alias in view.
    pub unknown_qualifiers: Vec<(String, Span)>,
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
    /// Ask before a statement dbx calls dangerous (DDL, an UPDATE or DELETE
    /// without a limiting WHERE), on any database. The setting
    /// `confirm_dangerous`. Production writes are confirmed whatever it says.
    pub confirm_dangerous: bool,
    /// The user's yes to an `Assessment`: the run executes only if it is for
    /// exactly the statement, connection, database and session of this run.
    pub confirmation: Option<Confirmation>,
}

/// What a yes is bound to: a confirmation for anything else does not count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// The exact text that runs.
    pub statement: String,
    pub connection: ConnectionId,
    /// The database it runs on, as the adapter resolved it.
    pub database: String,
    pub session: SessionId,
}

/// Why a statement waits for the user's yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// It writes data.
    Writes,
    /// It changes the schema (DDL).
    ChangesSchema,
    /// It controls a transaction (`commit`, `rollback`, `begin`).
    TransactionControl,
    /// It reads, but calls functions dbx cannot prove read-only.
    Unproven,
    /// dbx flags it dangerous: DDL, or UPDATE/DELETE without a limiting WHERE.
    Dangerous,
    /// dbx cannot parse it, so it is treated as a write.
    Unparsed,
    /// A yes was given, but for another statement or another target.
    Changed,
    /// It is a `USE` (or the like): it switches the session's database.
    SwitchesDatabase,
    /// An earlier `USE` in this tab's session may have moved it to a
    /// production database, so this statement is treated as aimed there.
    SessionSwitched,
    /// `EXPLAIN ANALYZE` runs the statement it explains.
    ExplainRuns,
    /// MySQL: the server can read its quotes or comments in more than one
    /// way (the SQL modes `NO_BACKSLASH_ESCAPES` and `ANSI_QUOTES`, and the
    /// server's version for `/*!NNNNN */`, decide), or dbx's own scanner reads
    /// them otherwise than the server does: it may be other statements than
    /// this client judged.
    Ambiguous,
    /// MySQL: `INTO OUTFILE` or `INTO DUMPFILE` writes a file on the host the
    /// server, and every database it has, is on.
    WritesFile,
}

impl Reason {
    /// The reason in words, for the confirmation.
    pub fn words(self) -> &'static str {
        match self {
            Reason::Writes => "it writes data",
            Reason::ChangesSchema => "it changes the schema",
            Reason::TransactionControl => "it controls a transaction",
            Reason::Unproven => "it calls functions dbx cannot prove read-only",
            // Not "DDL or an unscoped UPDATE/DELETE": dbx flags more than
            // that (`SET`, `COMMIT`, locking clauses), and says only "dangerous".
            Reason::Dangerous => "dbx flags it dangerous",
            Reason::Unparsed => "dbx cannot parse it, so it is treated as a write",
            Reason::Changed => "the statement or where it runs changed since it was confirmed",
            Reason::SwitchesDatabase => "it switches the session's database; later statements in this tab will be confirmed",
            Reason::SessionSwitched => "an earlier USE in this tab switched its database, so this may run on production",
            Reason::ExplainRuns => "EXPLAIN ANALYZE runs the statement it explains",
            Reason::Ambiguous => "the server may read its quotes or comments differently from this client (its SQL mode and version decide), which could make it other statements than it looks",
            Reason::WritesFile => "it writes a file on the database server's host",
        }
    }
}

/// The adapter's answer to a statement that needs confirming before it runs
/// (`CoreErrorKind::NeedsConfirmation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    /// It runs on, or names, a database marked production.
    pub production: bool,
    pub reasons: Vec<Reason>,
    /// For a production target found through the SQL (the database the tab
    /// runs on is not marked): the marked databases it names.
    pub named: Vec<String>,
    /// dbx cannot tell which database it writes to, and counts it as
    /// production.
    pub target_unknown: bool,
    /// The connection's name, safe to draw.
    pub connection_name: String,
    /// What a yes sends back.
    pub confirmation: Confirmation,
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
    /// For a column with no type name only: the rows (indexes into the
    /// result's rows, ascending) whose value the database gave as a number.
    /// SQLite types each value, and an expression or a column declared
    /// without a type has no type name, so this is all that tells the
    /// integer `42` from the text `'42'`. An export writes these as numbers.
    pub numbers: Vec<usize>,
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

/// History is shared with the dbx desktop app (slice 18): one store, written
/// through dbx's own persistence. Only statements typed as SQL are read here
/// (source `sql`; the MCP server's are not ours).
///
/// One statement this client ran, to be saved. The adapter adds the id and the
/// time, and the fields this client leaves empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHistoryEntry {
    pub connection: ConnectionId,
    pub connection_name: String,
    pub database: String,
    /// The statement that ran, as it was sent.
    pub sql: String,
    pub elapsed: Duration,
    pub success: bool,
    /// The error text, cleaned, for a run that failed.
    pub error: Option<String>,
    /// Rows changed, for a statement that returned no result set.
    pub affected: Option<u64>,
}

/// One statement in the store, whoever ran it. Everything in it was written by
/// another program (the desktop app, another client): it is drawn only through
/// the painter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    /// Empty for an entry an old version of the desktop app wrote.
    pub connection: ConnectionId,
    pub connection_name: String,
    pub database: String,
    pub sql: String,
    /// When it ran, as the list shows it (local time, `2026-11-25 14:03`).
    pub when: String,
    pub elapsed: Duration,
    pub success: bool,
    pub error: Option<String>,
    pub affected: Option<u64>,
}

/// Where the next page of a search starts: the last entry of this one. Opaque
/// to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCursor {
    pub executed_at: String,
    pub id: String,
}

/// A search of the history, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryQuery {
    /// Matched without regard to case against the statement, the connection's
    /// name and the database. Empty matches everything.
    pub search_text: String,
    /// Only this connection's entries; `None` is every connection.
    pub connection: Option<ConnectionId>,
    /// The page after the one this cursor ended.
    pub cursor: Option<HistoryCursor>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    /// Where the next page starts; `None` at the end.
    pub next: Option<HistoryCursor>,
    /// How many entries match, all pages together.
    pub total: usize,
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
    /// Nothing ran: the statement waits for the user's yes
    /// (`CoreError::assessment` says why).
    NeedsConfirmation,
    /// Nothing ran: the request was refused, and the message says why and
    /// what to change.
    Refused,
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
    /// For `NeedsConfirmation`: why, and what a yes sends back.
    pub assessment: Option<Box<Assessment>>,
    /// The run reached the server and what it did there is not known: a
    /// cancel (or the time limit) stopped a run of more than one statement,
    /// or a statement not proven read-only, so earlier statements, or part of
    /// this one, may have run; or the session was lost after the statement
    /// was sent. Saved to the history as a run whose outcome is unknown, never
    /// shown as merely "stopped".
    pub outcome_unknown: bool,
}

impl CoreError {
    pub fn new(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), statement: None, position: None, assessment: None, outcome_unknown: false }
    }

    /// Nothing ran: `assessment` says why it waits for a yes.
    pub fn needs_confirmation(assessment: Assessment) -> Self {
        let message = format!("Not run: {} needs confirming.", if assessment.production { "a statement on production" } else { "a dangerous statement" });
        let mut e = Self::new(CoreErrorKind::NeedsConfirmation, message);
        e.statement = Some(assessment.confirmation.statement.clone());
        e.assessment = Some(Box::new(assessment));
        e
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
    /// What a kind of database can do, before any connection of it exists
    /// (the form's starting port). A connection carries its own.
    fn capabilities(&self, kind: &DbKind) -> Capabilities;

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

    /// The select that reads one page of `table`: `limit` rows from row
    /// `offset`, ordered by `order_by` (the primary key's columns, which dbx
    /// does not order by itself; empty for a table without one). Every
    /// column, whole: the large-value preview is never asked for.
    fn table_page_sql(&self, kind: &DbKind, table: &TablePath, order_by: &[String], limit: usize, offset: usize) -> String;

    /// The statement that counts the rows of `table`; its one cell is the
    /// count.
    fn table_count_sql(&self, kind: &DbKind, table: &TablePath) -> String;

    /// The bytes of a result in one format, built by dbx's formats from the
    /// exact text the client holds (`ExportRequest`). No I/O, but the work
    /// grows with the rows: the shell runs it off the event loop, the app only
    /// for the small text a copy may send.
    fn export_bytes(&self, request: &ExportRequest) -> Result<Vec<u8>, String>;

    /// The columns of `request` that dbx's SQL INSERT leaves out, by name:
    /// the overlay lists them before anything is written.
    fn export_skipped_columns(&self, _request: &ExportRequest) -> Vec<String> {
        Vec::new()
    }
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
    /// The foreign keys a table has (dbx's `list_foreign_keys_core`): the
    /// table's own columns that refer to another table. Keys of other tables
    /// that refer to this one are not included.
    async fn foreign_keys(&self, at: &TablePath) -> Result<Vec<ForeignKey>, CoreError>;
    /// What the statement `sql` (one statement, the text of the buffer between
    /// two positions) refers to: dbx's `analyze_sql_references`, on the
    /// dialect of `kind`. `Ok(None)` says the text does not parse, which a
    /// half-typed statement does not; the caller reads tokens then. It does no
    /// I/O, but it is a call like the others (answered later, a stale answer
    /// dropped) so that typing never waits for a parse.
    async fn analyze_references(&self, kind: &DbKind, sql: &str) -> Result<Option<References>, CoreError>;
    /// A prefix search of names for the completion popup: schemas, tables and
    /// views, or functions. Like the schema calls it can be abandoned: the
    /// caller drops the future and the work stops.
    async fn complete(&self, request: &CompletionRequest) -> Result<Completion, CoreError>;
    /// The schema unqualified names resolve in on this database (`select
    /// current_schema()`), `None` when the server says there is none.
    async fn current_schema(&self, at: &DatabasePath) -> Result<Option<String>, CoreError>;
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

    /// Saves one run to the history the desktop app shares. The store trims to
    /// the retention limit, which is the desktop's setting.
    async fn save_history(&self, entry: &NewHistoryEntry) -> Result<(), CoreError>;
    /// Searches the history (source `sql` only), newest first.
    async fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, CoreError>;

    /// Dumps a database, or one schema, to `request.path` through dbx's
    /// database export, sending its progress to `progress` as it comes. Ends
    /// when dbx has ended and the dump's own server session is closed. A
    /// cancelled dump ends with `Ok`, its last progress `Cancelled`.
    async fn dump(&self, id: DumpId, request: DumpRequest, progress: DumpSink) -> Result<DumpEnd, CoreError>;
    /// Asks dump `id` to stop. dbx notices at its next check and sends a
    /// final `Cancelled` progress.
    fn cancel_dump(&self, id: DumpId);
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
