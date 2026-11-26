//! Layer 2: the acceptance tests of slice 23a (SQLite), run on `App` with a
//! `FakeCore` whose connection has the shape a SQLite file has: no database
//! level, no schema level. What dbx does with a file is tested in
//! `tests/sqlite.rs` and `tests/ground_truth_sqlite.rs`.

use super::complete_tests::{names, relation, text, type_and_settle};
use super::connections_tests::conn;
use super::export::ExportEnv;
use super::form_tests::form_rows;
use super::run_tests::dbp;
use super::testing::*;
use super::{Command, CoreCall};
use crate::core::{Call, CandidateKind, Catalogue, CoreErrorKind, DbKind, DumpContent, FakeCore, Scripted, Table, TableKind};
use crate::harness::Harness;

fn labels(h: &mut Harness) -> Vec<String> {
    const LABELS: [&str; 10] = ["type", "name", "host", "file", "port", "user", "password", "database", "tls", "production"];
    form_rows(h)
        .iter()
        .map(|r| r.trim().split(' ').next().unwrap_or("").to_string())
        .filter(|l| LABELS.contains(&l.as_str()))
        .collect()
}

async fn form() -> Harness {
    let mut h = Harness::start(FakeCore::empty());
    h.settle().await;
    h
}

/// The new-connection form with its type set to SQLite and focus on the name.
async fn sqlite_form() -> Harness {
    let mut h = form().await;
    h.press("ctrl+g n");
    h.press("up");
    h.press("right");
    h.press("down");
    h
}

fn saved(commands: &[Command]) -> Vec<crate::core::ConnectionDraft> {
    commands
        .iter()
        .filter_map(|c| match c {
            Command::Core(_, CoreCall::SaveConnection(d)) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_new_connection_form_offers_a_type_and_postgres_keeps_every_field() {
    let mut h = form().await;
    h.press("ctrl+g n");
    let rows = labels(&mut h);
    for want in ["type", "name", "host", "port", "user", "password", "database", "tls", "production"] {
        assert!(rows.iter().any(|r| r == want), "{want} in {rows:?}");
    }
    let all = form_rows(&mut h).join("\n");
    assert!(all.contains("PostgreSQL") && all.contains("SQLite"), "both types are offered:\n{all}");
}

#[tokio::test]
async fn the_sqlite_form_shows_a_name_and_a_file_and_no_host_port_user_password_or_tls() {
    let mut h = sqlite_form().await;
    let rows = labels(&mut h);
    assert_eq!(rows, ["type", "name", "file", "production"], "{rows:?}\n{}", form_rows(&mut h).join("\n"));
    // Back to PostgreSQL: the server fields are back, with the port.
    h.press("up right");
    let rows = labels(&mut h);
    assert!(rows.iter().any(|r| r == "host") && rows.iter().any(|r| r == "port"), "{rows:?}");
    assert!(form_rows(&mut h).join("\n").contains("5432"));
}

fn the_form(h: &Harness) -> &crate::app::form::ConnectionForm {
    match &h.app.ui.overlay {
        Some(super::overlay::Overlay::Form(f)) => f,
        _ => panic!("no form"),
    }
}

#[tokio::test]
async fn switching_the_type_keeps_what_was_typed_for_each_type_and_brings_it_back() {
    let mut h = form().await;
    h.press("ctrl+g n");
    h.type_text("keep me");
    h.press("tab");
    h.type_text("db.example");
    h.press("tab");
    h.press("backspace backspace backspace backspace");
    h.type_text("6543");
    h.press("tab");
    h.type_text("alice");
    h.press("tab");
    h.type_text("s3cret");
    h.press("tab");
    h.type_text("appdb");
    h.press("tab space");
    // Up to the type row, and on to SQLite.
    h.press("up up up up up up up");
    assert_eq!(the_form(&h).focus, super::form::Field::Kind);
    h.press("right");
    assert_eq!(the_form(&h).kind, DbKind::Sqlite);
    let rows = form_rows(&mut h).join("\n");
    assert!(!rows.contains("db.example"), "the file row starts empty, not with the host:\n{rows}");
    h.press("down down");
    h.type_text("/tmp/a.db");
    // Back to PostgreSQL: everything typed for it is back.
    h.press("up up left");
    let f = the_form(&h);
    assert_eq!(f.kind, DbKind::Postgres);
    let texts = |f: &crate::app::form::ConnectionForm| [f.name.text(), f.host.text(), f.port.text(), f.user.text(), f.password.text(), f.database.text()].map(str::to_string);
    assert_eq!(texts(f), ["keep me", "db.example", "6543", "alice", "s3cret", "appdb"].map(str::to_string));
    assert!(f.tls, "the TLS box too");
    // And on to SQLite again: its file is back.
    h.press("right");
    assert_eq!(the_form(&h).host.text(), "/tmp/a.db");
    assert_eq!(the_form(&h).name.text(), "keep me");
    let drafts = saved(&h.press_fx("enter"));
    assert_eq!(drafts.len(), 1);
    assert_eq!((drafts[0].kind.clone(), drafts[0].host.as_str(), drafts[0].port, drafts[0].password.as_deref()), (DbKind::Sqlite, "/tmp/a.db", None, None));
}

#[tokio::test]
async fn a_click_on_the_type_rows_label_changes_nothing_and_a_click_on_a_choice_switches() {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mut h = form().await;
    h.press("ctrl+g n");
    let click = |h: &mut Harness, x: u16, y: u16| {
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            let fx = h.app.update(super::Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }), base());
            h.run(fx);
        }
    };
    let (_, row) = find_text(&mut h.app, 100, 34, "PostgreSQL").expect("the choices");
    let line = frame_rows(&mut h.app, 100, 34)[row as usize].clone();
    let at = |text: &str| line[..line.find(text).unwrap()].chars().count() as u16;
    let label = line[..line.find("PostgreSQL").unwrap()].rfind("type").map(|b| line[..b].chars().count() as u16).expect("the label");
    let sx = at("SQLite");
    click(&mut h, label + 1, row);
    assert_eq!(the_form(&h).kind, DbKind::Postgres, "the label is not a choice");
    click(&mut h, sx + 9, row);
    assert_eq!(the_form(&h).kind, DbKind::Postgres, "past the last choice is not a choice");
    click(&mut h, sx + 1, row);
    assert_eq!(the_form(&h).kind, DbKind::Sqlite, "a click on a choice switches");
}

