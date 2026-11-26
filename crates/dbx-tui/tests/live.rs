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
    Candidate, CandidateKind, Cell, CellKind, CompletionRequest, ConnectionDraft, ConnectionId, Core, CoreErrorKind, Cursor, DatabasePath, Dialect, DbxCore, RunId, RunRequest,
    SchemaPath, SearchKind, SessionId, Syntax, TabId, TableKind, TablePath, DEFAULT_ROW_CAP,
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
        confirm_dangerous: false, // The gate has its own tests (slice 17).
        confirmation: None,
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
    // What is asserted is the work done: the 20 s statement ended by being
    // cancelled (the kind below), while the lock was held. The bound only stops
    // a hang from stalling the suite; it is generous (below the statement's own
    // 20 s, well above any load the machine is under) because the fixed 5 s one
    // failed under load, where the cancel landed but the task was slow to run.
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), running).await.expect("cancelled before the statement ran out").unwrap();
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
    CompletionRequest { at: database(id, db), schema: schema.map(String::from), kind, prefix: prefix.into(), limit: 100, global: false }
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

#[tokio::test]
async fn the_foreign_keys_of_film_actor_name_film_and_actor() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let keys = core.foreign_keys(&table(&id, "pagila", "public", "film_actor")).await.expect("foreign keys");
    eprintln!("OBSERVED film_actor keys: {keys:?}");
    let has = |column: &str, to: &str, to_column: &str| keys.iter().any(|k| k.column == column && k.ref_table == to && k.ref_column == to_column && k.ref_schema.as_deref().is_none_or(|s| s == "public"));
    assert!(has("film_id", "film", "film_id"), "{keys:?}");
    assert!(has("actor_id", "actor", "actor_id"), "{keys:?}");
    // Only the table's own keys: `film` has none that refer to film_actor.
    let film = core.foreign_keys(&table(&id, "pagila", "public", "film")).await.expect("foreign keys");
    assert!(film.iter().all(|k| k.ref_table != "film_actor"), "{film:?}");
}

#[tokio::test]
async fn a_search_across_schemas_finds_a_name_that_two_schemas_have() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let all = core.complete(&CompletionRequest { global: true, ..search(&id, "pagila", None, SearchKind::Relation, "film") }).await.expect("search");
    assert!(all.candidates.iter().any(|c| c.name == "film" && c.schema.as_deref() == Some("public")), "{all:?}");
}

// ---- capabilities (slice 14b) -----------------------------------------------------

#[tokio::test]
async fn a_postgres_connection_reports_what_dbxs_descriptor_says_for_postgres() {
    require_fixtures().await;
    let s = Scratch::new();
    let saved = s.core().save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.expect("save");
    // plugins/connection-types/postgres.yaml: schemaAware and treeSchema
    // (databases, then schemas), queryExecution, metadataBrowse,
    // tableDataEdit, sqlExplain, defaultPort 5432, dialect PostgreSQL,
    // singleConnectionPool false.
    let caps = &saved.capabilities;
    assert!(caps.databases && caps.schemas, "{caps:?}");
    assert!(caps.query && caps.browse && caps.edit_data && caps.explain, "{caps:?}");
    assert_eq!(caps.default_port, Some(5432));
    assert_eq!(caps.dialect, Dialect::PostgreSql);
    assert!(!caps.single_connection);
    // The same answer before any connection exists, for the form.
    assert_eq!(&s.core().capabilities(&saved.kind), caps);
    assert_eq!(s.core().connections().await.unwrap()[0].capabilities, *caps);
}

#[tokio::test]
async fn a_scratch_sqlite_connection_has_no_database_or_schema_level_and_one_connection() {
    let s = Scratch::new();
    let core = s.core();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("scratch.db");
    core.seed_config(serde_json::json!({
        "id": "lite-1",
        "name": "scratch sqlite",
        "db_type": "sqlite",
        "host": "",
        "port": 0,
        "username": "",
        "password": "",
        "database": file.to_string_lossy(),
        "ssl": false,
        "driver_profile": null,
    }))
    .await
    .expect("seed");
    let list = core.connections().await.expect("list");
    let caps = &list.iter().find(|c| c.id.0 == "lite-1").expect("listed").capabilities;
    // plugins/connection-types/sqlite.yaml: localFile, singleConnectionPool,
    // no schemaAware or treeSchema, defaultPort absent, sqlExplain false.
    assert!(!caps.databases && !caps.schemas, "{caps:?}");
    assert!(caps.single_connection, "{caps:?}");
    assert!(caps.query && caps.edit_data && !caps.explain, "{caps:?}");
    assert_eq!(caps.default_port, None);
    assert_eq!(caps.dialect, Dialect::Sqlite);
    // dbx can browse it, and since slice 23a so does this client, with a form
    // of its own (a name and a file), completion and a fixed schema, `main`.
    assert!(caps.browse && caps.edit_in_form && caps.local_file && caps.completes, "{caps:?}");
    assert!(caps.fixed_schema, "{caps:?}");
}

#[tokio::test]
async fn an_empty_database_name_is_the_connections_own_database() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    // Saved with `pagila`: an empty name must not reach dbx as a database.
    let request = RunRequest { database: Some(String::new()), ..run_request(&id, "", 1, "select current_database()") };
    let result = core.run(RunId(rand_id()), request).await.expect("run");
    assert_eq!(result.rows, vec![vec![Cell::Value("pagila".into())]]);
    assert!(core.schemas(&database(&id, "")).await.expect("schemas").contains(&"public".to_string()));
    assert_eq!(core.current_schema(&database(&id, "")).await.expect("current"), Some("public".into()));
}

// ---- slice 16: browsing a table ----------------------------------------------

#[allow(dead_code)]
#[path = "ground_truth/psql.rs"]
mod psql;

/// The columns of `path` that are its primary key, as the tree reads them.
async fn key_of(core: &DbxCore, path: &TablePath) -> Vec<String> {
    core.columns(path).await.expect("columns").into_iter().filter(|c| c.primary_key).map(|c| c.name).collect()
}

/// The page of `path` the client asks for (`offset` rows in, `limit` rows long)
/// and what it returns, run as a browse tab runs it: the whole generated text
/// as the selection, the row cap the page size.
async fn page_of(core: &DbxCore, id: &ConnectionId, path: &TablePath, limit: usize, offset: usize) -> (String, dbx_tui::core::ResultSet) {
    let key = key_of(core, path).await;
    let sql = core.table_page_sql(&dbx_tui::core::DbKind::Postgres, path, &key, limit, offset);
    let lines: Vec<&str> = sql.split('\n').collect();
    let end = Cursor { row: lines.len() - 1, col: lines.last().unwrap().chars().count() };
    let request = RunRequest {
        selection: Some((Cursor { row: 0, col: 0 }, end)),
        row_cap: limit,
        ..run_request(id, &path.schema.database.database, 1, &sql)
    };
    let set = core.run(RunId(rand_id()), request).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    (sql, set)
}

/// What `psql` says `sql` returns on `edge`, every value in row order.
fn psql_values(sql: &str) -> Vec<Option<String>> {
    let sql = sql.trim_end().trim_end_matches(';').to_string();
    psql::query_all(&[("page".to_string(), sql)]).remove("page").expect("answered")
}

fn flat(rows: &[Vec<Cell>]) -> Vec<Option<String>> {
    rows.iter()
        .flat_map(|r| r.iter().map(|c| match c { Cell::Null => None, Cell::Value(v) => Some(v.clone()) }))
        .collect()
}

