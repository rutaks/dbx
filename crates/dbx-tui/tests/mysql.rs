//! Layer 1, slice 23b: `DbxCore` against the MySQL fixture (`up-mysql.sh`:
//! MySQL 8.4 on 127.0.0.1:53307, root / dbxtui, databases `sakila`, `edge` and
//! `edge_other`). Run:
//! `cargo test -p dbx-tui --test mysql -- --test-threads=1`
//!
//! Scratch store, scratch key and scratch HOME (`tests/common/mod.rs`).
//! Scratch tables and databases are made and dropped by guards; fixture tables
//! are never written. `tests/ground_truth_mysql.rs` compares every cell.

mod common;

use common::*;
use dbx_tui::core::{
    Candidate, CandidateKind, Cell, CompletionRequest, ConnectionDraft, ConnectionId, Core, CoreError, CoreErrorKind, Cursor, DatabasePath, DbKind, DbxCore, DumpContent, DumpId,
    DumpStatus, ExportFormat, ExportRequest, Reason, ResultSet, RunId, RunRequest, SchemaPath, SearchKind, SessionId, Syntax, TabId, TableKind, TablePath, DEFAULT_ROW_CAP,
};

const PASSWORD: &str = "dbxtui";
const CONTAINER: &str = "dbxtui-fixtures-mysql-mysql-1";

async fn require_fixtures() {
    let up = tokio::net::TcpStream::connect("127.0.0.1:53307").await.is_ok();
    assert!(up, "the fixture MySQL is not listening on 127.0.0.1:53307: run crates/dbx-tui/tests/fixtures/up-mysql.sh");
}

/// What the `mysql` client prints for `sql` (tab-separated, no header, NULL as
/// the word, through `docker exec`, so the statement is not one of the tests'
/// connections). `database` is the one it starts in.
fn mysql(database: &str, sql: &str) -> String {
    mysql_as(database, sql, &[])
}

fn mysql_as(database: &str, sql: &str, extra: &[&str]) -> String {
    let mut args = vec!["exec", "-i", CONTAINER, "mysql", "-uroot", "-pdbxtui", "--default-character-set=utf8mb4", "-N", "-B"];
    args.extend(extra);
    args.extend(["-e", sql, database]);
    let out = std::process::Command::new("docker").args(&args).output().expect("docker exec mysql");
    assert!(out.status.success(), "mysql {sql}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string()
}

/// Runs the SQL file at `path` through the `mysql` client in the container,
/// starting in `database` (the file's own `USE` is not obeyed to a place it
/// should not: a test reads the file first).
fn mysql_file(database: &str, path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new("docker")
        .args(["exec", "-i", CONTAINER, "mysql", "-uroot", "-pdbxtui", "--default-character-set=utf8mb4", "--force", database])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("docker exec mysql");
    let mut stdin = child.stdin.take().expect("stdin");
    let bytes = std::fs::read(path).expect("the file");
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let out = child.wait_with_output().expect("mysql output");
    let _ = writer.join();
    let stderr: String = String::from_utf8_lossy(&out.stderr).lines().filter(|l| !l.contains("Using a password")).collect::<Vec<_>>().join("\n");
    if stderr.is_empty() {
        Ok(())
    } else {
        Err(stderr)
    }
}

/// A MySQL connection to the fixture, as the form saves it.
fn my_draft(name: &str, database: Option<&str>) -> ConnectionDraft {
    ConnectionDraft {
        id: None,
        name: name.into(),
        kind: DbKind::MySql,
        host: "127.0.0.1".into(),
        port: Some(53307),
        database: database.map(str::to_string),
        username: "root".into(),
        password: Some(PASSWORD.into()),
        tls: false,
        production: false,
    }
}

async fn saved(core: &DbxCore, database: &str) -> ConnectionId {
    core.save_connection(&my_draft("my", Some(database))).await.expect("save").id
}

/// The fixture saved as another dbx head would, with these markers.
async fn seeded_marked(core: &DbxCore, id: &str, production: bool, marked: &[&str], read_only: bool) -> ConnectionId {
    core.seed_config(serde_json::json!({
        "id": id, "name": format!("marked {id}"), "db_type": "mysql", "host": "127.0.0.1", "port": 53307,
        "username": "root", "password": PASSWORD, "database": "edge", "ssl": false, "driver_profile": null,
        "is_production": production, "production_databases": marked, "read_only": read_only,
    }))
    .await
    .expect("seed")
}

fn database(id: &ConnectionId, name: &str) -> DatabasePath {
    DatabasePath { connection: id.clone(), database: name.into() }
}

fn schema(id: &ConnectionId, db: &str) -> SchemaPath {
    SchemaPath { database: database(id, db), schema: String::new() }
}

fn table(id: &ConnectionId, db: &str, name: &str) -> TablePath {
    TablePath { schema: schema(id, db), table: name.into() }
}

fn tab_session(tab: u32) -> SessionId {
    SessionId { tab: TabId(tab), serial: 1 }
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1_000);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// A run request for `buffer` in `tab` on `database`, the cursor at its end.
fn run_request(id: &ConnectionId, database: &str, tab: u32, buffer: &str) -> RunRequest {
    let rows: Vec<&str> = buffer.split('\n').collect();
    let cursor = Cursor { row: rows.len() - 1, col: rows.last().unwrap().chars().count() };
    RunRequest {
        connection: id.clone(),
        database: Some(database.into()),
        session: tab_session(tab),
        buffer: buffer.into(),
        cursor,
        selection: None,
        row_cap: DEFAULT_ROW_CAP,
        confirm_dangerous: false, // The gate has its own tests, below.
        confirmation: None,
    }
}

/// A run of exactly `buffer` (all of it selected, as a browse tab runs its
/// page), the danger check as `confirm_dangerous`.
fn whole(id: &ConnectionId, database: &str, tab: u32, buffer: &str, confirm_dangerous: bool) -> RunRequest {
    let lines: Vec<&str> = buffer.split('\n').collect();
    let end = Cursor { row: lines.len() - 1, col: lines.last().unwrap().chars().count() };
    RunRequest { selection: Some((Cursor { row: 0, col: 0 }, end)), confirm_dangerous, ..run_request(id, database, tab, buffer) }
}

fn texts(rows: &[Vec<Cell>]) -> Vec<Vec<Option<String>>> {
    rows.iter()
        .map(|r| r.iter().map(|c| match c { Cell::Null => None, Cell::Value(v) => Some(v.clone()) }).collect())
        .collect()
}

fn one(set: &ResultSet) -> String {
    match set.rows.first().and_then(|r| r.first()) {
        Some(Cell::Value(v)) => v.clone(),
        other => panic!("one value expected, got {other:?}"),
    }
}

async fn run(core: &DbxCore, id: &ConnectionId, db: &str, sql: &str) -> Result<ResultSet, CoreError> {
    core.run(RunId(rand_id()), run_request(id, db, 1, sql)).await
}

async fn run_in(core: &DbxCore, id: &ConnectionId, db: &str, tab: u32, sql: &str) -> Result<ResultSet, CoreError> {
    core.run(RunId(rand_id()), run_request(id, db, tab, sql)).await
}

/// A scratch table `(id int primary key, v varchar(20))` with rows 1..=5, in
/// `db`, dropped when the guard is.
struct ScratchTable {
    db: String,
    name: String,
}

impl ScratchTable {
    fn new(db: &str, what: &str) -> Self {
        let name = format!("scratch_{}_{what}", std::process::id());
        mysql(db, &format!("drop table if exists `{name}`; create table `{name}` (id int primary key, v varchar(20)); insert into `{name}` values (1,'v1'),(2,'v2'),(3,'v3'),(4,'v4'),(5,'v5')"));
        ScratchTable { db: db.into(), name }
    }
    /// `` `db`.`name` ``
    fn t(&self) -> String {
        format!("`{}`.`{}`", self.db, self.name)
    }
    /// The table's rows, as the `mysql` client shows them, in key order.
    fn rows(&self) -> Vec<String> {
        mysql(&self.db, &format!("select concat(id, '|', v) from `{}` order by id", self.name)).lines().map(str::to_string).collect()
    }
}

impl Drop for ScratchTable {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["exec", CONTAINER, "mysql", "-uroot", "-pdbxtui", "-e", &format!("drop table if exists `{}`.`{}`", self.db, self.name)])
            .output();
    }
}

/// A scratch database, dropped when the guard is.
struct ScratchDatabase(String);

impl ScratchDatabase {
    fn new(what: &str) -> Self {
        let name = format!("dbxtui_{what}_{}", std::process::id());
        mysql("mysql", &format!("drop database if exists `{name}`; create database `{name}` default character set utf8mb4"));
        ScratchDatabase(name)
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["exec", CONTAINER, "mysql", "-uroot", "-pdbxtui", "-e", &format!("drop database if exists `{}`", self.0)])
            .output();
    }
}

// ---- the tree: databases, tables, columns -----------------------------------------

