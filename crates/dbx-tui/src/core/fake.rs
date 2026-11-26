//! `FakeCore`: the test double for the Core seam.
//!
//! Replies are scripted per method, as a queue. Each scripted reply can be a
//! value or an error and can be held back by a delay. When a queue is empty the
//! method answers with an empty success, so tests script only what they need.
//! Every call is recorded, and `cancel` ends a delayed `run` with `Cancelled`
//! (unless the reply was scripted `deaf`: a cancel that does not land).
//!
//! Sessions are modelled as dbx has them: a run opens its session (by
//! `SessionId` and database), and closing a session forgets it, but does not
//! end a run in flight on it (dbx returns at once and drops a busy connection
//! only when the run lets go of it). `session_open` says which are open.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;

use super::*;

/// One scripted reply: a result and how long to wait before giving it.
#[derive(Debug, Clone)]
pub struct Scripted<T> {
    result: Result<T, CoreError>,
    delay: Duration,
    deaf: bool,
}

impl<T> Scripted<T> {
    pub fn ok(value: T) -> Self {
        Self { result: Ok(value), delay: Duration::ZERO, deaf: false }
    }

    pub fn err(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        Self { result: Err(CoreError::new(kind, message)), delay: Duration::ZERO, deaf: false }
    }

    /// An error the test built itself (one carrying an assessment, say).
    pub fn fail(error: CoreError) -> Self {
        Self { result: Err(error), delay: Duration::ZERO, deaf: false }
    }

    /// Hold the reply back for `delay` (tokio time, so tests can pause it).
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// For a run: a cancel does not stop it (the server never got the
    /// cancel, or ignores it). It still ends if its session is closed.
    pub fn deaf(mut self) -> Self {
        self.deaf = true;
        self
    }
}

/// A call the fake received, with the arguments that identify it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Connections,
    SaveConnection(String),
    RemoveConnection(ConnectionId),
    TestConnection(String),
    Databases(ConnectionId),
    Schemas(DatabasePath),
    Tables(SchemaPath),
    Columns(TablePath),
    ForeignKeys(TablePath),
    Analyze(String),
    Complete(CompletionRequest),
    CurrentSchema(DatabasePath),
    Release(DatabasePath),
    Run(RunId),
    Cancel(RunId),
    CloseSession(SessionId, DatabasePath),
    History,
    Dump(DumpId, DumpRequest),
    CancelDump(DumpId),
}

/// One scripted dump: the progress it sends, each after a delay (tokio
/// time), and how it ends when nobody cancels it.
#[derive(Debug, Clone)]
pub struct ScriptedDump {
    steps: Vec<(Duration, DumpProgress)>,
    end: Result<DumpEnd, CoreError>,
    lag: usize,
    /// What the fake writes to the request's path as it starts, truncating
    /// what is there: dbx opening the file it was given.
    writes: Option<Vec<u8>>,
}

impl ScriptedDump {
    /// Sends `steps`, then ends with the last of them.
    pub fn steps(steps: Vec<(Duration, DumpProgress)>) -> Self {
        let last = steps.last().map(|(_, p)| p.clone());
        ScriptedDump { steps, end: Ok(DumpEnd { last }), lag: 0, writes: None }
    }

    /// Sends `steps`, then fails with `message`.
    pub fn failing(steps: Vec<(Duration, DumpProgress)>, message: &str) -> Self {
        ScriptedDump { steps, end: Err(CoreError::new(CoreErrorKind::Other, message)), lag: 0, writes: None }
    }

    /// Sends `steps`, then returns `Ok` with no final progress at all.
    pub fn ending_without_a_report(steps: Vec<(Duration, DumpProgress)>) -> Self {
        ScriptedDump { steps, end: Ok(DumpEnd { last: None }), lag: 0, writes: None }
    }

    /// Fails as `error`, whatever it is (a refusal, say).
    pub fn failing_with(error: CoreError) -> Self {
        ScriptedDump { steps: Vec::new(), end: Err(error), lag: 0, writes: None }
    }

    /// Writes `bytes` to the request's path as the dump starts, replacing what
    /// is there, as dbx does with the file it is given.
    pub fn writing(mut self, bytes: &[u8]) -> Self {
        self.writes = Some(bytes.to_vec());
        self
    }