#[tokio::test]
async fn million_rows_pages_one_two_and_the_one_holding_999999_match_psql_cell_for_cell() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let path = table(&id, "edge", "public", "million_rows");
    assert_eq!(key_of(&core, &path).await, vec!["id".to_string()]);
    let mut seen_999_999 = false;
    for offset in [0, 1000, 999_000] {
        let (sql, set) = page_of(&core, &id, &path, 1000, offset).await;
        assert_eq!(set.rows.len(), 1000, "{sql}");
        assert!(sql.contains(r#"ORDER BY "id""#), "the page is in the key's order: {sql}");
        assert!(!set.truncated, "a page of exactly the row cap is not cut: {sql}");
        let ours = flat(&set.rows);
        let theirs = psql_values(&sql);
        assert_eq!(ours.len(), 3000);
        assert_eq!(ours, theirs, "offset {offset}: {sql}");
        let first: Vec<_> = set.rows.iter().map(|r| r[0].clone()).collect();
        assert_eq!(first[0], Cell::Value((offset + 1).to_string()), "stable order: pages run on from one another");
        seen_999_999 |= first.contains(&Cell::Value("999999".into()));
    }
    assert!(seen_999_999, "the page at 999,000 holds id 999,999");
    // The last page is the end of the table, and there is nothing after it.
    let (_, last) = page_of(&core, &id, &path, 1000, 999_000).await;
    assert_eq!(last.rows.last().map(|r| r[0].clone()), Some(Cell::Value("1000000".into())));
    let (_, past) = page_of(&core, &id, &path, 1000, 1_000_000).await;
    assert!(past.rows.is_empty(), "past the end is empty, not an error");
}

#[tokio::test]
async fn the_count_statement_gives_the_number_of_rows() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let path = table(&id, "edge", "public", "million_rows");
    let sql = core.table_count_sql(&dbx_tui::core::DbKind::Postgres, &path);
    let request = RunRequest { row_cap: 1, ..run_request(&id, "edge", 1, &sql) };
    let set = core.run(RunId(rand_id()), request).await.expect("count");
    assert_eq!(set.rows, vec![vec![Cell::Value("1000000".into())]], "{sql}");
}

#[tokio::test]
async fn a_hundred_kilobyte_value_arrives_whole_and_null_empty_and_the_text_null_stay_distinct() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let gt = table(&id, "edge", "public", "gt_text");
    let (sql, set) = page_of(&core, &id, &gt, 1000, 0).await;
    let t = set.columns.iter().position(|c| c.name == "t").expect("column t");
    let big = set.rows.iter().find(|r| r[0] == Cell::Value("21".into())).expect("row 21");
    let Cell::Value(text) = &big[t] else { panic!("NULL: {sql}") };
    assert_eq!(text.len(), 102_400, "nothing is cut for preview: {sql}");
    assert_eq!(text, &"0123456789abcdef".repeat(6400));

    let tv = table(&id, "edge", "public", "text_values");
    let (_, set) = page_of(&core, &id, &tv, 1000, 0).await;
    let v = set.columns.iter().position(|c| c.name == "v").unwrap();
    let cell = |id: &str| set.rows.iter().find(|r| r[0] == Cell::Value(id.into())).map(|r| r[v].clone()).expect("row");
    assert_eq!(cell("1"), Cell::Null);
    assert_eq!(cell("2"), Cell::Value(String::new()));
    assert_eq!(cell("3"), Cell::Value("NULL".into()));
}

#[tokio::test]
async fn a_table_without_a_key_mixed_case_names_and_a_second_schema_open_and_page() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;

    let no_pk = table(&id, "edge", "public", "no_pk");
    assert!(key_of(&core, &no_pk).await.is_empty());
    let (sql, set) = page_of(&core, &id, &no_pk, 1000, 0).await;
    assert!(!sql.to_lowercase().contains("order by"), "nothing to order by: {sql}");
    assert_eq!(set.rows.len(), 3, "{sql}");

    let mixed = table(&id, "edge", "public", "Mixed Case");
    assert_eq!(key_of(&core, &mixed).await, vec!["Id".to_string()]);
    let (sql, set) = page_of(&core, &id, &mixed, 1000, 0).await;
    assert_eq!(flat(&set.rows), vec![Some("1".into()), Some("reserved".into()), Some("spaced".into())], "{sql}");
    assert_eq!(flat(&set.rows), psql_values(&sql));

    let things = table(&id, "edge", "other", "things");
    let (sql, set) = page_of(&core, &id, &things, 1000, 0).await;
    assert_eq!(flat(&set.rows), vec![Some("1".into()), Some("in the second schema".into())], "{sql}");
    assert_eq!(flat(&set.rows), psql_values(&sql));

    // A composite key orders by both columns.
    let composite = table(&id, "edge", "public", "composite_key");
    let (sql, set) = page_of(&core, &id, &composite, 2, 1).await;
    assert_eq!(set.rows.len(), 2, "{sql}");
    assert_eq!(flat(&set.rows), psql_values(&sql));
    assert_eq!(set.rows[0][1], Cell::Value("y".into()), "second row of (a, b) order: {sql}");
}

/// True when `sql` holds one statement: outside double-quoted names, the only
/// `;` allowed is the last character.
fn is_one_statement(sql: &str) -> bool {
    let mut quoted = false;
    let body = sql.trim_end();
    for (i, ch) in body.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ';' if !quoted && i + 1 != body.len() => return false,
            _ => {}
        }
    }
    !quoted
}

#[tokio::test]
async fn a_table_named_with_quotes_and_a_second_statement_opens_as_one_statement_and_returns_its_row() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let path = table(&id, "edge", "public", "\"x\"; select pg_sleep(0) --\"");
    let (sql, set) = page_of(&core, &id, &path, 1000, 0).await;
    assert!(is_one_statement(&sql), "one statement: {sql}");
    assert_eq!(flat(&set.rows), vec![Some("1".into()), Some("one known row".into())], "{sql}");
    let count = core.table_count_sql(&dbx_tui::core::DbKind::Postgres, &path);
    assert!(is_one_statement(&count), "one statement: {count}");
    let counted = core.run(RunId(rand_id()), run_request(&id, "edge", 2, &count)).await.unwrap_or_else(|e| panic!("{count}: {e}"));
    assert_eq!(flat(&counted.rows), vec![Some("1".into())], "{count}");
}

// ---- slice 17: confirmations ------------------------------------------------------

/// `psql` on `edge` in the fixture container: its output, rows on lines.
fn psql_edge(sql: &str) -> String {
    let out = std::process::Command::new("docker")
        .args(["exec", "dbxtui-fixtures-postgres-1", "psql", "-U", "postgres", "-d", "edge", "--no-psqlrc", "-v", "ON_ERROR_STOP=1", "-tA", "-c", sql])
        .output()
        .expect("docker exec psql");
    assert!(out.status.success(), "psql: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A scratch schema on `edge`, `scratch_17_<pid>_<what>`, holding table `t`
/// with rows 1 to 5. Dropped when it goes, a failed test included. Never a
/// fixture table.
struct ScratchTable {
    schema: String,
}

impl ScratchTable {
    fn new(what: &str) -> Self {
        let schema = format!("scratch_17_{}_{what}", std::process::id());
        psql_edge(&format!(
            "drop schema if exists {schema} cascade; create schema {schema}; \
             create table {schema}.t (id int primary key, v text); \
             insert into {schema}.t select i, 'v' || i from generate_series(1, 5) i"
        ));
        ScratchTable { schema }
    }

    fn t(&self) -> String {
        format!("{}.t", self.schema)
    }

    /// The table's rows, as `psql` shows them, in key order.
    fn rows(&self) -> Vec<String> {
        psql_edge(&format!("select id || '|' || v from {} order by id", self.t())).lines().map(str::to_string).collect()
    }
}

impl Drop for ScratchTable {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["exec", "dbxtui-fixtures-postgres-1", "psql", "-U", "postgres", "-d", "edge", "--no-psqlrc", "-c", &format!("drop schema if exists {} cascade", self.schema)])
            .output();
    }
}

