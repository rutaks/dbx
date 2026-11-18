//! Layer 1 against the Docker fixtures: `DbxCore` saving, editing and testing
//! connections to a real Postgres.
//!
//! Needs `tests/fixtures/up.sh` first (Postgres on 127.0.0.1:55433, user
//! `postgres`, password `dbxtui`). Run:
//! `cargo test -p dbx-tui --test live -- --test-threads=1`
//!
//! Scratch store, scratch key and scratch HOME, as in `store.rs` (see
//! `tests/common/mod.rs`): never the keychain, never a real store.

mod common;

use common::*;
use dbx_tui::core::{
    Candidate, CandidateKind, Cell, CellKind, CompletionRequest, ConnectionDraft, ConnectionId, Core, CoreErrorKind, Cursor, DatabasePath, DbxCore, RunId, RunRequest,
    SchemaPath, SearchKind, SessionId, TabId, TableKind, TablePath, DEFAULT_ROW_CAP,
};

const FIXTURE_PASSWORD: &str = "dbxtui";
const WRONG_PASSWORD: &str = "wrong-pw-Qx7-never-shown";

fn fixture(name: &str, password: &str) -> ConnectionDraft {
    ConnectionDraft { password: Some(password.into()), production: false, ..draft(name) }
}

/// Fails with a pointer to `up.sh` when the fixtures are not up.
async fn require_fixtures() {
    let up = tokio::net::TcpStream::connect("127.0.0.1:55433").await.is_ok();
    assert!(up, "the fixture Postgres is not listening on 127.0.0.1:55433: run crates/dbx-tui/tests/fixtures/up.sh");
}

#[tokio::test]
async fn a_saved_connection_runs_after_the_store_is_reopened_which_proves_the_password_round_tripped() {
    require_fixtures().await;
    let s = Scratch::new();
    let saved = s.core().save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.expect("save");

    // A new process: dbx reads the store and decrypts the password with the key.
    let reopened = s.core();
    let list = reopened.connections().await.expect("reopen");
    assert_eq!(list, vec![saved.clone()]);
    reopened.probe_select_one(&saved.id).await.expect("a run succeeds with the decrypted password");
}

#[tokio::test]
async fn a_test_with_the_right_password_succeeds_and_leaves_nothing_behind() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    core.test_connection(&fixture("unsaved", FIXTURE_PASSWORD)).await.expect("the fixture accepts it");
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new(), "no temporary entry");
    assert_eq!(core.connections().await.unwrap(), vec![], "and nothing was saved");
}

#[tokio::test]
async fn a_test_with_a_wrong_password_fails_with_dbxs_message_and_leaves_no_temporary_entry() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let err = core.test_connection(&fixture("unsaved", WRONG_PASSWORD)).await.expect_err("wrong password");
    assert_eq!(err.kind, CoreErrorKind::Connect, "{err:?}");
    assert!(err.message.to_lowercase().contains("password authentication failed"), "dbx's message: {}", err.message);
    assert!(!err.message.contains(WRONG_PASSWORD), "no secret in it: {}", err.message);
    assert!(!format!("{err:?}").contains(WRONG_PASSWORD));
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new(), "dbx's runtime list has no temporary entry");
    assert_eq!(s.core().connections().await.unwrap(), vec![]);
}

#[tokio::test]
async fn testing_an_edit_with_a_blank_password_uses_the_stored_password() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    let blank = ConnectionDraft { id: Some(saved.id.clone()), password: None, ..fixture("pagila", "") };
    core.test_connection(&blank).await.expect("the stored password is used");
    let wrong = ConnectionDraft { password: Some(WRONG_PASSWORD.into()), ..blank };
    core.test_connection(&wrong).await.expect_err("a typed password wins over the stored one");
    assert_eq!(core.runtime_config_ids().await, vec![saved.id.0.clone()], "only the saved connection is listed");
}

#[tokio::test]
async fn editing_the_port_of_a_connected_connection_makes_the_next_run_use_the_new_port() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    core.probe_select_one(&saved.id).await.expect("connected: a pool for port 55433 now exists");

    // Nothing listens on port 1. If the old pool were kept, this run would still succeed.
    let edit = |port| ConnectionDraft { id: Some(saved.id.clone()), port: Some(port), password: None, ..fixture("pagila", "") };
    core.save_connection(&edit(1)).await.expect("edit the port");
    let err = core.probe_select_one(&saved.id).await.expect_err("the next run uses port 1, not the old pool");
    assert_eq!(err.kind, CoreErrorKind::Connect);

    core.save_connection(&edit(55433)).await.expect("edit it back");
    core.probe_select_one(&saved.id).await.expect("and the next run is back on the fixture");
}

#[tokio::test]
async fn editing_only_the_name_of_a_connected_connection_keeps_it_connected() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    core.probe_select_one(&saved.id).await.unwrap();
    let rename = ConnectionDraft { id: Some(saved.id.clone()), password: None, ..fixture("renamed", "") };
    core.save_connection(&rename).await.expect("rename");
    core.probe_select_one(&saved.id).await.expect("still works");
}

// ---- slice 04: the schema tree's calls ------------------------------------

async fn saved_fixture(core: &DbxCore) -> ConnectionId {
    core.save_connection(&fixture("pg", FIXTURE_PASSWORD)).await.expect("save").id
}

fn database(id: &ConnectionId, name: &str) -> DatabasePath {
    DatabasePath { connection: id.clone(), database: name.into() }
}

fn schema(id: &ConnectionId, db: &str, name: &str) -> SchemaPath {
    SchemaPath { database: database(id, db), schema: name.into() }
}

fn table(id: &ConnectionId, db: &str, schema_name: &str, name: &str) -> TablePath {
    TablePath { schema: schema(id, db, schema_name), table: name.into() }
}

#[tokio::test]
async fn every_database_on_the_server_is_listed_not_only_the_one_the_connection_was_saved_with() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await; // saved with database `pagila`
    let dbs = core.databases(&id).await.expect("databases");
    for want in ["pagila", "edge", "postgres"] {
        assert!(dbs.iter().any(|d| d == want), "{want} in {dbs:?}");
    }
    eprintln!("OBSERVED databases: {dbs:?}");
}