#[tokio::test]
async fn every_database_is_listed_with_the_system_ones_as_dbx_gives_them() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await; // saved with `sakila`
    let dbs = core.databases(&id).await.expect("databases");
    eprintln!("OBSERVED databases: {dbs:?}");
    for want in ["sakila", "edge", "edge_other"] {
        assert!(dbs.iter().any(|d| d == want), "{want} in {dbs:?}");
    }
    let server: Vec<String> = mysql("mysql", "show databases").lines().map(str::to_string).collect();
    let missing: Vec<&String> = server.iter().filter(|d| !dbs.contains(d)).collect();
    eprintln!("OBSERVED on the server but not listed: {missing:?}");
    for system in ["information_schema", "mysql", "performance_schema", "sys"] {
        assert_eq!(dbs.iter().any(|d| d == system), server.iter().any(|d| d == system), "{system}: dbx lists the system databases it is given by the server");
    }
}

#[tokio::test]
async fn the_tables_of_sakila_are_the_servers_with_views_told_apart() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await; // saved with `edge`: another database is browsed through its own pool
    let listed = core.tables(&schema(&id, "sakila")).await.expect("tables");
    let want: Vec<String> = mysql("information_schema", "select concat(table_name, ':', if(table_type = 'VIEW', 'view', 'table')) from tables where table_schema = 'sakila' order by table_name").lines().map(str::to_string).collect();
    assert_eq!(want.len(), 23, "16 tables and 7 views: {want:?}");
    let mut got: Vec<String> = listed.iter().map(|t| format!("{}:{}", t.name, if t.kind == TableKind::View { "view" } else { "table" })).collect();
    got.sort();
    let mut want_sorted = want.clone();
    want_sorted.sort();
    assert_eq!(got, want_sorted);
    // A database with names that need quoting.
    let edge = core.tables(&schema(&id, "edge")).await.expect("edge tables");
    for name in ["Mixed Case", "order", "a`b", "`x`; select sleep(0) #`", "gt_view"] {
        assert!(edge.iter().any(|t| t.name == name), "{name:?} in {:?}", edge.iter().map(|t| &t.name).collect::<Vec<_>>());
    }
    assert!(edge.iter().find(|t| t.name == "gt_view").is_some_and(|t| t.kind == TableKind::View));
}

#[tokio::test]
async fn the_columns_of_film_are_the_servers_with_the_primary_key_marked() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let cols = core.columns(&table(&id, "sakila", "film")).await.expect("columns");
    let want: Vec<String> = mysql(
        "information_schema",
        "select concat(column_name, '|', column_type, '|', is_nullable, '|', column_key) from columns where table_schema = 'sakila' and table_name = 'film' order by ordinal_position",
    )
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(cols.len(), want.len());
    for (c, w) in cols.iter().zip(&want) {
        let parts: Vec<&str> = w.split('|').collect();
        assert_eq!(c.name, parts[0]);
        assert_eq!(c.nullable, parts[2] == "YES", "{}: nullable", c.name);
        assert_eq!(c.primary_key, parts[3] == "PRI", "{}: primary key", c.name);
        eprintln!("OBSERVED film.{}: dbx type {:?}, the server's {:?}", c.name, c.db_type, parts[1]);
    }
    assert_eq!(cols.iter().filter(|c| c.primary_key).map(|c| c.name.as_str()).collect::<Vec<_>>(), ["film_id"]);
    // A composite key lists both of its columns.
    let key: Vec<String> = core.columns(&table(&id, "edge", "composite_key")).await.unwrap().into_iter().filter(|c| c.primary_key).map(|c| c.name).collect();
    assert_eq!(key, ["a", "b"]);
}

// ---- completion --------------------------------------------------------------------

fn search(id: &ConnectionId, db: &str, schema: Option<&str>, kind: SearchKind, prefix: &str) -> CompletionRequest {
    CompletionRequest { at: database(id, db), schema: schema.map(str::to_string), kind, prefix: prefix.into(), limit: 100, global: false }
}

fn kinds(found: &[Candidate]) -> Vec<(&str, CandidateKind)> {
    found.iter().map(|c| (c.name.as_str(), c.kind)).collect()
}

#[tokio::test]
async fn a_search_for_fil_in_sakila_returns_the_film_tables_and_view_and_the_database_may_be_in_either_field() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let want = [
        ("film", CandidateKind::Table),
        ("film_actor", CandidateKind::Table),
        ("film_category", CandidateKind::Table),
        ("film_list", CandidateKind::View),
        ("film_text", CandidateKind::Table),
    ];
    // The database to search is `schema` when it is given, else `database`
    // (dbx's MySQL search): the pool is the database's.
    for (db, schema) in [("sakila", None), ("sakila", Some("sakila")), ("edge", Some("sakila"))] {
        let found = core.complete(&search(&id, db, schema, SearchKind::Relation, "fil")).await.expect("complete");
        assert_eq!(kinds(&found.candidates), want, "database {db}, schema {schema:?}");
        assert!(found.candidates.iter().all(|c| c.schema.as_deref() == Some("sakila")), "{:?}", found.candidates);
    }
    // In the other direction: a search of `edge` from a pool on sakila.
    let found = core.complete(&search(&id, "sakila", Some("edge"), SearchKind::Relation, "Mix")).await.expect("complete");
    assert_eq!(kinds(&found.candidates), [("Mixed Case", CandidateKind::Table)]);
}

#[tokio::test]
async fn schemas_are_the_databases_and_functions_and_the_current_database_are_found() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let found = core.complete(&search(&id, "sakila", None, SearchKind::Schema, "edge")).await.expect("schemas");
    assert_eq!(kinds(&found.candidates), [("edge", CandidateKind::Schema), ("edge_other", CandidateKind::Schema)]);
    let found = core.complete(&search(&id, "sakila", Some("sakila"), SearchKind::Function, "get_cus")).await.expect("functions");
    eprintln!("OBSERVED functions: {:?}", found.candidates);
    assert!(found.candidates.iter().any(|c| c.name == "get_customer_balance"), "{:?}", found.candidates);
    // The current schema is the current database, from `select database()`.
    assert_eq!(core.current_schema(&database(&id, "sakila")).await.unwrap(), Some("sakila".to_string()));
    assert_eq!(core.current_schema(&database(&id, "edge_other")).await.unwrap(), Some("edge_other".to_string()));
    // Postgres's statement is an error on MySQL.
    let e = run(&core, &id, "sakila", "select current_schema()").await.expect_err("no such function");
    assert!(e.message.contains("current_schema does not exist"), "{}", e.message);
}

#[tokio::test]
async fn the_foreign_keys_of_film_actor_name_film_and_actor() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let keys = core.foreign_keys(&table(&id, "sakila", "film_actor")).await.expect("keys");
    let mut got: Vec<(String, String, String)> = keys.iter().map(|k| (k.column.clone(), k.ref_table.clone(), k.ref_column.clone())).collect();
    got.sort();
    eprintln!("OBSERVED film_actor keys: {keys:?}");
    assert_eq!(got, [("actor_id".to_string(), "actor".to_string(), "actor_id".to_string()), ("film_id".to_string(), "film".to_string(), "film_id".to_string())]);
}

// ---- running: sessions, transactions, errors ---------------------------------------

#[tokio::test]
async fn a_tabs_session_keeps_its_variables_and_temporary_tables_and_another_tab_does_not_see_them() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    run_in(&core, &id, "edge", 1, "set @x = 41").await.expect("set");
    run_in(&core, &id, "edge", 1, "create temporary table tmp_a (a int)").await.expect("temporary table");
    run_in(&core, &id, "edge", 1, "insert into tmp_a values (7)").await.expect("insert");
    assert_eq!(one(&run_in(&core, &id, "edge", 1, "select @x + 1").await.unwrap()), "42");
    assert_eq!(one(&run_in(&core, &id, "edge", 1, "select a from tmp_a").await.unwrap()), "7");
    assert_eq!(texts(&run_in(&core, &id, "edge", 2, "select @x").await.unwrap().rows), [[None]], "tab 2 has its own session");
    assert!(run_in(&core, &id, "edge", 2, "select a from tmp_a").await.is_err());
    // A USE lasts in the session and not in the tab's other session.
    run_in(&core, &id, "edge", 1, "use edge_other").await.expect("use");
    assert_eq!(one(&run_in(&core, &id, "edge", 1, "select database()").await.unwrap()), "edge_other");
    assert_eq!(one(&run_in(&core, &id, "edge", 2, "select database()").await.unwrap()), "edge");
    // Closing a session ends it; the next run is a new one.
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
    assert_eq!(texts(&run_in(&core, &id, "edge", 1, "select @x").await.unwrap().rows), [[None]], "a new session: nothing set");
    assert_eq!(one(&run_in(&core, &id, "edge", 1, "select database()").await.unwrap()), "edge", "and it starts where the tab's database is");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
    core.close_session(tab_session(2), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_transaction_opened_in_a_tab_stays_open_until_commit_or_rollback_like_it_does_on_postgres() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let t = ScratchTable::new("edge", "tx");
    // dbx would roll an opened transaction back at the end of the run that
    // opened it (its default for an auto-commit tab), after which the UPDATE
    // below would commit by itself and ROLLBACK would change nothing.
    run(&core, &id, "edge", "begin").await.expect("begin");
    run(&core, &id, "edge", &format!("update {} set v = 'tx' where id = 1", t.t())).await.expect("update");
    assert_eq!(one(&run(&core, &id, "edge", &format!("select v from {} where id = 1", t.t())).await.unwrap()), "tx", "the tab sees its own change");
    assert_eq!(t.rows()[0], "1|v1", "another connection does not: it is not committed");
    run(&core, &id, "edge", "rollback").await.expect("rollback");
    assert_eq!(one(&run(&core, &id, "edge", &format!("select v from {} where id = 1", t.t())).await.unwrap()), "v1", "rolled back");
    assert_eq!(t.rows()[0], "1|v1");
    // And a commit keeps it.
    run(&core, &id, "edge", "start transaction").await.expect("start");
    run(&core, &id, "edge", &format!("update {} set v = 'kept' where id = 2", t.t())).await.expect("update");
    assert_eq!(t.rows()[1], "2|v2");
    run(&core, &id, "edge", "commit").await.expect("commit");
    assert_eq!(t.rows()[1], "2|kept");
    // Closing the tab's session with a transaction open rolls it back.
    run(&core, &id, "edge", "begin").await.unwrap();
    run(&core, &id, "edge", &format!("update {} set v = 'lost' where id = 3", t.t())).await.unwrap();
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(t.rows()[2], "3|v3", "closing the session ends the transaction without committing it");
}