/// The fixture saved with these markers, as the desktop app would save it.
async fn seeded_marked(core: &DbxCore, id: &str, production: bool, marked: &[&str], read_only: bool) -> ConnectionId {
    core.seed_config(serde_json::json!({
        "id": id, "name": format!("marked {id}"), "db_type": "postgres", "host": "127.0.0.1", "port": 55433,
        "username": "postgres", "password": FIXTURE_PASSWORD, "database": "edge", "ssl": false, "driver_profile": null,
        "is_production": production, "production_databases": marked, "read_only": read_only,
    }))
    .await
    .expect("seed")
}

/// A run of exactly `sql` on `edge`, the danger check as `confirm_dangerous`.
fn exactly(id: &ConnectionId, sql: &str, confirm_dangerous: bool) -> RunRequest {
    RunRequest { confirm_dangerous, ..run_request(id, "edge", 1, sql) }
}

/// Refused, then run with the yes: exactly row 1 goes.
async fn refused_then_confirmed(core: &DbxCore, id: &ConnectionId, table: &ScratchTable) {
    let before = table.rows();
    let sql = format!("delete from {} where id = 1", table.t());
    let refused = core.run(RunId(rand_id()), exactly(id, &sql, true)).await.expect_err("refused");
    assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation, "{refused:?}");
    let assessment = refused.assessment.expect("with the assessment");
    assert!(assessment.production);
    assert_eq!(assessment.reasons, vec![dbx_tui::core::Reason::Writes]);
    assert_eq!(assessment.confirmation.statement, sql);
    assert_eq!(table.rows(), before, "nothing ran");

    // A yes for id = 1 sent with id = 2 runs nothing.
    let other = format!("delete from {} where id = 2", table.t());
    let wrong = RunRequest { confirmation: Some(assessment.confirmation.clone()), ..exactly(id, &other, true) };
    let again = core.run(RunId(rand_id()), wrong).await.expect_err("asked again");
    assert_eq!(again.kind, CoreErrorKind::NeedsConfirmation);
    assert_eq!(table.rows(), before, "still nothing ran");

    let yes = RunRequest { confirmation: Some(assessment.confirmation), ..exactly(id, &sql, true) };
    let set = core.run(RunId(rand_id()), yes).await.expect("confirmed, it runs");
    assert_eq!(set.affected, 1);
    assert_eq!(table.rows(), before[1..].to_vec(), "exactly row 1 is gone, every other row as it was");
    core.close_session(tab_session(1), &database(id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_delete_on_a_connection_marked_production_runs_only_with_the_matching_yes() {
    require_fixtures().await;
    let table = ScratchTable::new("conn");
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "prod", true, &[], false).await;
    refused_then_confirmed(&core, &id, &table).await;
}

#[tokio::test]
async fn a_delete_on_a_database_marked_production_runs_only_with_the_matching_yes() {
    require_fixtures().await;
    let table = ScratchTable::new("db");
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "marked-db", false, &["edge"], false).await;
    refused_then_confirmed(&core, &id, &table).await;
}

#[tokio::test]
async fn an_unscoped_delete_outside_production_is_asked_with_the_setting_on_and_runs_with_it_off() {
    require_fixtures().await;
    let table = ScratchTable::new("danger");
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "plain", false, &[], false).await;
    let before = table.rows();
    let sql = format!("delete from {}", table.t());
    let refused = core.run(RunId(rand_id()), exactly(&id, &sql, true)).await.expect_err("asked");
    assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation);
    let assessment = refused.assessment.expect("why");
    assert!(!assessment.production);
    assert_eq!(assessment.reasons, vec![dbx_tui::core::Reason::Dangerous]);
    assert_eq!(table.rows(), before, "nothing ran");
    let set = core.run(RunId(rand_id()), exactly(&id, &sql, false)).await.expect("the check is off");
    assert_eq!(set.affected, 5);
    assert!(table.rows().is_empty());
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_read_only_connection_refuses_the_write_with_dbxs_error_and_no_question() {
    require_fixtures().await;
    let table = ScratchTable::new("ro");
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "ro", true, &[], true).await;
    let before = table.rows();
    let sql = format!("delete from {} where id = 1", table.t());
    let refused = core.run(RunId(rand_id()), exactly(&id, &sql, true)).await.expect_err("refused");
    assert_ne!(refused.kind, CoreErrorKind::NeedsConfirmation, "{refused:?}");
    assert!(refused.message.to_lowercase().contains("read-only"), "dbx's message: {}", refused.message);
    assert_eq!(table.rows(), before);
}

#[tokio::test]
async fn generated_selects_and_a_proven_select_run_on_production_without_a_question() {
    require_fixtures().await;
    let table = ScratchTable::new("read");
    let s = Scratch::new();
    let core = s.core();
    let id = seeded_marked(&core, "prod-read", true, &[], false).await;
    let path = TablePath { schema: schema(&id, "edge", &table.schema), table: "t".into() };
    for sql in [
        format!("select * from {} order by id", table.t()),
        core.table_page_sql(&dbx_tui::core::DbKind::Postgres, &path, &["id".to_string()], 1000, 0),
        core.table_count_sql(&dbx_tui::core::DbKind::Postgres, &path),
    ] {
        core.run(RunId(rand_id()), exactly(&id, &sql, true)).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    core.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

#[tokio::test]
async fn a_production_flag_set_while_the_client_runs_is_seen_by_the_next_write() {
    require_fixtures().await;
    let table = ScratchTable::new("late");
    let s = Scratch::new();
    let client = s.core();
    let id = seeded_marked(&client, "late", false, &[], false).await;
    // Warm: the client has loaded its config, and a scoped write is not asked.
    let before = table.rows();
    let first = format!("delete from {} where id = 5", table.t());
    client.run(RunId(rand_id()), exactly(&id, &first, true)).await.expect("not production yet: it runs");
    assert_eq!(table.rows(), before[..4].to_vec());
    // The desktop app marks the connection while the client is open: another
    // head, the same store.
    let desktop = s.core();
    seeded_marked(&desktop, "late", true, &[], false).await;
    let sql = format!("delete from {} where id = 1", table.t());
    let refused = client.run(RunId(rand_id()), exactly(&id, &sql, true)).await.expect_err("now asked");
    assert_eq!(refused.kind, CoreErrorKind::NeedsConfirmation, "{refused:?}");
    let assessment = refused.assessment.as_ref().expect("why");
    assert!(assessment.production);
    assert_eq!(table.rows(), before[..4].to_vec(), "nothing ran");
    // A proven read stays on the fast path, and the mark is not lost.
    client.run(RunId(rand_id()), exactly(&id, "select 1", true)).await.expect("a proven read runs");
    // The flag set at the desktop and nothing about secrets in the question.
    assert!(!format!("{refused:?}").contains(FIXTURE_PASSWORD));
    client.close_session(tab_session(1), &database(&id, "edge")).await.unwrap();
}

// ---- slice 19: export ---------------------------------------------------------------

use dbx_tui::core::{Column, DbKind, ExportFormat, ExportRequest};

/// The tables whose export is read back, in every format.
const EXPORTED: [&str; 5] = ["text_values", "numbers", "gt_text", "gt_json", "gt_integers"];

/// A table read through the client, with what `psql` says its cells are.
struct Held {
    table: &'static str,
    set: dbx_tui::core::ResultSet,
    /// `psql`'s text of every cell, by row and column: the truth the exports
    /// are read back against. NaN and the infinities are NULL here, which is
    /// what dbx hands the client (design section 2, a known loss), and what
    /// every export holds.
    want: Vec<Vec<Option<String>>>,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `public.<table>` read through `DbxCore`, ordered by id, and through `psql`.
async fn held(core: &DbxCore, id: &ConnectionId, table: &'static str) -> Held {
    let sql = format!("select * from public.{table} order by id");
    let request = RunRequest { row_cap: 1000, ..run_request(id, "edge", 1, &sql) };
    let set = core.run(RunId(rand_id()), request).await.unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert!(!set.truncated, "{table} fits the cap");
    let queries: Vec<(String, String)> = set
        .columns
        .iter()
        .map(|c| (c.name.clone(), format!("select x.{} from public.{table} as x order by x.id", quote_ident(&c.name))))
        .collect();
    let by_column = psql::query_all(&queries);
    let mut want = vec![vec![None; set.columns.len()]; set.rows.len()];
    for (c, column) in set.columns.iter().enumerate() {
        let values = &by_column[&column.name];
        assert_eq!(values.len(), set.rows.len(), "{table}.{}: psql and the client read as many rows", column.name);
        for (r, v) in values.iter().enumerate() {
            want[r][c] = v.clone();
        }
    }
    for (r, row) in set.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if *cell == Cell::Null && matches!(want[r][c].as_deref(), Some("NaN" | "Infinity" | "-Infinity")) {
                assert_eq!((table, set.columns[c].name.as_str()), ("numbers", "approx"), "only the known loss is let through");
                want[r][c] = None;
            }
        }
    }
    let held_texts = texts(&set.rows);
    for (r, row) in want.iter().enumerate() {
        for (c, w) in row.iter().enumerate() {
            let short = |v: &Option<String>| v.as_ref().map(|s| s.chars().take(60).collect::<String>());
            assert!(held_texts[r][c] == *w, "{table} row {} column {}: held {:?}, psql {:?} (the rows held are psql's, but for NaN and the infinities)", r + 1, set.columns[c].name, short(&held_texts[r][c]), short(w));
        }
    }
    Held { table, set, want }
}

fn export_request(h: &Held, format: ExportFormat) -> ExportRequest {
    ExportRequest {
        format,
        columns: h.set.columns.clone(),
        rows: h.set.rows.clone(),
        kind: Some(DbKind::Postgres),
        table: h.table.into(),
        schema: String::new(),
        null_empty: false,
    }
}

/// A read-back of CSV: each field as text, and whether it was quoted.
fn parse_csv(text: &str) -> Vec<Vec<(String, bool)>> {
    let mut records = Vec::new();
    let mut record: Vec<(String, bool)> = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        let mut field = String::new();
        let quoted = chars.peek() == Some(&'"');
        if quoted {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        field.push('"');
                    }
                    Some('"') => break,
                    Some(c) => field.push(c),
                    None => panic!("a quoted field is never closed"),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' || c == '\n' {
                    break;
                }
                field.push(c);
                chars.next();
            }
        }
        record.push((field, quoted));
        match chars.next() {
            Some(',') => {}
            Some('\n') => records.push(std::mem::take(&mut record)),
            None => {
                records.push(record);
                return records;
            }
            Some(c) => panic!("after a field comes a comma or a line end, not {c:?}"),
        }
    }
}

