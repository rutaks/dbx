//! Layer 1, slice 23a: `DbxCore` against SQLite files. No Docker: the
//! fixtures are built by `tests/fixtures/sqlite/build.sh` with `/usr/bin/sqlite3`
//! into `target/sqlite-fixtures`, and every test opens a COPY of a built file
//! in a temporary directory (`common::sqlite_copy`), so none writes a fixture.
//!
//! Run: `cargo test -p dbx-tui --test sqlite -- --test-threads=1`
//!
//! Scratch store, scratch key and scratch HOME, as in `live.rs` (see
//! `common/mod.rs`): never the keychain, never a real store.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use dbx_tui::core::{
    Cell, CompletionRequest, ConnectionId, Core, CoreErrorKind, Cursor, DatabasePath, DbKind, DbxCore, ExportFormat, ExportRequest, RunId, RunRequest, SchemaPath, SearchKind,
    SessionId, Syntax, TabId, TablePath, DEFAULT_ROW_CAP,
};

const SQLITE3: &str = "/usr/bin/sqlite3";

/// `sqlite3 -batch -readonly` on `db`: the text it prints for `sql`.
fn sqlite3(db: &Path, sql: &str) -> String {
    let out = std::process::Command::new(SQLITE3).args(["-batch", "-readonly"]).arg(db).arg(sql).output().expect("sqlite3");
    assert!(out.status.success(), "sqlite3 {sql}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("UTF-8")
}

async fn saved(core: &DbxCore, name: &str, file: &Path) -> ConnectionId {
    core.save_connection(&sqlite_draft(name, file)).await.expect("save the SQLite connection").id
}

fn here(id: &ConnectionId) -> DatabasePath {
    DatabasePath { connection: id.clone(), database: String::new() }
}

fn schema(id: &ConnectionId) -> SchemaPath {
    SchemaPath { database: here(id), schema: String::new() }
}

fn table(id: &ConnectionId, name: &str) -> TablePath {
    TablePath { schema: schema(id), table: name.into() }
}

fn request(id: &ConnectionId, tab: u32, sql: &str) -> RunRequest {
    let rows: Vec<&str> = sql.split('\n').collect();
    RunRequest {
        connection: id.clone(),
        database: Some(String::new()),
        session: SessionId { tab: TabId(tab), serial: 1 },
        buffer: sql.into(),
        cursor: Cursor { row: rows.len() - 1, col: rows.last().unwrap().chars().count() },
        selection: None,
        row_cap: DEFAULT_ROW_CAP,
        confirm_dangerous: false,
        confirmation: None,
    }
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

async fn run(core: &DbxCore, id: &ConnectionId, tab: u32, sql: &str) -> Result<dbx_tui::core::ResultSet, dbx_tui::core::CoreError> {
    core.run(RunId(rand_id()), request(id, tab, sql)).await
}

async fn one(core: &DbxCore, id: &ConnectionId, sql: &str) -> String {
    let set = run(core, id, 1, sql).await.unwrap_or_else(|e| panic!("{sql}: {}", e.message));
    match &set.rows[0][0] {
        Cell::Value(v) => v.clone(),
        Cell::Null => "NULL".into(),
    }
}

// ---- the form's side: what is saved --------------------------------------------

#[tokio::test]
async fn a_sqlite_file_is_saved_with_its_path_and_nothing_else_and_is_browsable() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("sakila");
    let id = saved(&core, "sakila", &file).await;
    let list = core.connections().await.unwrap();
    let c = list.iter().find(|c| c.id == id).expect("listed");
    assert_eq!(c.kind, DbKind::Sqlite);
    assert_eq!(c.host, file.to_string_lossy());
    assert_eq!((c.port, c.database.as_deref(), c.username.as_str(), c.tls), (None, None, "", false), "{c:?}");
    assert!(c.capabilities.browse && c.capabilities.edit_in_form, "{:?}", c.capabilities);
    assert!(!c.capabilities.databases && !c.capabilities.schemas && c.capabilities.single_connection);
}

#[tokio::test]
async fn a_file_that_is_missing_or_not_a_regular_file_is_refused_by_a_test_and_a_save_with_a_message() {
    let s = Scratch::new();
    let core = s.core();
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.db");
    for draft in [sqlite_draft("missing", &missing), sqlite_draft("a directory", dir.path())] {
        for err in [core.test_connection(&draft).await.expect_err("test refused"), core.save_connection(&draft).await.expect_err("save refused")] {
            assert_eq!(err.kind, CoreErrorKind::Refused, "{err:?}");
            assert!(err.message.contains("regular file") && err.message.contains(&*draft.host), "{}", err.message);
        }
    }
    assert!(!missing.exists(), "refusing must not create the file");
    assert_eq!(core.connections().await.unwrap(), vec![], "nothing was saved");
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new());
    // A real file is accepted by a test, and leaves no temporary entry.
    let (_d, file) = sqlite_copy("edge");
    core.test_connection(&sqlite_draft("ok", &file)).await.expect("a real file");
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new());
}