#[tokio::test]
async fn schemas_of_a_database_leave_out_the_system_schemas() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let schemas = core.schemas(&database(&id, "pagila")).await.expect("schemas");
    assert!(schemas.iter().any(|n| n == "public"), "{schemas:?}");
    for hidden in ["pg_catalog", "information_schema"] {
        assert!(!schemas.iter().any(|n| n == hidden), "{hidden} is hidden: {schemas:?}");
    }
    assert!(!schemas.iter().any(|n| n.starts_with("pg_toast")), "{schemas:?}");
    eprintln!("OBSERVED pagila schemas: {schemas:?}");
}

#[tokio::test]
async fn another_database_lists_its_own_schemas_through_its_own_pool() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let edge = core.schemas(&database(&id, "edge")).await.expect("edge schemas");
    assert!(edge.iter().any(|n| n == "other"), "the edge database has a second schema: {edge:?}");
    let pagila = core.schemas(&database(&id, "pagila")).await.expect("pagila schemas");
    assert!(!pagila.iter().any(|n| n == "other"), "and pagila does not: {pagila:?}");
}

#[tokio::test]
async fn tables_and_views_are_told_apart_and_partitions_of_a_partitioned_table_are_left_out() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let tables = core.tables(&schema(&id, "pagila", "public")).await.expect("tables");
    let kind = |name: &str| tables.iter().find(|t| t.name == name).map(|t| t.kind);
    assert_eq!(kind("film"), Some(TableKind::Table), "{tables:?}");
    assert_eq!(kind("film_list"), Some(TableKind::View));
    assert_eq!(kind("payment"), Some(TableKind::Table), "the partitioned table itself stays");
    let partitions: Vec<&str> = tables.iter().map(|t| t.name.as_str()).filter(|n| n.starts_with("payment_p")).collect();
    assert_eq!(partitions, Vec::<&str>::new(), "its partitions do not clutter the list");
    eprintln!("OBSERVED pagila.public: {} entries", tables.len());
    for t in &tables {
        eprintln!("  {:?} {}", t.kind, t.name);
    }
}

#[tokio::test]
async fn columns_carry_type_nullability_primary_key_and_enum_type() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let cols = core.columns(&table(&id, "pagila", "public", "film")).await.expect("columns");
    let col = |name: &str| cols.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("{name} in {cols:?}"));
    assert!(col("film_id").primary_key);
    assert!(!col("film_id").nullable);
    assert!(!col("title").primary_key && !col("title").nullable);
    assert!(col("description").nullable);
    assert_eq!(col("rating").db_type, "mpaa_rating", "an enum shows its type's name");
    eprintln!("OBSERVED film columns: {cols:#?}");
}

#[tokio::test]
async fn a_view_and_a_materialized_view_list_their_columns_like_a_table() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    for name in ["film_list", "rental_by_category"] {
        let cols = core.columns(&table(&id, "pagila", "public", name)).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!cols.is_empty(), "{name} has columns");
        assert!(cols.iter().all(|c| !c.db_type.is_empty()), "{name}: {cols:?}");
    }
}

#[tokio::test]
async fn a_table_named_with_mixed_case_and_spaces_in_the_edge_database_comes_back_exactly() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let tables = core.tables(&schema(&id, "edge", "public")).await.expect("tables");
    assert!(tables.iter().any(|t| t.name == "Mixed Case"), "{tables:?}");
    let cols = core.columns(&table(&id, "edge", "public", "Mixed Case")).await.expect("columns of a quoted name");
    assert!(cols.iter().any(|c| c.name == "select"), "a reserved word as a column name: {cols:?}");
}

#[tokio::test]
async fn a_failed_listing_says_why_without_the_password_and_the_next_call_works() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let bad = core.save_connection(&fixture("bad", WRONG_PASSWORD)).await.unwrap().id;
    let err = core.databases(&bad).await.expect_err("wrong password");
    assert_eq!(err.kind, CoreErrorKind::Connect, "{err:?}");
    assert!(err.message.to_lowercase().contains("password authentication failed"), "{}", err.message);
    assert!(!err.message.contains(WRONG_PASSWORD) && !format!("{err:?}").contains(WRONG_PASSWORD));

    let good = saved_fixture(&core).await;
    let missing = core.schemas(&database(&good, "no_such_database")).await.expect_err("no such database");
    assert!(missing.message.contains("no_such_database"), "{}", missing.message);
    core.databases(&good).await.expect("a failure leaves the connection usable");

    let unknown = core.databases(&ConnectionId("nope".into())).await.expect_err("no such connection");
    assert!(!unknown.message.is_empty());
}

#[tokio::test]
async fn a_table_with_two_parents_is_listed_once() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let tables = core.tables(&schema(&id, "edge", "public")).await.expect("tables");
    let count = |name: &str| tables.iter().filter(|t| t.name == name).count();
    assert_eq!(count("inherit_child"), 1, "one row, though pg_inherits has two for it: {tables:?}");
    assert_eq!((count("inherit_parent_a"), count("inherit_parent_b")), (1, 1), "{tables:?}");
    let mut names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "no name appears twice: {tables:?}");
}