/// What dbx's formula guard acts on (`needs_formula_guard`, the first byte
/// after spaces), for the cells the read-back may find altered.
fn starts_like_a_formula(value: &str) -> bool {
    matches!(value.trim_start_matches(' ').bytes().next(), Some(b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'))
}

#[tokio::test]
async fn csv_read_back_equals_psqls_values_with_null_as_backslash_n_but_for_the_formula_guard() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let mut guarded = 0;
    for table in EXPORTED {
        let h = held(&core, &id, table).await;
        let bytes = core.export_bytes(&export_request(&h, ExportFormat::Csv)).unwrap_or_else(|e| panic!("{table}: {e}"));
        let records = parse_csv(&String::from_utf8(bytes).expect("UTF-8"));
        let names: Vec<String> = h.set.columns.iter().map(|c| c.name.clone()).collect();
        assert_eq!(records[0].iter().map(|f| f.0.clone()).collect::<Vec<_>>(), names, "{table}: the header");
        assert_eq!(records.len(), h.want.len() + 1, "{table}: a record per row");
        for (r, want_row) in h.want.iter().enumerate() {
            for (c, want) in want_row.iter().enumerate() {
                let (value, quoted) = &records[r + 1][c];
                let got = if !quoted && value == "\\N" { None } else { Some(value.clone()) };
                match (want, &got) {
                    (w, g) if w == g => {}
                    (Some(w), Some(g)) if *g == format!("'{w}") && starts_like_a_formula(w) => guarded += 1,
                    _ => panic!("{table} row {} column {}: psql {want:?}, the file {got:?}", r + 1, names[c]),
                }
            }
        }
    }
    eprintln!("OBSERVED CSV: {guarded} cells carry the formula guard (a quote in front)");
    assert!(guarded > 0, "the fixtures hold values the guard alters, so the loss is exercised");
}

#[tokio::test]
async fn csv_with_an_empty_null_literal_writes_null_bare_and_the_empty_string_quoted() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let h = held(&core, &id, "text_values").await;
    let request = ExportRequest { null_empty: true, ..export_request(&h, ExportFormat::Csv) };
    let records = parse_csv(&String::from_utf8(core.export_bytes(&request).unwrap()).unwrap());
    let v = h.set.columns.iter().position(|c| c.name == "v").unwrap();
    assert_eq!(records[1][v], (String::new(), false), "NULL: an empty field with no quotes");
    assert_eq!(records[2][v], (String::new(), true), "the empty string: quoted");
    assert_eq!(records[3][v], ("NULL".to_string(), true), "the text NULL: itself");
}

#[tokio::test]
async fn json_read_back_keeps_null_empty_and_the_text_null_apart_and_2_to_the_53_plus_1_exact() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    for table in EXPORTED {
        let h = held(&core, &id, table).await;
        let bytes = core.export_bytes(&export_request(&h, ExportFormat::Json)).unwrap_or_else(|e| panic!("{table}: {e}"));
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        let rows = value.as_array().expect("an array of rows");
        assert_eq!(rows.len(), h.want.len(), "{table}");
        for (r, row) in rows.iter().enumerate() {
            let keys: Vec<&str> = row.as_object().expect("an object").keys().map(String::as_str).collect();
            assert_eq!(keys, h.set.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), "{table}: the keys are in column order");
            for (c, column) in h.set.columns.iter().enumerate() {
                let got = match &row[&column.name] {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    other => panic!("{table}.{}: {other:?}", column.name),
                };
                assert_eq!(got, h.want[r][c], "{table} row {} column {}", r + 1, column.name);
            }
        }
        if table == "text_values" {
            assert_eq!(rows[0]["v"], serde_json::Value::Null);
            assert_eq!(rows[1]["v"], serde_json::json!(""));
            assert_eq!(rows[2]["v"], serde_json::json!("NULL"));
        }
        if table == "gt_integers" {
            assert_eq!(rows[8]["i8"].as_i64(), Some(9_007_199_254_740_993), "2^53 + 1 is a JSON number holding every digit");
            assert_eq!(rows[2]["i8"].as_i64(), Some(i64::MIN));
        }
    }
}

/// A scratch schema on `edge`, dropped when it goes (a failed test included).
struct ExportSchema(String);

impl ExportSchema {
    fn new(what: &str) -> Self {
        let name = format!("scratch_19_{}_{what}", std::process::id());
        psql_script(&format!("drop schema if exists {name} cascade; create schema {name};"));
        ExportSchema(name)
    }
}

impl Drop for ExportSchema {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["exec", "dbxtui-fixtures-postgres-1", "psql", "-U", "postgres", "-d", "edge", "--no-psqlrc", "-c", &format!("drop schema if exists {} cascade", self.0)])
            .stdin(std::process::Stdio::null())
            .output();
    }
}