#[tokio::test]
async fn a_regular_file_that_is_not_a_sqlite_database_is_refused_by_its_header() {
    let s = Scratch::new();
    let core = s.core();
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("notes.db");
    std::fs::write(&text, "just some text, more than sixteen bytes of it\n").unwrap();
    let short = dir.path().join("short.db");
    std::fs::write(&short, "SQLite").unwrap();
    for path in [&text, &short] {
        let before = std::fs::read(path).unwrap();
        let draft = sqlite_draft("not sqlite", path);
        for err in [core.test_connection(&draft).await.expect_err("test refused"), core.save_connection(&draft).await.expect_err("save refused")] {
            assert_eq!(err.kind, CoreErrorKind::Refused, "{err:?}");
            assert!(err.message.contains("not a SQLite database") && err.message.contains(&*draft.host), "{}", err.message);
        }
        assert_eq!(std::fs::read(path).unwrap(), before, "the file is untouched");
    }
    assert_eq!(core.connections().await.unwrap(), vec![]);
    // An empty file is an empty database to SQLite: it is accepted.
    let empty = dir.path().join("empty.db");
    std::fs::write(&empty, "").unwrap();
    core.test_connection(&sqlite_draft("empty", &empty)).await.expect("an empty file is a database");
}

#[tokio::test]
async fn a_relative_path_is_saved_made_absolute_from_the_working_directory() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    // The same file, named from the working directory: `../` up to the root.
    let cwd = std::env::current_dir().unwrap();
    let ups = cwd.components().count() - 1;
    let relative = format!("./{}{}", "../".repeat(ups), file.strip_prefix("/").unwrap().display());
    assert!(Path::new(&relative).is_relative());
    let mut draft = sqlite_draft("relative", Path::new(&relative));
    core.test_connection(&draft).await.expect("the relative path names the file");
    let saved = core.save_connection(&draft).await.expect("saved");
    assert!(Path::new(&saved.host).is_absolute(), "{}", saved.host);
    assert_eq!(saved.host, format!("{}/{}", cwd.display(), &relative[2..]), "joined to the working directory, `./` dropped, nothing resolved");
    use std::os::unix::fs::MetadataExt;
    let (a, b) = (std::fs::metadata(&saved.host).unwrap(), std::fs::metadata(&file).unwrap());
    assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()), "the same file");
    assert_eq!(one(&core, &saved.id, "select count(*) from parent").await, "2");
    // `~user/` is refused (dbx would look the user up; this client does not).
    draft.host = "~root/x.db".into();
    let err = core.save_connection(&draft).await.expect_err("refused");
    assert_eq!(err.kind, CoreErrorKind::Refused, "{err:?}");
}

// ---- browsing ---------------------------------------------------------------------

#[tokio::test]
async fn sakilas_tables_columns_count_and_completion_match_sqlite3() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("sakila");
    let id = saved(&core, "sakila", &file).await;
    // No database level and no schema level: the connection lists tables.
    let tables = core.tables(&schema(&id)).await.expect("tables");
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    let want = sqlite3(&file, "select name from sqlite_master where type in ('table', 'view') and name not like 'sqlite_%' order by name");
    assert_eq!(names, want.lines().collect::<Vec<_>>());
    let n: usize = sqlite3(&file, "select count(*) from sqlite_master where type in ('table', 'view')").trim().parse().unwrap();
    assert_eq!(tables.len(), n);
    assert!(tables.iter().any(|t| t.name == "customer_list" && t.kind == dbx_tui::core::TableKind::View), "{tables:?}");
    let columns = core.columns(&table(&id, "film")).await.expect("columns");
    let got: Vec<(&str, &str, bool, bool)> = columns.iter().map(|c| (c.name.as_str(), c.db_type.as_str(), c.nullable, c.primary_key)).collect();
    let pragma = sqlite3(&file, "select name, type, \"notnull\", pk from pragma_table_info('film')");
    let want: Vec<(String, String, bool, bool)> = pragma
        .lines()
        .map(|l| {
            let p: Vec<&str> = l.split('|').collect();
            (p[0].to_string(), p[1].to_string(), p[2] == "0", p[3] != "0")
        })
        .collect();
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((g.0, g.1, g.2, g.3), (w.0.as_str(), w.1.as_str(), w.2, w.3));
    }
    assert_eq!(one(&core, &id, "select count(*) from film").await, "1000");
    // Completion of `fil`, the way the app asks for the tables of a schema.
    for schema_name in [Some(String::new()), Some("main".to_string()), None] {
        let found = core
            .complete(&CompletionRequest { at: here(&id), schema: schema_name.clone(), kind: SearchKind::Relation, prefix: "fil".into(), limit: 50, global: false })
            .await
            .expect("complete");
        let mut names: Vec<&str> = found.candidates.iter().map(|c| c.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["film", "film_actor", "film_category", "film_list", "film_text"], "schema {schema_name:?}");
    }
}

