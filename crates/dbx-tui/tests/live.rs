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
    ConnectionDraft, ConnectionId, Core, CoreErrorKind, DatabasePath, DbxCore, SchemaPath, TableKind, TablePath,
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