/// The fixture server's view of the connections made over TCP to `db`
/// (`pg_stat_activity`, through `docker exec` into the fixture container, so
/// the query is not one of them): pid, state and application name.
fn server_connections(db: &str) -> Vec<String> {
    let sql = format!(
        "select pid || ' ' || coalesce(state, '') || ' ' || coalesce(application_name, '') from pg_stat_activity \
         where datname = '{db}' and client_addr is not null and backend_type = 'client backend' order by pid"
    );
    let out = std::process::Command::new("docker")
        .args(["exec", "dbxtui-fixtures-postgres-1", "psql", "-U", "postgres", "-d", "postgres", "--no-psqlrc", "-tA", "-c", &sql])
        .output()
        .expect("docker exec psql");
    assert!(out.status.success(), "psql: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

async fn wait_for_server_connections(db: &str, want: usize) -> Vec<String> {
    for _ in 0..50 {
        let now = server_connections(db);
        if now.len() == want {
            return now;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    server_connections(db)
}

#[tokio::test]
async fn releasing_a_database_closes_its_server_connections_and_the_next_call_opens_them_again() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await; // saved with database `pagila`
    let before = server_connections("edge");
    core.schemas(&database(&id, "edge")).await.expect("schemas of edge");
    let open = server_connections("edge");
    eprintln!("OBSERVED pg_stat_activity for edge, before: {before:?}");
    eprintln!("OBSERVED pg_stat_activity for edge, after expanding it: {open:?}");
    assert!(open.len() > before.len(), "listing a database opens a server connection to it: {before:?} then {open:?}");

    core.release(&database(&id, "edge")).await.expect("release");
    let after = wait_for_server_connections("edge", before.len()).await;
    eprintln!("OBSERVED pg_stat_activity for edge, after release: {after:?}");
    assert_eq!(after.len(), before.len(), "released: {after:?}");

    // Nothing is lost: the next call opens what it needs again.
    let schemas = core.schemas(&database(&id, "edge")).await.expect("schemas again");
    assert!(schemas.iter().any(|n| n == "other"), "{schemas:?}");
    assert!(server_connections("edge").len() > before.len());
    core.release(&database(&id, "edge")).await.unwrap();

    // Releasing what was never opened, or for a connection that is not there, is not an error.
    core.release(&database(&id, "postgres")).await.expect("never opened");
    core.release(&database(&ConnectionId("nope".into()), "edge")).await.expect("unknown connection");
    wait_for_server_connections("edge", before.len()).await;
}

// ---- slice 05: running statements -------------------------------------------

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
    }
}

/// The first session of tab `tab`.
fn tab_session(tab: u32) -> SessionId {
    SessionId { tab: TabId(tab), serial: 1 }
}

fn texts(rows: &[Vec<Cell>]) -> Vec<Vec<Option<String>>> {
    rows.iter()
        .map(|r| r.iter().map(|c| match c { Cell::Null => None, Cell::Value(v) => Some(v.clone()) }).collect())
        .collect()
}

#[tokio::test]
async fn select_1_returns_one_integer_column_and_the_cell_1() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let started = std::time::Instant::now();
    let set = core.run(RunId(1), run_request(&id, "pagila", 1, "select 1")).await.expect("select 1");
    eprintln!("OBSERVED select 1: wall {:?}, dbx elapsed {:?}, columns {:?}", started.elapsed(), set.elapsed, set.columns);
    assert_eq!(set.columns.len(), 1);
    assert_eq!(set.columns[0].kind, CellKind::Integer);
    assert_eq!(set.rows, vec![vec![Cell::Value("1".into())]]);
    assert_eq!(set.ran.trim(), "select 1");
    assert!(!set.truncated);

    let started = std::time::Instant::now();
    let again = core.run(RunId(2), run_request(&id, "pagila", 1, "select 1")).await.unwrap();
    eprintln!("OBSERVED select 1 again on the tab's session: wall {:?}, dbx elapsed {:?}", started.elapsed(), again.elapsed);
}

#[tokio::test]
async fn a_syntax_error_is_a_sql_error_with_its_position_in_the_statement_that_ran() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let err = core
        .run(RunId(1), run_request(&id, "pagila", 1, "select 1;\nselect title\n  fro film"))
        .await
        .expect_err("a syntax error");
    eprintln!("OBSERVED syntax error: {err:?}");
    assert_eq!(err.kind, CoreErrorKind::Sql);
    assert!(err.message.contains("syntax error"), "{}", err.message);
    assert!(!err.message.contains("SQL text omitted"), "{}", err.message);
    let ran = err.statement.as_deref().expect("the statement that ran");
    assert_eq!(ran.trim(), "select title\n  fro film", "only the statement under the cursor ran");
    let at = err.position.expect("Postgres gives a position");
    assert_eq!((at.line, at.column), (2, 7), "`film`, on the statement's second line");
}

#[tokio::test]
async fn set_search_path_lasts_for_the_tabs_next_run_and_not_in_another_tabs_session() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let run = |tab, sql: &str| core.run(RunId(rand_id()), run_request(&id, "edge", tab, sql));
    run(1, "set search_path to other").await.expect("set");
    let found = run(1, "select name from things").await.expect("`things` is in schema other");
    assert_eq!(texts(&found.rows), vec![vec![Some("in the second schema".to_string())]]);
    let elsewhere = run(2, "select name from things").await.expect_err("tab 2 has its own session");
    assert_eq!(elsewhere.kind, CoreErrorKind::Sql, "{elsewhere:?}");
    let path = run(2, "show search_path").await.unwrap();
    assert_eq!(texts(&path.rows), vec![vec![Some("\"$user\", public".to_string())]]);
    let path = run(1, "show search_path").await.unwrap();
    assert_eq!(texts(&path.rows), vec![vec![Some("other".to_string())]]);
}

#[tokio::test]
async fn closing_a_tabs_session_ends_it_and_releasing_the_database_does_not() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let run = |sql: &str| core.run(RunId(rand_id()), run_request(&id, "edge", 1, sql));
    run("set search_path to other").await.unwrap();
    // The sidebar collapsing `edge` releases its pool; the tab's session is not that pool.
    core.release(&database(&id, "edge")).await.unwrap();
    assert_eq!(texts(&run("show search_path").await.unwrap().rows), vec![vec![Some("other".to_string())]]);
    let before = server_connections("edge");
    core.close_session(tab_session(1), &database(&id, "edge")).await.expect("close");
    let after = wait_for_server_connections("edge", before.len() - 1).await;
    eprintln!("OBSERVED edge connections with the tab's session open: {before:?}; after closing it: {after:?}");
    assert_eq!(after.len(), before.len() - 1, "the session's server connection is gone");
    // The next run starts a new session, with the default search path.
    assert_eq!(texts(&run("show search_path").await.unwrap().rows), vec![vec![Some("\"$user\", public".to_string())]]);
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
    core.close_session(tab_session(9), &database(&id, "edge")).await.expect("closing a session that never existed is fine");
}