#[tokio::test]
async fn the_current_schema_of_a_sqlite_file_is_main_without_asking_the_file() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    assert_eq!(core.current_schema(&here(&id)).await.expect("current"), Some("main".to_string()));
}

#[tokio::test]
async fn foreign_keys_of_a_sqlite_table_are_listed_for_join_completion() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let keys = core.foreign_keys(&table(&id, "child")).await.expect("foreign keys");
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert_eq!((keys[0].column.as_str(), keys[0].ref_table.as_str(), keys[0].ref_column.as_str()), ("parent_id", "parent", "id"));
    eprintln!("OBSERVED foreign key: {:?}", keys[0]);
    let sakila = sqlite_copy("sakila");
    let id = saved(&core, "sakila", &sakila.1).await;
    let keys = core.foreign_keys(&table(&id, "film")).await.expect("foreign keys");
    let mut got: Vec<(String, String)> = keys.iter().map(|k| (k.column.clone(), k.ref_table.clone())).collect();
    got.sort();
    assert_eq!(got, vec![("language_id".into(), "language".into()), ("original_language_id".into(), "language".into())]);
}

#[tokio::test]
async fn a_pages_of_a_table_match_sqlite3_with_the_same_order() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("sakila");
    let id = saved(&core, "sakila", &file).await;
    let film = table(&id, "film");
    for (limit, offset) in [(50, 0), (50, 100), (50, 950), (7, 999), (50, 1000)] {
        let sql = core.table_page_sql(&DbKind::Sqlite, &film, &["film_id".to_string()], limit, offset);
        let got = run(&core, &id, 1, &sql).await.unwrap_or_else(|e| panic!("{sql}: {}", e.message));
        let titles: Vec<String> = got.rows.iter().map(|r| if let Cell::Value(v) = &r[got.columns.iter().position(|c| c.name == "title").unwrap()] { v.clone() } else { "NULL".into() }).collect();
        let want = sqlite3(&file, &format!("select title from film order by film_id limit {limit} offset {offset}"));
        assert_eq!(titles, want.lines().collect::<Vec<_>>(), "limit {limit} offset {offset}: {sql}");
    }
    let count = core.table_count_sql(&DbKind::Sqlite, &film);
    assert_eq!(one(&core, &id, &count).await, "1000", "{count}");
    // A table with a quote and a second statement in its name.
    let edge = sqlite_copy("edge");
    let id = saved(&core, "edge", &edge.1).await;
    let odd = table(&id, "x\"; select 1 --");
    let sql = core.table_page_sql(&DbKind::Sqlite, &odd, &["id".to_string()], 10, 0);
    let got = run(&core, &id, 1, &sql).await.unwrap_or_else(|e| panic!("{sql}: {}", e.message));
    assert_eq!(got.rows, vec![vec![Cell::Value("1".into()), Cell::Value("one known row".into())]]);
    // Tables named `"x"` and `[x]`, beside a plain `x`: each page and count
    // reads its own table, never `x`.
    for (name, rows) in [("\"x\"", 2), ("[x]", 3), ("x", 1)] {
        let t = table(&id, name);
        let sql = core.table_page_sql(&DbKind::Sqlite, &t, &["id".to_string()], 10, 0);
        let got = run(&core, &id, 1, &sql).await.unwrap_or_else(|e| panic!("{name}: {sql}: {}", e.message));
        let want = sqlite3(&file_of(&edge), &format!("select id || '|' || v from {} order by id", quote(name)));
        let got_text: Vec<String> = got.rows.iter().map(|r| format!("{}|{}", text_of(&r[0]), text_of(&r[1]))).collect();
        assert_eq!(got_text, want.lines().collect::<Vec<_>>(), "{name}: {sql}");
        assert_eq!(got.rows.len(), rows, "{name}");
        let count = core.table_count_sql(&DbKind::Sqlite, &t);
        assert_eq!(one(&core, &id, &count).await, rows.to_string(), "{name}: {count}");
    }
}

fn file_of(copy: &(tempfile::TempDir, std::path::PathBuf)) -> std::path::PathBuf {
    copy.1.clone()
}

fn text_of(c: &Cell) -> String {
    match c {
        Cell::Value(v) => v.clone(),
        Cell::Null => "NULL".into(),
    }
}

// ---- sessions, queueing, cancelling ---------------------------------------------------

const LONG: &str = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 3000000) select count(*) from c";

