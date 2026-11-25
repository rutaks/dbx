//! Layer 2: the acceptance tests of slice 16 (table browse), run on `App` with
//! a `FakeCore` behind the shell's command runner. What dbx builds for a page
//! is tested in `core/dbx.rs` and `tests/live.rs`; here the fake spells it.

use std::time::Duration;

use super::connections_tests::conn;
use super::query::{Count, QueryTab, RunState};
use super::run_tests::{col, mode_bar, notice_text, results_rows, runs, title};
use super::testing::*;
use super::tree::RowKind;
use super::*;
use crate::core::{
    Call, Capabilities, Cell, CellKind, ColumnInfo, Connection, ConnectionId, CoreErrorKind, DbKind, FakeCore, ResultSet, SchemaPath, Scripted, Syntax,
    Table, TableKind, TablePath,
};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

fn cid() -> ConnectionId {
    ConnectionId("id-local pg".into())
}

fn path(database: &str, schema: &str, table: &str) -> TablePath {
    TablePath {
        schema: SchemaPath { database: DatabasePath { connection: cid(), database: database.into() }, schema: schema.into() },
        table: table.into(),
    }
}

fn film() -> TablePath {
    path("pagila", "public", "film")
}

fn info(name: &str, pk: bool) -> ColumnInfo {
    ColumnInfo { name: name.into(), db_type: "integer".into(), nullable: !pk, primary_key: pk, default: None }
}

/// `n` rows of `film`, ids from `from`.
fn page(from: usize, n: usize) -> ResultSet {
    ResultSet {
        columns: vec![col("film_id", "integer", CellKind::Integer), col("title", "text", CellKind::Text)],
        rows: (from..from + n).map(|i| vec![Cell::Value(i.to_string()), Cell::Value(format!("film {i}"))]).collect(),
        truncated: false,
        elapsed: Duration::from_millis(12),
        messages: vec![],
        affected: 0,
        ran: "select ...".into(),
        rerun: crate::core::Rerun::Proven,
    }
}

fn counted(n: &str) -> ResultSet {
    ResultSet { columns: vec![col("cnt", "bigint", CellKind::Integer)], rows: vec![vec![Cell::Value(n.into())]], ..page(0, 0) }
}

/// The page select the fake builds for `film`, ordered by its key.
fn film_sql(h: &Harness, offset: usize) -> String {
    h.fake.table_page_sql(&DbKind::Postgres, &film(), &["film_id".into()], 1000, offset)
}

/// `local pg` > `pagila` > `public` open, showing `film` (key `film_id`) and
/// `payment` (no key), the cursor on `film`, its columns not loaded yet.
async fn tree_of(connection: Connection) -> Harness {
    let fake = FakeCore::new();
    // The generated text is in an editor, whose names the app looks up.
    fake.script_catalogue(super::complete_tests::pagila());
    fake.script_connections(Scripted::ok(vec![connection]));
    fake.script_databases(Scripted::ok(vec!["pagila".into(), "edge".into()]));
    fake.script_schemas(Scripted::ok(vec!["public".into()]));
    let table = |name: &str| Table { name: name.into(), kind: TableKind::Table };
    fake.script_tables(Scripted::ok(vec![table("film"), table("payment")]));
    let mut h = Harness::start(fake);
    h.settle().await;
    for keys in ["enter", "j enter", "j enter", "j"] {
        h.press(keys);
        h.settle().await;
    }
    h
}

async fn tree() -> Harness {
    tree_of(conn("local pg", DbKind::Postgres)).await
}

/// `tree`, with `film`'s columns loaded (the tree already has them).
async fn film_loaded() -> Harness {
    let mut h = tree().await;
    h.fake.script_columns(Scripted::ok(vec![info("film_id", true), info("title", false)]));
    h.press("enter");
    h.settle().await;
    h
}

