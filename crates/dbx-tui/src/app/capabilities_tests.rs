//! Layer 2: the acceptance tests of slice 14b (capabilities), run on `App`
//! with a `FakeCore` whose connections have the shapes MySQL and SQLite have.

use super::connections_tests::{conn, sidebar_rows};
use super::run_tests::{mode_bar, notice_text, runs};
use super::testing::*;
use super::Command;
use crate::core::{Call, Capabilities, ConnectionId, DatabasePath, DbKind, FakeCore, SchemaPath, Scripted, Table, TableKind};
use crate::harness::Harness;

fn cid(name: &str) -> ConnectionId {
    ConnectionId(format!("id-{name}"))
}

fn table(name: &str) -> Table {
    Table { name: name.into(), kind: TableKind::Table }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The tree's rows as text, trimmed, without the title rows.
fn tree(h: &mut Harness) -> Vec<String> {
    sidebar_rows(h, 114, 30).into_iter().skip(2).map(|r| r.trim_end().to_string()).filter(|r| !r.is_empty()).collect()
}

async fn started(fake: FakeCore, connection: crate::core::Connection) -> Harness {
    fake.script_connections(Scripted::ok(vec![connection]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h
}

#[tokio::test]
async fn a_connection_without_schemas_lists_databases_and_then_tables_directly() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["shop", "blog"])));
    fake.script_tables(Scripted::ok(vec![table("orders"), table("items")]));
    let mut h = started(fake, conn("my", DbKind::MySql)).await;
    h.press("enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    let rows = tree(&mut h);
    let text = rows.join("\n");
    assert!(text.contains("blog") && text.contains("shop"), "{text}");
    assert!(text.contains("orders") && text.contains("items"), "tables directly under the database:\n{text}");
    let db = DatabasePath { connection: cid("my"), database: "shop".into() };
    let calls = h.fake.calls();
    assert!(!calls.iter().any(|c| matches!(c, Call::Schemas(_))), "no schema level to ask about: {calls:?}");
    assert!(
        calls.contains(&Call::Tables(SchemaPath { database: db, schema: String::new() })),
        "the tables of the database, with no schema name: {calls:?}"
    );
}

#[tokio::test]
async fn a_connection_without_databases_lists_its_tables_directly() {
    let fake = FakeCore::new();
    fake.script_tables(Scripted::ok(vec![table("notes"), table("tags")]));
    let mut h = started(fake, conn("lite", DbKind::Sqlite)).await;
    h.press("enter");
    h.settle().await;
    let text = tree(&mut h).join("\n");
    assert!(text.contains("notes") && text.contains("tags"), "{text}");
    let calls = h.fake.calls();
    let none = DatabasePath { connection: cid("lite"), database: String::new() };
    assert_eq!(calls, vec![Call::Connections, Call::Tables(SchemaPath { database: none, schema: String::new() })]);
}

#[tokio::test]
async fn a_connection_with_schemas_and_no_databases_lists_schemas_and_then_their_tables() {
    let fake = FakeCore::new();
    fake.script_schemas(Scripted::ok(names(&["main", "aux"])));
    fake.script_tables(Scripted::ok(vec![table("t1")]));
    let mut c = conn("duck", DbKind::Sqlite);
    c.capabilities = Capabilities { databases: false, schemas: true, ..Capabilities::sqlite() };
    let mut h = started(fake, c).await;
    h.press("enter");
    h.settle().await;
    let text = tree(&mut h).join("\n");
    assert!(text.contains("aux") && text.contains("main"), "{text}");
    h.press("j enter");
    h.settle().await;
    let text = tree(&mut h).join("\n");
    assert!(text.contains("t1"), "{text}");
    let calls = h.fake.calls();
    let none = DatabasePath { connection: cid("duck"), database: String::new() };
    assert!(!calls.iter().any(|c| matches!(c, Call::Databases(_))), "{calls:?}");
    assert_eq!(calls[1], Call::Schemas(none.clone()));
    assert_eq!(calls[2], Call::Tables(SchemaPath { database: none, schema: "main".into() }));
}

#[tokio::test]
async fn closing_a_tree_without_a_database_level_releases_nothing() {
    let fake = FakeCore::new();
    fake.script_tables(Scripted::ok(vec![table("notes")]));
    fake.script_columns(Scripted::ok(vec![]));
    let mut h = started(fake, conn("lite", DbKind::Sqlite)).await;
    h.press("enter");
    h.settle().await;
    // A table's columns are cached under a path that has no database name.
    h.press("j enter");
    h.settle().await;
    h.press("g");
    let commands = h.press_fx("h");
    assert!(!commands.iter().any(|c| matches!(c, Command::Release(_))), "{commands:?}");
}