#[tokio::test]
async fn two_tabs_on_one_file_both_finish() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let started = Instant::now();
    let spawn = |tab: u32| {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move {
            let t = Instant::now();
            let r = run(&core, &id, tab, LONG).await;
            (t.elapsed(), started.elapsed(), r)
        })
    };
    let (a, b) = (spawn(1), spawn(2));
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    for (took, ended, r) in [&a, &b] {
        let set = r.as_ref().unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(set.rows, vec![vec![Cell::Value("3000000".into())]]);
        eprintln!("OBSERVED two tabs: took {took:?}, ended {ended:?} after the start");
    }
    // One statement alone, for what the two together cost.
    let t = Instant::now();
    one(&core, &id, LONG).await;
    eprintln!("OBSERVED one tab alone: {:?}", t.elapsed());
}

#[tokio::test]
async fn closing_one_tabs_session_does_not_end_another_tabs_work_or_the_connection() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    // Both tabs have a session on the file.
    assert_eq!(run(&core, &id, 1, "select 1").await.unwrap().rows.len(), 1);
    assert_eq!(run(&core, &id, 2, "select 2").await.unwrap().rows.len(), 1);
    let long = {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move { run(&core, &id, 2, LONG).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    core.close_session(SessionId { tab: TabId(1), serial: 1 }, &here(&id)).await.expect("close tab 1's session");
    let set = long.await.unwrap().unwrap_or_else(|e| panic!("tab 2's run ended: {}", e.message));
    assert_eq!(set.rows, vec![vec![Cell::Value("3000000".into())]]);
    assert_eq!(one(&core, &id, "select count(*) from parent").await, "2", "the connection still works");
    // And the closed tab's session opens again when the tab runs once more.
    assert_eq!(run(&core, &id, 1, "select 1").await.unwrap().rows.len(), 1);
}

const FOREVER: &str = "with recursive c(x) as (select 1 union all select x + 1 from c where x < 1000000000) select count(*) from c";

#[tokio::test]
async fn a_cancelled_recursive_statement_reports_cancelled_and_the_next_statement_on_the_file_works() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let running = {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move { core.run(RunId(9001), request(&id, 1, FOREVER)).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cancelled_at = Instant::now();
    core.cancel(RunId(9001));
    let result = tokio::time::timeout(Duration::from_secs(20), running).await.expect("a cancelled run returns").unwrap();
    let err = result.expect_err("cancelled");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    eprintln!("OBSERVED sqlite cancel: the run returned {:?} after the cancel", cancelled_at.elapsed());
    // The same tab, and another tab, run again at once.
    let t = Instant::now();
    assert_eq!(run(&core, &id, 1, "select count(*) from parent").await.unwrap().rows, vec![vec![Cell::Value("2".into())]]);
    assert_eq!(run(&core, &id, 2, "select count(*) from parent").await.unwrap().rows, vec![vec![Cell::Value("2".into())]]);
    eprintln!("OBSERVED after the cancel, the next statements took {:?}", t.elapsed());
}

#[tokio::test]
async fn a_write_waiting_on_another_tabs_lock_is_cancelled_and_says_what_it_knows() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    // Tab 1 holds the write lock: a transaction is open on its session.
    run(&core, &id, 1, "begin immediate").await.expect("tab 1 begins");
    run(&core, &id, 1, "insert into parent values (50, 'held')").await.expect("tab 1 writes");
    let waiting = {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move {
            let t = Instant::now();
            let r = core.run(RunId(9101), request(&id, 2, "insert into parent values (51, 'waits')")).await;
            (t.elapsed(), r)
        })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    let cancelled_at = Instant::now();
    core.cancel(RunId(9101));
    let (took, result) = tokio::time::timeout(Duration::from_secs(60), waiting).await.expect("the waiting write returned").unwrap();
    eprintln!("OBSERVED a write waiting on a lock: returned {took:?} after it started, {:?} after the cancel: {:?}", cancelled_at.elapsed(), result.as_ref().map(|_| "ok").map_err(|e| (e.kind, e.message.clone())));
    // The lock was never given up: the write gave up waiting and wrote nothing.
    let err = result.expect_err("the write did not run");
    assert_eq!(err.kind, CoreErrorKind::Cancelled, "{err:?}");
    assert_eq!(sqlite3(&file, "select count(*) from parent where id = 51").trim(), "0", "row 51 was not written");
    assert_eq!(one(&core, &id, "select count(*) from parent where id = 51").await, "0", "nor in tab 1's transaction");
    // Tab 1 can roll back and the file is whole.
    run(&core, &id, 1, "rollback").await.expect("tab 1 rolls back");
    assert_eq!(sqlite3(&file, "select count(*) from parent where id >= 50").trim(), "0");
}

#[tokio::test]
async fn a_write_that_finishes_after_its_cancel_because_the_lock_came_free_is_never_called_cancelled() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    run(&core, &id, 1, "begin immediate").await.expect("tab 1 begins");
    run(&core, &id, 1, "insert into parent values (50, 'held')").await.expect("tab 1 writes");
    let waiting = {
        let (core, id) = (core.clone(), id.clone());
        tokio::spawn(async move { core.run(RunId(9201), request(&id, 2, "insert into parent values (51, 'waits')")).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    core.cancel(RunId(9201));
    // While tab 2 still waits on SQLite's busy handler, tab 1 lets go.
    tokio::time::sleep(Duration::from_millis(300)).await;
    run(&core, &id, 1, "commit").await.expect("tab 1 commits");
    let result = tokio::time::timeout(Duration::from_secs(60), waiting).await.expect("the waiting write returned").unwrap();
    let written = sqlite3(&file, "select count(*) from parent where id = 51").trim() == "1";
    eprintln!("OBSERVED a cancelled write whose lock came free: written {written}, said {:?}", result.as_ref().map(|s| (s.affected, s.messages.clone())).map_err(|e| (e.kind, e.message.clone())));
    match &result {
        Err(e) => assert!(!written && e.kind == CoreErrorKind::Cancelled, "only a write that did not happen is called cancelled: written {written}, {e:?}"),
        Ok(set) => {
            assert!(written, "a write reported as done is in the file");
            assert_eq!(set.affected, 1, "{set:?}");
            assert!(set.messages.iter().any(|m| m.contains("before the cancel")), "it says the cancel came too late: {:?}", set.messages);
        }
    }
    // What happens: SQLite's interrupt does not end the wait, but it is
    // remembered, and stops the statement as soon as the lock comes free.
    // The write never lands, and "cancelled" is the truth.
    assert!(!written && matches!(&result, Err(e) if e.kind == CoreErrorKind::Cancelled), "{result:?}");
    assert_eq!(sqlite3(&file, "select count(*) from parent where id = 50").trim(), "1", "tab 1's write committed");
}

#[tokio::test]
async fn dbxs_analysis_names_the_tables_and_aliases_of_a_sqlite_statement() {
    let s = Scratch::new();
    let core = s.core();
    let sql = "select f.title from film f join language l on l.language_id = f.language_id where f.film_id = 1";
    let refs = core.analyze_references(&DbKind::Sqlite, sql).await.expect("analysis").expect("it parses");
    let mut tables: Vec<(String, Option<String>)> = refs.tables.iter().map(|t| (t.name.clone(), t.alias.clone())).collect();
    tables.sort();
    assert_eq!(tables, vec![("film".to_string(), Some("f".to_string())), ("language".to_string(), Some("l".to_string()))]);
    assert!(refs.columns.iter().any(|c| c.name == "title" && c.qualifier.as_deref() == Some("f")), "{:?}", refs.columns);
    // A statement dbx cannot parse gives no analysis (and no error).
    assert_eq!(core.analyze_references(&DbKind::Sqlite, "select from from").await.expect("analysis"), None);
}

// ---- confirmations (slice 17 on SQLite) ------------------------------------------------

async fn saved_production(core: &DbxCore, name: &str, file: &Path) -> ConnectionId {
    let mut draft = sqlite_draft(name, file);
    draft.production = true;
    core.save_connection(&draft).await.expect("save the production SQLite connection").id
}

/// The reasons a run is asked about, or `None` when it ran.
async fn asked(core: &DbxCore, id: &ConnectionId, sql: &str, confirm_dangerous: bool) -> Option<Vec<dbx_tui::core::Reason>> {
    let mut r = request(id, 1, sql);
    r.confirm_dangerous = confirm_dangerous;
    match core.run(RunId(rand_id()), r).await {
        Err(e) if e.kind == CoreErrorKind::NeedsConfirmation => Some(e.assessment.expect("an assessment").reasons),
        _ => None,
    }
}

#[tokio::test]
async fn on_a_production_file_a_read_sqlite_proves_read_only_runs_and_a_write_asks() {
    use dbx_tui::core::Reason;
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("sakila");
    let id = saved_production(&core, "prod", &file).await;
    // SQLite prepares it on the file, read-only, and says it does not write.
    for sql in ["select * from film", "select count(*) from film", "with f as (select title from film) select * from f", "values (1)", "select 1; select 2"] {
        assert_eq!(asked(&core, &id, sql, true).await, None, "{sql} runs without a question");
    }
    // Writes ask, whatever SQLite would say.
    assert_eq!(asked(&core, &id, "insert into language (name) values ('x')", true).await.map(|r| r[0]), Some(Reason::Writes));
    assert_eq!(asked(&core, &id, "select 1; delete from language", true).await.map(|r| r[0]), Some(Reason::Writes));
    // A read SQLite cannot prepare on the file (a function dbx adds, a table
    // that is not there) or that loads code stays unproven, and asks.
    for sql in ["select unistr('a')", "select * from no_such_table", "select load_extension('/x')"] {
        assert_eq!(asked(&core, &id, sql, true).await, Some(vec![Reason::Unproven]), "{sql}");
    }
    // Statements SQLite calls read-only that change the connection or other
    // files ask, as dangerous, even with the danger setting off.
    for sql in ["ATTACH '/x' AS y", "VACUUM INTO '/x'", "PRAGMA writable_schema = 1"] {
        for setting in [true, false] {
            let reasons = asked(&core, &id, sql, setting).await.unwrap_or_else(|| panic!("{sql} asks"));
            assert!(reasons.contains(&Reason::Dangerous), "{sql}: {reasons:?}");
        }
    }
    for sql in ["begin", "commit"] {
        assert_eq!(asked(&core, &id, sql, false).await.map(|r| r[0]), Some(Reason::TransactionControl), "{sql}");
    }
    assert!(!Path::new("/x").exists());
}

#[tokio::test]
async fn off_production_attach_vacuum_into_and_writable_schema_ask_as_dangerous_and_select_1_runs() {
    use dbx_tui::core::Reason;
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    for sql in ["ATTACH '/x' AS y", "attach database '/x' as y", "VACUUM INTO '/x'", "vacuum", "PRAGMA writable_schema = 1", "pragma writable_schema=on"] {
        let reasons = asked(&core, &id, sql, true).await.unwrap_or_else(|| panic!("{sql} asks"));
        assert!(reasons.contains(&Reason::Dangerous), "{sql}: {reasons:?}");
    }
    assert_eq!(asked(&core, &id, "select 1", true).await, None);
    assert_eq!(asked(&core, &id, "select * from parent", true).await, None);
    assert!(!Path::new("/x").exists());
}

#[tokio::test]
async fn load_more_reruns_a_read_sqlite_proves_asks_for_one_it_cannot_and_never_reruns_a_write() {
    use dbx_tui::core::Rerun;
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let rerun = |sql: &'static str| {
        let (core, id) = (&core, &id);
        async move { run(core, id, 1, sql).await.unwrap_or_else(|e| panic!("{sql}: {}", e.message)).rerun }
    };
    assert_eq!(rerun("select * from numbers_10k").await, Rerun::Proven);
    assert_eq!(rerun("with c as (select id from numbers_10k) select * from c").await, Rerun::Proven);
    assert_eq!(rerun("values (1), (2)").await, Rerun::Proven);
    // `unistr` is dbx's own function on its connection: SQLite cannot prepare
    // the statement without it, so it is not proven.
    assert_eq!(rerun("select unistr('a')").await, Rerun::ReadOnlyUnproven);
    assert_eq!(rerun("insert into parent values (90, 'p') returning id").await, Rerun::Write);
}

// ---- export and dump ---------------------------------------------------------------------

/// Every table of edge.db with the columns it is read and compared by.
const EDGE_TABLES: [&str; 19] = [
    "text_values", "integers", "reals", "texts", "blobs", "documents", "dates", "typed", "untyped", "no_pk", "composite_key", "Mixed Case", "x\"; select 1 --", "empty_table", "parent", "child", "x", "\"x\"", "[x]",
];

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// The text `sqlite3 -quote` gives for every row of `table` in rowid order.
fn quoted_rows(db: &Path, table: &str) -> String {
    let out = std::process::Command::new(SQLITE3)
        .args(["-batch", "-readonly", "-quote"])
        .arg(db)
        .arg(format!("select * from {} order by rowid", quote(table)))
        .output()
        .expect("sqlite3");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn sql_insert_exports_of_the_edge_tables_run_into_a_scratch_copy_which_then_equals_the_source() {
    let s = Scratch::new();
    let core = s.core();
    let (dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    // A scratch file with the same tables and nothing in them.
    let scratch = dir.path().join("scratch.db");
    let schema_sql = sqlite3(&file, ".schema");
    let status = std::process::Command::new(SQLITE3).arg(&scratch).arg(&schema_sql).status().unwrap();
    assert!(status.success());
    let mut differences = Vec::new();
    for name in EDGE_TABLES {
        let set = run(&core, &id, 1, &format!("select * from {} order by rowid", quote(name))).await.unwrap_or_else(|e| panic!("{name}: {}", e.message));
        if set.rows.is_empty() {
            // The client does not export a result with no rows ("nothing to insert").
            assert_eq!(name, "empty_table");
            continue;
        }
        let export = ExportRequest { format: ExportFormat::SqlInsert, columns: set.columns.clone(), rows: set.rows.clone(), kind: Some(DbKind::Sqlite), table: name.into(), schema: String::new(), null_empty: false };
        let sql = match core.export_bytes(&export) {
            Ok(b) => String::from_utf8(b).expect("UTF-8"),
            Err(e) => panic!("{name}: {e}"),
        };
        let status = std::process::Command::new(SQLITE3).arg(&scratch).arg(&sql).status().unwrap();
        assert!(status.success(), "{name}: the INSERTs ran: {}", sql.chars().take(400).collect::<String>());
        let (want, got) = (quoted_rows(&file, name), quoted_rows(&scratch, name));
        if want != got {
            let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
            for i in 0..w.len().max(g.len()) {
                if w.get(i) != g.get(i) {
                    differences.push(format!("{name} row {}:\n   source  {:?}\n   scratch {:?}", i + 1, w.get(i), g.get(i)));
                }
            }
        }
    }
    eprintln!("OBSERVED sql-insert differences: {}", differences.len());
    for d in &differences {
        eprintln!("OBSERVED   {d}");
    }
    // Equal, but for what dbx's decoding gives the client and the INSERTs
    // cannot say (KNOWN_DIFFERENCES-sqlite.md, README "What each format loses"):
    // infinity is NULL, a blob is the text `0x..`, and an integer past 2^53
    // in a column with no declared type (dbx gives it as text) is written as
    // text. Nothing else differs: the other numbers of that column are
    // written as numbers.
    let keys: Vec<&str> = differences.iter().map(|d| d.lines().next().unwrap().trim_end_matches(':')).collect();
    assert_eq!(
        keys,
        [
            "reals row 9",
            "reals row 10",
            "blobs row 1",
            "blobs row 2",
            "blobs row 3",
            "blobs row 4",
            "blobs row 6",
            "untyped row 4",
            "untyped row 6",
            "untyped row 8",
            "untyped row 9",
        ],
        "a scratch copy built from the exports equals the source except for the known losses"
    );
}

#[tokio::test]
async fn a_json_export_writes_the_numbers_of_an_untyped_column_as_numbers_and_its_text_as_text() {
    let s = Scratch::new();
    let core = s.core();
    let (_dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let set = run(&core, &id, 1, "select v, count(*) over () as n, 1.5 * 2 as x from untyped order by id").await.expect("read");
    assert!(set.columns.iter().all(|c| c.db_type.is_empty()), "no column has a type name: {:?}", set.columns);
    let export = ExportRequest { format: ExportFormat::Json, columns: set.columns.clone(), rows: set.rows.clone(), kind: Some(DbKind::Sqlite), table: "untyped".into(), schema: String::new(), null_empty: false };
    let json: serde_json::Value = serde_json::from_slice(&core.export_bytes(&export).expect("JSON")).expect("valid JSON");
    let v: Vec<serde_json::Value> = json.as_array().expect("rows").iter().map(|r| r["v"].clone()).collect();
    use serde_json::json;
    // integer, real, text, blob, null, big integer, number as text, infinity, i64 min
    assert_eq!(
        v,
        [json!(1), json!(1.5), json!("x"), json!("0x00ff"), json!(null), json!("9007199254740993"), json!("42"), json!(null), json!("-9223372036854775808")],
    );
    assert!(json.as_array().unwrap().iter().all(|r| r["n"] == json!(9) && r["x"] == json!(3.0)), "{json}");
}

/// Dumps `file` (a connection saved for it) to `<dir>/out/dump.sql` and
/// loads that into `<dir>/restored.db`; returns the restored path and what
/// sqlite3 said while loading.
async fn dump_and_restore(core: &DbxCore, dir: &Path, file: &Path) -> (std::path::PathBuf, String) {
    let id = saved(core, "dumped", file).await;
    let out = dir.join("out");
    std::fs::create_dir(&out).unwrap();
    let path = out.join("dump.sql");
    let request = dbx_tui::core::DumpRequest {
        at: here(&id),
        schema: String::new(),
        content: dbx_tui::core::DumpContent::StructureAndData,
        drop_tables: false,
        path: path.to_string_lossy().into_owned(),
        prevent_overwrite: true,
    };
    let end = core.dump(dbx_tui::core::DumpId(rand_id()), request, std::sync::Arc::new(|_| {})).await;
    eprintln!("OBSERVED sqlite dump: {end:?}");
    end.expect("dbx dumps a SQLite file");
    let restored = dir.join("restored.db");
    let text = std::fs::read(&path).expect("the dump");
    let mut child = std::process::Command::new(SQLITE3).arg(&restored).stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&text).unwrap();
    }
    let done = child.wait_with_output().unwrap();
    (restored, String::from_utf8_lossy(&done.stderr).into_owned())
}

#[tokio::test]
async fn what_a_dump_of_the_edge_file_loses_is_exactly_what_the_readme_lists() {
    let s = Scratch::new();
    let core = s.core();
    let (dir, file) = sqlite_copy("edge");
    let (restored, errors) = dump_and_restore(&core, dir.path(), &file).await;
    eprintln!("OBSERVED restore errors: {errors}");
    let objects = |db: &Path| sqlite3(db, "select type || ' ' || name from sqlite_master where name not like 'sqlite_%' order by 1");
    eprintln!("OBSERVED objects only in the source: {:?}", objects(&file).lines().filter(|l| !objects(&restored).lines().any(|r| r == *l)).collect::<Vec<_>>());
    let mut differing = Vec::new();
    for name in EDGE_TABLES {
        let (want, got) = (quoted_rows(&file, name), quoted_rows(&restored, name));
        if want != got {
            let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
            for i in 0..w.len().max(g.len()) {
                if w.get(i) != g.get(i) {
                    differing.push(format!("{name} row {}: source {:?} restored {:?}", i + 1, w.get(i), g.get(i)));
                }
            }
        }
    }
    for d in &differing {
        eprintln!("OBSERVED dump difference: {d}");
    }
    // Each is a line of README "What dbx's dump loses on SQLite": infinity
    // written as NULL, a blob written as the text `0x..`, a time zone dropped
    // from a date-time, a number in an untyped column written as text.
    let keys: Vec<&str> = differing.iter().map(|d| d.split(':').next().unwrap()).collect();
    assert_eq!(
        keys,
        [
            "reals row 9",
            "reals row 10",
            "blobs row 1",
            "blobs row 2",
            "blobs row 3",
            "blobs row 4",
            "blobs row 6",
            "dates row 2",
            "untyped row 4",
            "untyped row 6",
            "untyped row 8",
            "untyped row 9",
        ]
    );
    assert_eq!(errors.trim(), "", "the file loads with no error");
    assert!(objects(&file).lines().all(|l| objects(&restored).lines().any(|r| r == l)), "every table, view and index came back");
}

#[tokio::test]
async fn a_dump_restores_into_a_new_file_with_the_same_row_counts() {
    let s = Scratch::new();
    let core = s.core();
    let (dir, file) = sqlite_copy("sakila");
    let (restored, errors) = dump_and_restore(&core, dir.path(), &file).await;
    eprintln!("OBSERVED restore errors: {errors}");
    assert_eq!(errors.trim(), "", "Sakila's dump loads with no error");
    let tables = sqlite3(&file, "select name from sqlite_master where type = 'table' and name not like 'sqlite_%' order by name");
    assert!(tables.lines().count() >= 16, "{tables}");
    for name in tables.lines() {
        let q = format!("select count(*) from {}", quote(name));
        assert_eq!(sqlite3(&restored, &q), sqlite3(&file, &q), "{name}");
    }
}

#[tokio::test]
async fn a_dump_into_the_connections_own_file_or_its_journal_wal_or_shm_is_refused_and_nothing_is_written() {
    let s = Scratch::new();
    let core = s.core();
    let (dir, file) = sqlite_copy("edge");
    let id = saved(&core, "edge", &file).await;
    let before = std::fs::read(&file).unwrap();
    let link = dir.path().join("link.db");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let hard = dir.path().join("hard.db");
    std::fs::hard_link(&file, &hard).unwrap();
    let side = |suffix: &str| std::path::PathBuf::from(format!("{}{suffix}", file.display()));
    // A sibling that is there (an empty -shm someone left) and ones that are not.
    std::fs::write(side("-shm"), "").unwrap();
    let dotted = dir.path().join(".").join("edge.db");
    for target in [file.clone(), link.clone(), hard.clone(), dotted, side("-wal"), side("-journal"), side("-shm")] {
        for prevent_overwrite in [false, true] {
            let request = dbx_tui::core::DumpRequest {
                at: here(&id),
                schema: String::new(),
                content: dbx_tui::core::DumpContent::StructureAndData,
                drop_tables: true,
                path: target.to_string_lossy().into_owned(),
                prevent_overwrite,
            };
            let err = core.dump(dbx_tui::core::DumpId(rand_id()), request, std::sync::Arc::new(|_| {})).await.expect_err("refused");
            assert_eq!(err.kind, CoreErrorKind::Refused, "{}: {err:?}", target.display());
            assert!(err.message.contains("the connection's own file") || err.message.contains("SQLite keeps"), "{}", err.message);
        }
    }
    assert_eq!(std::fs::read(&file).unwrap(), before, "the file is untouched");
    assert!(!side("-wal").exists() && !side("-journal").exists());
    assert_eq!(std::fs::read(side("-shm")).unwrap(), b"", "the -shm is untouched");
    // Another name in the same directory is dumped.
    let request = dbx_tui::core::DumpRequest {
        at: here(&id),
        schema: String::new(),
        content: dbx_tui::core::DumpContent::StructureAndData,
        drop_tables: false,
        path: dir.path().join("edge.db.sql").to_string_lossy().into_owned(),
        prevent_overwrite: true,
    };
    core.dump(dbx_tui::core::DumpId(rand_id()), request, std::sync::Arc::new(|_| {})).await.expect("a dump beside the file");
}