    /// After a cancel, `n` more steps are still sent before `Cancelled`.
    pub fn lag(mut self, n: usize) -> Self {
        self.lag = n;
        self
    }
}

/// What the fake database holds, for completion: answered by content (the
/// prefix, the schema, the kind) and not in a fixed order, because the app
/// asks for what it needs as it needs it. Every call it serves waits `delay`
/// first (tokio time).
#[derive(Debug, Clone, Default)]
pub struct Catalogue {
    pub current_schema: Option<String>,
    pub schemas: Vec<String>,
    /// (schema, name, kind)
    pub relations: Vec<(String, String, CandidateKind)>,
    /// (schema, name, what it returns)
    pub functions: Vec<(String, String, String)>,
    /// (schema, table, columns)
    pub columns: Vec<(String, String, Vec<ColumnInfo>)>,
    /// (schema, table, the keys of that table)
    pub foreign_keys: Vec<(String, String, Vec<ForeignKey>)>,
    pub delay: Duration,
}

type Queue<T> = Mutex<VecDeque<Scripted<T>>>;

#[derive(Default)]
pub struct FakeCore {
    connections: Queue<Vec<Connection>>,
    save_connection: Queue<Connection>,
    remove_connection: Queue<()>,
    test_connection: Queue<()>,
    databases: Queue<Vec<String>>,
    schemas: Queue<Vec<String>>,
    tables: Queue<Vec<Table>>,
    columns: Queue<Vec<ColumnInfo>>,
    run: Queue<ResultSet>,
    close_session: Queue<()>,
    history: Queue<HistoryPage>,
    save_history: Queue<()>,
    /// Every search `history` was asked, and every entry `save_history` took.
    history_queries: Mutex<Vec<HistoryQuery>>,
    saved_history: Mutex<Vec<NewHistoryEntry>>,
    catalogue: Mutex<Option<Catalogue>>,
    calls: Mutex<Vec<Call>>,
    cancelled: Mutex<HashSet<RunId>>,
    cancel_signal: Notify,
    dumps: Mutex<VecDeque<ScriptedDump>>,
    dump_cancels: Mutex<HashSet<DumpId>>,
    /// The sessions open: opened by a run, forgotten by a close.
    sessions: Mutex<HashSet<(SessionId, DatabasePath)>>,
    default_connections_empty: std::sync::atomic::AtomicBool,
    syntax_calls: Mutex<SyntaxCalls>,
    /// What `analyze_references` answers, by the exact text asked about.
    analyses: Mutex<Vec<(String, References)>>,
    /// What `capabilities` answers for a kind, when a test set it.
    kind_capabilities: Mutex<Vec<(DbKind, Capabilities)>>,
}

impl Capabilities {
    /// A PostgreSQL-shaped connection: databases, schemas, everything on.
    pub fn postgres() -> Self {
        Capabilities {
            databases: true,
            schemas: true,
            query: true,
            browse: true,
            edit_data: true,
            explain: true,
            edit_in_form: true,
            default_port: Some(5432),
            dialect: Dialect::PostgreSql,
            single_connection: false,
            dump_loses: true,
            local_file: false,
            completes: true,
            fixed_schema: false,
        }
    }

    /// A MySQL-shaped connection: databases and then tables, no schemas.
    pub fn mysql() -> Self {
        Capabilities {
            schemas: false,
            edit_in_form: false,
            default_port: Some(3306),
            dialect: Dialect::MySql,
            dump_loses: false,
            completes: false,
            ..Self::postgres()
        }
    }

    /// A SQLite-shaped connection: a file with tables, no databases and no
    /// schemas, one server connection, and `main` for the schema names
    /// resolve in.
    pub fn sqlite() -> Self {
        Capabilities {
            databases: false,
            schemas: false,
            explain: false,
            edit_in_form: true,
            default_port: None,
            dialect: Dialect::Sqlite,
            single_connection: true,
            dump_loses: false,
            local_file: true,
            completes: true,
            fixed_schema: true,
            ..Self::postgres()
        }
    }

