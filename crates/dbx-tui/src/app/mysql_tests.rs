//! Layer 2: the acceptance tests of slice 23b (MySQL), run on `App` with a
//! `FakeCore` whose connection has the shape a MySQL server has: databases and
//! then tables, no schema level, the current database standing where
//! Postgres's current schema stands. What dbx does against a server is tested
//! in `tests/mysql.rs` and `tests/ground_truth_mysql.rs`.

use super::complete_tests::{column, names, relation, text, type_and_settle};
use super::connections_tests::{conn, sidebar_rows};
use super::export::ExportEnv;
use super::form_tests::form_rows;
use super::run_tests::dbp;
use super::testing::*;
use super::{Command, CoreCall};
use crate::core::{Call, CandidateKind, Catalogue, ConnectionDraft, DbKind, DatabasePath, FakeCore, SchemaPath, Scripted, Table, TableKind};
use crate::harness::Harness;

fn labels(h: &mut Harness) -> Vec<String> {
    const LABELS: [&str; 10] = ["type", "name", "host", "file", "port", "user", "password", "database", "tls", "production"];
    form_rows(h)
        .iter()
        .map(|r| r.trim().split(' ').next().unwrap_or("").to_string())
        .filter(|l| LABELS.contains(&l.as_str()))
        .collect()
}