#[tokio::test]
async fn the_row_cap_cuts_a_long_result_and_says_it_was_truncated() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let mut request = run_request(&id, "pagila", 1, "select generate_series(1, 50) as n");
    request.row_cap = 10;
    let set = core.run(RunId(1), request).await.unwrap();
    assert_eq!(set.rows.len(), 10);
    assert!(set.truncated);
    let set = core.run(RunId(2), run_request(&id, "pagila", 1, "select generate_series(1, 50) as n")).await.unwrap();
    assert_eq!((set.rows.len(), set.truncated), (50, false));
}

#[tokio::test]
async fn a_statement_without_a_result_set_reports_the_rows_it_changed() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let run = |sql: &str| core.run(RunId(rand_id()), run_request(&id, "edge", 1, sql));
    // A temporary table lives in the tab's session and goes with it.
    run("create temp table scratch_05 (x int)").await.expect("create");
    let set = run("insert into scratch_05 values (1), (2), (3)").await.expect("insert");
    eprintln!("OBSERVED insert: columns {:?}, affected {}, messages {:?}", set.columns, set.affected, set.messages);
    assert!(set.columns.is_empty());
    assert_eq!(set.affected, 3);
    let empty = run("select x from scratch_05 where x > 99").await.unwrap();
    assert_eq!((empty.columns.len(), empty.rows.len()), (1, 0), "an empty result keeps its header");
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn nothing_at_the_cursor_is_an_error_and_nothing_is_sent() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let err = core.run(RunId(1), run_request(&id, "pagila", 1, "  \n ")).await.expect_err("nothing to run");
    assert_eq!(err.kind, CoreErrorKind::Other, "{err:?}");
}

#[tokio::test]
async fn edge_case_cells_come_back_as_exact_text_and_null_is_not_empty_or_the_text_null() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let run = |sql: &str| core.run(RunId(rand_id()), run_request(&id, "edge", 1, sql));
    let text = run("select id, v from text_values order by id").await.unwrap();
    eprintln!("OBSERVED text_values: {:?} {:?}", text.columns, texts(&text.rows));
    let v: Vec<Option<String>> = text.rows.iter().map(|r| texts(std::slice::from_ref(r))[0][1].clone()).collect();
    assert_eq!(v[0], None);
    assert_eq!(v[1].as_deref(), Some(""));
    assert_eq!(v[2].as_deref(), Some("NULL"));
    assert_eq!(v[5].as_deref(), Some("line1\nline2\n\tindented"));
    assert_eq!(v[6].as_deref(), Some("日本語 😀 👨‍👩‍👧 ａｂｃ"));
    let numbers = run("select big, exact, approx from numbers order by id").await.unwrap();
    eprintln!("OBSERVED numbers: {:?} {:?}", numbers.columns, texts(&numbers.rows));
    let n = texts(&numbers.rows);
    assert_eq!(n[0][0].as_deref(), Some("9007199254740993"));
    assert_eq!(n[0][1].as_deref(), Some("12345678901234567890.123456789"));
    assert_eq!(n[1][0].as_deref(), Some("-9223372036854775808"));
    assert_eq!(n[1][2].as_deref(), Some("0.30000000000000004"));
    assert_eq!(numbers.columns.iter().map(|c| c.kind).collect::<Vec<_>>(), vec![CellKind::Integer, CellKind::Decimal, CellKind::Float]);
    for sql in ["select * from temporal order by id", "select * from other_types order by id", "select 1::int as i, now() as t"] {
        let set = run(sql).await.unwrap();
        eprintln!("OBSERVED {sql}: {:?} {:?}", set.columns, texts(&set.rows));
    }
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1_000);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// The fixture connection saved as another dbx head would, with
/// `query_timeout_secs` set.
async fn seeded_with_timeout(core: &DbxCore, secs: u64) -> ConnectionId {
    seeded_with_timeout_at(core, secs, 55433).await
}

/// `seeded_with_timeout`, reached through `port` (a proxy).
async fn seeded_with_timeout_at(core: &DbxCore, secs: u64, port: u16) -> ConnectionId {
    core.seed_config(serde_json::json!({
        "id": format!("timeout-{secs}-{port}"),
        "name": format!("timeout {secs}"),
        "db_type": "postgres",
        "host": "127.0.0.1",
        "port": port,
        "username": "postgres",
        "password": FIXTURE_PASSWORD,
        "database": "pagila",
        "ssl": false,
        "driver_profile": null,
        "query_timeout_secs": secs,
    }))
    .await
    .expect("seed")
}

#[tokio::test]
async fn a_connection_with_a_1_s_query_timeout_cuts_a_5_s_statement_and_says_whose_limit_it_was() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_with_timeout(&core, 1).await;
    let started = std::time::Instant::now();
    let err = core.run(RunId(1), run_request(&id, "pagila", 1, "select pg_sleep(5)")).await.expect_err("cut");
    eprintln!("OBSERVED timeout after {:?}: {err:?}", started.elapsed());
    assert_eq!(err.kind, CoreErrorKind::Timeout, "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    assert!(err.message.contains("1 s") && err.message.contains("connection"), "{}", err.message);
}

/// dbx's own default is 30 s when a run asks for nothing; a run of ours asks
/// for no limit. Slow (31 s), so run on demand: `-- --ignored`.
#[tokio::test]
#[ignore]
async fn with_no_timeout_on_the_connection_a_statement_runs_past_dbxs_30_s_default() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let set = core.run(RunId(1), run_request(&id, "pagila", 1, "select pg_sleep(31) as slept")).await.expect("not cut at 30 s");
    assert_eq!(set.rows.len(), 1);
}