#[tokio::test]
async fn a_timestamp_is_shown_in_the_sessions_time_zone_as_the_server_gives_it_and_nothing_converts_it() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let sql = "select ts, ts6, dt, t from gt_temporal where id = 1";
    let utc = run(&core, &id, "edge", sql).await.expect("utc");
    assert_eq!(texts(&utc.rows), [[Some("2024-03-05 08:15:12".to_string()), Some("2024-03-05 08:15:12.123456".to_string()), Some("2024-03-05 13:45:12".to_string()), Some("13:45:12".to_string())]], "the server's zone is UTC; a DATETIME has none");
    run(&core, &id, "edge", "set time_zone = '+05:30'").await.expect("set the session's zone");
    let shifted = run(&core, &id, "edge", sql).await.expect("shifted");
    assert_eq!(
        texts(&shifted.rows),
        [[Some("2024-03-05 13:45:12".to_string()), Some("2024-03-05 13:45:12.123456".to_string()), Some("2024-03-05 13:45:12".to_string()), Some("13:45:12".to_string())]],
        "a TIMESTAMP moves with the session's zone, a DATETIME does not"
    );
    let theirs = mysql("edge", &format!("set time_zone = '+05:30'; {sql}"));
    assert_eq!(theirs, "2024-03-05 13:45:12\t2024-03-05 13:45:12.123456\t2024-03-05 13:45:12\t13:45:12", "the mysql client says the same");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_syntax_error_is_a_sql_error_with_the_servers_words_and_the_statement_that_ran() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let e = run(&core, &id, "edge", "selec 1").await.expect_err("syntax error");
    eprintln!("OBSERVED error: kind {:?}, message {:?}, position {:?}", e.kind, e.message, e.position);
    assert_eq!(e.kind, CoreErrorKind::Sql);
    assert!(e.message.contains("1064") && e.message.contains("syntax"), "{}", e.message);
    assert_eq!(e.statement.as_deref(), Some("selec 1"));
    assert!(!e.message.contains(PASSWORD));
    let e = run(&core, &id, "edge", "select * from no_such_table").await.expect_err("no table");
    assert!(e.message.contains("1146") && e.message.contains("doesn't exist"), "{}", e.message);
}

// ---- cancel ---------------------------------------------------------------------------

/// How many statements containing `tag` the server is running (read through
/// `docker exec`, whose own connection is left out).
fn running_on_server(tag: &str) -> usize {
    mysql("information_schema", &format!("select count(*) from processlist where info like '%{tag}%' and id <> connection_id() and command = 'Query'")).trim().parse().expect("a count")
}

async fn gone_within(tag: &str, secs: u64) -> Option<std::time::Duration> {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(secs) {
        if running_on_server(tag) == 0 {
            return Some(started.elapsed());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    None
}

fn tag(what: &str) -> String {
    format!("dbxtui_{what}_{}", std::process::id())
}

#[tokio::test]
async fn select_sleep_30_cancelled_after_a_second_reports_cancelled_leaves_nothing_in_the_processlist_and_the_session_survives() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved(&core, "edge").await;
    // What the session holds before the cancel.
    run(&core, &id, "edge", "set @x = 1").await.expect("set");
    run(&core, &id, "edge", "create temporary table tmp_c (a int)").await.unwrap();
    run(&core, &id, "edge", "insert into tmp_c values (7)").await.unwrap();
    let connection = one(&run(&core, &id, "edge", "select connection_id()").await.unwrap());
    let probe = tag("cancel");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select sleep(30) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "edge", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(running_on_server(&probe), 1, "the statement is running on the server");
    let asked = std::time::Instant::now();
    core.cancel(RunId(1));
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), running).await.expect("within three seconds").unwrap();
    let took = asked.elapsed();
    let err = result.expect_err("cancelled");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    let gone = gone_within(&probe, 1).await.expect("information_schema.processlist shows no such statement");
    eprintln!("OBSERVED cancel: the run returned {took:?} after the cancel; the server showed nothing running {gone:?} later");
    // dbx cancels with `KILL QUERY` from another connection: the session's
    // connection is still the same one, with its variable and temporary table.
    assert_eq!(one(&run(&core, &id, "edge", "select connection_id()").await.unwrap()), connection, "the same server connection");
    assert_eq!(one(&run(&core, &id, "edge", "select @x").await.unwrap()), "1", "a SET made before the cancel still holds");
    assert_eq!(one(&run(&core, &id, "edge", "select a from tmp_c").await.unwrap()), "7", "so does a temporary table");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_long_select_the_server_ends_with_error_1317_is_reported_cancelled_and_the_session_survives() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved(&core, "edge").await;
    run(&core, &id, "edge", "set @z = 9").await.expect("set");
    let probe = tag("heavy");
    // Three copies of the 16,049 payments joined, with a condition the server
    // has to test row by row (a bare count(*) of a cross join is multiplied
    // out at once): far longer than the test waits.
    let sql = format!("select count(*) as {probe} from sakila.payment a, sakila.payment b, sakila.payment c where a.amount + b.amount + c.amount > 1000");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), sql.clone());
        async move { core.run(RunId(5), run_request(&id, "edge", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(running_on_server(&probe), 1, "running on the server");
    core.cancel(RunId(5));
    let err = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("within five seconds").unwrap().expect_err("cancelled");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    assert!(!err.message.contains("SLEEP"), "this one is the server's own error, not the early answer of SLEEP(): {}", err.message);
    assert!(gone_within(&probe, 2).await.is_some(), "nothing left running");
    assert_eq!(one(&run(&core, &id, "edge", "select @z").await.unwrap()), "9", "the session is the one it was");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_write_the_server_answers_after_a_cancel_is_shown_with_its_result_and_a_note_not_as_cancelled() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved(&core, "edge").await;
    let t = ScratchTable::new("edge", "answered");
    // SLEEP returns 1 when it is interrupted, with no error: the INSERT goes on
    // with that value and the row is written. Calling this run "cancelled"
    // would say a row was not written that was.
    let sql = format!("insert into {} values (10, concat('s', sleep(30)))", t.t());
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), sql.clone());
        async move { core.run(RunId(6), run_request(&id, "edge", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    core.cancel(RunId(6));
    let set = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("within five seconds").unwrap().expect("a write is shown with its result");
    assert_eq!(set.affected, 1);
    assert!(set.messages.iter().any(|m| m.contains("cancel or the time limit was asked for")), "{:?}", set.messages);
    assert_eq!(t.rows().last().map(String::as_str), Some("10|s1"), "the row was written, with SLEEP's answer to being interrupted");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_cancel_that_lands_before_the_run_starts_stops_a_write_before_it_reaches_the_server() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let t = ScratchTable::new("edge", "late");
    // The cancel lands before the run starts (so nothing reaches the server): that is `Cancelled`.
    core.cancel(RunId(91));
    let e = core.run(RunId(91), run_request(&id, "edge", 1, &format!("update {} set v = 'x' where id = 1", t.t()))).await.expect_err("cancelled");
    assert_eq!(e.kind, CoreErrorKind::Cancelled);
    assert_eq!(t.rows()[0], "1|v1", "and the update never ran");
}

#[tokio::test]
async fn a_cancel_that_arrives_before_the_run_has_registered_is_not_lost() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    core.cancel(RunId(77));
    let err = core.run(RunId(77), run_request(&id, "edge", 1, "select sleep(30)")).await.expect_err("cancelled before it started");
    assert_eq!(err.kind, CoreErrorKind::Cancelled);
}

#[tokio::test]
async fn a_one_second_timeout_on_sleep_30_returns_timeout_leaves_nothing_running_and_the_session_survives() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    core.seed_config(serde_json::json!({
        "id": "timeout", "name": "timeout", "db_type": "mysql", "host": "127.0.0.1", "port": 53307,
        "username": "root", "password": PASSWORD, "database": "edge", "ssl": false, "driver_profile": null, "query_timeout_secs": 1,
    }))
    .await
    .expect("seed");
    let id = ConnectionId("timeout".into());
    run(&core, &id, "edge", "set @y = 5").await.unwrap();
    let probe = tag("timeout");
    let err = run(&core, &id, "edge", &format!("select sleep(30) as {probe}")).await.expect_err("timed out");
    assert_eq!(err.kind, CoreErrorKind::Timeout, "{err:?}");
    assert!(gone_within(&probe, 2).await.is_some(), "nothing left running");
    assert_eq!(one(&run(&core, &id, "edge", "select @y").await.unwrap()), "5", "the session is the one it was");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

// ---- the confirmation gate, against the server --------------------------------------

/// `delete` of row 1 of `target` run from `edge` with the danger check on: asked
/// as production, nothing changed; then run with the matching yes: exactly
/// row 1 goes.
async fn refused_then_confirmed(core: &DbxCore, id: &ConnectionId, target: &ScratchTable, named: &[&str]) {
    let before = target.rows();
    let sql = format!("delete from {} where id = 1", target.t());
    let refused = core.run(RunId(rand_id()), whole(id, "edge", 1, &sql, true)).await.expect_err("refused");
    assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation, "{refused:?}");
    let assessment = refused.assessment.expect("with the assessment");
    assert!(assessment.production);
    assert_eq!(assessment.reasons, vec![Reason::Writes]);
    assert_eq!(assessment.named, named);
    assert_eq!(assessment.confirmation.statement, sql);
    assert_eq!(target.rows(), before, "nothing ran");
    // A yes for id = 1 sent with id = 2 runs nothing.
    let other = format!("delete from {} where id = 2", target.t());
    let wrong = RunRequest { confirmation: Some(assessment.confirmation.clone()), ..whole(id, "edge", 1, &other, true) };
    assert_eq!(core.run(RunId(rand_id()), wrong).await.expect_err("asked again").kind, CoreErrorKind::NeedsConfirmation);
    assert_eq!(target.rows(), before, "still nothing ran");
    let yes = RunRequest { confirmation: Some(assessment.confirmation), ..whole(id, "edge", 1, &sql, true) };
    let set = core.run(RunId(rand_id()), yes).await.expect("confirmed, it runs");
    assert_eq!(set.affected, 1);
    assert_eq!(target.rows(), before[1..].to_vec(), "exactly row 1 is gone, every other row as it was");
    core.close_session(tab_session(1), &database(id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_delete_in_a_database_marked_production_is_asked_from_another_database_and_a_delete_in_the_tabs_own_is_not() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "marked-db", false, &["edge_other"], false).await;
    let there = ScratchTable::new("edge_other", "prod");
    let here = ScratchTable::new("edge", "plain");
    // The statement names `edge_other` while the tab is on `edge`.
    refused_then_confirmed(&core, &id, &there, &["edge_other"]).await;
    // The same on a scratch table in `edge` is not asked.
    let before = here.rows();
    let sql = format!("delete from {} where id = 1", here.t());
    let set = core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, true)).await.expect("not asked");
    assert_eq!(set.affected, 1);
    assert_eq!(here.rows(), before[1..].to_vec());
    // On the tab `edge_other` itself the plain name is production too.
    let sql = format!("delete from `{}` where id = 2", there.name);
    let refused = core.run(RunId(rand_id()), whole(&id, "edge_other", 1, &sql, true)).await.expect_err("asked");
    assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation);
    assert!(refused.assessment.expect("why").production);
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_delete_on_a_connection_marked_production_runs_only_with_the_matching_yes() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "prod", true, &[], false).await;
    let t = ScratchTable::new("edge", "conn");
    refused_then_confirmed(&core, &id, &t, &[]).await;
}