    /// The shape a fake connection of `kind` has unless a test gives it one.
    pub fn like(kind: &DbKind) -> Self {
        match kind {
            DbKind::Postgres => Self::postgres(),
            DbKind::MySql => Self::mysql(),
            DbKind::Sqlite => Self::sqlite(),
            DbKind::Other(_) => Capabilities::default(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn push<T>(queue: &Queue<T>, reply: Scripted<T>) {
    lock(queue).push_back(reply);
}

/// Pops the next scripted reply (or the default) and waits out its delay.
async fn reply<T>(queue: &Queue<T>, default: impl FnOnce() -> Result<T, CoreError>) -> Result<T, CoreError> {
    let next = lock(queue).pop_front();
    match next {
        Some(s) => {
            if !s.delay.is_zero() {
                tokio::time::sleep(s.delay).await;
            }
            s.result
        }
        None => default(),
    }
}

fn unscripted<T>(what: &str) -> Result<T, CoreError> {
    Err(CoreError::new(CoreErrorKind::Other, format!("FakeCore: no scripted reply for {what}")))
}

impl FakeCore {
    /// A fake with nothing scripted: every call is an error until scripted.
    pub fn new() -> Self {
        Self::default()
    }

    /// A fake that answers the connection list with no connections, as an
    /// empty store would, and leaves everything else unscripted.
    pub fn empty() -> Self {
        let fake = Self::default();
        fake.default_connections_empty.store(true, std::sync::atomic::Ordering::SeqCst);
        fake
    }

    /// Whether a run opened `session` on `at` and no close has ended it since.
    pub fn session_open(&self, session: SessionId, at: &DatabasePath) -> bool {
        lock(&self.sessions).contains(&(session, at.clone()))
    }

    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    fn record(&self, call: Call) {
        lock(&self.calls).push(call);
    }

    pub fn script_dump(&self, d: ScriptedDump) {
        lock(&self.dumps).push_back(d);
    }

    pub fn script_connections(&self, r: Scripted<Vec<Connection>>) {
        push(&self.connections, r);
    }
    pub fn script_save_connection(&self, r: Scripted<Connection>) {
        push(&self.save_connection, r);
    }
    pub fn script_remove_connection(&self, r: Scripted<()>) {
        push(&self.remove_connection, r);
    }
    pub fn script_test_connection(&self, r: Scripted<()>) {
        push(&self.test_connection, r);
    }
    pub fn script_databases(&self, r: Scripted<Vec<String>>) {
        push(&self.databases, r);
    }
    pub fn script_schemas(&self, r: Scripted<Vec<String>>) {
        push(&self.schemas, r);
    }
    pub fn script_tables(&self, r: Scripted<Vec<Table>>) {
        push(&self.tables, r);
    }
    pub fn script_columns(&self, r: Scripted<Vec<ColumnInfo>>) {
        push(&self.columns, r);
    }
    /// Gives the fake a database to answer completion from.
    pub fn script_catalogue(&self, c: Catalogue) {
        *lock(&self.catalogue) = Some(c);
    }
    /// Changes how long the catalogue's calls take from now on.
    pub fn set_catalogue_delay(&self, delay: Duration) {
        if let Some(c) = lock(&self.catalogue).as_mut() {
            c.delay = delay;
        }
    }
    /// Makes `analyze_references` answer `references` for exactly `sql`. Any
    /// other text does not parse, as far as the fake is concerned (`None`).
    pub fn script_analysis(&self, sql: &str, references: References) {
        lock(&self.analyses).push((sql.to_string(), references));
    }
    pub fn script_run(&self, r: Scripted<ResultSet>) {
        push(&self.run, r);
    }
    pub fn script_close_session(&self, r: Scripted<()>) {
        push(&self.close_session, r);
    }
    pub fn script_history(&self, r: Scripted<HistoryPage>) {
        push(&self.history, r);
    }
    /// The next `save_history` answers `r`; with nothing scripted a save
    /// succeeds (every plain run saves, and most tests do not care).
    pub fn script_save_history(&self, r: Scripted<()>) {
        push(&self.save_history, r);
    }
    /// The entries `save_history` has taken, in order (a save that was
    /// scripted to fail is in the list too: it was asked for).
    pub fn saved_history(&self) -> Vec<NewHistoryEntry> {
        lock(&self.saved_history).clone()
    }
    /// The searches `history` has been asked, in order.
    pub fn history_queries(&self) -> Vec<HistoryQuery> {
        lock(&self.history_queries).clone()
    }
}

/// How often the fake was asked the synchronous questions (`Syntax`), for
/// tests that assert the app asks once, not per frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxCalls {
    pub type_names: Vec<DbKind>,
    pub statement_ranges: usize,
}

impl FakeCore {
    /// What the fake says a kind of database can do (`Syntax::capabilities`).
    pub fn script_capabilities(&self, kind: DbKind, capabilities: Capabilities) {
        let mut all = lock(&self.kind_capabilities);
        all.retain(|(k, _)| k != &kind);
        all.push((kind, capabilities));
    }

    pub fn syntax_calls(&self) -> SyntaxCalls {
        lock(&self.syntax_calls).clone()
    }
}

/// The type names the fake gives unless told otherwise.
const FAKE_TYPES: [&str; 4] = ["varchar", "timestamptz", "integer", "text"];

impl Syntax for FakeCore {
    fn capabilities(&self, kind: &DbKind) -> Capabilities {
        let set = lock(&self.kind_capabilities).iter().find(|(k, _)| k == kind).map(|(_, c)| c.clone());
        set.unwrap_or_else(|| Capabilities::like(kind))
    }

    fn type_names(&self, kind: &DbKind) -> Vec<String> {
        lock(&self.syntax_calls).type_names.push(kind.clone());
        FAKE_TYPES.iter().map(|t| t.to_string()).collect()
    }

    /// The statement the caret is in, where statements end at `;` (the `;`
    /// is not part of the range, as dbx's text leaves it out).
    fn statement_range(&self, _kind: &DbKind, buffer: &str, cursor: Cursor) -> Option<(Cursor, Cursor)> {
        lock(&self.syntax_calls).statement_ranges += 1;
        let at = byte_offset(buffer, cursor);
        let mut start = 0;
        loop {
            let end = buffer[start..].find(';').map_or(buffer.len(), |i| start + i);
            // A caret just after the `;` is still in the statement it ends.
            if at <= end + 1 || end == buffer.len() {
                let text = &buffer[start..end];
                let lead = text.len() - text.trim_start().len();
                let (from, to) = (start + lead, start + lead + text.trim().len());
                return (to > from).then(|| (cursor_at(buffer, from), cursor_at(buffer, to)));
            }
            start = end + 1;
        }
    }

    /// `select * from schema.table order by a, b limit n offset m`, in the
    /// fake's own plain spelling (what the real adapter builds is tested in
    /// `dbx.rs`).
    fn table_page_sql(&self, _kind: &DbKind, table: &TablePath, order_by: &[String], limit: usize, offset: usize) -> String {
        let order = if order_by.is_empty() { String::new() } else { format!(" order by {}", order_by.join(", ")) };
        format!("select * from {}{order} limit {limit} offset {offset}", fake_table_name(table))
    }

    fn table_count_sql(&self, _kind: &DbKind, table: &TablePath) -> String {
        format!("select count(*) from {}", fake_table_name(table))
    }

    /// dbx's own formats: they need no connection, so the fake does not
    /// invent others.
    fn export_bytes(&self, request: &ExportRequest) -> Result<Vec<u8>, String> {
        super::dbx::export_bytes_for(request)
    }

    fn export_skipped_columns(&self, request: &ExportRequest) -> Vec<String> {
        super::dbx::export_skipped_columns_for(request)
    }
}

fn fake_table_name(table: &TablePath) -> String {
    if table.schema.schema.is_empty() {
        table.table.clone()
    } else {
        format!("{}.{}", table.schema.schema, table.table)
    }
}

#[async_trait]
impl Core for FakeCore {
    async fn connections(&self) -> Result<Vec<Connection>, CoreError> {
        self.record(Call::Connections);
        let empty = self.default_connections_empty.load(std::sync::atomic::Ordering::SeqCst);
        reply(&self.connections, || if empty { Ok(Vec::new()) } else { unscripted("connections") }).await
    }
    async fn save_connection(&self, draft: &ConnectionDraft) -> Result<Connection, CoreError> {
        self.record(Call::SaveConnection(draft.name.clone()));
        reply(&self.save_connection, || unscripted("save_connection")).await
    }
    async fn remove_connection(&self, id: &ConnectionId) -> Result<(), CoreError> {
        self.record(Call::RemoveConnection(id.clone()));
        reply(&self.remove_connection, || unscripted("call")).await
    }
    async fn test_connection(&self, draft: &ConnectionDraft) -> Result<(), CoreError> {
        self.record(Call::TestConnection(draft.name.clone()));
        reply(&self.test_connection, || unscripted("call")).await
    }
    async fn databases(&self, id: &ConnectionId) -> Result<Vec<String>, CoreError> {
        self.record(Call::Databases(id.clone()));
        reply(&self.databases, || unscripted("databases")).await
    }
    async fn schemas(&self, at: &DatabasePath) -> Result<Vec<String>, CoreError> {
        self.record(Call::Schemas(at.clone()));
        reply(&self.schemas, || unscripted("schemas")).await
    }
    async fn tables(&self, at: &SchemaPath) -> Result<Vec<Table>, CoreError> {
        self.record(Call::Tables(at.clone()));
        reply(&self.tables, || unscripted("tables")).await
    }
    async fn columns(&self, at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError> {
        self.record(Call::Columns(at.clone()));
        let from_catalogue = || {
            let held = lock(&self.catalogue).clone();
            match held {
                Some(c) => Ok((c.delay, c.columns.iter().find(|(s, t, _)| *s == at.schema.schema && *t == at.table).map(|(_, _, cols)| cols.clone()))),
                None => Err(()),
            }
        };
        if lock(&self.columns).is_empty() {
            if let Ok((delay, cols)) = from_catalogue() {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                return Ok(cols.unwrap_or_default());
            }
        }
        reply(&self.columns, || unscripted("columns")).await
    }

    async fn foreign_keys(&self, at: &TablePath) -> Result<Vec<ForeignKey>, CoreError> {
        self.record(Call::ForeignKeys(at.clone()));
        let Some(c) = lock(&self.catalogue).clone() else { return unscripted("foreign_keys") };
        if !c.delay.is_zero() {
            tokio::time::sleep(c.delay).await;
        }
        Ok(c.foreign_keys.iter().find(|(s, t, _)| *s == at.schema.schema && *t == at.table).map(|(_, _, keys)| keys.clone()).unwrap_or_default())
    }

    async fn analyze_references(&self, _kind: &DbKind, sql: &str) -> Result<Option<References>, CoreError> {
        self.record(Call::Analyze(sql.to_string()));
        Ok(lock(&self.analyses).iter().find(|(text, _)| text == sql).map(|(_, r)| r.clone()))
    }

    async fn complete(&self, request: &CompletionRequest) -> Result<Completion, CoreError> {
        self.record(Call::Complete(request.clone()));
        let Some(c) = lock(&self.catalogue).clone() else { return unscripted("complete") };
        if !c.delay.is_zero() {
            tokio::time::sleep(c.delay).await;
        }
        let wanted = request.prefix.to_lowercase();
        let starts = |name: &str| name.to_lowercase().starts_with(&wanted);
        let in_schema = |schema: &String| request.schema.as_ref().is_none_or(|s| s == schema);
        let mut found: Vec<Candidate> = match request.kind {
            SearchKind::Schema => c
                .schemas
                .iter()
                .filter(|n| starts(n))
                .map(|n| Candidate { name: n.clone(), kind: CandidateKind::Schema, schema: Some(n.clone()), detail: None })
                .collect(),
            SearchKind::Relation => c
                .relations
                .iter()
                .filter(|(s, n, _)| in_schema(s) && starts(n))
                .map(|(s, n, k)| Candidate { name: n.clone(), kind: *k, schema: Some(s.clone()), detail: None })
                .collect(),
            SearchKind::Function => c
                .functions
                .iter()
                .filter(|(s, n, _)| in_schema(s) && starts(n))
                .map(|(s, n, r)| Candidate { name: n.clone(), kind: CandidateKind::Function, schema: Some(s.clone()), detail: Some(r.clone()) })
                .collect(),
        };
        found.sort_by(|a, b| a.name.cmp(&b.name));
        let incomplete = found.len() >= request.limit;
        found.truncate(request.limit);
        Ok(Completion { candidates: found, incomplete })
    }

    async fn current_schema(&self, at: &DatabasePath) -> Result<Option<String>, CoreError> {
        self.record(Call::CurrentSchema(at.clone()));
        let Some(c) = lock(&self.catalogue).clone() else { return unscripted("current_schema") };
        if !c.delay.is_zero() {
            tokio::time::sleep(c.delay).await;
        }
        Ok(c.current_schema)
    }

    async fn release(&self, at: &DatabasePath) -> Result<(), CoreError> {
        self.record(Call::Release(at.clone()));
        Ok(())
    }

    async fn run(&self, run: RunId, request: RunRequest) -> Result<ResultSet, CoreError> {
        self.record(Call::Run(run));
        let cancelled = || CoreError::new(CoreErrorKind::Cancelled, "cancelled");
        let is_cancelled = || lock(&self.cancelled).contains(&run);
        // A run cancelled before it started never touches the script.
        if is_cancelled() {
            return Err(cancelled());
        }
        let session = (request.session, DatabasePath { connection: request.connection, database: request.database.unwrap_or_default() });
        lock(&self.sessions).insert(session);
        let next = lock(&self.run).pop_front();
        let deaf = next.as_ref().is_some_and(|s| s.deaf);
        let answer = async {
            match next {
                Some(s) => {
                    tokio::time::sleep(s.delay).await;
                    s.result
                }
                None => unscripted("run"),
            }
        };
        let cancel = async {
            if deaf {
                return std::future::pending().await;
            }
            // Register for the signal before checking the flag, so a cancel
            // that lands in between is not missed.
            loop {
                let signalled = self.cancel_signal.notified();
                tokio::pin!(signalled);
                signalled.as_mut().enable();
                if is_cancelled() {
                    return;
                }
                signalled.await;
            }
        };
        // Biased, cancel first: a cancel always beats a reply that is ready.
        tokio::select! {
            biased;
            _ = cancel => Err(cancelled()),
            r = answer => r,
        }
    }

    fn cancel(&self, run: RunId) {
        self.record(Call::Cancel(run));
        lock(&self.cancelled).insert(run);
        self.cancel_signal.notify_waiters();
    }

    /// Plays the next scripted dump: each step's progress after its delay. A
    /// cancel cuts the wait short; then `lag` more of the steps are sent at
    /// once (dbx noticing late), then `Cancelled`, and it ends `Ok`.
    async fn dump(&self, id: DumpId, request: DumpRequest, progress: DumpSink) -> Result<DumpEnd, CoreError> {
        let path = request.path.clone();
        self.record(Call::Dump(id, request));
        let Some(script) = lock(&self.dumps).pop_front() else { return unscripted("dump") };
        if let Some(bytes) = &script.writes {
            let _ = std::fs::write(&path, bytes);
        }
        let is_cancelled = || lock(&self.dump_cancels).contains(&id);
        let mut steps = script.steps.into_iter();
        let mut last = DumpProgress::new(DumpStatus::Running);
        let mut cancelled = is_cancelled();
        while !cancelled {
            let Some((delay, step)) = steps.next() else { break };
            if delay.is_zero() {
                last = step.clone();
                progress(step);
                cancelled = is_cancelled();
                continue;
            }
            let wait = tokio::time::sleep(delay);
            tokio::pin!(wait);
            loop {
                let signalled = self.cancel_signal.notified();
                tokio::pin!(signalled);
                signalled.as_mut().enable();
                if is_cancelled() {
                    cancelled = true;
                    break;
                }
                tokio::select! {
                    _ = &mut wait => break,
                    _ = signalled => {}
                }
            }
            if cancelled {
                // The step dbx was in the middle of is still reported.
                steps = std::iter::once((Duration::ZERO, step)).chain(steps).collect::<Vec<_>>().into_iter();
                break;
            }
            last = step.clone();
            progress(step);
        }
        if !cancelled {
            return script.end;
        }
        for (_, step) in steps.take(script.lag) {
            last = step.clone();
            progress(step);
        }
        let end = DumpProgress { status: DumpStatus::Cancelled, ..last };
        progress(end.clone());
        Ok(DumpEnd { last: Some(end) })
    }

    fn cancel_dump(&self, id: DumpId) {
        self.record(Call::CancelDump(id));
        lock(&self.dump_cancels).insert(id);
        self.cancel_signal.notify_waiters();
    }

    async fn close_session(&self, session: SessionId, at: &DatabasePath) -> Result<(), CoreError> {
        self.record(Call::CloseSession(session, at.clone()));
        let result = reply(&self.close_session, || unscripted("call")).await;
        if result.is_ok() {
            lock(&self.sessions).remove(&(session, at.clone()));
        }
        result
    }

    async fn save_history(&self, entry: &NewHistoryEntry) -> Result<(), CoreError> {
        lock(&self.saved_history).push(entry.clone());
        if lock(&self.save_history).is_empty() {
            return Ok(());
        }
        reply(&self.save_history, || unscripted("save_history")).await
    }

    async fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, CoreError> {
        self.record(Call::History);
        lock(&self.history_queries).push(query.clone());
        reply(&self.history, || unscripted("history")).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    fn conn(name: &str) -> Connection {
        Connection { id: ConnectionId(name.into()), name: name.into(), kind: DbKind::Postgres, capabilities: Capabilities::postgres(), host: "localhost".into(), port: None, database: None, username: String::new(), tls: false, production: false, production_databases: Vec::new(), query_timeout: None }
    }

    fn draft() -> ConnectionDraft {
        ConnectionDraft {
            id: None,
            name: "d".into(),
            kind: DbKind::Postgres,
            host: "localhost".into(),
            port: Some(5432),
            database: Some("d".into()),
            username: "u".into(),
            password: None,
            tls: false,
            production: false,
        }
    }

    fn result_set() -> ResultSet {
        ResultSet {
            columns: vec![],
            rows: vec![],
            truncated: false,
            elapsed: Duration::ZERO,
            messages: vec![],
            affected: 0,
            ran: "select 1".into(),
            rerun: crate::core::Rerun::Proven,
        }
    }

    fn request() -> RunRequest {
        RunRequest {
            connection: ConnectionId("a".into()),
            database: None,
            session: SessionId { tab: TabId(1), serial: 1 },
            buffer: "select 1".into(),
            cursor: Cursor::default(),
            selection: None,
            row_cap: 1000,
            confirm_dangerous: true,
            confirmation: None,
        }
    }

    #[tokio::test]
    async fn fake_core_replays_scripted_replies_in_order_then_reports_unscripted() {
        let core = FakeCore::new();
        core.script_connections(Scripted::ok(vec![conn("a")]));
        core.script_connections(Scripted::ok(vec![conn("b"), conn("c")]));
        assert_eq!(core.connections().await.unwrap(), vec![conn("a")]);
        assert_eq!(core.connections().await.unwrap().len(), 2);
        let err = core.connections().await.unwrap_err();
        assert_eq!(err.kind, CoreErrorKind::Other);
        assert!(err.message.contains("no scripted reply"), "{err}");
    }

    #[tokio::test]
    async fn fake_core_empty_answers_the_connection_list_with_nothing() {
        assert_eq!(FakeCore::empty().connections().await.unwrap(), vec![]);
    }

    #[tokio::test]
    async fn every_unscripted_call_is_an_error_not_a_silent_success() {
        let core = FakeCore::new();
        let id = ConnectionId("a".into());
        let db = DatabasePath { connection: id.clone(), database: "d".into() };
        let schema = SchemaPath { database: db.clone(), schema: "s".into() };
        let table = TablePath { schema: schema.clone(), table: "t".into() };
        let draft = draft();
        let results = [
            core.connections().await.err(),
            core.save_connection(&draft).await.err(),
            core.remove_connection(&id).await.err(),
            core.test_connection(&draft).await.err(),
            core.databases(&id).await.err(),
            core.schemas(&db).await.err(),
            core.tables(&schema).await.err(),
            core.columns(&table).await.err(),
            core.foreign_keys(&table).await.err(),
            core.complete(&CompletionRequest { at: db.clone(), schema: None, kind: SearchKind::Relation, prefix: String::new(), limit: 10, global: false }).await.err(),
            core.current_schema(&db).await.err(),
            core.run(RunId(1), request()).await.err(),
            core.close_session(SessionId { tab: TabId(1), serial: 1 }, &db).await.err(),
            core.history(&HistoryQuery::default()).await.err(),
        ];
        for (i, r) in results.iter().enumerate() {
            let e = r.as_ref().unwrap_or_else(|| panic!("call {i} succeeded unscripted"));
            assert_eq!(e.kind, CoreErrorKind::Other, "call {i}");
        }
    }

    #[tokio::test]
    async fn a_run_cancelled_before_it_starts_is_cancelled_every_time_and_keeps_its_script() {
        for trial in 0..200 {
            let core = FakeCore::new();
            core.script_run(Scripted::ok(result_set()));
            core.cancel(RunId(1));
            let err = core.run(RunId(1), request()).await.unwrap_err();
            assert_eq!(err.kind, CoreErrorKind::Cancelled, "trial {trial}");
            // The scripted reply was not consumed by the cancelled run.
            assert_eq!(core.run(RunId(2), request()).await.unwrap(), result_set(), "trial {trial}");
        }
    }

    #[tokio::test]
    async fn fake_core_returns_scripted_errors() {
        let core = FakeCore::new();
        core.script_connections(Scripted::err(CoreErrorKind::Store, "store is locked"));
        let err = core.connections().await.unwrap_err();
        assert_eq!(err.kind, CoreErrorKind::Store);
        assert_eq!(err.message, "store is locked");
    }

    #[tokio::test(start_paused = true)]
    async fn fake_core_holds_a_reply_back_for_its_scripted_delay() {
        let core = Arc::new(FakeCore::new());
        core.script_connections(Scripted::ok(vec![conn("a")]).after(Duration::from_secs(5)));
        let task = tokio::spawn({
            let core = core.clone();
            async move { core.connections().await }
        });
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "reply arrived before its delay");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(task.await.unwrap().unwrap(), vec![conn("a")]);
    }

    #[tokio::test(start_paused = true)]
    async fn fake_core_cancel_ends_a_delayed_run_with_cancelled() {
        let core = Arc::new(FakeCore::new());
        core.script_run(Scripted::err(CoreErrorKind::Sql, "never seen").after(Duration::from_secs(60)));
        let task = tokio::spawn({
            let core = core.clone();
            async move { core.run(RunId(7), request()).await }
        });
        tokio::task::yield_now().await;
        core.cancel(RunId(7));
        let err = task.await.unwrap().unwrap_err();
        assert_eq!(err.kind, CoreErrorKind::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn a_deaf_run_ignores_a_cancel_and_ends_only_at_its_scripted_time() {
        let core = Arc::new(FakeCore::new());
        core.script_run(Scripted::ok(result_set()).after(Duration::from_secs(10)).deaf());
        let task = tokio::spawn({
            let core = core.clone();
            async move { core.run(RunId(7), request()).await }
        });
        tokio::task::yield_now().await;
        core.cancel(RunId(7));
        tokio::time::advance(Duration::from_secs(9)).await;
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "the cancel did not land");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(task.await.unwrap().unwrap(), result_set());
    }

    #[tokio::test(start_paused = true)]
    async fn closing_a_session_forgets_it_but_leaves_a_run_in_flight_on_it_to_finish_as_dbx_does() {
        let core = Arc::new(FakeCore::new());
        core.script_run(Scripted::ok(result_set()).after(Duration::from_secs(10)));
        core.script_close_session(Scripted::ok(()));
        let here = tokio::spawn({
            let core = core.clone();
            async move { core.run(RunId(1), request()).await }
        });
        tokio::task::yield_now().await;
        let at = DatabasePath { connection: ConnectionId("a".into()), database: String::new() };
        let other = SessionId { tab: TabId(1), serial: 2 };
        assert!(core.session_open(request().session, &at), "the run opened its session");
        assert!(!core.session_open(other, &at));
        core.close_session(request().session, &at).await.unwrap();
        assert!(!core.session_open(request().session, &at));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(here.await.unwrap().unwrap(), result_set(), "the run was not ended by the close");
    }

    #[tokio::test]
    async fn fake_core_records_the_calls_it_receives() {
        let core = FakeCore::new();
        let _ = core.connections().await;
        let _ = core.run(RunId(3), request()).await;
        core.cancel(RunId(3));
        assert_eq!(core.calls(), vec![Call::Connections, Call::Run(RunId(3)), Call::Cancel(RunId(3))]);
    }
}