/// Runs a script on `edge` through `psql -f -`: any size, stopping at the
/// first error.
fn psql_script(sql: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("docker")
        .args(["exec", "-i", "dbxtui-fixtures-postgres-1", "env", "PGCLIENTENCODING=UTF8", "psql", "-U", "postgres", "-d", "edge", "--no-psqlrc", "-q", "-v", "ON_ERROR_STOP=1", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("docker exec psql");
    let mut stdin = child.stdin.take().expect("stdin");
    let script = sql.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let out = child.wait_with_output().expect("psql output");
    writer.join().expect("writer").expect("script written");
    assert!(out.status.success(), "psql: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn sql_inserts_run_into_a_scratch_table_give_back_the_rows_held_row_by_row() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let scratch = ExportSchema::new("sql");
    for table in EXPORTED {
        let h = held(&core, &id, table).await;
        psql_script(&format!("create table {}.{table} (like public.{table});", scratch.0));
        let request = ExportRequest { schema: scratch.0.clone(), ..export_request(&h, ExportFormat::SqlInsert) };
        let sql = String::from_utf8(core.export_bytes(&request).unwrap_or_else(|e| panic!("{table}: {e}"))).expect("UTF-8");
        assert!(sql.contains(&scratch.0), "{table}: the statements name the scratch schema: {}", sql.chars().take(300).collect::<String>());
        psql_script(&sql);
        let queries: Vec<(String, String)> = h
            .set
            .columns
            .iter()
            .map(|c| (c.name.clone(), format!("select x.{} from {}.{table} as x order by x.id", quote_ident(&c.name), scratch.0)))
            .collect();
        let by_column = psql::query_all(&queries);
        for (r, want_row) in h.want.iter().enumerate() {
            for (c, want) in want_row.iter().enumerate() {
                let got = by_column[&h.set.columns[c].name].get(r).cloned().unwrap_or_else(|| panic!("{table}: row {} is missing from the scratch table", r + 1));
                assert_eq!(&got, want, "{table} row {} column {}", r + 1, h.set.columns[c].name);
            }
        }
        assert_eq!(by_column[&h.set.columns[0].name].len(), h.want.len(), "{table}: no row more");
    }
}

#[tokio::test]
async fn a_quoted_column_and_table_name_reload_from_the_insert_file_with_the_same_values() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let scratch = ExportSchema::new("quoted");
    let table = r#""t"; drop table decoy; --""#;
    let hostile = r#""x"; drop table decoy; --""#;
    let quoted = r#""a""#;
    psql_script(&format!(
        "create table {s}.decoy (id int); create table {s}.{t} ({h} text, {q} text, plain text);",
        s = scratch.0,
        t = quote_ident(table),
        h = quote_ident(hostile),
        q = quote_ident(quoted)
    ));
    let text = |v: &str| Cell::Value(v.to_string());
    let column = |n: &str| Column { name: n.into(), db_type: "text".into(), kind: dbx_tui::core::CellKind::Text, numbers: Vec::new() };
    let request = ExportRequest {
        format: ExportFormat::SqlInsert,
        columns: vec![column(hostile), column(quoted), column("plain")],
        rows: vec![vec![text("one"), text("it's"), Cell::Null], vec![text("two"), text("b"), text("c")]],
        kind: Some(DbKind::Postgres),
        table: table.into(),
        schema: scratch.0.clone(),
        null_empty: false,
    };
    let sql = String::from_utf8(core.export_bytes(&request).expect("built")).expect("UTF-8");
    psql_script(&sql);
    let queries: Vec<(String, String)> = [hostile, quoted, "plain"]
        .iter()
        .map(|c| (c.to_string(), format!("select x.{} from {}.{} as x order by 1", quote_ident(c), scratch.0, quote_ident(table))))
        .collect();
    let got = psql::query_all(&queries);
    assert_eq!(got[hostile], vec![Some("one".to_string()), Some("two".to_string())]);
    assert_eq!(got[quoted], vec![Some("b".to_string()), Some("it's".to_string())]);
    assert_eq!(got["plain"].len(), 2);
    let decoy = psql::query_all(&[("d".to_string(), format!("select count(*) from {}.decoy", scratch.0))]);
    assert_eq!(decoy["d"], vec![Some("0".to_string())], "the decoy table is still there");
}

/// The text of one sheet of an Excel export.
fn sheet1(bytes: &[u8]) -> String {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("an Excel file is a zip");
    let mut sheet = zip.by_name("xl/worksheets/sheet1.xml").expect("the sheet");
    let mut text = String::new();
    sheet.read_to_string(&mut text).expect("UTF-8");
    text
}

/// One cell's XML, by reference (`C2`).
fn cell_xml(sheet: &str, reference: &str) -> String {
    let start = sheet.find(&format!("<c r=\"{reference}\"")).unwrap_or_else(|| panic!("no cell {reference}"));
    let rest = &sheet[start..];
    let open_end = rest.find('>').expect("a tag");
    if rest[..open_end].ends_with('/') {
        rest[..=open_end].to_string()
    } else {
        rest[..rest.find("</c>").expect("a closed cell") + 4].to_string()
    }
}

#[tokio::test]
async fn excel_holds_null_as_an_empty_cell_the_empty_string_as_empty_text_and_2_to_the_53_plus_1_as_text_with_every_digit() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let tv = held(&core, &id, "text_values").await;
    let sheet = sheet1(&core.export_bytes(&export_request(&tv, ExportFormat::Excel)).expect("xlsx"));
    // text_values: id, label, v. Row 1 is the header.
    let null = cell_xml(&sheet, "C2");
    assert!(null.ends_with("/>") && !null.contains("<v>") && !null.contains("<is>"), "NULL: a cell with no value: {null}");
    let empty = cell_xml(&sheet, "C3");
    assert!(empty.contains("t=\"inlineStr\"") && empty.contains("<t></t>"), "the empty string: a text cell holding nothing: {empty}");
    let text = cell_xml(&sheet, "C4");
    assert!(text.contains("<t>NULL</t>"), "the text NULL: itself: {text}");
    let gi = held(&core, &id, "gt_integers").await;
    let sheet = sheet1(&core.export_bytes(&export_request(&gi, ExportFormat::Excel)).expect("xlsx"));
    // gt_integers: id, label, i2, i4, i8, o. 2^53 + 1 is row 9, so sheet row 10.
    let big = cell_xml(&sheet, "E10");
    assert!(big.contains("t=\"inlineStr\"") && big.contains("<t>9007199254740993</t>"), "2^53 + 1: text with every digit: {big}");
    let small = cell_xml(&sheet, "E3");
    assert!(small.contains("<v>-1</v>") && !big.contains("<v>"), "a short integer is a number: {small}");
}

#[tokio::test]
async fn what_dbx_does_to_other_values_in_an_excel_file() {
    // No server needed: dbx's formats work on values. Observed, and the README
    // lists each.
    let s = Scratch::new();
    let core = s.core();
    let col = |name: &str, db_type: &str, kind| Column { name: name.into(), db_type: db_type.into(), kind, numbers: Vec::new() };
    let long = "x".repeat(40_000);
    let request = ExportRequest {
        format: ExportFormat::Excel,
        columns: vec![
            col("f", "text", dbx_tui::core::CellKind::Text),
            col("ts", "timestamp", dbx_tui::core::CellKind::Temporal),
            col("n", "numeric", dbx_tui::core::CellKind::Decimal),
            col("big", "numeric", dbx_tui::core::CellKind::Decimal),
            col("long", "text", dbx_tui::core::CellKind::Text),
        ],
        rows: vec![vec![
            Cell::Value("=1+1".into()),
            Cell::Value("2024-03-05 13:45:12.123456".into()),
            Cell::Value("1.50".into()),
            Cell::Value("12345678901234567890.123456789".into()),
            Cell::Value(long.clone()),
        ]],
        kind: Some(DbKind::Postgres),
        table: "t".into(),
        schema: String::new(),
        null_empty: false,
    };
    let sheet = sheet1(&core.export_bytes(&request).expect("xlsx"));
    let f = cell_xml(&sheet, "A2");
    assert!(f.contains("t=\"inlineStr\"") && f.contains("<t>=1+1</t>") && !f.contains("<f>"), "no formula guard in Excel files, and no formula: {f}");
    let ts = cell_xml(&sheet, "B2");
    eprintln!("OBSERVED a timestamp: {ts}");
    assert!(!ts.contains("inlineStr") && ts.contains("<v>"), "a timestamp becomes an Excel date: {ts}");
    let n = cell_xml(&sheet, "C2");
    eprintln!("OBSERVED numeric 1.50: {n}");
    assert!(n.contains("<v>1.50</v>") && !n.contains("inlineStr"), "{n}");
    let big = cell_xml(&sheet, "D2");
    assert!(big.contains("inlineStr") && big.contains("12345678901234567890.123456789"), "a number past 15 digits stays text: {big}");
    let long_cell = cell_xml(&sheet, "E2");
    assert!(long_cell.contains(&long), "a value over Excel's 32,767-character limit is written whole: {} chars", long_cell.len());
}

#[tokio::test]
async fn markdown_read_back_differs_from_psql_only_by_the_listed_losses() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let mut lost = std::collections::BTreeSet::new();
    for table in EXPORTED {
        let h = held(&core, &id, table).await;
        let text = String::from_utf8(core.export_bytes(&export_request(&h, ExportFormat::Markdown)).unwrap_or_else(|e| panic!("{table}: {e}"))).expect("UTF-8");
        let lines: Vec<&str> = text.strip_suffix('\n').expect("ends with a line break").split('\n').collect();
        assert_eq!(lines.len(), h.want.len() + 2, "{table}: header, separator, a line per row");
        let cells = |line: &str| -> Vec<String> {
            line.strip_prefix("| ").and_then(|l| l.strip_suffix(" |")).unwrap_or_else(|| panic!("a table line: {line:?}")).split(" | ").map(str::to_string).collect()
        };
        for (r, want_row) in h.want.iter().enumerate() {
            let got = cells(lines[r + 2]);
            assert_eq!(got.len(), want_row.len(), "{table} row {}", r + 1);
            for (c, want) in want_row.iter().enumerate() {
                let expected = match want {
                    None => {
                        lost.insert("NULL is the text NULL");
                        "NULL".to_string()
                    }
                    Some(s) => s.replace('|', "\\|").replace("\r\n", "<br>").replace('\n', "<br>"),
                };
                if want.as_deref().is_some_and(|s| s.contains('|')) {
                    lost.insert("| becomes \\|");
                }
                if want.as_deref().is_some_and(|s| s.contains('\n')) {
                    lost.insert("a line break becomes <br>");
                }
                if want.as_deref().is_some_and(|s| s.ends_with(' ')) {
                    lost.insert("trailing spaces are padding");
                }
                // The cell is padded to its column's width, so trailing spaces cannot be told apart.
                assert_eq!(got[c].trim_end_matches(' '), expected.trim_end_matches(' '), "{table} row {} column {}", r + 1, h.set.columns[c].name);
            }
        }
    }
    eprintln!("OBSERVED Markdown losses exercised: {lost:?}");
    assert!(lost.contains("NULL is the text NULL") && lost.contains("a line break becomes <br>") && lost.contains("trailing spaces are padding"));
}