#[tokio::test]
async fn a_use_taints_its_session_so_a_write_afterwards_is_asked_until_the_session_closes() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "tainted", false, &["edge_other"], false).await;
    let here = ScratchTable::new("edge", "taint");
    // The USE itself is asked, and its yes switches the session.
    let refused = core.run(RunId(rand_id()), whole(&id, "edge", 1, "use edge_other", true)).await.expect_err("asked");
    let a = refused.assessment.expect("why");
    assert!(a.production && a.reasons == [Reason::SwitchesDatabase], "{a:?}");
    let yes = RunRequest { confirmation: Some(a.confirmation), ..whole(&id, "edge", 1, "use edge_other", true) };
    core.run(RunId(rand_id()), yes).await.expect("confirmed");
    // (`database()` is a function dbx cannot prove read-only, so it would be asked: a count of a table only edge_other has.)
    assert_eq!(one(&run(&core, &id, "edge", "select count(*) from t").await.unwrap()), "3", "the session moved: unqualified `t` is edge_other's");
    // A write by a name the tab's database has is now judged as aimed at production: the session may be anywhere.
    let sql = format!("delete from `{}` where id = 2", here.name);
    let refused = core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, true)).await.expect_err("asked");
    let a = refused.assessment.expect("why");
    assert!(a.production && a.reasons.contains(&Reason::SessionSwitched), "{a:?}");
    assert_eq!(here.rows().len(), 5, "nothing ran");
    // A proven read still runs.
    assert_eq!(one(&run(&core, &id, "edge", "select count(*) from t").await.unwrap()), "3", "unqualified `t` is edge_other's now");
    // A `USE edge` back to the unmarked database is asked too (the session is
    // tainted: anything not proven read-only is), and does not lift the taint.
    let back = core.run(RunId(rand_id()), whole(&id, "edge", 1, "use edge", true)).await.expect_err("asked");
    let a = back.assessment.expect("why");
    assert!(a.production && a.reasons.contains(&Reason::SessionSwitched), "{a:?}");
    let yes = RunRequest { confirmation: Some(a.confirmation), ..whole(&id, "edge", 1, "use edge", true) };
    core.run(RunId(rand_id()), yes).await.expect("confirmed");
    let still = core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, true)).await.expect_err("still asked");
    assert!(still.assessment.expect("why").reasons.contains(&Reason::SessionSwitched));
    // A closed session is a new one, on the tab's database: not tainted.
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
    core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, true)).await.expect("a new session, not asked");
    assert_eq!(here.rows().len(), 4);
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn text_the_servers_lexer_reads_differently_from_a_comment_scanner_cannot_get_a_write_past_the_gate() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "hidden", false, &["edge_other"], false).await;
    let there = ScratchTable::new("edge_other", "hidden");
    let before = there.rows();
    // The server runs both statements of each of these when sent whole (a
    // selection is sent as written); it is a write to production in each.
    for sql in [
        format!("select 5 --1; update {} set v = 'hidden' where id = 2", there.t()),
        format!("select 1 /*!50000 ; update {} set v = 'hidden' where id = 2 */", there.t()),
        format!("select 1 /* /* */; update {} set v = 'hidden' where id = 2; /* */", there.t()),
        format!("/*!50000 update {} set v = 'hidden' where id = 2 */", there.t()),
        format!("select 'a\\'; update {} set v = 'hidden' where id = 2; --'", there.t()),
    ] {
        let refused = core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, false)).await.expect_err(&format!("asked: {sql}"));
        assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation, "{sql}");
        assert!(refused.assessment.expect("why").production, "{sql}");
        assert_eq!(there.rows(), before, "nothing ran: {sql}");
    }
    // The proof the server really runs such text: on a connection nothing guards it does.
    let open = saved(&core, "edge").await;
    let sql = format!("select 5 --1; update {} set v = 'hidden' where id = 2", there.t());
    core.run(RunId(rand_id()), whole(&open, "edge", 1, &sql, false)).await.expect("runs");
    assert_eq!(there.rows()[1], "2|hidden", "both statements ran: the gate was the only thing in the way");
    core.close_session(tab_session(1), &database(&open, "edge")).await.unwrap();
}

include!("fixtures/mysql/gate_corpus.rs");

/// The SQL modes the differential test runs every text under: the server's
/// default and the two that change how quotes are read.
const SQL_MODES: [(&str, &str); 4] =
    [("default", ""), ("NO_BACKSLASH_ESCAPES", "NO_BACKSLASH_ESCAPES"), ("ANSI_QUOTES", "ANSI_QUOTES"), ("both", "NO_BACKSLASH_ESCAPES,ANSI_QUOTES")];

fn set_mode(extra: &str) -> String {
    if extra.is_empty() {
        "set session sql_mode = default".to_string()
    } else {
        format!("set session sql_mode = concat(@@global.sql_mode, ',{extra}')")
    }
}

/// The statements the server ran for `sql`, sent whole in tab `tab`'s
/// session: `performance_schema` keeps each statement of a multi-statement
/// text as an event of its own, with its own text.
async fn statements_the_server_ran(core: &DbxCore, id: &ConnectionId, db: &str, tab: u32, sql: &str) -> Vec<String> {
    let base = "set @dbxtui_base = (select coalesce(max(event_id), 0) from performance_schema.events_statements_history where thread_id = ps_current_thread_id())";
    run_in(core, id, db, tab, base).await.expect("the base");
    let _ = core.run(RunId(rand_id()), whole(id, db, tab, sql, false)).await;
    let ran = run_in(core, id, db, tab, "select sql_text from performance_schema.events_statements_history where thread_id = ps_current_thread_id() and event_id > @dbxtui_base order by event_id")
        .await
        .expect("the history");
    texts(&ran.rows).into_iter().filter_map(|r| r.into_iter().next().flatten()).collect()
}