#[tokio::test]
async fn the_sqlite_form_saves_the_path_as_the_host_with_no_port_user_database_or_tls() {
    let mut h = sqlite_form().await;
    h.type_text("sakila copy");
    h.press("tab");
    h.type_text("/tmp/sakila.db");
    let drafts = saved(&h.press_fx("enter"));
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    let d = &drafts[0];
    assert_eq!(d.kind, DbKind::Sqlite);
    assert_eq!((d.name.as_str(), d.host.as_str()), ("sakila copy", "/tmp/sakila.db"));
    assert_eq!((d.port, d.database.as_deref(), d.username.as_str(), d.tls, d.password.as_ref()), (None, None, "", false, None));
}

#[tokio::test]
async fn an_empty_path_is_refused_on_the_file_row_and_nothing_is_sent() {
    let mut h = sqlite_form().await;
    h.type_text("nothing");
    let sent = saved(&h.press_fx("enter"));
    assert!(sent.is_empty(), "{sent:?}");
    let rows = form_rows(&mut h).join("\n");
    assert!(rows.contains("File is required"), "{rows}");
}

#[tokio::test]
async fn a_file_the_core_refuses_is_said_on_the_form_with_the_path_and_the_form_stays_open() {
    let mut h = sqlite_form().await;
    h.fake.script_save_connection(Scripted::err(CoreErrorKind::Refused, "/tmp/nope.db is not a regular file. Give the path of an existing SQLite file."));
    h.type_text("lost");
    h.press("tab");
    h.type_text("/tmp/nope.db");
    h.press("enter");
    h.settle().await;
    let rows = form_rows(&mut h).join("\n");
    assert!(rows.contains("not a regular file") && rows.contains("/tmp/nope.db"), "{rows}");
    assert!(rows.contains("lost"), "what was typed is kept:\n{rows}");
}