fn saved(commands: &[Command]) -> Vec<ConnectionDraft> {
    commands
        .iter()
        .filter_map(|c| match c {
            Command::Core(_, CoreCall::SaveConnection(d)) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

/// The new-connection form with its type set to MySQL and focus on the type row.
async fn mysql_form() -> Harness {
    let mut h = Harness::start(FakeCore::empty());
    h.settle().await;
    h.press("ctrl+g n");
    h.press("up");
    h.press("right");
    h
}

#[tokio::test]
async fn the_mysql_form_offers_every_server_field_and_its_port_starts_at_3306_from_capabilities() {
    let mut h = mysql_form().await;
    let rows = labels(&mut h);
    assert_eq!(rows, ["type", "name", "host", "port", "user", "password", "database", "tls", "production"], "{rows:?}");
    let all = form_rows(&mut h).join("\n");
    assert!(all.contains("● MySQL"), "MySQL is the type in force:\n{all}");
    assert!(all.contains("3306") && !all.contains("5432"), "the type's default port, not Postgres's:\n{all}");
    // Back to PostgreSQL and on again: each has its own port.
    h.press("left");
    assert!(form_rows(&mut h).join("\n").contains("5432"));
    h.press("right");
    assert!(form_rows(&mut h).join("\n").contains("3306"));
}

#[tokio::test]
async fn the_mysql_form_saves_a_server_connection_with_a_password_a_database_and_tls() {
    let mut h = mysql_form().await;
    h.press("down");
    h.type_text("shop");
    h.press("tab");
    h.type_text("db.example");
    h.press("tab tab");
    h.type_text("alice");
    h.press("tab");
    h.type_text("s3cret");
    h.press("tab");
    h.type_text("sakila");
    h.press("tab space");
    let drafts = saved(&h.press_fx("enter"));
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    let d = &drafts[0];
    assert_eq!(d.kind, DbKind::MySql);
    assert_eq!((d.name.as_str(), d.host.as_str(), d.port, d.username.as_str()), ("shop", "db.example", Some(3306), "alice"));
    assert_eq!((d.password.as_deref(), d.database.as_deref(), d.tls), (Some("s3cret"), Some("sakila"), true));
}

fn sakila_catalogue() -> Catalogue {
    use CandidateKind::{Table as T, View as V};
    Catalogue {
        // What the server answered to `select database()`.
        current_schema: Some("sakila".into()),
        schemas: vec!["edge".into(), "edge_other".into(), "sakila".into()],
        relations: vec![
            relation("sakila", "actor", T),
            relation("sakila", "film", T),
            relation("sakila", "film_actor", T),
            relation("sakila", "film_category", T),
            relation("sakila", "film_list", V),
            relation("sakila", "film_text", T),
            relation("edge", "Order Items", T),
            relation("edge_other", "t", T),
        ],
        columns: vec![
            ("sakila".into(), "film".into(), vec![column("film_id", "smallint unsigned", true), column("title", "varchar(128)", false)]),
            ("edge".into(), "Order Items".into(), vec![column("id", "int", true), column("select", "varchar(20)", false)]),
        ],
        ..Catalogue::default()
    }
}

/// `my` opened in the sidebar, `sakila` given to the tab, the editor focused.
async fn my_editing() -> Harness {
    let fake = FakeCore::new();
    fake.script_catalogue(sakila_catalogue());
    my_editing_on(fake).await
}

async fn my_editing_on(fake: FakeCore) -> Harness {
    fake.script_connections(Scripted::ok(vec![conn("my", DbKind::MySql)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "edge_other".into(), "sakila".into()]));
    fake.script_tables(Scripted::ok(vec![Table { name: "film".into(), kind: TableKind::Table }]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("j j j u ctrl+g l");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("my", "sakila")));
    h
}

#[tokio::test]
async fn the_tree_of_a_mysql_connection_goes_from_database_to_table_with_no_schema_level() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn("my", DbKind::MySql)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "sakila".into()]));
    fake.script_tables(Scripted::ok(vec![Table { name: "film".into(), kind: TableKind::Table }, Table { name: "actor".into(), kind: TableKind::Table }]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("j j enter");
    h.settle().await;
    let tree: String = sidebar_rows(&mut h, 114, 30).join("\n");
    assert!(tree.contains("sakila") && tree.contains("film") && tree.contains("actor"), "{tree}");
    let calls = h.fake.calls();
    assert!(!calls.iter().any(|c| matches!(c, Call::Schemas(_))), "{calls:?}");
    let db = DatabasePath { connection: dbp("my", "sakila").connection, database: "sakila".into() };
    assert!(calls.contains(&Call::Tables(SchemaPath { database: db, schema: String::new() })), "{calls:?}");
    assert!(!tree.contains("not supported yet"), "MySQL is browsed now:\n{tree}");
}

#[tokio::test]
async fn completion_on_mysql_asks_the_current_database_and_searches_its_tables() {
    let mut h = my_editing().await;
    type_and_settle(&mut h, "select * from fil").await;
    let mut found = names(&h);
    found.sort();
    assert_eq!(found, ["film", "film_actor", "film_category", "film_list", "film_text"]);
    assert_eq!(names(&h)[0], "film", "the exact name first");
    h.press("enter");
    assert!(text(&h).starts_with("select * from film"), "{}", text(&h));
    let calls = h.fake.calls();
    assert!(calls.contains(&Call::CurrentSchema(dbp("my", "sakila"))), "the current database is asked once: {calls:?}");
    assert!(
        calls.iter().any(|c| matches!(c, Call::Complete(r) if r.schema.as_deref() == Some("sakila") && r.at == dbp("my", "sakila"))),
        "the tables of `sakila` were searched, with the database as the schema the Core takes it in: {calls:?}"
    );
}

#[tokio::test]
async fn another_database_is_qualified_by_its_name_and_its_tables_complete_after_the_dot() {
    let mut h = my_editing().await;
    type_and_settle(&mut h, "select * from edge_other.").await;
    assert_eq!(names(&h), ["t"], "{:?}", names(&h));
    let calls = h.fake.calls();
    assert!(
        calls.iter().any(|c| matches!(c, Call::Complete(r) if r.schema.as_deref() == Some("edge_other"))),
        "the tables of edge_other were searched: {calls:?}"
    );
}

#[tokio::test]
async fn a_table_name_that_needs_backticks_is_inserted_with_them_and_a_plain_one_without() {
    let mut h = my_editing().await;
    type_and_settle(&mut h, "select * from edge.Ord").await;
    assert_eq!(names(&h), ["Order Items"], "{:?}", names(&h));
    h.press("enter");
    // (The setting that adds an alias adds one after the name.)
    assert_eq!(text(&h), "select * from edge.`Order Items` oi");
    let mut h = my_editing().await;
    type_and_settle(&mut h, "select * from fil").await;
    h.press("enter");
    assert_eq!(text(&h), "select * from film f");
}

#[tokio::test]
async fn columns_of_a_mysql_table_complete_in_a_where_clause() {
    let mut h = my_editing().await;
    type_and_settle(&mut h, "select * from film where ti").await;
    h.settle().await;
    assert_eq!(names(&h).first().map(String::as_str), Some("title"), "{:?}", names(&h));
}

#[tokio::test]
async fn a_live_template_on_mysql_completes_its_table_from_the_current_database() {
    let mut h = my_editing().await;
    type_and_settle(&mut h, "sel").await;
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "select * from ;");
    type_and_settle(&mut h, "film_c").await;
    assert_eq!(names(&h)[0], "film_category", "{:?}", names(&h));
    h.press("tab");
    assert_eq!(text(&h), "select * from film_category;", "the tables of `sakila`, the database the tab is on");
    let calls = h.fake.calls();
    assert!(calls.contains(&Call::CurrentSchema(dbp("my", "sakila"))), "{calls:?}");
}