/// A run sent whole that waits for a yes gets it, as the user would give it.
async fn confirmed(core: &DbxCore, id: &ConnectionId, db: &str, tab: u32, sql: &str) -> ResultSet {
    let asked = core.run(RunId(rand_id()), whole(id, db, tab, sql, false)).await;
    match asked {
        Ok(set) => set,
        Err(e) => {
            assert_eq!(e.kind, CoreErrorKind::NeedsConfirmation, "{sql}: {e:?}");
            let yes = RunRequest { confirmation: Some(e.assessment.expect("why").confirmation), ..whole(id, db, tab, sql, false) };
            core.run(RunId(rand_id()), yes).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"))
        }
    }
}

/// The client charsets the differential test runs every text under: dbx's
/// default, and each one with a two-byte character that can end in `\` or
/// `` ` `` (`CHARSET_CORPUS`). Set with a confirmed `SET NAMES` in the guarded
/// session, as a user would.
const CHARSETS: [&str; 6] = ["utf8mb4", "gbk", "sjis", "cp932", "gb18030", "big5"];

/// Every text the differential runs: the corpus, and the charset corpus (which
/// claims nothing: its second statement is hidden only under UTF-8).
fn differential_texts() -> Vec<(&'static str, bool)> {
    GATE_CORPUS.iter().copied().chain(CHARSET_CORPUS.iter().map(|t| (*t, false))).collect()
}

/// The differential test of the gate (slice 23b review): every text of the
/// corpus, with a second statement after it, under each client charset and
/// SQL mode. What the server ran is read from `performance_schema`; what the
/// gate read is `mysql_statements_read`; a write is sent through a production
/// connection's gate, unconfirmed, and must never reach the table; whatever
/// that gate lets through unasked ran on the server as one SELECT, or failed
/// to parse.
#[tokio::test]
async fn no_text_of_the_corpus_runs_a_statement_past_the_gate_that_it_did_not_see_in_any_sql_mode() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let db = ScratchDatabase::new("gate");
    mysql(&db.0, "create table w (k int primary key)");
    let open = saved(&core, &db.0).await;
    let guarded = seeded_marked(&core, "guarded", true, &[], false).await;
    let written = |k: usize| mysql(&db.0, &format!("select count(*) from w where k = {k}")) == "1";
    let corpus = differential_texts();
    let (mut table, mut failures) = (Vec::new(), Vec::new());
    for (c, cs) in CHARSETS.iter().enumerate() {
        if *cs != "utf8mb4" {
            run_in(&core, &open, &db.0, 1, &format!("set names {cs}")).await.expect("the charset");
            confirmed(&core, &guarded, &db.0, 2, &format!("set names {cs}")).await;
            // Results come back as UTF-8, which is what dbx reads them as;
            // the server reads statements in `character_set_client`.
            run_in(&core, &open, &db.0, 1, "set character_set_results = utf8mb4").await.expect("the results charset");
            confirmed(&core, &guarded, &db.0, 2, "set character_set_results = utf8mb4").await;
        }
        let client = one(&run_in(&core, &open, &db.0, 1, "select @@character_set_client").await.expect("the charset"));
        assert_eq!(client, *cs, "the session reads statements as {cs}");
        let thread = one(&confirmed(&core, &guarded, &db.0, 2, "select ps_current_thread_id()").await);
        for (m, (mode, extra)) in SQL_MODES.iter().enumerate() {
            let label = format!("{cs} x {mode}");
            run_in(&core, &open, &db.0, 1, &set_mode(extra)).await.expect("the mode");
            confirmed(&core, &guarded, &db.0, 2, &set_mode(extra)).await;
            let (mut ran_mark, mut ran_many, mut ran_write, mut asked) = (0, 0, 0, 0);
            for (n, (text, hidden)) in corpus.iter().enumerate() {
                let mark = with_statement(text, "select 'MARK'");
                let ran = statements_the_server_ran(&core, &open, &db.0, 1, &mark).await;
                ran_many += usize::from(ran.iter().filter(|s| !s.starts_with("set @dbxtui_base")).count() > 1);
                let server_ran_mark = ran.iter().any(|s| s.trim_start().starts_with("select 'MARK'"));
                let read = dbx_tui::core::mysql_statements_read(&mark, *cs != "utf8mb4");
                if server_ran_mark {
                    ran_mark += 1;
                    if !read.as_ref().is_none_or(|r| r.iter().any(|s| s == "select 'MARK'")) {
                        failures.push(format!("{label}: the server ran `select 'MARK'` as a statement of its own, and the gate read the text, unambiguously, as {read:?}: {mark:?} (the server ran {ran:?})"));
                    }
                }
                if *hidden && server_ran_mark {
                    failures.push(format!("{label}: the corpus says the second statement is hidden, and the server ran it: {mark:?}"));
                }
                // The write: what the server does with it unguarded, and that
                // the gate never lets it through.
                let (k_open, k_gated) = ((c * SQL_MODES.len() + m) * 1000 + n, (c * SQL_MODES.len() + m) * 1000 + 500 + n);
                let _ = core.run(RunId(rand_id()), whole(&open, &db.0, 1, &with_statement(text, &format!("insert into w values ({k_open})")), false)).await;
                let gated = with_statement(text, &format!("insert into w values ({k_gated})"));
                let base = one(&run_in(&core, &open, &db.0, 1, &format!("select coalesce(max(event_id), 0) from performance_schema.events_statements_history where thread_id = {thread}")).await.expect("the base"));
                let r = core.run(RunId(rand_id()), whole(&guarded, &db.0, 2, &gated, false)).await;
                let was_asked = matches!(&r, Err(e) if e.kind == CoreErrorKind::NeedsConfirmation);
                if !was_asked {
                    // Let through: the server ran one SELECT, or nothing.
                    let events = run_in(&core, &open, &db.0, 1, &format!("select event_name from performance_schema.events_statements_history where thread_id = {thread} and event_id > {base} order by event_id")).await.expect("the history");
                    // dbx's own: a ping before the run, a rollback after an error.
                    let events: Vec<String> = texts(&events.rows)
                        .into_iter()
                        .filter_map(|r| r.into_iter().next().flatten())
                        .filter(|e| !e.starts_with("statement/com/") && e != "statement/sql/rollback")
                        .collect();
                    if events.len() > 1 || events.iter().any(|e| e != "statement/sql/select" && e != "statement/sql/error") {
                        failures.push(format!("{label}: let through unasked, and the server ran {events:?}: {gated:?} ({r:?})"));
                    }
                }
                if written(k_open) {
                    ran_write += 1;
                    if !was_asked {
                        failures.push(format!("{label}: the server runs the write in {gated:?}, and the gate did not ask: {r:?}"));
                    }
                }
                asked += usize::from(was_asked);
                if written(k_gated) {
                    failures.push(format!("{label}: written past the gate: {gated:?} ({r:?})"));
                }
            }
            table.push(format!(
                "{label}: {} texts; the server ran more than one statement for {ran_many}, the second `select 'MARK'` for {ran_mark} and the write for {ran_write}; the gate asked about {asked} of the writes",
                corpus.len()
            ));
        }
        // A new session for the next charset: it starts as the connection does.
        for tab in [1, 2] {
            let id = if tab == 1 { &open } else { &guarded };
            core.close_session(tab_session(tab), &database(id, &db.0)).await.unwrap();
        }
    }
    eprintln!("OBSERVED the differential corpus:\n{}", table.join("\n"));
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// A connection whose URL parameters set a client charset that can swallow a
/// backslash or a backtick (dbx sends `SET NAMES` with it at connect): the
/// texts that hide a write from a UTF-8 reading are asked about on
/// production, and nothing is written; the same connection unguarded writes.
#[tokio::test]
async fn a_connection_whose_charset_parameter_can_swallow_a_backslash_asks_about_text_that_is_not_ascii() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let db = ScratchDatabase::new("charset");
    mysql(&db.0, "create table w (k int primary key)");
    let written = |k: usize| mysql(&db.0, &format!("select count(*) from w where k = {k}")) == "1";
    let mut observed = Vec::new();
    for (c, cs) in CHARSETS.iter().enumerate().skip(1) {
        let seed = |id: String, production: bool| {
            serde_json::json!({
                "id": id, "name": format!("charset {cs}"), "db_type": "mysql", "host": "127.0.0.1", "port": 53307,
                "username": "root", "password": PASSWORD, "database": db.0, "ssl": false, "driver_profile": null,
                "url_params": format!("charset={cs}"), "is_production": production, "production_databases": [], "read_only": false,
            })
        };
        let guarded = core.seed_config(seed(format!("guarded-{cs}"), true)).await.expect("seed");
        let open = core.seed_config(seed(format!("open-{cs}"), false)).await.expect("seed");
        let client = one(&run_in(&core, &open, &db.0, 1, "select @@character_set_client").await.expect("the charset"));
        assert_eq!(client, *cs, "dbx set the charset at connect");
        let (mut ran, mut asked) = (0, 0);
        for (n, text) in CHARSET_CORPUS.iter().enumerate() {
            let (k_open, k_gated) = (c * 100 + n, c * 100 + 50 + n);
            let _ = core.run(RunId(rand_id()), whole(&open, &db.0, 1, &with_statement(text, &format!("insert into w values ({k_open})")), false)).await;
            ran += usize::from(written(k_open));
            let gated = with_statement(text, &format!("insert into w values ({k_gated})"));
            let r = core.run(RunId(rand_id()), whole(&guarded, &db.0, 2, &gated, false)).await;
            let was_asked = matches!(&r, Err(e) if e.kind == CoreErrorKind::NeedsConfirmation);
            asked += usize::from(was_asked);
            assert!(was_asked, "{cs}: not asked: {gated:?} ({r:?})");
            assert!(!written(k_gated), "{cs}: written past the gate: {gated:?}");
        }
        observed.push(format!("{cs}: the server ran the hidden write for {ran} of {} texts unguarded; the gate asked about {asked}", CHARSET_CORPUS.len()));
        for (tab, id) in [(1, &open), (2, &guarded)] {
            core.close_session(tab_session(tab), &database(id, &db.0)).await.unwrap();
        }
    }
    eprintln!("OBSERVED charset parameters:\n{}", observed.join("\n"));
    assert!(observed.iter().any(|o| !o.contains("for 0 of")), "the server ran a hidden write somewhere: the test can fail");
}