// ---- slice 20: database dump -------------------------------------------------------
//
// Every dump runs on a scratch store (dbx records the destination directory in
// the store it runs with) and writes under a temporary directory. The fixture
// databases are only read; a restore goes into a scratch database created for
// the test and dropped afterwards, a failed test included.

use dbx_tui::core::{DumpContent, DumpEnd, DumpId, DumpProgress, DumpRequest, DumpStatus};

const CONTAINER: &str = "dbxtui-fixtures-postgres-1";

/// `psql` on database `db`: a script on stdin, stopping at the first error.
/// Its output, rows on lines.
fn psql_on(db: &str, script: &[u8]) -> Result<String, String> {
    psql_with(db, script, true)
}

/// `psql_on`, going on past errors when `stop` is false: then `Ok` carries
/// the errors psql printed, one per line, without their line numbers.
fn psql_with(db: &str, script: &[u8], stop: bool) -> Result<String, String> {
    use std::io::Write;
    let on_error = if stop { "ON_ERROR_STOP=1" } else { "ON_ERROR_STOP=0" };
    let mut child = std::process::Command::new("docker")
        .args(["exec", "-i", CONTAINER, "env", "PGCLIENTENCODING=UTF8", "psql", "-U", "postgres", "-d", db, "--no-psqlrc", "-q", "-tA", "-v", on_error, "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("docker exec psql");
    let mut stdin = child.stdin.take().expect("stdin");
    let script = script.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&script));
    let out = child.wait_with_output().expect("psql output");
    let _ = writer.join();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(stderr);
    }
    if stop {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // `psql:<stdin>:123: ERROR:  ...` -> `ERROR:  ...`
    let errors: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.find("ERROR:").map(|at| l[at..].to_string()))
        .collect();
    Ok(errors.join("\n"))
}

/// The scratch database a dump is restored into: `dbxtui_restore_<pid>_<n>`
/// (a counter, so tests running side by side never drop each other's), made
/// empty for the test and dropped when it goes.
struct RestoreDb(String);

impl RestoreDb {
    fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = format!("dbxtui_restore_{}_{}", std::process::id(), COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        assert!(name.starts_with("dbxtui_restore_"), "never a fixture database");
        psql_on("postgres", format!("drop database if exists {name} with (force);\ncreate database {name};\n").as_bytes()).expect("create the scratch database");
        RestoreDb(name)
    }

    /// Loads a dump file with `ON_ERROR_STOP`.
    fn load(&self, file: &std::path::Path) -> Result<String, String> {
        psql_on(&self.0, &std::fs::read(file).expect("the dump file"))
    }

    /// Loads a dump file going on past errors; returns the distinct errors.
    fn load_past_errors(&self, file: &std::path::Path) -> std::collections::BTreeSet<String> {
        let errors = psql_with(&self.0, &std::fs::read(file).expect("the dump file"), false).expect("psql ran");
        errors.lines().map(str::to_string).collect()
    }

    /// Starts again from an empty database.
    fn recreate(&self) {
        psql_on("postgres", format!("drop database if exists {0} with (force);\ncreate database {0};\n", self.0).as_bytes()).expect("recreate the scratch database");
    }
}

impl Drop for RestoreDb {
    fn drop(&mut self) {
        let _ = psql_on("postgres", format!("drop database if exists {} with (force);\n", self.0).as_bytes());
    }
}

#[test]
fn two_restore_databases_alive_together_do_not_drop_each_other() {
    let a = RestoreDb::new();
    let b = RestoreDb::new();
    assert_ne!(a.0, b.0, "each has its own name");
    psql_on(&a.0, b"create table keep (x integer); insert into keep values (1);").expect("a is usable");
    drop(b);
    assert_eq!(psql_on(&a.0, b"select count(*) from keep;").expect("a is still there").trim(), "1");
}