/// Lets the Core's replies arrive until the active tab's run has ended (a
/// reply of the fake with no delay still waits for the timer's next tick).
async fn land(h: &mut Harness) {
    for _ in 0..200 {
        h.settle().await;
        if h.app.model.tab(h.app.active_tab()).is_none_or(|t| t.run.in_flight().is_none()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// `film` opened: its tab active, the first page of `n` rows landed.
async fn browsing(n: usize) -> Harness {
    let mut h = film_loaded().await;
    h.fake.script_run(Scripted::ok(page(1, n)));
    h.press("o");
    land(&mut h).await;
    h
}

fn model_tab(h: &Harness) -> &QueryTab {
    h.app.model.tab(h.app.active_tab()).expect("the active tab")
}

fn on_row(h: &Harness) -> Option<String> {
    h.app.cursor_row().map(|r| match &r.kind {
        RowKind::Table { name, .. } => name.clone(),
        other => format!("{other:?}"),
    })
}

/// Moves the sidebar cursor down to the table `name`.
fn go_to_table(h: &mut Harness, name: &str) {
    for _ in 0..20 {
        if on_row(h).as_deref() == Some(name) {
            return;
        }
        h.press("j");
    }
    panic!("no row for {name}");
}

fn results_text(h: &mut Harness) -> String {
    results_rows(h).join("\n")
}

fn runs_so_far(h: &Harness) -> usize {
    h.fake.calls().iter().filter(|c| matches!(c, Call::Run(_))).count()
}

// ---- opening --------------------------------------------------------------------

#[tokio::test]
async fn o_on_film_opens_a_tab_named_film_and_emits_one_run_of_the_adapters_select() {
    let mut h = film_loaded().await;
    let commands = h.press_fx("o");
    let sent = runs(&commands);
    assert_eq!(sent.len(), 1, "{commands:?}");
    assert_eq!(sent[0].2.buffer, film_sql(&h, 0));
    assert_eq!(sent[0].2.row_cap, 1000, "the page is the tab's row cap");
    assert_eq!(sent[0].2.database.as_deref(), Some("pagila"));
    assert_eq!(h.app.model.tabs.len(), 2, "a new tab");
    assert_eq!(h.app.active_tab(), TabId(2));
    assert!(title(&mut h).contains("2 film"), "{}", title(&mut h));
    assert_eq!(h.app.ui.tab.editor.text(), film_sql(&h, 0), "the editor holds the select");
    assert_eq!(model_tab(&h).target, Some(DatabasePath { connection: cid(), database: "pagila".into() }));
    assert!(model_tab(&h).run.in_flight().is_some(), "it runs at once");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle, "tab 1 is left alone");
}

#[tokio::test]
async fn f4_opens_the_table_too() {
    let mut h = film_loaded().await;
    assert_eq!(runs(&h.press_fx("f4")).len(), 1);
    assert_eq!(h.app.model.tabs.len(), 2);
}

#[tokio::test]
async fn o_on_a_table_whose_columns_are_not_loaded_asks_for_them_and_then_opens_it() {
    let mut h = tree().await;
    h.fake.script_columns(Scripted::ok(vec![info("film_id", true), info("title", false)]));
    h.fake.script_run(Scripted::ok(page(1, 3)));
    let commands = h.press_fx("o");
    assert!(runs(&commands).is_empty(), "nothing runs before the key is known: {commands:?}");
    assert_eq!(h.app.model.tabs.len(), 1);
    land(&mut h).await;
    assert!(h.fake.calls().contains(&Call::Columns(film())));
    assert_eq!(h.app.model.tabs.len(), 2);
    assert_eq!(runs_so_far(&h), 1);
    assert_eq!(h.app.ui.tab.editor.text(), film_sql(&h, 0), "ordered by the key the columns named");
}

#[tokio::test]
async fn o_on_a_connection_a_schema_or_a_column_does_nothing_and_shows_no_hint() {
    let mut h = film_loaded().await;
    assert!(mode_bar(&mut h).contains("browse"), "the hint is on a table: {}", mode_bar(&mut h));
    // A column of film.
    h.press("j");
    assert!(h.app.cursor_row().is_some_and(|r| matches!(r.kind, RowKind::Column(_))));
    assert!(h.press_fx("o").is_empty());
    assert!(!mode_bar(&mut h).contains("browse"), "{}", mode_bar(&mut h));
    // The schema, the database and the connection, going up.
    h.press("g");
    for name in ["connection", "database", "schema"] {
        if name != "connection" {
            h.press("j");
        }
        let commands = h.press_fx("o");
        assert!(commands.is_empty(), "{name}: {commands:?}");
        assert!(!mode_bar(&mut h).contains("browse"), "{name}: {}", mode_bar(&mut h));
    }
    assert_eq!(h.app.model.tabs.len(), 1);
}

#[tokio::test]
async fn o_on_a_table_that_already_has_a_tab_activates_it() {
    let mut h = browsing(1000).await;
    let browse = h.app.active_tab();
    let one = h.app.model.tabs[0].id;
    let fx = h.app.activate_tab(one);
    h.run(fx);
    h.press("ctrl+g h");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    let commands = h.press_fx("o");
    assert!(runs(&commands).is_empty(), "{commands:?}");
    assert_eq!(h.app.model.tabs.len(), 2, "no third tab");
    assert_eq!(h.app.active_tab(), browse);
}

#[tokio::test]
async fn o_where_the_connection_cannot_be_queried_says_so_and_opens_nothing() {
    let mut c = conn("local pg", DbKind::Postgres);
    c.capabilities = Capabilities { query: false, ..Capabilities::postgres() };
    let mut h = tree_of(c).await;
    h.fake.script_columns(Scripted::ok(vec![info("film_id", true)]));
    h.press("enter");
    land(&mut h).await;
    assert!(!mode_bar(&mut h).contains("browse"), "{}", mode_bar(&mut h));
    assert!(h.press_fx("o").is_empty());
    assert_eq!(h.app.model.tabs.len(), 1);
    let said = h.app.ui.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(said.contains("cannot be queried"), "{said}");
}

// ---- paging ---------------------------------------------------------------------

#[tokio::test]
async fn the_header_names_the_table_and_the_rows_on_screen() {
    let mut h = browsing(1000).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1–1,000"), "{text}");
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    h.press("]");
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1,001–2,000"), "{text}");
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    h.press("[");
    land(&mut h).await;
    assert!(results_text(&mut h).contains("film · rows 1–1,000"));
}

#[tokio::test]
async fn a_page_answered_needs_confirmation_asks_and_y_fetches_that_page() {
    let mut h = browsing(1000).await;
    // The fake's own answer to this run never comes: the test gives it.
    h.fake.script_run(Scripted::ok(page(1, 1)).after(Duration::from_secs(3600)));
    let commands = h.press_fx("]");
    super::confirm_tests::ask(&mut h, &commands, true, &[crate::core::Reason::Unproven]);
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::ConfirmRun(_))), "{:?}", h.app.ui.overlay);
    assert_eq!(model_tab(&h).run, RunState::Idle);
    assert_eq!(model_tab(&h).browse.as_ref().and_then(|b| b.fetch.clone()), None, "no page is on its way");
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.offset), Some(0), "still the first page");
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    let sent = runs(&h.press_fx("y"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2.buffer, film_sql(&h, 1000));
    assert!(sent[0].2.confirmation.is_some());
    land(&mut h).await;
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.offset), Some(1000), "the page landed as a page");
}