#[tokio::test]
async fn a_cancel_lands_even_while_another_core_call_holds_the_state_lock() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved_fixture(&core).await;
    let running = tokio::spawn({
        let (core, id) = (core.clone(), id.clone());
        async move { core.run(RunId(77), run_request(&id, "pagila", 1, "select pg_sleep(20)")).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let started = std::time::Instant::now();
    {
        let _busy = core.hold_state_lock().await;
        core.cancel(RunId(77));
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), running).await.expect("cancelled promptly").unwrap();
    eprintln!("OBSERVED cancel under a held lock: {:?} after {:?}", result.as_ref().err(), started.elapsed());
    assert_eq!(result.expect_err("cancelled").kind, CoreErrorKind::Cancelled);
}

// ---- slice 06: cancel, timeouts, sessions -------------------------------------------

/// How many backends on the fixture are running a statement containing
/// `tag` (read through `docker exec`, so this query is not one of them).
fn running_on_server(tag: &str) -> usize {
    let sql = format!(
        "select count(*) from pg_stat_activity where query like '%{tag}%' and pid <> pg_backend_pid() and state = 'active'"
    );
    let out = std::process::Command::new("docker")
        .args(["exec", "dbxtui-fixtures-postgres-1", "psql", "-U", "postgres", "-d", "postgres", "--no-psqlrc", "-tA", "-c", &sql])
        .output()
        .expect("docker exec psql");
    assert!(out.status.success(), "psql: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().parse().expect("a count")
}

/// Waits up to `secs` for nothing to be running with `tag`; returns how long it took.
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

/// A unique tag for one test's statements.
fn tag(what: &str) -> String {
    format!("dbxtui_{what}_{}", std::process::id())
}

#[tokio::test]
async fn pg_sleep_30_cancelled_after_a_second_returns_cancelled_within_three_seconds_leaves_nothing_running_and_the_tab_runs_again() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved_fixture(&core).await;
    let probe = tag("cancel");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select pg_sleep(30) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "pagila", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(running_on_server(&probe), 1, "the statement is running on the server");
    let asked = std::time::Instant::now();
    core.cancel(RunId(1));
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), running).await.expect("within three seconds").unwrap();
    let took = asked.elapsed();
    let err = result.expect_err("cancelled");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    let gone = gone_within(&probe, 1).await.expect("pg_stat_activity shows no such statement");
    eprintln!("OBSERVED cancel: the run returned {took:?} after the cancel; the server showed nothing running {gone:?} later");
    let next = core.run(RunId(2), run_request(&id, "pagila", 1, "select 1")).await.expect("the next run on the same tab");
    assert_eq!(next.rows, vec![vec![Cell::Value("1".into())]]);
}

#[tokio::test]
async fn a_cancel_that_arrives_before_the_run_has_registered_is_not_lost() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let probe = tag("early");
    // The cancel comes first, as when ctrl+c follows f5 before dbx has
    // registered the run.
    core.cancel(RunId(5));
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        core.run(RunId(5), run_request(&id, "pagila", 1, &format!("select pg_sleep(30) as {probe}"))),
    )
    .await
    .expect("a cancelled run returns promptly");
    assert_eq!(result.expect_err("cancelled").kind, CoreErrorKind::Cancelled);
    eprintln!("OBSERVED early cancel: returned after {:?}", started.elapsed());
    assert_eq!(running_on_server(&probe), 0);
}

#[tokio::test]
async fn a_one_second_timeout_on_pg_sleep_30_returns_timeout_and_leaves_nothing_running() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_with_timeout(&core, 1).await;
    let probe = tag("timeout");
    let started = std::time::Instant::now();
    let err = core
        .run(RunId(1), run_request(&id, "pagila", 1, &format!("select pg_sleep(30) as {probe}")))
        .await
        .expect_err("cut");
    eprintln!("OBSERVED timeout after {:?}: {err:?}", started.elapsed());
    assert_eq!(err.kind, CoreErrorKind::Timeout, "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    let gone = gone_within(&probe, 3).await;
    eprintln!("OBSERVED after the timeout, nothing running on the server after {gone:?}");
    assert!(gone.is_some(), "a timed-out statement is stopped on the server too");
}

#[tokio::test]
async fn closing_one_session_of_a_tab_leaves_a_later_session_of_the_same_tab_on_the_same_database_open() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let on = |serial: u32, sql: &str| {
        let mut r = run_request(&id, "edge", 1, sql);
        r.session = SessionId { tab: TabId(1), serial };
        r
    };
    // The tab on edge (session 1), then elsewhere, then on edge again (session 3).
    core.run(RunId(1), on(1, "set search_path to other")).await.unwrap();
    core.run(RunId(2), on(3, "create temp table kept_06 (x int)")).await.unwrap();
    // The late close of the first session.
    core.close_session(SessionId { tab: TabId(1), serial: 1 }, &database(&id, "edge")).await.unwrap();
    let still = core.run(RunId(3), on(3, "select count(*) from kept_06")).await.expect("session 3 still has its temp table");
    assert_eq!(still.rows, vec![vec![Cell::Value("0".into())]]);
    let fresh = core.run(RunId(4), on(1, "show search_path")).await.unwrap();
    assert_eq!(texts(&fresh.rows), vec![vec![Some("\"$user\", public".to_string())]], "session 1 was closed");
    for serial in [1, 3] {
        core.close_session(SessionId { tab: TabId(1), serial }, &database(&id, "edge")).await.unwrap();
    }
}

/// What the second cancel ("stop waiting") relies on and says: closing a
/// session whose statement is still running returns at once and does not
/// stop the statement. dbx keeps a busy Postgres connection until the run
/// lets go of it; the statement runs to its end and the connection goes after.
#[tokio::test]
async fn closing_a_busy_session_returns_at_once_and_its_connection_goes_only_when_the_statement_ends() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved_fixture(&core).await;
    let probe = tag("close_busy");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select pg_sleep(3) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "pagila", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let during = server_connections("pagila");
    let asked = std::time::Instant::now();
    core.close_session(tab_session(1), &database(&id, "pagila")).await.expect("close");
    let close_took = asked.elapsed();
    eprintln!("OBSERVED close_session of a busy session returned after {close_took:?}");
    assert!(close_took < std::time::Duration::from_millis(500), "{close_took:?}");
    assert_eq!(running_on_server(&probe), 1, "the statement still runs");
    assert_eq!(server_connections("pagila").len(), during.len(), "and its connection is still there");
    let result = running.await.unwrap();
    eprintln!("OBSERVED the run under a closed session ended after {:?}: {:?}", asked.elapsed(), result.as_ref().map(|r| r.rows.len()));
    assert!(result.is_ok(), "it ran to its end");
    let after = wait_for_server_connections("pagila", during.len() - 1).await;
    assert_eq!(after.len(), during.len() - 1, "the connection went once the statement ended: {after:?}");
}