#[tokio::test]
async fn a_cancelled_run_of_more_than_one_statement_says_earlier_statements_may_have_run() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved(&core, "edge").await;
    let t = ScratchTable::new("edge", "multi");
    let probe = tag("multi");
    // The server ends the second statement with error 1317 on the kill; the
    // first has run by then.
    let sql = format!(
        "update {} set v = 'changed' where id = 1; select count(*) as {probe} from sakila.payment a, sakila.payment b, sakila.payment c where a.amount + b.amount + c.amount > 1000",
        t.t()
    );
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), sql.clone());
        async move { core.run(RunId(41), whole(&id, "edge", 1, &sql, false)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(running_on_server(&probe), 1, "the second statement is running");
    core.cancel(RunId(41));
    let err = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("within five seconds").unwrap().expect_err("cancelled");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    assert!(err.outcome_unknown, "{err:?}");
    assert!(err.message.contains("earlier statements may have run"), "{}", err.message);
    assert_eq!(t.rows()[0], "1|changed", "and the first one had");
    // `SLEEP()` answers a kill with 1: the run ends with the first statement's
    // result, and says the statements after it may have run.
    let sql = format!("update {} set v = 'again' where id = 2; select sleep(5)", t.t());
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), sql.clone());
        async move { core.run(RunId(42), whole(&id, "edge", 1, &sql, false)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    core.cancel(RunId(42));
    let set = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("within five seconds").unwrap().expect("the first statement's result");
    assert!(set.messages.iter().any(|m| m.contains("may have run")), "{:?}", set.messages);
    assert_eq!(t.rows()[1], "2|again");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_read_dbx_cannot_prove_that_answers_after_a_cancel_is_shown_with_a_note_not_dropped_as_cancelled() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved(&core, "edge").await;
    let running = tokio::spawn({
        let (core, id) = (core.clone(), id.clone());
        async move { core.run(RunId(43), run_request(&id, "edge", 1, "select sleep(30) as s, database() as d")).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    core.cancel(RunId(43));
    let set = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("within five seconds").unwrap().expect("shown");
    assert_eq!(texts(&set.rows), vec![vec![Some("1".to_string()), Some("edge".to_string())]]);
    assert!(set.messages.iter().any(|m| m.contains("cancel or the time limit was asked for")), "{:?}", set.messages);
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn the_statements_only_mysql_has_are_asked_on_a_production_connection_and_change_nothing_until_confirmed() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "mysql-only", false, &["edge_other"], false).await;
    let t = ScratchTable::new("edge", "only");
    let before = mysql("mysql", "select @@global.max_connections, @@global.local_infile");
    for sql in [
        format!("handler {} open", t.t()),
        format!("lock tables {} write", t.t()),
        "set global max_connections = 7".to_string(),
        "set persist max_connections = 7".to_string(),
        "load data local infile '/etc/hosts' into table edge_other.t".to_string(),
        format!("select * from {} into outfile '/tmp/dbxtui_{}.txt'", t.t(), std::process::id()),
        format!("select * from {} into dumpfile '/tmp/dbxtui_{}.bin'", t.t(), std::process::id()),
        "kill 1".to_string(),
        "flush tables".to_string(),
        format!("prepare s from 'delete from {}'", t.t()),
        "execute s".to_string(),
        format!("call {}.nothing()", "edge_other"),
        format!("select edge_other.nothing(1) from {}", t.t()),
    ] {
        let refused = core.run(RunId(rand_id()), whole(&id, "edge", 1, &sql, false)).await.expect_err(&format!("asked: {sql}"));
        assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation, "{sql}");
        assert!(refused.assessment.expect("why").production, "{sql}");
    }
    assert_eq!(t.rows().len(), 5);
    assert_eq!(mysql("mysql", "select @@global.max_connections, @@global.local_infile"), before, "no global changed");
    assert_eq!(mysql("information_schema", "select count(*) from processlist where command = 'Killed'"), "0");
    assert_eq!(mysql("edge", &format!("select count(*) from information_schema.processlist where info like '%dbxtui_{}%' and id <> connection_id()", std::process::id())), "0");
}

#[tokio::test]
async fn load_data_local_infile_does_not_read_a_file_of_the_client_even_when_the_server_allows_it() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let t = ScratchTable::new("edge", "infile");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rows.tsv");
    std::fs::write(&file, "10\tfrom the client\n").unwrap();
    // The server's half of the switch, set back by the guard.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            let _ = std::process::Command::new("docker").args(["exec", CONTAINER, "mysql", "-uroot", "-pdbxtui", "-e", "set global local_infile = 0"]).output();
        }
    }
    let _reset = Reset;
    mysql("mysql", "set global local_infile = 1");
    let sql = format!("load data local infile '{}' into table {}", file.display(), t.t());
    let e = run(&core, &id, "edge", &sql).await.expect_err("the client does not send the file");
    // dbx sets no local-infile handler: the server's request for the file is
    // answered by closing the connection, so the session is lost with it,
    // after the statement was sent: what it did is not known to the client
    // (here, below, nothing), and it is not called a failure to connect.
    eprintln!("OBSERVED load data local infile with the server's switch on: {:?} {}", e.kind, e.message);
    assert!(e.outcome_unknown || e.kind == CoreErrorKind::Sql, "{e:?}");
    assert_ne!(e.kind, CoreErrorKind::Connect, "{e:?}");
    assert!(e.message.contains("closed") || e.message.contains("disabled") || e.message.contains("LOCAL"), "{}", e.message);
    assert_eq!(t.rows().len(), 5, "no row came from the client's file");
    assert_eq!(mysql("edge", &format!("select count(*) from {} where v like '%from the client%'", t.t())), "0");
    // dbx then refuses one statement, saying the session was lost (nothing it
    // held is on the new connection); the one after runs on it.
    let lost = run(&core, &id, "edge", "select 1").await.expect_err("the session is lost, and dbx says so");
    assert!(lost.message.contains("MYSQL_CLIENT_SESSION_LOST") && lost.message.contains("not executed"), "{}", lost.message);
    run(&core, &id, "edge", "select 1").await.expect("the tab runs again on a new connection");
    mysql("mysql", "set global local_infile = 0");
}

#[tokio::test]
async fn release_closes_the_server_connections_of_a_browsed_database_and_the_next_call_opens_them_again() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let on = |db: &str| -> usize { mysql("information_schema", &format!("select count(*) from processlist where db = '{db}' and id <> connection_id()")).trim().parse().unwrap() };
    let before = on("edge_other");
    core.tables(&schema(&id, "edge_other")).await.expect("tables of edge_other");
    let open = on("edge_other");
    assert!(open > before, "listing a database opens a server connection to it: {before} then {open}");
    core.release(&database(&id, "edge_other")).await.expect("release");
    let mut after = on("edge_other");
    for _ in 0..50 {
        if after == before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        after = on("edge_other");
    }
    assert_eq!(after, before, "released");
    assert!(core.tables(&schema(&id, "edge_other")).await.expect("again").iter().any(|t| t.name == "t"));
    core.release(&database(&id, "edge_other")).await.unwrap();
    core.release(&database(&ConnectionId("nope".into()), "edge")).await.expect("unknown connection");
}

#[tokio::test]
async fn tls_asked_for_is_a_tls_session_and_not_asked_for_is_taken_as_the_server_offers() {
    require_fixtures().await;
    // The binary installs the process-wide crypto provider at startup (dbx's TLS needs one).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let s = Scratch::new();
    let core = s.core();
    let on = core.save_connection(&ConnectionDraft { tls: true, ..my_draft("tls", Some("edge")) }).await.unwrap().id;
    let cipher = one(&run(&core, &on, "edge", "select variable_value from performance_schema.session_status where variable_name = 'Ssl_cipher'").await.expect("ssl status"));
    eprintln!("OBSERVED with the TLS box checked: Ssl_cipher {cipher:?}");
    assert!(!cipher.is_empty(), "the session is encrypted");
    let off = core.save_connection(&my_draft("plain", Some("edge"))).await.unwrap().id;
    let cipher = one(&run(&core, &off, "edge", "select variable_value from performance_schema.session_status where variable_name = 'Ssl_cipher'").await.expect("ssl status"));
    eprintln!("OBSERVED with it unchecked: Ssl_cipher {cipher:?}");
    core.test_connection(&ConnectionDraft { tls: true, ..my_draft("tls", Some("edge")) }).await.expect("a test with TLS");
}