#[tokio::test]
async fn right_bracket_emits_the_run_for_offset_1000_and_the_editor_follows() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    let commands = h.press_fx("]");
    let sent = runs(&commands);
    assert_eq!(sent.len(), 1, "{commands:?}");
    assert_eq!(sent[0].2.buffer, film_sql(&h, 1000));
    land(&mut h).await;
    assert_eq!(h.app.ui.tab.editor.text(), film_sql(&h, 1000), "the editor holds what the page ran");
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.offset), Some(1000));
}

#[tokio::test]
async fn left_bracket_on_the_first_page_emits_nothing_and_on_the_second_goes_back() {
    let mut h = browsing(1000).await;
    assert!(h.press_fx("[").is_empty());
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    h.press("]");
    land(&mut h).await;
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    let back = runs(&h.press_fx("["));
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].2.buffer, film_sql(&h, 0));
}

#[tokio::test]
async fn a_page_of_fewer_than_1000_rows_makes_right_bracket_do_nothing() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(page(1001, 14)));
    h.press("]");
    land(&mut h).await;
    assert!(results_text(&mut h).contains("film · rows 1,001–1,014"), "{}", results_text(&mut h));
    assert!(h.press_fx("]").is_empty());
    assert!(!mode_bar(&mut h).contains("next page"), "no hint where the key does nothing: {}", mode_bar(&mut h));
    assert!(mode_bar(&mut h).contains("prev page"), "{}", mode_bar(&mut h));
}