#[tokio::test]
async fn removing_a_connection_after_its_cancelled_run_has_ended_leaves_nothing_running() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved_fixture(&core).await;
    let probe = tag("remove");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select pg_sleep(30) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "pagila", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    // The app cancels first; the shell holds the removal until the run ends.
    core.cancel(RunId(1));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), running).await.expect("the run ended");
    core.remove_connection(&id).await.expect("remove");
    assert!(gone_within(&probe, 2).await.is_some());
}

/// A local proxy to the fixture that passes every Postgres connection through
/// except a cancel request (protocol code 80877102 near the start of a
/// connection), which it holds open unanswered: a cancel that never reaches
/// the server. Returns its port.
async fn cancel_swallowing_proxy() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let Ok(upstream) = tokio::net::TcpStream::connect("127.0.0.1:55433").await else { return };
                let (mut cr, mut cw) = client.into_split();
                let (mut ur, mut uw) = upstream.into_split();
                let down = tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut ur, &mut cw).await;
                });
                let mut seen = Vec::new();
                let mut buf = vec![0u8; 65536];
                loop {
                    let n = match cr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if seen.len() < 64 {
                        seen.extend_from_slice(&buf[..n]);
                        if seen.windows(4).take(60).any(|w| w == 80877102u32.to_be_bytes()) {
                            // Held: never forwarded, never answered.
                            std::future::pending::<()>().await;
                        }
                    }
                    if uw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                let _ = uw.shutdown().await;
                let _ = down.await;
            });
        }
    });
    port
}

#[tokio::test]
async fn a_cancel_that_never_reaches_the_server_does_not_end_the_run_before_the_statement_has() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let port = cancel_swallowing_proxy().await;
    let id = core
        .save_connection(&ConnectionDraft { port: Some(port), ..fixture("proxied", FIXTURE_PASSWORD) })
        .await
        .expect("save")
        .id;
    let probe = tag("unlanded");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select pg_sleep(4) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "pagila", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let asked = std::time::Instant::now();
    core.cancel(RunId(1));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(running_on_server(&probe), 1, "the cancel did not reach the server");
    assert!(!running.is_finished(), "so the run has not ended: the statement is still running");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), running).await.expect("ends with the statement").unwrap();
    eprintln!("OBSERVED a cancel that never reached the server: the run ended {:?} after it, with {:?}", asked.elapsed(), result.as_ref().err().map(|e| e.kind));
    assert_eq!(result.expect_err("cancelled").kind, CoreErrorKind::Cancelled);
    assert_eq!(running_on_server(&probe), 0, "when the run ends, the statement has");
    core.run(RunId(2), run_request(&id, "pagila", 1, "select 1")).await.expect("the session works");
}

// ---- slice 06 review: the timeout path, and sessions the wait must not reopen -------

#[tokio::test]
async fn a_timeout_whose_cancel_never_reaches_the_server_is_not_reported_before_the_statement_has_ended() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let port = cancel_swallowing_proxy().await;
    let id = seeded_with_timeout_at(&core, 1, port).await;
    let probe = tag("timeout_unlanded");
    let started = std::time::Instant::now();
    let err = core
        .run(RunId(1), run_request(&id, "pagila", 1, &format!("select pg_sleep(4) as {probe}")))
        .await
        .expect_err("timed out");
    eprintln!("OBSERVED a timeout whose cancel never arrived: the run ended after {:?}: {err:?}", started.elapsed());
    assert_eq!(err.kind, CoreErrorKind::Timeout, "{err:?}");
    assert_eq!(running_on_server(&probe), 0, "when the run reports the timeout, the statement has ended");
    assert!(started.elapsed() >= std::time::Duration::from_millis(3_500), "not before: {:?}", started.elapsed());
}

#[tokio::test]
async fn a_set_made_before_a_timed_out_statement_is_still_in_effect_afterwards_on_the_same_connection() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_with_timeout(&core, 1).await;
    let run = |n: u64, sql: &str| core.run(RunId(n), run_request(&id, "pagila", 1, sql));
    run(1, "set search_path to other").await.expect("set");
    let pid = texts(&run(2, "select pg_backend_pid()").await.unwrap().rows);
    let probe = tag("timeout_session");
    let err = run(3, &format!("select pg_sleep(30) as {probe}")).await.expect_err("timed out");
    assert_eq!(err.kind, CoreErrorKind::Timeout, "{err:?}");
    assert!(err.message.contains("was stopped"), "{}", err.message);
    assert_eq!(running_on_server(&probe), 0);
    assert_eq!(texts(&run(4, "select pg_backend_pid()").await.unwrap().rows), pid, "the same connection");
    let path = run(5, "show search_path").await.expect("show");
    assert_eq!(texts(&path.rows), vec![vec![Some("other".to_string())]], "the session survived the timeout");
    core.close_session(tab_session(1), &database(&id, "pagila")).await.unwrap();
}