#[tokio::test]
async fn marks_on_mysql_resolve_a_table_of_the_current_database_and_mark_a_column_it_has_not() {
    use super::theme::Role;
    use crate::core::{ColumnReference, Cursor, References, Span, TableReference};
    let span = |from: usize, to: usize| Span { start: Cursor { row: 0, col: from }, end: Cursor { row: 0, col: to } };
    let sql = "select title, nope from film";
    let refs = References {
        tables: vec![TableReference { name: "film".into(), schema: None, alias: None, span: span(24, 28), scope: 0 }],
        columns: vec![
            ColumnReference { name: "title".into(), qualifier: None, span: span(7, 12), scope: 0 },
            ColumnReference { name: "nope".into(), qualifier: None, span: span(14, 18), scope: 0 },
        ],
        scopes: vec![(0, None)],
        ..References::default()
    };
    let fake = FakeCore::new();
    fake.script_catalogue(sakila_catalogue());
    fake.script_analysis(sql, refs);
    let mut h = my_editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    h.app.ui.tab.editor.set_text(sql);
    h.press("end");
    for _ in 0..6 {
        h.advance(std::time::Duration::from_millis(250));
        h.settle().await;
    }
    let role_of = |h: &mut Harness, word: &str| {
        let (x, y) = find_text(&mut h.app, 100, 30, word).unwrap_or_else(|| panic!("{word} on screen"));
        role_at(&mut h.app, x, y).fg
    };
    assert_eq!(role_of(&mut h, "title"), Some(Role::SynColumn));
    assert_eq!(role_of(&mut h, "nope"), Some(Role::SynUnresolved));
    assert_eq!(role_of(&mut h, "film"), Some(Role::SynIdentifier), "a table of `sakila` is not marked missing");
}

#[tokio::test]
async fn the_dump_key_on_a_mysql_database_names_mysqls_own_losses() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = conn("my", DbKind::MySql);
    c.capabilities.dump_loses = true;
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![c]));
    fake.script_databases(Scripted::ok(vec!["sakila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("j");
    h.app.ui.export_env = ExportEnv::fixed(&dir.path().to_string_lossy(), "20261126-101500");
    h.press("D");
    let screen: String = frame_rows(&mut h.app, 100, 34).join(" ").chars().filter(|c| !"│┌┐└┘─".contains(*c)).collect();
    let words = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(words.contains("Dump sakila"), "{words}");
    assert!(words.contains("known not to restore whole"), "{words}");
    let mine = crate::app::dump::known_losses(crate::core::Dialect::MySql);
    assert_ne!(mine, crate::app::dump::KNOWN_LOSSES, "MySQL has a list of its own");
    let head: String = mine.chars().take(24).collect();
    assert!(words.contains(&head), "MySQL's own losses are named ({head}): {words}");
    assert!(!words.contains("domains") && !words.contains("partitions"), "not Postgres's: {words}");
}