#[tokio::test]
async fn a_wrong_password_says_dbxs_message_without_the_password_and_leaves_no_temporary_entry() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let wrong = "wrong-pw-Qx7-never-shown";
    let e = core.test_connection(&ConnectionDraft { password: Some(wrong.into()), ..my_draft("bad", Some("edge")) }).await.expect_err("wrong password");
    assert_eq!(e.kind, CoreErrorKind::Connect, "{e:?}");
    eprintln!("OBSERVED a wrong password: {}", e.message);
    assert!(e.message.to_lowercase().contains("access denied"), "{}", e.message);
    assert!(!e.message.contains(wrong) && !format!("{e:?}").contains(wrong));
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new());
    // The default port comes from the type.
    assert_eq!(core.capabilities(&DbKind::MySql).default_port, Some(3306));
}

// ---- browsing a table -------------------------------------------------------------

/// The columns of `path` that are its primary key, in the table's column order.
async fn key_of(core: &DbxCore, path: &TablePath) -> Vec<String> {
    core.columns(path).await.expect("columns").into_iter().filter(|c| c.primary_key).map(|c| c.name).collect()
}

/// The page of `path` the client asks for, run as a browse tab runs it: the
/// whole generated text as the selection, the row cap the page size.
async fn page_of(core: &DbxCore, id: &ConnectionId, path: &TablePath, limit: usize, offset: usize) -> (String, ResultSet) {
    let key = key_of(core, path).await;
    let sql = core.table_page_sql(&DbKind::MySql, path, &key, limit, offset);
    let request = RunRequest { row_cap: limit, ..whole(id, &path.schema.database.database, 1, &sql, false) };
    let set = core.run(RunId(rand_id()), request).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    (sql, set)
}

/// What `mysql` says `sql` returns on `db`: every value, NULL as `None`, rows
/// in the order it gives them. `-B` escapes tab and line break as `\t` and
/// `\n`, so a line is a row.
fn mysql_values(db: &str, sql: &str) -> Vec<Option<String>> {
    let sql = sql.trim_end().trim_end_matches(';');
    mysql(db, sql)
        .lines()
        .flat_map(|l| l.split('\t').map(|v| if v == "NULL" { None } else { Some(v.to_string()) }).collect::<Vec<_>>())
        .collect()
}

fn flat(rows: &[Vec<Cell>]) -> Vec<Option<String>> {
    rows.iter().flat_map(|r| r.iter().map(|c| match c { Cell::Null => None, Cell::Value(v) => Some(v.clone()) })).collect()
}

#[tokio::test]
async fn film_pages_one_two_and_the_last_match_the_mysql_client_cell_for_cell_and_the_count_is_1000() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let path = table(&id, "sakila", "film");
    assert_eq!(key_of(&core, &path).await, ["film_id"]);
    for (limit, offset) in [(50, 0), (50, 50), (50, 950), (7, 993)] {
        let (sql, set) = page_of(&core, &id, &path, limit, offset).await;
        assert!(sql.contains("ORDER BY `film_id`"), "{sql}");
        assert_eq!(set.rows.len(), limit.min(1000 - offset), "{sql}");
        assert_eq!(flat(&set.rows), mysql_values("sakila", &sql), "{sql}");
    }
    let count = core.table_count_sql(&DbKind::MySql, &path);
    let n = run(&core, &id, "sakila", &count).await.expect("count");
    assert_eq!(one(&n), "1000", "{count}");
}

#[tokio::test]
async fn a_table_with_a_composite_key_is_ordered_by_both_columns_and_one_with_no_key_says_it_has_no_order() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let path = table(&id, "edge", "composite_key");
    assert_eq!(key_of(&core, &path).await, ["a", "b"]);
    let (sql, set) = page_of(&core, &id, &path, 10, 0).await;
    assert!(sql.contains("ORDER BY `a`, `b`"), "{sql}");
    assert_eq!(flat(&set.rows), mysql_values("edge", &sql));
    assert_eq!(set.rows.iter().map(|r| format!("{:?}{:?}", r[0], r[1])).collect::<Vec<_>>().len(), 4);
    assert_eq!(texts(&set.rows).iter().map(|r| format!("{}{}", r[0].clone().unwrap(), r[1].clone().unwrap())).collect::<Vec<_>>(), ["11", "12", "21", "22"], "in key order, not insertion order");
    let none = table(&id, "edge", "no_pk");
    assert!(key_of(&core, &none).await.is_empty());
    let (sql, set) = page_of(&core, &id, &none, 10, 0).await;
    assert!(!sql.contains("ORDER BY"), "{sql}");
    assert_eq!(set.rows.len(), 4);
}

#[tokio::test]
async fn names_with_backticks_and_a_second_statement_inside_are_quoted_whole_and_run_as_one_statement() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    for (name, want) in [("a`b", "backtick in the name"), ("`x`; select sleep(0) #`", "one known row"), ("Mixed Case", "reserved"), ("order", "a")] {
        let path = table(&id, "edge", name);
        let key = key_of(&core, &path).await;
        let (sql, set) = page_of(&core, &id, &path, 10, 0).await;
        eprintln!("OBSERVED {name:?}: {sql}");
        assert!(!set.rows.is_empty(), "{name}: {sql}");
        assert!(texts(&set.rows).iter().any(|r| r.iter().any(|c| c.as_deref() == Some(want))), "{name}: {:?}", texts(&set.rows));
        assert!(set.columns.len() >= 2, "{name}");
        let n = run(&core, &id, "edge", &core.table_count_sql(&DbKind::MySql, &path)).await.expect("count");
        assert_eq!(one(&n), set.rows.len().to_string(), "{name}: the count");
        let _ = key;
    }
    // The hostile one is a single statement: no second result, and the text
    // is the name inside backticks, doubled.
    let path = table(&id, "edge", "`x`; select sleep(0) #`");
    let sql = core.table_page_sql(&DbKind::MySql, &path, &["id".to_string()], 10, 0);
    assert!(sql.contains("```x``; select sleep(0) #```"), "{sql}");
}

// ---- export ---------------------------------------------------------------------------

async fn held(core: &DbxCore, id: &ConnectionId, table: &str, order: &str) -> (ResultSet, String) {
    let sql = format!("select * from `edge`.`{}` order by {order}", table.replace('`', "``"));
    let set = core.run(RunId(rand_id()), RunRequest { row_cap: 1000, ..run_request(id, "edge", 1, &sql) }).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    (set, sql)
}

fn export_request(set: &ResultSet, table: &str, format: ExportFormat, schema: &str) -> ExportRequest {
    ExportRequest { format, columns: set.columns.clone(), rows: set.rows.clone(), kind: Some(DbKind::MySql), table: table.into(), schema: schema.into(), null_empty: false }
}

#[tokio::test]
async fn sql_inserts_of_edge_tables_run_into_a_scratch_database_give_back_the_same_rows() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let scratch = ScratchDatabase::new("export");
    let dir = tempfile::tempdir().unwrap();
    let mut lost = Vec::new();
    for (table, order) in [("text_values", "id"), ("numbers", "id"), ("gt_integers", "id"), ("gt_decimals", "id"), ("gt_text", "id"), ("composite_key", "a, b"), ("order", "`group`"), ("Mixed Case", "`Id`"), ("a`b", "id"), ("gt_floats", "id"), ("gt_temporal", "id"), ("gt_misc", "id")] {
        let (set, _) = held(&core, &id, table, order).await;
        mysql("mysql", &format!("create table `{0}`.`{1}` like `edge`.`{1}`", scratch.0, table.replace('`', "``")));
        let request = export_request(&set, table, ExportFormat::SqlInsert, &scratch.0);
        let sql = String::from_utf8(core.export_bytes(&request).unwrap_or_else(|e| panic!("{table}: {e}"))).expect("UTF-8");
        assert!(sql.contains(&scratch.0), "{table}: the statements name the scratch database: {}", sql.chars().take(300).collect::<String>());
        let file = dir.path().join("insert.sql");
        std::fs::write(&file, sql).unwrap();
        let loaded = mysql_file(&scratch.0, &file);
        let q = table.replace('`', "``");
        let same = loaded.is_ok()
            && mysql("mysql", &format!("checksum table `edge`.`{q}`, `{}`.`{q}`", scratch.0)).lines().map(|l| l.split('\t').nth(1).unwrap_or("").to_string()).collect::<std::collections::HashSet<_>>().len() == 1;
        eprintln!("OBSERVED export of {table}: loaded {:?}, identical {same}", loaded.as_ref().err().map(|e| e.chars().take(120).collect::<String>()));
        if !same {
            lost.push(table);
        }
    }
    eprintln!("OBSERVED tables whose INSERT export does not give the same rows back: {lost:?}");
    assert_eq!(lost, EXPORT_KNOWN_LOSSES, "the tables the README lists as not surviving an INSERT export, and no others");
    assert_float_loss(&scratch.0);
}