#[tokio::test]
async fn closing_the_session_of_a_run_whose_cancel_is_slow_leaves_no_connection_for_it_once_the_run_ends() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let port = cancel_swallowing_proxy().await;
    let id = core
        .save_connection(&ConnectionDraft { port: Some(port), ..fixture("proxied", FIXTURE_PASSWORD) })
        .await
        .expect("save")
        .id;
    let probe = tag("slow_close");
    let running = tokio::spawn({
        let (core, id, sql) = (core.clone(), id.clone(), format!("select pg_sleep(3) as {probe}"));
        async move { core.run(RunId(1), run_request(&id, "pagila", 1, &sql)).await }
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let during = server_connections("pagila");
    // Stop waiting: the cancel goes nowhere, and the session is closed while
    // the statement still runs.
    core.cancel(RunId(1));
    let closed = std::time::Instant::now();
    core.close_session(tab_session(1), &database(&id, "pagila")).await.expect("close");
    eprintln!("OBSERVED close_session of a busy session returned after {:?}", closed.elapsed());
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), running).await.expect("the run ends").unwrap();
    eprintln!("OBSERVED the run of a closed session ended with {:?}; still running on the server: {}", result.as_ref().err().map(|e| e.kind), running_on_server(&probe));
    // Its session is closed, so nothing waits on it: the statement may still
    // run (the cancel went nowhere) and ends on its own.
    assert!(gone_within(&probe, 5).await.is_some());
    let after = wait_for_server_connections("pagila", during.len() - 1).await;
    eprintln!("OBSERVED pagila connections while running {during:?}, after {after:?}");
    assert_eq!(after.len(), during.len() - 1, "no connection is left, or opened again, for the closed session");
}

// ---- slice 07: the result grid's tables, the row cap and "rerunnable" -------

async fn run_capped(core: &DbxCore, id: &ConnectionId, sql: &str, cap: usize) -> dbx_tui::core::ResultSet {
    let request = RunRequest { row_cap: cap, ..run_request(id, "edge", 1, sql) };
    core.run(RunId(rand_id()), request).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn only_a_proven_select_comes_back_rerunnable_and_one_calling_functions_is_unproven() {
    use dbx_tui::core::Rerun::{Proven, ReadOnlyUnproven, Write};
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    for (sql, want) in [
        ("select id from text_values order by id", Proven),
        ("with t as (select 1 as a) select * from t", Proven),
        ("values (1), (2)", Proven),
        ("table text_values", Proven),
        // Read-only to dbx's denylist, but a function it cannot vouch for.
        ("select set_config('dbxtui.scratch', '1', false)", ReadOnlyUnproven),
        // Writes that change nothing here (no row matches), but that dbx and we
        // still know must not be repeated.
        ("update text_values set v = v where false returning id", Write),
        ("delete from text_values where false returning id", Write),
        ("with d as (delete from text_values where false returning id) select * from d", Write),
        ("select * from text_values for update", Write),
    ] {
        let set = run_capped(&core, &id, sql, 10).await;
        assert_eq!(set.rerun, want, "{sql}");
    }
}

#[tokio::test]
async fn the_120_column_table_comes_back_whole_and_every_cell_names_its_own_column_and_row() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let started = std::time::Instant::now();
    let set = run_capped(&core, &id, "select * from wide_120 order by id", DEFAULT_ROW_CAP).await;
    eprintln!("OBSERVED wide_120: {} columns x {} rows in {:?}; c003 of row 7: {:?}", set.columns.len(), set.rows.len(), started.elapsed(), set.rows[6][3]);
    assert_eq!((set.columns.len(), set.rows.len(), set.truncated), (121, 400, false));
    assert_eq!(set.columns[1].name, "c001");
    assert_eq!(set.columns[120].name, "c120");
    let kinds: Vec<CellKind> = set.columns.iter().skip(1).take(4).map(|c| c.kind).collect();
    assert_eq!(kinds, vec![CellKind::Text, CellKind::Decimal, CellKind::Json, CellKind::Integer]);
    for (r, row) in set.rows.iter().enumerate() {
        let id = r + 1;
        assert_eq!(row.len(), 121, "row {id} is as wide as the header");
        for (n, cell) in row.iter().enumerate().skip(1) {
            let cell = match cell {
                Cell::Value(v) => v.clone(),
                Cell::Null => panic!("c{n:03} of row {id} is NULL"),
            };
            match n % 4 {
                1 => assert_eq!(cell, format!("c{n:03}-r{id:03}")),
                2 => assert_eq!(cell, format!("{}.50", n * 1000 + id)),
                3 => assert!(cell.replace(' ', "").contains(&format!("\"c\":{n}")) && cell.contains(&format!("{id}")), "{cell}"),
                _ => assert_eq!(cell, (n * 1000 + id).to_string()),
            }
        }
    }
}

#[tokio::test]
async fn the_million_row_table_is_cut_at_the_cap_and_a_larger_cap_gives_the_same_rows_first() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let sql = "select id, txt, num from million_rows order by id";
    let small = run_capped(&core, &id, sql, 1_000).await;
    let started = std::time::Instant::now();
    let big = run_capped(&core, &id, sql, 2_000).await;
    eprintln!("OBSERVED million_rows: cap 1,000 in {:?}, cap 2,000 in {:?}", small.elapsed, started.elapsed());
    assert_eq!((small.rows.len(), small.truncated), (1_000, true));
    assert_eq!((big.rows.len(), big.truncated), (2_000, true));
    assert_eq!(big.rows[..1_000], small.rows[..], "the first rows are the same ones: load more only appends");
    assert_eq!(texts(&big.rows)[1_999][1].as_deref(), Some("row 2000"));
    assert!(small.rerun == dbx_tui::core::Rerun::Proven && big.rerun == dbx_tui::core::Rerun::Proven);
    assert_eq!(big.ran, sql, "the statement that ran is what load more repeats");
    let all = run_capped(&core, &id, "select id from million_rows where id <= 1500 order by id", 2_000).await;
    assert_eq!((all.rows.len(), all.truncated), (1_500, false), "not cut when the cap is not reached");
}

#[tokio::test]
async fn wide_and_combining_text_comes_back_exactly() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let set = run_capped(&core, &id, "select id, v from width_text order by id", 100).await;
    let v: Vec<String> = texts(&set.rows).into_iter().map(|r| r[1].clone().unwrap()).collect();
    assert_eq!(v[1], "日本語のテキスト");
    assert_eq!(v[4], "e\u{301}e\u{301}e\u{301} a\u{308}");
    assert_eq!(v[5], "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}");
    assert_eq!(v[9], "\u{1F1EF}\u{1F1F5}");
    assert_eq!(v[7], "ｱｲｳｴｵ");
}