#[tokio::test(start_paused = true)]
async fn right_bracket_while_the_tab_runs_is_refused_with_a_notice_and_emits_no_command() {
    let mut h = film_loaded().await;
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    h.fake.script_run(Scripted::ok(page(1001, 1000)).after(Duration::from_secs(5)));
    h.press("o");
    land(&mut h).await;
    h.press("]");
    assert!(model_tab(&h).run.in_flight().is_some(), "page 2 is running");
    let again = h.press_fx("]");
    assert!(again.is_empty(), "{again:?}");
    assert!(notice_text(&h).contains("still running"), "{}", notice_text(&h));
    let back = h.press_fx("[");
    assert!(back.is_empty(), "{back:?}");
    land(&mut h).await;
    assert_eq!(runs_so_far(&h), 2, "two runs in all");
}

#[tokio::test(start_paused = true)]
async fn the_earlier_page_stays_on_screen_under_earlier_run_while_the_next_runs() {
    let mut h = film_loaded().await;
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    h.fake.script_run(Scripted::ok(page(1001, 1000)).after(Duration::from_secs(5)));
    h.press("o");
    land(&mut h).await;
    h.press("]");
    let text = results_text(&mut h);
    assert!(text.contains("earlier run:") && text.contains("film · rows 1–1,000"), "{text}");
    assert!(text.contains("film 1"), "the rows stay: {text}");
}

#[tokio::test]
async fn a_page_that_fails_leaves_the_earlier_page_and_says_what_failed() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::err(CoreErrorKind::Other, "boom"));
    h.press("]");
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1–1,000"), "{text}");
    assert!(notice_text(&h).contains("boom"), "{}", notice_text(&h));
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.offset), Some(0));
    // The failure stays on the pane when the notice is gone, and the page
    // under it is the earlier run's.
    h.app.ui.notice = None;
    let text = results_text(&mut h);
    assert!(text.contains("earlier run: film · rows 1–1,000"), "{text}");
    assert!(text.contains("failed") && text.contains("boom"), "{text}");
    assert!(text.contains("film 1"), "the rows stay: {text}");
    // And the page can be asked for again; landing clears the failure.
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    assert_eq!(runs(&h.press_fx("]")).len(), 1);
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1,001–2,000") && !text.contains("earlier run") && !text.contains("boom"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn paging_keys_while_the_first_page_is_loading_are_refused_with_the_running_notice() {
    let mut h = film_loaded().await;
    h.fake.script_run(Scripted::ok(page(1, 1000)).after(Duration::from_secs(5)));
    h.press("o");
    assert!(model_tab(&h).run.in_flight().is_some(), "the first page is loading");
    for key in ["]", "["] {
        h.app.ui.notice = None;
        let fx = h.press_fx(key);
        assert!(fx.is_empty(), "{key}: {fx:?}");
        assert!(notice_text(&h).contains("still running"), "{key}: {}", notice_text(&h));
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    land(&mut h).await;
    assert_eq!(runs_so_far(&h), 1, "only the first page ran");
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_count_leaves_the_page_header_as_it_was_and_shows_the_count_as_cancelled() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(counted("5")).after(Duration::from_secs(30)));
    h.press("#");
    h.press("ctrl+c");
    let cancelling = results_text(&mut h);
    assert!(cancelling.contains("film · rows 1–1,000") && !cancelling.contains("earlier run"), "{cancelling}");
    tokio::time::advance(Duration::from_secs(31)).await;
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1–1,000") && !text.contains("earlier run"), "{text}");
    assert!(text.contains("cancelled"), "the count is shown as cancelled: {text}");
    assert!(text.contains("film 1"), "{text}");
    // A cancelled page is still an earlier run's.
    h.fake.script_run(Scripted::ok(page(1001, 1000)).after(Duration::from_secs(30)));
    h.press("]");
    h.press("ctrl+c");
    tokio::time::advance(Duration::from_secs(31)).await;
    land(&mut h).await;
    assert!(results_text(&mut h).contains("earlier run: film · rows 1–1,000"), "{}", results_text(&mut h));
}