#[tokio::test]
async fn a_connection_that_cannot_run_queries_says_so_by_name_and_emits_no_run() {
    let mut c = conn("registry", DbKind::Postgres);
    c.database = Some("main".into());
    c.capabilities.query = false;
    let mut h = started(FakeCore::new(), c).await;
    h.press("u ctrl+g l");
    h.type_text("select 1");
    let mut commands = h.press_fx("f5");
    commands.extend(h.press_fx("ctrl+enter"));
    assert!(runs(&commands).is_empty(), "{commands:?}");
    let note = notice_text(&h);
    assert!(note.contains("registry") && note.contains("cannot be queried"), "{note}");
    assert!(!h.fake.calls().iter().any(|c| matches!(c, Call::Run(_))));
    let _ = mode_bar(&mut h);
}

#[tokio::test]
async fn the_new_connection_form_starts_with_the_port_capabilities_give() {
    let fake = FakeCore::empty();
    fake.script_capabilities(DbKind::Postgres, Capabilities { default_port: Some(6543), ..Capabilities::postgres() });
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("ctrl+g n");
    let rows = frame_rows(&mut h.app, 114, 30);
    let port = rows.iter().find(|r| r.trim_start().contains("port") && r.contains("65")).cloned();
    assert!(port.is_some_and(|r| r.contains("6543")), "{rows:#?}");
}

/// The files in `src/app` that may name a `DbKind` variant. The editor's
/// dialect choices read `Dialect` from `Capabilities`; nothing in the app
/// branches on which database a connection is, so the list is empty. A new
/// entry needs a reason here.
const MAY_NAME_DB_KIND: &[&str] = &[];

#[test]
fn no_file_in_app_branches_on_the_kind_of_database() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut offenders = Vec::new();
    for path in files {
        let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        // Tests build connections of every kind; the rule is about the code.
        if name.ends_with("_tests.rs") || name == "tests.rs" || name == "testing.rs" || MAY_NAME_DB_KIND.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // An inline test module (`mod x {`) ends the code; a `#[cfg(test)]` on
        // an item, or on a `mod x;` line that names a file, does not.
        let mut code = text.as_str();
        for (at, _) in text.match_indices("#[cfg(test)]\nmod ") {
            if text[at..].lines().nth(1).is_some_and(|l| l.trim_end().ends_with('{')) {
                code = &text[..at];
                break;
            }
        }
        if code.contains("DbKind::") {
            offenders.push(name);
        }
    }
    assert!(offenders.is_empty(), "these branch on DbKind; read Capabilities instead: {offenders:?}");
}

fn metadata_calls(h: &Harness) -> Vec<Call> {
    h.fake
        .calls()
        .into_iter()
        .filter(|c| {
            matches!(
                c,
                Call::Analyze(_) | Call::Complete(_) | Call::CurrentSchema(_) | Call::Columns(_) | Call::ForeignKeys(_) | Call::Schemas(_)
            )
        })
        .collect()
}

#[tokio::test]
async fn a_postgres_dialect_connection_that_cannot_browse_asks_the_core_for_no_names() {
    let mut c = conn("ux", DbKind::Postgres);
    c.database = Some("main".into());
    c.capabilities.browse = false;
    let mut h = started(FakeCore::new(), c).await;
    h.press("u ctrl+g l");
    h.type_text("select * from or");
    h.settle().await;
    h.advance(std::time::Duration::from_secs(5));
    h.settle().await;
    h.press("ctrl+space");
    h.settle().await;
    h.app.ui.tab.editor.set_text("select * from t t1 join u on t1.");
    h.press("ctrl+space");
    h.settle().await;
    h.advance(std::time::Duration::from_secs(5));
    h.settle().await;
    assert_eq!(metadata_calls(&h), vec![], "no completion or marks call for a connection that cannot browse");
}

#[tokio::test]
async fn a_connection_without_a_database_level_and_a_default_database_is_not_a_target() {
    let mut c = conn("lite", DbKind::Sqlite);
    c.database = None;
    c.capabilities.databases = false;
    let mut h = started(FakeCore::new(), c).await;
    h.press("u");
    assert_eq!(h.app.model.tabs[0].target, None);
    let note = h.app.ui.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(note.contains("no database of its own"), "{note}");
}

#[tokio::test]
async fn a_listing_that_comes_back_for_the_wrong_level_leaves_the_node_not_loaded() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["shop"])));
    let mut h = started(fake, conn("my", DbKind::MySql)).await;
    let node = crate::app::NodeId::Connection(cid("my"));
    h.app.model.schema.set(node.clone(), crate::app::tree::Children::Loading);
    let mut fx = super::Effects::default();
    h.app.on_listed(node.clone(), Ok(crate::app::Listing::Tables(vec![])), &mut fx);
    assert!(matches!(h.app.model.schema.state(&node), crate::app::tree::Children::NotLoaded));
}

#[tokio::test]
async fn a_connection_that_cannot_browse_or_query_says_both_in_the_sidebar() {
    let mut c = conn("registry", DbKind::Postgres);
    c.capabilities.browse = false;
    c.capabilities.query = false;
    let mut h = started(FakeCore::new(), c).await;
    h.press("enter");
    let text = tree(&mut h).join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("not supported yet") && text.contains("cannot be queried"), "{text}");
}