/// Loads a million rows once and reports what it cost. Not part of the suite
/// (`cargo test -p dbx-tui --test live -- --ignored observe`): it takes a few
/// seconds and a few hundred megabytes.
#[tokio::test]
#[ignore]
async fn observe_loading_a_million_rows() {
    require_fixtures().await;
    let rss = || {
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0)
    };
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let _warm = run_capped(&core, &id, "select 1", 10).await;
    for cap in [1_000, 10_000, 100_000, 1_000_000] {
        let before = rss();
        let started = std::time::Instant::now();
        let set = run_capped(&core, &id, "select id, txt, num from million_rows order by id", cap).await;
        let took = started.elapsed();
        let held = rss();
        eprintln!(
            "OBSERVED cap {cap}: {} rows in {took:?}; rss {} MB -> {} MB while held ({} bytes per row)",
            set.rows.len(),
            before / 1024,
            held / 1024,
            held.saturating_sub(before) * 1024 / set.rows.len() as u64
        );
        drop(set);
    }
}

// ---- slice 11: the completion popup's calls ----------------------------------

fn search(id: &ConnectionId, db: &str, schema: Option<&str>, kind: SearchKind, prefix: &str) -> CompletionRequest {
    CompletionRequest { at: database(id, db), schema: schema.map(String::from), kind, prefix: prefix.into(), limit: 100 }
}

fn kinds(found: &[Candidate]) -> Vec<(&str, CandidateKind)> {
    let mut out: Vec<(&str, CandidateKind)> = found.iter().map(|c| (c.name.as_str(), c.kind)).collect();
    out.sort_by_key(|(n, _)| *n);
    out
}

#[tokio::test]
async fn a_prefix_search_for_fil_in_public_returns_the_film_tables_and_the_film_view_with_their_kinds() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let found = core.complete(&search(&id, "pagila", Some("public"), SearchKind::Relation, "fil")).await.expect("search");
    assert_eq!(
        kinds(&found.candidates),
        [
            ("film", CandidateKind::Table),
            ("film_actor", CandidateKind::Table),
            ("film_category", CandidateKind::Table),
            ("film_list", CandidateKind::View),
        ],
        "{found:?}"
    );
    assert!(!found.incomplete);
    assert!(found.candidates.iter().all(|c| c.schema.as_deref() == Some("public")));
    // Without regard to case.
    let upper = core.complete(&search(&id, "pagila", Some("public"), SearchKind::Relation, "FIL")).await.expect("search");
    assert_eq!(kinds(&upper.candidates), kinds(&found.candidates));
    eprintln!("OBSERVED fil in public: {found:?}");
}

#[tokio::test]
async fn a_search_keeps_the_name_as_stored_and_cuts_at_its_limit_saying_so() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let found = core.complete(&search(&id, "edge", Some("public"), SearchKind::Relation, "Mix")).await.expect("search");
    assert_eq!(kinds(&found.candidates), [("Mixed Case", CandidateKind::Table)], "{found:?}");
    let cut = core.complete(&CompletionRequest { limit: 3, ..search(&id, "pagila", Some("public"), SearchKind::Relation, "") }).await.expect("search");
    assert_eq!(cut.candidates.len(), 3);
    assert!(cut.incomplete, "stopped at the limit: there is more");
}

#[tokio::test]
async fn schemas_and_functions_are_searched_the_same_way() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let schemas = core.complete(&search(&id, "pagila", None, SearchKind::Schema, "")).await.expect("schemas");
    assert!(schemas.candidates.iter().any(|c| c.name == "public" && c.kind == CandidateKind::Schema), "{schemas:?}");
    assert!(schemas.candidates.iter().all(|c| !c.name.starts_with("pg_") && c.name != "information_schema"), "no system schemas: {schemas:?}");
    let functions = core.complete(&search(&id, "pagila", Some("pg_catalog"), SearchKind::Function, "low")).await.expect("functions");
    // One candidate per overload (`lower(text)` returns text, `lower(anyrange)` anyelement), each with its result type.
    let lowers: Vec<_> = functions.candidates.iter().filter(|c| c.name == "lower").collect();
    assert!(lowers.iter().any(|c| c.kind == CandidateKind::Function && c.detail.as_deref() == Some("text")), "{functions:?}");
    assert!(lowers.len() >= 2, "overloads are listed separately: {lowers:?}");
    eprintln!("OBSERVED functions low in pg_catalog: {functions:?}");
}

#[tokio::test]
async fn dbxs_function_search_does_not_return_aggregates_which_is_why_the_client_lists_the_common_ones_itself() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    // `count` is an aggregate (`pg_proc.prokind = 'a'`); the search asks for functions and procedures only.
    let found = core.complete(&search(&id, "pagila", Some("pg_catalog"), SearchKind::Function, "coun")).await.expect("functions");
    assert!(found.candidates.iter().all(|c| c.name != "count"), "if dbx now returns aggregates the client's own list can go: {found:?}");
}

#[tokio::test]
async fn the_current_schema_is_what_unqualified_names_resolve_in_and_a_bad_database_says_why() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    assert_eq!(core.current_schema(&database(&id, "pagila")).await.expect("current_schema").as_deref(), Some("public"));
    let missing = core.current_schema(&database(&id, "no_such_database")).await.expect_err("no such database");
    assert!(missing.message.contains("no_such_database"), "{}", missing.message);
    assert!(!missing.message.contains(PASSWORD));
    let unknown = core.complete(&search(&ConnectionId("nope".into()), "pagila", None, SearchKind::Schema, "")).await.expect_err("no such connection");
    assert!(!unknown.message.is_empty());
}

#[tokio::test]
async fn abandoning_a_search_stops_its_work_and_the_next_call_works() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let id = saved_fixture(&core).await;
    // A search dropped before it answers: the future is gone, as when the app abandons a request.
    let slow = {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move { core.complete(&search(&id, "pagila", Some("public"), SearchKind::Relation, "")).await })
    };
    slow.abort();
    let _ = slow.await;
    let found = core.complete(&search(&id, "pagila", Some("public"), SearchKind::Relation, "fil")).await.expect("the next search");
    assert_eq!(found.candidates.len(), 4);
}