/// The `edge` tables whose SQL INSERT export (built from the text the grid
/// holds) does not load into a copy of the table with the same rows. Each is a
/// column type whose text is not what a literal of that type reads (README
/// "MySQL", Export).
const EXPORT_KNOWN_LOSSES: [&str; 1] = ["gt_floats"];

/// What is lost in `gt_floats` once its rows are in `db`: the FLOAT column
/// holds the six digits the grid showed (a different single-precision number
/// for five of the rows), and the DOUBLE column holds what it held, bit for bit.
fn assert_float_loss(db: &str) {
    let exact = |db: &str, col: &str| mysql(db, &format!("select group_concat(cast({col} as double) order by id) from gt_floats"));
    assert_eq!(exact(db, "f8"), exact("edge", "f8"), "the DOUBLE column comes back exactly");
    let (a, b) = (exact(db, "f4"), exact("edge", "f4"));
    let differing = a.split(',').zip(b.split(',')).filter(|(x, y)| x != y).count();
    eprintln!("OBSERVED FLOAT after the round trip: {a}\n                  before:                  {b}");
    assert_eq!(differing, 5, "the five FLOAT cells the ground truth lists as LOSS");
}

// ---- dump ----------------------------------------------------------------------------------

async fn dump_to(core: &DbxCore, id: &ConnectionId, db: &str, content: DumpContent, path: &std::path::Path) -> (Vec<dbx_tui::core::DumpProgress>, Result<dbx_tui::core::DumpEnd, CoreError>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let request = dbx_tui::core::DumpRequest { at: database(id, db), schema: String::new(), content, drop_tables: false, path: path.to_string_lossy().into_owned(), prevent_overwrite: true };
    let end = core.dump(DumpId(rand_id()), request, std::sync::Arc::new(move |p| sink.lock().unwrap().push(p))).await;
    let seen = seen.lock().unwrap().clone();
    (seen, end)
}

/// What a database holds, to compare a restore with its source: each base
/// table's row count and `CHECKSUM TABLE`, and how many views, routines and
/// triggers there are.
fn fingerprint(db: &str) -> Vec<String> {
    let tables: Vec<String> = mysql("information_schema", &format!("select table_name from tables where table_schema = '{db}' and table_type = 'BASE TABLE' order by table_name")).lines().map(str::to_string).collect();
    let mut out = Vec::new();
    for t in &tables {
        let n = mysql(db, &format!("select count(*) from `{}`", t.replace('`', "``")));
        let sum = mysql(db, &format!("checksum table `{}`", t.replace('`', "``"))).split('\t').nth(1).unwrap_or("").to_string();
        out.push(format!("table {t}: {n} rows, checksum {sum}"));
        // The structure as the server writes it back: columns, keys, foreign keys, charset.
        let ddl = mysql(db, &format!("show create table `{}`", t.replace('`', "``")));
        out.push(format!("ddl {t}: {}", ddl.split_once('\t').map_or(ddl.as_str(), |(_, d)| d)));
    }
    out.push(format!("views: {}", mysql("information_schema", &format!("select group_concat(table_name order by table_name) from views where table_schema = '{db}'"))));
    out.push(format!("routines: {}", mysql("information_schema", &format!("select group_concat(routine_type, ' ', routine_name order by routine_name) from routines where routine_schema = '{db}'"))));
    out.push(format!("triggers: {}", mysql("information_schema", &format!("select group_concat(trigger_name order by trigger_name) from triggers where trigger_schema = '{db}'"))));
    out
}

#[tokio::test]
async fn a_whole_sakila_dump_restores_into_a_scratch_database_with_the_same_rows_views_routines_and_triggers() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sakila.sql");
    let started = std::time::Instant::now();
    let (seen, end) = dump_to(&core, &id, "sakila", DumpContent::StructureAndData, &path).await;
    let end = end.expect("the dump ends");
    eprintln!("OBSERVED sakila dump: {:?}, {} progress reports, {} bytes, last {:?}", started.elapsed(), seen.len(), std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), end.last);
    assert_eq!(end.last.as_ref().map(|p| p.status), Some(DumpStatus::Done), "{end:?}");
    assert_eq!(end.last.as_ref().map(|p| p.errors), Some(0), "{end:?}");
    assert!(seen.iter().any(|p| p.status == DumpStatus::Running && p.rows > 0), "progress came while it ran");
    // The file names no database: loading it cannot touch `sakila`. (It is read
    // before it is loaded, and loaded in a scratch database only.)
    let text = std::fs::read_to_string(&path).unwrap();
    for line in text.lines().filter(|l| !l.starts_with('(') && !l.starts_with("INSERT")) {
        let l = line.to_lowercase();
        assert!(!l.starts_with("use ") && !l.contains("create database") && !l.contains("drop database") && !l.contains("`sakila`."), "the dump names a database: {line}");
    }
    let restore = ScratchDatabase::new("restore");
    mysql_file(&restore.0, &path).expect("the dump loads whole");
    let (want, got) = (fingerprint("sakila"), fingerprint(&restore.0));
    assert_eq!(got, want, "every table's rows and checksum, and its views, routines and triggers");
    assert_eq!(want.len(), 35, "16 tables, each with its structure, and three summary lines: {want:?}");
    assert!(want[0].contains("actor: 200 rows"), "{want:?}");
    // Only read: the fixture is as it was.
    assert_eq!(mysql("sakila", "select count(*) from film"), "1000");
}

#[tokio::test]
async fn a_structure_only_dump_has_no_rows_and_a_dump_never_overwrites_a_file_that_is_there() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "sakila").await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("structure.sql");
    let (_, end) = dump_to(&core, &id, "sakila", DumpContent::StructureOnly, &path).await;
    assert_eq!(end.expect("ends").last.as_ref().map(|p| p.status), Some(DumpStatus::Done));
    let text = std::fs::read_to_string(&path).unwrap();
    // (A trigger's body names INSERT INTO too: a row's statement starts a line.)
    assert!(text.contains("CREATE TABLE `film`") && !text.lines().any(|l| l.starts_with("INSERT INTO")), "structure only");
    let before = std::fs::read(&path).unwrap();
    let (_, second) = dump_to(&core, &id, "sakila", DumpContent::StructureAndData, &path).await;
    assert!(second.is_err(), "dbx refuses to touch a file that is there");
    assert_eq!(std::fs::read(&path).unwrap(), before, "and it is as it was");
}

#[tokio::test]
async fn the_edge_dump_restores_but_for_the_values_the_dump_writes_as_text() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved(&core, "edge").await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge.sql");
    let (_, end) = dump_to(&core, &id, "edge", DumpContent::StructureAndData, &path).await;
    let end = end.expect("ends");
    eprintln!("OBSERVED edge dump: {:?}", end.last);
    assert_eq!(end.last.as_ref().map(|p| p.status), Some(DumpStatus::Done), "{end:?}");
    assert_eq!(end.last.as_ref().map(|p| p.errors), Some(0), "{end:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.to_lowercase().contains("create database") && !text.contains("`edge`."));
    let restore = ScratchDatabase::new("edge");
    mysql_file(&restore.0, &path).expect("the dump loads whole, with no error from the mysql client");
    let (want, got) = (fingerprint("edge"), fingerprint(&restore.0));
    let differ: Vec<(&String, &String)> = want.iter().zip(&got).filter(|(a, b)| a != b).collect();
    eprintln!("OBSERVED lines of the edge fingerprint that differ after the restore:\n{differ:#?}");
    let lost: Vec<String> = differ.iter().map(|(a, _)| a.split(':').next().unwrap_or("").to_string()).collect();
    assert_eq!(lost, EDGE_DUMP_KNOWN_LOSSES, "the tables the README lists as not restoring whole, and no others");
    assert_float_loss(&restore.0);
}

/// The `edge` tables (and summary lines) that do not come back the same from
/// dbx's dump (README "MySQL", Dump): the FLOAT column's six digits.
const EDGE_DUMP_KNOWN_LOSSES: [&str; 1] = ["table gt_floats"];

#[tokio::test]
async fn a_dump_of_a_connection_with_a_data_charset_is_refused_with_dbxs_words() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    core.seed_config(serde_json::json!({
        "id": "gbk", "name": "gbk", "db_type": "mysql", "host": "127.0.0.1", "port": 53307,
        "username": "root", "password": PASSWORD, "database": "edge", "ssl": false, "driver_profile": null,
        "url_params": "dataCharset=GBK",
    }))
    .await
    .expect("seed");
    let id = ConnectionId("gbk".into());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gbk.sql");
    let (_, end) = dump_to(&core, &id, "edge", DumpContent::StructureAndData, &path).await;
    let e = end.expect_err("refused");
    eprintln!("OBSERVED a dump with dataCharset: {:?} {}", e.kind, e.message);
    assert!(e.message.contains("dataCharset") || e.message.contains("MYSQL_DATA_CHARSET"), "{}", e.message);
    assert!(!path.exists() || std::fs::read(&path).unwrap().is_empty(), "nothing was written");
}

#[allow(dead_code)]
fn unused(core: &DbxCore) {
    let _ = (Syntax::capabilities(core, &DbKind::MySql), TableKind::Table);
}