#[tokio::test]
async fn load_more_is_not_offered_in_a_browse_tab() {
    let mut h = film_loaded().await;
    h.fake.script_run(Scripted::ok(ResultSet { truncated: true, ..page(1, 1000) }));
    h.press("o");
    land(&mut h).await;
    assert!(model_tab(&h).browse.is_some(), "a browse tab, whose page the row cap cut");
    assert!(!mode_bar(&mut h).contains("more"), "{}", mode_bar(&mut h));
    assert!(h.press_fx("m").is_empty());
    assert!(notice_text(&h).is_empty(), "not even a refusal: {}", notice_text(&h));
}

#[tokio::test]
async fn paging_after_the_text_was_edited_is_refused_and_keeps_the_typing() {
    let mut h = browsing(1000).await;
    h.press("ctrl+g k");
    h.type_text("-- mine ");
    h.press("ctrl+g j");
    let text = h.app.ui.tab.editor.text();
    assert!(h.press_fx("]").is_empty());
    assert!(notice_text(&h).contains("edited"), "{}", notice_text(&h));
    assert_eq!(h.app.ui.tab.editor.text(), text, "nothing typed is lost");
}

// ---- the order ------------------------------------------------------------------

#[tokio::test]
async fn a_table_with_no_primary_key_shows_the_note_and_one_with_a_key_does_not() {
    let mut h = browsing(1000).await;
    let keyed = results_text(&mut h);
    assert!(!keyed.contains("no primary key"), "{keyed}");
    // `payment` has no key.
    let one = h.app.model.tabs[0].id;
    let fx = h.app.activate_tab(one);
    h.run(fx);
    h.press("ctrl+g h");
    go_to_table(&mut h, "payment");
    h.fake.script_columns(Scripted::ok(vec![info("amount", false), info("note", false)]));
    h.fake.script_run(Scripted::ok(page(1, 5)));
    h.press("o");
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("payment · rows 1–5"), "{text}");
    assert!(text.contains("no primary key: rows may move between pages"), "{text}");
    let sql = h.app.ui.tab.editor.text();
    assert!(!sql.contains("order by"), "nothing to order by: {sql}");
}

// ---- the count ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn hash_asks_once_and_the_header_says_of_question_mark_until_the_answer_comes() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(counted("1000000")).after(Duration::from_secs(3)));
    let asked = runs(&h.press_fx("#"));
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2.buffer, h.fake.table_count_sql(&DbKind::Postgres, &film()));
    assert_eq!(asked[0].2.session, model_tab(&h).session(), "in the tab's session");
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1–1,000 of ?"), "{text}");
    assert!(text.contains("film 1"), "the page stays: {text}");
    // Not again, while it runs.
    assert!(h.press_fx("#").is_empty());
    h.settle().await;
    tokio::time::advance(Duration::from_secs(3)).await;
    land(&mut h).await;
    let text = results_text(&mut h);
    assert!(text.contains("film · rows 1–1,000 of 1,000,000"), "{text}");
    assert!(h.press_fx("#").is_empty(), "nor once it is known");
    assert_eq!(runs_so_far(&h), 2, "one page, one count");
    assert!(!mode_bar(&mut h).contains("count"), "no hint where the key does nothing: {}", mode_bar(&mut h));
}