#[tokio::test]
async fn editing_a_sqlite_connection_shows_its_file_and_no_server_fields() {
    let mut c = conn("lite", DbKind::Sqlite);
    c.host = "/data/lite.db".into();
    c.port = None;
    c.username = String::new();
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![c]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("e");
    let rows = labels(&mut h);
    assert_eq!(rows, ["type", "name", "file", "production"], "{rows:?}");
    assert!(form_rows(&mut h).join("\n").contains("/data/lite.db"));
    // An edit keeps its own type: the type row does not change it.
    h.press("up right");
    assert!(labels(&mut h).iter().all(|r| r != "host"));
}

fn main_catalogue() -> Catalogue {
    use CandidateKind::Table as T;
    Catalogue {
        current_schema: None,
        schemas: vec!["main".into()],
        relations: vec![
            relation("main", "actor", T),
            relation("main", "film", T),
            relation("main", "film_actor", T),
            relation("main", "film_category", T),
            relation("main", "film_text", T),
        ],
        columns: vec![("main".into(), "film".into(), vec![super::complete_tests::column("film_id", "INTEGER", true), super::complete_tests::column("title", "VARCHAR(255)", false)])],
        ..Catalogue::default()
    }
}

async fn lite_editing() -> Harness {
    let fake = FakeCore::new();
    fake.script_catalogue(main_catalogue());
    fake.script_connections(Scripted::ok(vec![conn("lite", DbKind::Sqlite)]));
    fake.script_tables(Scripted::ok(vec![Table { name: "film".into(), kind: TableKind::Table }]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("g u ctrl+g l");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("lite", "")));
    h
}

#[tokio::test]
async fn a_sqlite_connection_gets_completion_and_current_schema_is_never_asked() {
    let mut h = lite_editing().await;
    type_and_settle(&mut h, "select * from fil").await;
    let mut found = names(&h);
    found.sort();
    assert_eq!(found, ["film", "film_actor", "film_category", "film_text"]);
    assert_eq!(names(&h)[0], "film", "the exact name first");
    h.press("enter");
    assert!(text(&h).starts_with("select * from film"), "{}", text(&h));
    let calls = h.fake.calls();
    assert!(!calls.iter().any(|c| matches!(c, Call::CurrentSchema(_))), "no query for the schema: {calls:?}");
    let asked = calls.iter().any(|c| matches!(c, Call::Complete(r) if r.schema.as_deref() == Some("main")));
    assert!(asked, "the tables of `main` were searched: {calls:?}");
}

#[tokio::test]
async fn columns_of_a_sqlite_table_complete_in_a_where_clause() {
    let mut h = lite_editing().await;
    type_and_settle(&mut h, "select * from film where ti").await;
    h.settle().await;
    assert_eq!(names(&h).first().map(String::as_str), Some("title"), "{:?}", names(&h));
    let calls = h.fake.calls();
    assert!(!calls.iter().any(|c| matches!(c, Call::CurrentSchema(_))), "{calls:?}");
}

#[tokio::test]
async fn the_dump_key_on_a_sqlite_connection_dumps_the_file_by_its_connections_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = conn("lite", DbKind::Sqlite);
    c.capabilities.dump_loses = true;
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![c]));
    fake.script_tables(Scripted::ok(Vec::new()));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.app.ui.export_env = ExportEnv::fixed(&dir.path().to_string_lossy(), "20261126-101500");
    assert!(super::run_tests::mode_bar(&mut h).contains("D dump"), "{}", super::run_tests::mode_bar(&mut h));
    h.press("D");
    let screen: String = frame_rows(&mut h.app, 100, 34).join(" ").chars().filter(|c| !"│┌┐└┘─".contains(*c)).collect();
    let words = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(words.contains("Dump lite"), "titled by the connection: {words}");
    assert!(words.contains("lite-20261126-101500.sql"), "{words}");
    assert!(words.contains("blobs") && words.contains("known not to restore whole"), "SQLite's own losses are named: {words}");
    assert!(!words.contains("domains") && !words.contains("partitions"), "not Postgres's: {words}");
    let sent: Vec<_> = h.press_fx("enter").into_iter().filter_map(|c| if let Command::Dump(_, _, r) = c { Some(r) } else { None }).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].at, dbp("lite", ""));
    assert_eq!(sent[0].schema, "");
    assert_eq!(sent[0].content, DumpContent::StructureAndData);
}