/// `schema.table|rows|md5 of the rows in order` for every table of `db`
/// outside the system schemas, `only` (when given) limiting it to those.
fn fingerprint(db: &str, only: Option<&[&str]>) -> Vec<String> {
    let filter = match only {
        Some(names) => format!(" and n.nspname || '.' || c.relname in ({})", names.iter().map(|n| format!("'{}'", n.replace('\'', "''"))).collect::<Vec<_>>().join(", ")),
        None => String::new(),
    };
    let script = format!(
        "select format('select %L || ''|'' || count(*) || ''|'' || coalesce(md5(string_agg(t::text, E''\\n'' order by t::text)), '''') from %I.%I t', \
         n.nspname || '.' || c.relname, n.nspname, c.relname) \
         from pg_class c join pg_namespace n on n.oid = c.relnamespace \
         where c.relkind in ('r', 'p') and n.nspname not in ('pg_catalog', 'information_schema') and left(n.nspname, 3) <> 'pg_'{filter} \
         order by 1 \\gexec\n"
    );
    let mut lines: Vec<String> = psql_on(db, script.as_bytes()).expect("fingerprint").lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    lines.sort();
    lines
}

/// Runs a dump to the end, collecting its progress.
async fn dump_to(core: &DbxCore, id: &ConnectionId, db: &str, schema: &str, content: DumpContent, path: &std::path::Path, prevent_overwrite: bool) -> (Vec<DumpProgress>, Result<DumpEnd, dbx_tui::core::CoreError>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let request = DumpRequest {
        at: database(id, db),
        schema: schema.into(),
        content,
        drop_tables: false,
        path: path.to_string_lossy().into_owned(),
        prevent_overwrite,
    };
    let end = core.dump(DumpId(rand_id()), request, std::sync::Arc::new(move |p| sink.lock().unwrap().push(p))).await;
    let seen = seen.lock().unwrap().clone();
    (seen, end)
}

#[tokio::test]
async fn a_whole_pagila_dump_restores_into_an_empty_database_with_every_tables_rows() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pagila.sql");
    let started = std::time::Instant::now();
    let (seen, end) = dump_to(&core, &id, "pagila", "", DumpContent::StructureAndData, &path, true).await;
    let end = end.expect("the dump ends");
    eprintln!("OBSERVED pagila dump: {:?}, {} progress reports, {} bytes, last {:?}", started.elapsed(), seen.len(), std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), end.last);
    assert_eq!(end.last.as_ref().map(|p| p.status), Some(DumpStatus::Done), "{end:?}");
    assert_eq!(end.last.as_ref().map(|p| p.errors), Some(0), "{end:?}");
    assert!(seen.iter().any(|p| p.status == DumpStatus::Running && p.rows > 0), "progress came while it ran");
    // dbx reports success, but the file does not load whole (README "Dump",
    // what dbx's dump loses): the first failure is the partitioned table.
    let restore = RestoreDb::new();
    let stopped = restore.load(&path).expect_err("KNOWN (dbx): pagila's dump does not load with ON_ERROR_STOP");
    assert!(stopped.contains("no partition of relation \"payment\" found for row"), "the first failure: {stopped}");
    // Loaded past its errors, every table but `film` and the payment
    // partitions comes back with the same rows.
    restore.recreate();
    let errors = restore.load_past_errors(&path);
    eprintln!("OBSERVED errors loading pagila's dump: {errors:#?}");
    let known = [
        "type \"year\" does not exist",
        "syntax error at or near \")\"",
        "no partition of relation \"payment\" found for row",
        "relation \"public.film\" does not exist",
    ];
    for k in known {
        assert!(errors.iter().any(|e| e.contains(k)), "KNOWN (dbx) error {k:?} not seen: {errors:#?}");
    }
    let lost = |line: &String| dbx_cannot_restore(line.split('|').next().unwrap_or(""));
    let want = fingerprint("pagila", None);
    assert!(want.len() >= 22, "{want:?}");
    let got = fingerprint(&restore.0, None);
    let (want_kept, want_lost): (Vec<_>, Vec<_>) = want.iter().cloned().partition(|l| !lost(l));
    assert_eq!(got.iter().filter(|l| !lost(l)).cloned().collect::<Vec<_>>(), want_kept, "every other table's rows, counted and hashed in order");
    for line in &want_lost {
        assert!(!got.contains(line), "KNOWN (dbx) loss now restores: {line} (update the README and this test)");
    }
    // Only read: the fixtures are as they were.
    check_fixtures();
}

/// The pagila tables dbx's dump does not give back (KNOWN, dbx): `film` (its
/// domain `year` is not written), six of the seven payment partitions (their
/// DDL is `PARTITION OF "public"."payment" (\n) FOR VALUES ...`, which does
/// not parse) and so the partitioned `payment` itself, whose rows dbx also
/// writes through the parent before any partition exists.
fn dbx_cannot_restore(table: &str) -> bool {
    table == "public.film" || table == "public.payment" || (table.starts_with("public.payment_p2022_0") && table != "public.payment_p2022_07")
}

/// `tests/fixtures/check.sh`: the fixture databases hold exactly what they
/// should, so a dump wrote nothing to them.
fn check_fixtures() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/check.sh");
    let out = std::process::Command::new("bash").arg(&script).stdin(std::process::Stdio::null()).output().expect("check.sh");
    assert!(out.status.success(), "check.sh: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn edges_exact_values_and_quoted_names_survive_a_dump_and_restore_but_nan_and_infinity_become_null() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge.sql");
    let started = std::time::Instant::now();
    let (_, end) = dump_to(&core, &id, "edge", "", DumpContent::StructureAndData, &path, true).await;
    let end = end.expect("the dump ends");
    eprintln!("OBSERVED edge dump: {:?}, {} bytes, last {:?}", started.elapsed(), std::fs::metadata(&path).unwrap().len(), end.last);
    assert_eq!(end.last.as_ref().map(|p| (p.status, p.errors)), Some((DumpStatus::Done, 0)), "{end:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(r#"CREATE TABLE "public"."Mixed Case""#), "the quoted name is written quoted");
    let restore = RestoreDb::new();
    let errors = restore.load_past_errors(&path);
    eprintln!("OBSERVED errors loading edge's dump: {errors:#?}");
    let exact = ["public.text_values", "public.gt_text", "public.gt_json", "public.Mixed Case", "other.things"];
    assert_eq!(fingerprint(&restore.0, Some(&exact)), fingerprint("edge", Some(&exact)), "exact values and quoted names");
    assert_eq!(fingerprint(&restore.0, Some(&exact)).len(), exact.len());
    // KNOWN (dbx): its dump writes NaN and the infinities of a float column as
    // NULL (`numbers.approx`, ids 3 and 4); everything else in the table is exact.
    let approx = |db: &str| psql_on(db, b"select id || '|' || coalesce(approx::text, 'NULL') from numbers order by id;").unwrap();
    assert_eq!(approx("edge").lines().collect::<Vec<_>>(), ["1|0.1", "2|0.30000000000000004", "3|NaN", "4|Infinity", "5|NULL"]);
    assert_eq!(approx(&restore.0).lines().collect::<Vec<_>>(), ["1|0.1", "2|0.30000000000000004", "3|NULL", "4|NULL", "5|NULL"]);
    let rest = |db: &str| psql_on(db, b"select string_agg(id || '|' || label || '|' || coalesce(big::text, 'NULL') || '|' || coalesce(exact::text, 'NULL'), ',' order by id) from numbers;").unwrap();
    assert_eq!(rest(&restore.0), rest("edge"));
    check_fixtures();
}