#[tokio::test(start_paused = true)]
async fn paging_waits_for_the_count_and_ctrl_c_cancels_it_back_to_unasked() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(counted("5")).after(Duration::from_secs(30)));
    h.press("#");
    assert!(h.press_fx("]").is_empty(), "paging waits");
    assert!(notice_text(&h).contains("still running"), "{}", notice_text(&h));
    h.press("ctrl+c");
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.count), Some(Count::NotAsked), "no guess");
    assert!(!results_text(&mut h).contains(" of "), "{}", results_text(&mut h));
    // The count's late reply changes nothing.
    tokio::time::advance(Duration::from_secs(31)).await;
    land(&mut h).await;
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.count), Some(Count::NotAsked));
}

#[tokio::test(start_paused = true)]
async fn a_count_reply_after_the_tab_moved_on_changes_nothing() {
    let mut h = browsing(1000).await;
    h.fake.script_run(Scripted::ok(counted("42")).after(Duration::from_secs(3)));
    h.press("#");
    assert_eq!(model_tab(&h).browse.as_ref().map(|b| b.count), Some(Count::Asking));
    // The tab is given another database: it no longer pages `film`.
    h.press("ctrl+g h");
    h.press("G");
    assert!(matches!(h.app.cursor_row().map(|r| r.kind.clone()), Some(RowKind::Database(d)) if d == "edge"), "{:?}", h.app.cursor_row());
    h.press("u");
    assert!(model_tab(&h).browse.is_none());
    tokio::time::advance(Duration::from_secs(3)).await;
    land(&mut h).await;
    assert!(model_tab(&h).browse.is_none());
    assert!(model_tab(&h).outcome.is_none(), "no result of the old table");
    assert!(!results_text(&mut h).contains("42"), "{}", results_text(&mut h));
}

// ---- leaving browse mode ----------------------------------------------------------

#[tokio::test]
async fn after_editing_and_running_the_tab_is_an_ordinary_query_tab_without_the_paging_keys() {
    let mut h = browsing(1000).await;
    assert!(mode_bar(&mut h).contains("next page"), "{}", mode_bar(&mut h));
    h.press("ctrl+g k");
    h.type_text("-- mine\n");
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    h.press("f5");
    land(&mut h).await;
    assert!(model_tab(&h).browse.is_none());
    h.press("ctrl+g j");
    assert_eq!(h.app.ui.focus, PaneId::Results);
    let bar = mode_bar(&mut h);
    assert!(!bar.contains("next page") && !bar.contains("prev page") && !bar.contains("count"), "{bar}");
    assert!(h.press_fx("]").is_empty());
    assert!(!results_text(&mut h).contains("film · rows"), "the header is the query's again: {}", results_text(&mut h));
    assert!(title(&mut h).contains("2 local pg / pagila"), "{}", title(&mut h));
}

#[tokio::test]
async fn running_the_unedited_page_again_keeps_the_tab_browsing() {
    let mut h = browsing(1000).await;
    h.press("ctrl+g k");
    h.fake.script_run(Scripted::ok(page(1, 1000)));
    h.press("f5");
    land(&mut h).await;
    assert!(model_tab(&h).browse.is_some());
}

#[tokio::test]
async fn the_paging_hints_are_clickable_and_do_what_their_key_does() {
    let mut h = browsing(1000).await;
    let _ = frame_rows(&mut h.app, 114, 30);
    let hint = h.app.ui.mode_hints.iter().find(|x| x.action == Action::NextPage).copied().expect("the hint is drawn");
    h.fake.script_run(Scripted::ok(page(1001, 1000)));
    let fx = click(&mut h.app, hint.rect);
    assert_eq!(runs(&fx.commands).len(), 1, "{:?}", fx.commands);
}
