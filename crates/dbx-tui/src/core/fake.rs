//! `FakeCore`: the test double for the Core seam.
//!
//! Replies are scripted per method, as a queue. Each scripted reply can be a
//! value or an error and can be held back by a delay. When a queue is empty the
//! method answers with an empty success, so tests script only what they need.
//! Every call is recorded, and `cancel` ends a delayed `run` with `Cancelled`.

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
}

impl<T> Scripted<T> {
    pub fn ok(value: T) -> Self {
        Self { result: Ok(value), delay: Duration::ZERO }
    }

    pub fn err(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        Self { result: Err(CoreError::new(kind, message)), delay: Duration::ZERO }
    }

    /// Hold the reply back for `delay` (tokio time, so tests can pause it).
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
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
    Release(DatabasePath),
    Run(RunId),
    Cancel(RunId),
    CloseSession(TabId),
    History,
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
    history: Queue<Vec<HistoryEntry>>,
    calls: Mutex<Vec<Call>>,
    cancelled: Mutex<HashSet<RunId>>,
    cancel_signal: Notify,
    default_connections_empty: std::sync::atomic::AtomicBool,
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

    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    fn record(&self, call: Call) {
        lock(&self.calls).push(call);
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
    pub fn script_run(&self, r: Scripted<ResultSet>) {
        push(&self.run, r);
    }
    pub fn script_close_session(&self, r: Scripted<()>) {
        push(&self.close_session, r);
    }
    pub fn script_history(&self, r: Scripted<Vec<HistoryEntry>>) {
        push(&self.history, r);
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
        reply(&self.columns, || unscripted("columns")).await
    }

    async fn release(&self, at: &DatabasePath) -> Result<(), CoreError> {
        self.record(Call::Release(at.clone()));
        Ok(())
    }

    async fn run(&self, run: RunId, _request: RunRequest) -> Result<ResultSet, CoreError> {
        self.record(Call::Run(run));
        let cancelled = || CoreError::new(CoreErrorKind::Cancelled, "cancelled");
        let is_cancelled = || lock(&self.cancelled).contains(&run);
        // A run cancelled before it started never touches the script.
        if is_cancelled() {
            return Err(cancelled());
        }
        let wait = async {
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
            _ = wait => Err(cancelled()),
            r = reply(&self.run, || unscripted("run")) => r,
        }
    }

    fn cancel(&self, run: RunId) {
        self.record(Call::Cancel(run));
        lock(&self.cancelled).insert(run);
        self.cancel_signal.notify_waiters();
    }

    async fn close_session(&self, tab: TabId) -> Result<(), CoreError> {
        self.record(Call::CloseSession(tab));
        reply(&self.close_session, || unscripted("call")).await
    }

    async fn history(&self, _query: HistoryQuery) -> Result<Vec<HistoryEntry>, CoreError> {
        self.record(Call::History);
        reply(&self.history, || unscripted("history")).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    fn conn(name: &str) -> Connection {
        Connection { id: ConnectionId(name.into()), name: name.into(), kind: DbKind::Postgres, host: "localhost".into(), port: None, database: None, username: String::new(), tls: false, production: false }
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
            ran: "select 1".into(),
        }
    }

    fn request() -> RunRequest {
        RunRequest {
            connection: ConnectionId("a".into()),
            database: None,
            tab: TabId(1),
            buffer: "select 1".into(),
            cursor: Cursor::default(),
            selection: None,
            row_cap: 1000,
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
            core.run(RunId(1), request()).await.err(),
            core.close_session(TabId(1)).await.err(),
            core.history(HistoryQuery::default()).await.err(),
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

    #[tokio::test]
    async fn fake_core_records_the_calls_it_receives() {
        let core = FakeCore::new();
        let _ = core.connections().await;
        let _ = core.run(RunId(3), request()).await;
        core.cancel(RunId(3));
        assert_eq!(core.calls(), vec![Call::Connections, Call::Run(RunId(3)), Call::Cancel(RunId(3))]);
    }
}