#[tokio::test]
async fn a_structure_only_dump_restores_the_tables_with_no_rows() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pagila-structure.sql");
    let (_, end) = dump_to(&core, &id, "pagila", "", DumpContent::StructureOnly, &path, true).await;
    assert_eq!(end.expect("ends").last.map(|p| p.status), Some(DumpStatus::Done));
    assert!(!std::fs::read_to_string(&path).unwrap().lines().any(|l| l.starts_with("INSERT INTO")), "no rows in the file");
    let restore = RestoreDb::new();
    restore.load_past_errors(&path);
    let got = fingerprint(&restore.0, None);
    let tables = |lines: &[String]| lines.iter().map(|l| l.split('|').next().unwrap().to_string()).collect::<Vec<_>>();
    // The same tables, but for those dbx's DDL cannot make (KNOWN: `film`'s
    // domain is not written; the partitions' DDL does not parse).
    // (`payment` itself is made; only its rows were lost above.)
    let want: Vec<String> = tables(&fingerprint("pagila", None)).into_iter().filter(|t| t == "public.payment" || !dbx_cannot_restore(t)).collect();
    assert_eq!(tables(&got), want);
    assert!(got.iter().all(|l| l.ends_with("|0|")), "every table is empty: {got:?}");
    check_fixtures();
}

#[tokio::test]
async fn a_file_that_is_there_is_refused_with_prevent_overwrite_and_left_byte_for_byte() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let dir = tempfile::tempdir().unwrap();
    for (db, schema) in [("edge", "other"), ("pagila", "")] {
        let path = dir.path().join(format!("{db}-taken.sql"));
        let mine: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        std::fs::write(&path, &mine).unwrap();
        let (_, end) = dump_to(&core, &id, db, schema, DumpContent::StructureAndData, &path, true).await;
        let err = end.expect_err("refused");
        assert!(err.message.contains("already exists") && err.message.contains(&*path.to_string_lossy()), "{err:?}");
        assert_eq!(std::fs::read(&path).unwrap(), mine, "{db}: the file is unchanged byte for byte");
    }
}

/// A scratch schema in `edge` with one table whose columns are named like
/// dbx's pass-through check (`"a"`, `` `b` ``, `[c]`), dropped when it goes.
struct QuotedColumns(String);

impl QuotedColumns {
    fn new() -> Self {
        let name = format!("dbxtui_quotecols_{}", std::process::id());
        let sql = format!(
            "drop schema if exists {name} cascade;\ncreate schema {name};\ncreate table {name}.t (id integer, \"\"\"a\"\"\" integer, \"`b`\" integer, \"[c]\" integer, \"say \"\"hi\"\"\" integer);\n"
        );
        psql_on("edge", sql.as_bytes()).expect("create the scratch schema");
        QuotedColumns(name)
    }
}

impl Drop for QuotedColumns {
    fn drop(&mut self) {
        let _ = psql_on("edge", format!("drop schema if exists {} cascade;\n", self.0).as_bytes());
    }
}

#[tokio::test]
async fn a_dump_is_refused_naming_the_columns_whose_names_dbx_would_not_quote() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let scratch = QuotedColumns::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quoted.sql");
    // One schema, and the whole database it is in: both refused, before dbx
    // is called, so nothing is written.
    for schema in [scratch.0.as_str(), ""] {
        let (seen, end) = dump_to(&core, &id, "edge", schema, DumpContent::StructureAndData, &path, true).await;
        let err = end.expect_err("refused");
        assert_eq!(err.kind, CoreErrorKind::Refused, "{err:?}");
        for want in [r#"t."a""#, "t.`b`", "t.[c]"] {
            assert!(err.message.contains(want), "{want} in {}", err.message);
        }
        assert!(err.message.contains(&scratch.0), "the schema is named: {}", err.message);
        assert!(!err.message.contains("say"), "a name that only has quotes inside is fine: {}", err.message);
        assert!(seen.is_empty(), "dbx was not called: {seen:?}");
        assert!(!path.exists(), "nothing was written");
    }
    // Another schema of the same database is not refused (the scan is for
    // the scope, not the whole database).
    let (_, end) = dump_to(&core, &id, "edge", "other", DumpContent::StructureOnly, &path, true).await;
    assert_eq!(end.expect("the other schema dumps").last.map(|p| p.status), Some(DumpStatus::Done));
    drop(scratch);
    check_fixtures();
}

/// The server sessions on `edge` running a statement on `million_rows`.
fn million_rows_readers() -> Vec<String> {
    psql_on("postgres", b"select pid from pg_stat_activity where datname = 'edge' and query ilike '%million_rows%' and pid <> pg_backend_pid();")
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether any of `pids` is still a session on the server.
fn still_there(pids: &[String]) -> Vec<String> {
    if pids.is_empty() {
        return Vec::new();
    }
    let sql = format!("select pid from pg_stat_activity where pid in ({});", pids.join(", "));
    psql_on("postgres", sql.as_bytes()).unwrap().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Dumps `edge` (`schema`, empty for all), cancels once `million_rows` is half
/// way, and returns the progress, the end and the export's server sessions.
async fn cancel_million_rows(core: &DbxCore, id: &ConnectionId, schema: &str, path: &std::path::Path) -> (Vec<DumpProgress>, DumpEnd, Vec<String>) {
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(Vec::<DumpProgress>::new()));
    let sink = seen.clone();
    let dump = DumpId(rand_id());
    let request = DumpRequest { at: database(id, "edge"), schema: schema.into(), content: DumpContent::StructureAndData, drop_tables: false, path: path.to_string_lossy().into_owned(), prevent_overwrite: true };
    let run = core.dump(dump, request, Arc::new(move |p| sink.lock().unwrap().push(p)));
    tokio::pin!(run);
    let (mut readers, mut asked, mut looks) = (Vec::new(), false, 0);
    let end = loop {
        tokio::select! {
            end = &mut run => break end,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                let midway = seen.lock().unwrap().iter().any(|p| p.object.contains("million_rows") && p.rows > 0);
                if midway && !asked {
                    // The export's session, seen reading the table (a few
                    // looks: between two pages it may be idle for a moment).
                    readers = million_rows_readers();
                    looks += 1;
                    if !readers.is_empty() || looks > 20 {
                        core.cancel_dump(dump);
                        asked = true;
                    }
                }
            }
        }
    };
    let seen = seen.lock().unwrap().clone();
    (seen, end.expect("a cancelled dump ends Ok"), readers)
}

#[tokio::test]
async fn a_dump_cancelled_midway_through_million_rows_ends_cancelled_and_leaves_no_session_of_its_own() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = saved_fixture(&core).await;
    let dir = tempfile::tempdir().unwrap();
    for schema in ["", "public"] {
        let path = dir.path().join(format!("edge-{}.sql", if schema.is_empty() { "all" } else { schema }));
        let started = std::time::Instant::now();
        let (seen, end, readers) = cancel_million_rows(&core, &id, schema, &path).await;
        assert!(end.cancelled(), "{end:?}");
        assert_eq!(seen.last().map(|p| p.status), Some(DumpStatus::Cancelled), "the last progress is Cancelled");
        assert!(!seen.iter().any(|p| p.status == DumpStatus::Done), "never Done");
        assert!(!readers.is_empty(), "the export was seen reading million_rows");
        // The export's session is closed before `dump` returns.
        let mut left = still_there(&readers);
        for _ in 0..20 {
            if left.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            left = still_there(&readers);
        }
        assert!(left.is_empty(), "the export's sessions are gone: {left:?}");
        let at_path = std::fs::metadata(&path).map(|m| m.len()).ok();
        let others: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        eprintln!("OBSERVED cancel of edge schema {schema:?} after {:?}: at the path {at_path:?}, in the directory {others:?}", started.elapsed());
        if schema.is_empty() {
            // The whole database goes schema by schema to temporary files,
            // and the file is opened only after the last: nothing is there.
            assert_eq!(at_path, None, "nothing at the path after a cancel of the whole database");
        } else {
            // One schema is written straight to the file: a partial file stays.
            let len = at_path.expect("a partial file");
            assert!(len > 0 && std::fs::read_to_string(&path).unwrap().contains("million_rows"), "{len}");
            std::fs::remove_file(&path).unwrap();
        }
    }
    check_fixtures();
}
