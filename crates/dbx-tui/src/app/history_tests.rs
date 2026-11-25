//! Layer 2: the acceptance tests of slice 18 (history), run on `App` with a
//! `FakeCore` behind the shell's command runner. What the store does with an
//! entry is tested in `tests/store.rs`; here the fake records what was asked.

use std::time::Duration;

use super::confirm_tests::needs;
use super::grid_tests::{focused, truncated_set};
use super::history::{PAGE, PAUSE};
use super::overlay::Overlay;
use super::query::RunState;
use super::run_tests::*;
use super::testing::*;
use super::theme::Role;
use super::*;
use crate::core::{CoreError, CoreErrorKind, HistoryCursor, HistoryEntry, HistoryPage, NewHistoryEntry, Reason, ResultSet, Scripted};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

fn entry(sql: &str, when: &str, connection: &str, database: &str, success: bool) -> HistoryEntry {
    HistoryEntry {
        id: format!("{when}-{sql}"),
        connection: cid(connection),
        connection_name: connection.into(),
        database: database.into(),
        sql: sql.into(),
        when: when.into(),
        elapsed: Duration::from_millis(7),
        success,
        error: (!success).then(|| "relation \"nope\" does not exist".to_string()),
        affected: None,
    }
}

/// An entry that ran here (`local pg / pagila`) and worked.
fn here(sql: &str, when: &str) -> HistoryEntry {
    entry(sql, when, "local pg", "pagila", true)
}

fn page_of(entries: Vec<HistoryEntry>) -> HistoryPage {
    let total = entries.len();
    HistoryPage { entries, next: None, total }
}

fn cursor(id: &str) -> HistoryCursor {
    HistoryCursor { executed_at: "2026-11-25T10:00:00.000Z".into(), id: id.into() }
}

/// `pagila` in the editor's tab, the history opened with these entries.
async fn history_open(entries: Vec<HistoryEntry>) -> Harness {
    let mut h = on_pagila().await;
    h.fake.script_history(Scripted::ok(page_of(entries)));
    h.press("ctrl+alt+e");
    h.settle().await;
    h
}

fn open(h: &Harness) -> bool {
    matches!(h.app.ui.overlay, Some(Overlay::History(_)))
}

fn screen(h: &mut Harness) -> Vec<String> {
    frame_text(&mut h.app, 114, 30)
}

fn screen_text(h: &mut Harness) -> String {
    screen(h).join("\n")
}

fn row_of(h: &mut Harness, text: &str) -> usize {
    screen(h).iter().position(|r| r.contains(text)).unwrap_or_else(|| panic!("{text:?} not on screen:\n{}", screen_text(h)))
}

fn ok_set(ran: &str) -> ResultSet {
    ResultSet { ran: ran.into(), ..rows_of(&[&["x"]]) }
}

/// A SQL error for `statement`.
fn sql_error(message: &str, statement: &str) -> CoreError {
    let mut e = CoreError::new(CoreErrorKind::Sql, message);
    e.statement = Some(statement.into());
    e
}

/// Lets the Core's replies arrive until no run is in flight (a reply of the
/// fake with no delay still waits for the timer's next tick), and the save
/// that follows has been taken.
async fn land(h: &mut Harness) {
    for _ in 0..200 {
        h.settle().await;
        if h.app.model.tabs.iter().all(|t| t.run.in_flight().is_none()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    h.settle().await;
}

/// Types `sql` in the editor of `on_pagila`, runs it, and lets the answer land.
async fn run_and_land(h: &mut Harness, sql: &str, reply: Scripted<ResultSet>) {
    h.fake.script_run(reply);
    h.type_text(sql);
    h.press("f5");
    land(h).await;
}

// ---- saving ---------------------------------------------------------------------

#[tokio::test]
async fn a_run_that_finishes_emits_one_save_with_the_expected_fields() {
    let mut h = on_pagila().await;
    run_and_land(&mut h, "select 1", Scripted::ok(ok_set("select 1"))).await;
    assert_eq!(
        h.fake.saved_history(),
        vec![NewHistoryEntry {
            connection: cid("local pg"),
            connection_name: "local pg".into(),
            database: "pagila".into(),
            sql: "select 1".into(),
            elapsed: Duration::from_millis(12),
            success: true,
            error: None,
            affected: None,
        }]
    );
}

#[tokio::test]
async fn the_reply_of_a_run_produces_exactly_one_save_command() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    let sent = runs(&h.press_fx("f5"));
    let fx = h.app.update(Event::Core(sent[0].0, CoreReply::Ran(Ok(ok_set("select 1")))), h.now());
    let saves: Vec<_> = fx.commands.iter().filter(|c| matches!(c, Command::Core(_, CoreCall::SaveHistory(_)))).collect();
    assert_eq!(saves.len(), 1, "{:?}", fx.commands);
    assert_eq!(fx.commands.len(), 1, "and nothing else: {:?}", fx.commands);
}

#[tokio::test]
async fn a_statement_that_changed_rows_saves_how_many_and_a_select_saves_none() {
    let mut h = on_pagila().await;
    let write = ResultSet { columns: vec![], rows: vec![], affected: 3, ran: "update t set a = 1".into(), rerun: crate::core::Rerun::Write, ..rows_of(&[]) };
    run_and_land(&mut h, "update t set a = 1", Scripted::ok(write)).await;
    let saved = h.fake.saved_history();
    assert_eq!(saved.len(), 1);
    assert_eq!((saved[0].success, saved[0].affected), (true, Some(3)));
}

#[tokio::test]
async fn a_failed_run_saves_success_false_with_the_error_and_the_statement_that_failed() {
    let mut h = on_pagila().await;
    run_and_land(&mut h, "select * from nope", Scripted::fail(sql_error("relation \"nope\" does not exist", "select * from nope"))).await;
    let saved = h.fake.saved_history();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0].sql, "select * from nope");
    assert!(!saved[0].success);
    assert_eq!(saved[0].error.as_deref(), Some("relation \"nope\" does not exist"));
    assert_eq!((saved[0].connection_name.as_str(), saved[0].database.as_str()), ("local pg", "pagila"));
    assert_eq!(saved[0].affected, None);
}

#[tokio::test]
async fn the_error_text_is_cleaned_before_it_is_saved() {
    let mut h = on_pagila().await;
    run_and_land(&mut h, "select 1", Scripted::fail(sql_error("bad\u{1b}[31m thing\u{7}\n", "select 1"))).await;
    let error = h.fake.saved_history()[0].error.clone().unwrap();
    assert!(!error.chars().any(char::is_control), "{error:?}");
}

#[tokio::test]
async fn a_timed_out_run_is_saved_as_a_failure_naming_the_limit() {
    let mut h = on_pagila().await;
    let mut e = CoreError::new(CoreErrorKind::Timeout, "The statement ran longer than this connection's query timeout of 1 s and was stopped.");
    e.statement = Some("select pg_sleep(30)".into());
    run_and_land(&mut h, "select pg_sleep(30)", Scripted::fail(e)).await;
    let saved = h.fake.saved_history();
    assert_eq!(saved.len(), 1);
    assert!(!saved[0].success && saved[0].error.as_deref().is_some_and(|m| m.contains("query timeout of 1 s")), "{saved:?}");
}

#[tokio::test]
async fn a_run_that_never_reached_the_database_saves_nothing() {
    let mut h = on_pagila().await;
    // No statement in the error: the Core refused before sending anything.
    run_and_land(&mut h, "select 1", Scripted::err(CoreErrorKind::Other, "That connection is no longer saved.")).await;
    let mut unreachable = CoreError::new(CoreErrorKind::Connect, "could not reach the server");
    unreachable.statement = Some("select 2".into());
    run_and_land(&mut h, "select 2", Scripted::fail(unreachable)).await;
    assert!(h.fake.saved_history().is_empty(), "{:?}", h.fake.saved_history());
}

#[tokio::test]
async fn a_run_refused_at_the_confirmation_saves_nothing_and_the_run_that_follows_the_yes_saves_once() {
    let mut h = on_pagila().await;
    h.type_text("delete from t");
    let commands = h.press_fx("f5");
    let request = runs(&commands)[0].2.clone();
    h.fake.script_run(Scripted::fail(needs(&request, true, &[Reason::Writes])));
    for _ in 0..50 {
        h.settle().await;
        if matches!(h.app.ui.overlay, Some(Overlay::ConfirmRun(_))) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(matches!(h.app.ui.overlay, Some(Overlay::ConfirmRun(_))));
    assert!(h.fake.saved_history().is_empty(), "refused: nothing ran");
    h.advance(Duration::from_millis(300));
    h.fake.script_run(Scripted::ok(ResultSet { columns: vec![], rows: vec![], affected: 1, ran: "delete from t".into(), ..rows_of(&[]) }));
    h.press("y");
    land(&mut h).await;
    let saved = h.fake.saved_history();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0].sql, "delete from t");
}

#[tokio::test]
async fn a_load_more_saves_nothing() {
    let mut h = focused(truncated_set(1_000)).await;
    land(&mut h).await;
    assert_eq!(h.fake.saved_history().len(), 1, "the run that made the result");
    h.fake.script_run(Scripted::ok(truncated_set(2_000)));
    h.press("m");
    land(&mut h).await;
    assert!(h.app.model.tabs[0].loading_more.is_none(), "the rows landed");
    assert_eq!(h.fake.saved_history().len(), 1, "load more repeats a statement already saved");
}

#[tokio::test]
async fn a_chosen_subquery_run_saves_what_ran() {
    let mut h = on_pagila().await;
    run_and_land(&mut h, "select 1; select 2", Scripted::ok(ok_set("select 1"))).await;
    assert_eq!(h.fake.saved_history()[0].sql, "select 1", "the statement that ran, from the result");
}

#[tokio::test(start_paused = true)]
async fn a_run_cancelled_and_stopped_saves_nothing_and_one_that_finished_first_saves_what_ran() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(ok_set("select slow")).after(Duration::from_secs(3)).deaf());
    h.type_text("select slow");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+c");
    tokio::time::advance(Duration::from_secs(3)).await;
    h.settle().await;
    let saved = h.fake.saved_history();
    assert_eq!(saved.len(), 1, "it finished before the cancel landed, and what it changed stands: {saved:?}");
    assert_eq!(saved[0].sql, "select slow");

    // A cancel that stopped it: nothing finished, nothing to claim.
    h.fake.script_run(Scripted::ok(ok_set("select slower")).after(Duration::from_secs(30)));
    h.press("ctrl+a");
    h.type_text("select slower");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+c");
    h.settle().await;
    assert_eq!(h.fake.saved_history().len(), 1, "{:?}", h.fake.saved_history());
}

#[tokio::test]
async fn a_failed_save_shows_one_notice_and_a_good_one_shows_none() {
    let mut h = on_pagila().await;
    // (What completion asks for after a run is answered, so it has nothing to say.)
    h.fake.script_catalogue(super::complete_tests::pagila());
    run_and_land(&mut h, "select 1", Scripted::ok(ok_set("select 1"))).await;
    assert_eq!(notice_text(&h), "", "{}", notice_text(&h));
    h.fake.script_save_history(Scripted::err(CoreErrorKind::Store, "database or disk is full"));
    h.type_text(" ");
    h.fake.script_run(Scripted::ok(ok_set("select 1")));
    h.press("f5");
    land(&mut h).await;
    let notice = notice_text(&h);
    assert!(notice.contains("history") && notice.contains("database or disk is full"), "{notice}");
    assert!(h.app.ui.notice.as_ref().is_some_and(|n| n.failure));
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle, "and nothing is blocked");
}

#[tokio::test]
async fn saves_leave_nothing_behind_in_the_request_table() {
    let mut h = on_pagila().await;
    // (What else the table tracks, the tab's run and completion's questions, settles.)
    for i in 0..3 {
        run_and_land(&mut h, &format!("select {i}"), Scripted::ok(ok_set("select"))).await;
    }
    let steady = h.app.model.requests.tracked();
    for i in 3..8 {
        run_and_land(&mut h, &format!("select {i}"), Scripted::ok(ok_set("select"))).await;
    }
    assert_eq!(h.fake.saved_history().len(), 8);
    assert_eq!(h.app.model.requests.tracked(), steady, "a save forgets itself when it is answered");
}

// ---- opening ----------------------------------------------------------------------

#[tokio::test]
async fn ctrl_alt_e_cmd_alt_e_and_the_prefix_e_open_the_overlay_and_send_one_search() {
    for keys in ["ctrl+alt+e", "cmd+alt+e", "ctrl+g e"] {
        let mut h = on_pagila().await;
        h.fake.script_history(Scripted::ok(page_of(vec![])));
        h.press(keys);
        h.settle().await;
        assert!(open(&h), "{keys}");
        let queries = h.fake.history_queries();
        assert_eq!(queries.len(), 1, "{keys}: {queries:?}");
        assert_eq!(queries[0].search_text, "");
        assert_eq!(queries[0].connection, Some(cid("local pg")), "this tab's connection is the default");
        assert_eq!(queries[0].cursor, None);
        assert_eq!(queries[0].limit, PAGE);
    }
}

#[tokio::test]
async fn it_opens_from_any_pane_and_the_same_key_closes_it() {
    let mut h = on_pagila().await;
    h.fake.script_history(Scripted::ok(page_of(vec![])));
    h.press("ctrl+g h");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    h.press("ctrl+alt+e");
    assert!(open(&h));
    h.press("ctrl+alt+e");
    assert!(!open(&h));
}

#[tokio::test]
async fn a_tab_with_no_database_looks_in_every_connection() {
    let mut h = opened().await;
    h.fake.script_history(Scripted::ok(page_of(vec![])));
    h.press("ctrl+alt+e");
    h.settle().await;
    assert_eq!(h.fake.history_queries()[0].connection, None);
    let text = screen_text(&mut h);
    assert!(text.contains("all connections"), "{text}");
}

#[tokio::test]
async fn the_mode_bar_offers_the_history_key_and_the_prefix_bar_lists_it() {
    let mut h = on_pagila().await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("ctrl+alt+e history"), "{bar}");
    h.press("ctrl+g");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("e history"), "{bar}");
}

// ---- the list ---------------------------------------------------------------------

#[tokio::test]
async fn the_overlay_lists_the_entries_newest_first_with_where_when_and_whether_it_worked() {
    let mut h = history_open(vec![
        here("select 'newest'", "2026-11-25 14:03"),
        entry("select * from nope", "2026-11-25 13:40", "local pg", "pagila", false),
        here("select 'oldest'", "2026-11-24 09:12"),
    ])
    .await;
    let (a, b, c) = (row_of(&mut h, "select 'newest'"), row_of(&mut h, "select * from nope"), row_of(&mut h, "select 'oldest'"));
    assert!(a < b && b < c, "{a} {b} {c}");
    let rows = screen(&mut h);
    assert!(rows[a].contains("2026-11-25 14:03") && rows[a].contains("local pg / pagila") && rows[a].contains('✓'), "{}", rows[a]);
    assert!(rows[b].contains('✗'), "{}", rows[b]);
    let x = rows[a].chars().position(|c| c == '✓').unwrap() as u16;
    assert_eq!(role_at(&mut h.app, x, a as u16).fg, Some(Role::Green));
    let x = rows[b].chars().position(|c| c == '✗').unwrap() as u16;
    assert_eq!(role_at(&mut h.app, x, b as u16).fg, Some(Role::Red));
}

#[tokio::test]
async fn a_statement_of_several_lines_shows_its_first_line_in_the_list_and_all_of_it_in_the_preview() {
    let mut h = history_open(vec![here("select a\n  from film\n where b = 1", "2026-11-25 14:03")]).await;
    let text = screen_text(&mut h);
    assert!(text.contains("select a"), "{text}");
    assert!(text.contains("from film") && text.contains("where b = 1"), "the preview shows it in full: {text}");
    let list_row = row_of(&mut h, "2026-11-25 14:03");
    assert!(!screen(&mut h)[list_row].contains("from film"), "the list row is its first line only");
}

#[tokio::test]
async fn up_and_down_move_the_highlight_and_the_preview_follows() {
    let mut h = history_open(vec![here("select 'first'", "2026-11-25 14:03"), here("select 'second one'\nunion select 2", "2026-11-25 14:02")]).await;
    assert!(!screen_text(&mut h).contains("union select 2"));
    h.press("down");
    assert!(screen_text(&mut h).contains("union select 2"), "{}", screen_text(&mut h));
    h.press("up");
    assert!(!screen_text(&mut h).contains("union select 2"));
}

#[tokio::test]
async fn a_failed_entry_shows_its_error_in_red_and_an_entry_from_elsewhere_says_so() {
    let mut h = history_open(vec![
        entry("select * from nope", "2026-11-25 13:40", "local pg", "pagila", false),
        entry("select 2", "2026-11-25 13:30", "local pg", "edge", true),
    ])
    .await;
    let text = screen_text(&mut h);
    assert!(text.contains("relation \"nope\" does not exist"), "{text}");
    assert!(!text.contains("Ran on"), "it ran here: {text}");
    h.press("down");
    let text = screen_text(&mut h);
    assert!(text.contains("Ran on local pg / edge"), "{text}");
    assert!(text.contains("This tab runs on local pg / pagila"), "{text}");
}

#[tokio::test]
async fn an_entry_with_control_characters_draws_them_replaced() {
    let nasty = entry("select '\u{1b}[31mred\u{7}'\nfrom \u{9b}t", "2026-11-25 14:03\u{1b}", "pg\u{1b}[2J", "d\u{8}b", false);
    let mut h = history_open(vec![HistoryEntry { error: Some("bad\u{1b}[0m".into()), ..nasty }]).await;
    let text = frame_rows(&mut h.app, 114, 30).join("\n");
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'), "{text:?}");
    assert!(text.contains('\u{fffd}'), "replaced, not dropped: {text}");
    assert!(text.contains("red"), "{text}");
}

#[tokio::test]
async fn an_empty_history_says_what_to_do_and_a_search_with_no_match_says_so() {
    let mut h = history_open(vec![]).await;
    let text = screen_text(&mut h);
    assert!(text.contains("Nothing here yet"), "{text}");
    assert!(text.contains("f5"), "it names the key: {text}");
    h.fake.script_history(Scripted::ok(page_of(vec![])));
    h.type_text("zzz");
    h.advance(PAUSE);
    h.settle().await;
    let text = screen_text(&mut h);
    assert!(text.contains("Nothing matches") && text.contains("zzz"), "{text}");
    assert!(text.contains("tab") && text.contains("all connections"), "it offers the wider search: {text}");
}

#[tokio::test]
async fn a_search_that_fails_says_why_and_what_to_do_and_typing_goes_on() {
    let mut h = on_pagila().await;
    h.fake.script_history(Scripted::err(CoreErrorKind::Store, "database is locked"));
    h.press("ctrl+alt+e");
    h.settle().await;
    let text = screen_text(&mut h);
    assert!(text.contains("could not be read") && text.contains("database is locked"), "{text}");
    assert!(text.contains("esc"), "{text}");
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 1", "2026-11-25 14:03")])));
    h.type_text("s");
    h.advance(PAUSE);
    h.settle().await;
    assert!(screen_text(&mut h).contains("select 1"));
}

// ---- searching ----------------------------------------------------------------------

#[tokio::test]
async fn typing_film_sends_one_search_with_that_text_after_the_pause_and_not_before() {
    let mut h = history_open(vec![here("select 1", "2026-11-25 14:03")]).await;
    h.fake.script_history(Scripted::ok(page_of(vec![here("select * from film", "2026-11-25 14:00")])));
    h.type_text("film");
    h.settle().await;
    assert_eq!(h.fake.history_queries().len(), 1, "nothing sent while typing");
    let due = h.app.next_deadline().expect("the search is due");
    assert!(due <= h.now() + PAUSE, "within the pause");
    h.advance(PAUSE - Duration::from_millis(1));
    h.settle().await;
    assert_eq!(h.fake.history_queries().len(), 1, "not a moment early");
    h.advance(Duration::from_millis(1));
    h.settle().await;
    let queries = h.fake.history_queries();
    assert_eq!(queries.len(), 2, "{queries:?}");
    assert_eq!(queries[1].search_text, "film");
    assert_eq!(queries[1].connection, Some(cid("local pg")));
    assert!(screen_text(&mut h).contains("select * from film"));
    assert!(h.app.next_deadline().is_none_or(|d| d > h.now() + Duration::from_secs(1)), "nothing left due");
}

#[tokio::test]
async fn more_typing_moves_the_pause_so_only_the_last_text_is_searched() {
    let mut h = history_open(vec![]).await;
    h.fake.script_history(Scripted::ok(page_of(vec![])));
    h.type_text("fi");
    h.advance(Duration::from_millis(150));
    h.type_text("lm");
    h.advance(Duration::from_millis(150));
    h.settle().await;
    assert_eq!(h.fake.history_queries().len(), 1, "the pause restarted");
    h.advance(Duration::from_millis(60));
    h.settle().await;
    let queries = h.fake.history_queries();
    assert_eq!(queries.len(), 2);
    assert_eq!(queries[1].search_text, "film");
}

#[tokio::test(start_paused = true)]
async fn a_stale_search_reply_is_dropped_and_repaints_nothing() {
    let mut h = on_pagila().await;
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 'stale'", "2026-11-25 14:03")])).after(Duration::from_secs(5)));
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 'fresh'", "2026-11-25 14:02")])));
    h.press("ctrl+alt+e");
    h.type_text("f");
    h.advance(PAUSE);
    h.settle().await;
    assert!(screen_text(&mut h).contains("select 'fresh'"));
    let repaints = h.repaints;
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    let text = screen_text(&mut h);
    assert!(text.contains("select 'fresh'") && !text.contains("select 'stale'"), "{text}");
    assert_eq!(h.repaints, repaints, "a stale reply repaints nothing");
}

#[tokio::test]
async fn a_reply_that_lands_after_the_overlay_closed_changes_nothing() {
    let mut h = on_pagila().await;
    let commands = h.press_fx("ctrl+alt+e");
    let Some(Command::Core(id, CoreCall::History(_))) = commands.iter().find(|c| matches!(c, Command::Core(_, CoreCall::History(_)))) else {
        panic!("a search: {commands:?}")
    };
    let id = *id;
    let commands = h.press_fx("esc");
    assert!(commands.contains(&Command::Abandon(id)), "the shell is told to let go of it: {commands:?}");
    assert!(!open(&h));
    // The reply lands anyway (it was already on its way): nothing changes.
    let page = page_of(vec![here("select 1", "2026-11-25 14:03")]);
    let fx = h.app.update(Event::Core(id, CoreReply::History(Ok(page))), h.now());
    assert!(!fx.repaint && fx.commands.is_empty(), "{fx:?}");
    assert!(h.app.ui.overlay.is_none());
}

#[tokio::test]
async fn tab_flips_between_this_connection_and_all_and_searches_at_once() {
    let mut h = history_open(vec![here("select 1", "2026-11-25 14:03")]).await;
    h.fake.script_history(Scripted::ok(page_of(vec![entry("select 'other'", "2026-11-25 14:00", "other", "x", true)])));
    h.press("tab");
    h.settle().await;
    let queries = h.fake.history_queries();
    assert_eq!(queries.len(), 2, "no pause for a switch");
    assert_eq!(queries[1].connection, None);
    assert!(screen_text(&mut h).contains("all connections"));
    h.fake.script_history(Scripted::ok(page_of(vec![])));
    h.press("tab");
    h.settle().await;
    assert_eq!(h.fake.history_queries()[2].connection, Some(cid("local pg")));
    assert!(screen_text(&mut h).contains("this connection"));
}

#[tokio::test]
async fn reaching_the_end_of_the_list_fetches_the_next_page_with_the_cursor_and_adds_it() {
    let mut h = on_pagila().await;
    let first = HistoryPage {
        entries: vec![here("select 'a'", "2026-11-25 14:03"), here("select 'b'", "2026-11-25 14:02"), here("select 'c'", "2026-11-25 14:01")],
        next: Some(cursor("c")),
        total: 5,
    };
    h.fake.script_history(Scripted::ok(first));
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 'd'", "2026-11-25 14:00"), here("select 'e'", "2026-11-25 13:59")])));
    h.press("ctrl+alt+e");
    h.settle().await;
    h.press("down");
    h.settle().await;
    assert_eq!(h.fake.history_queries().len(), 1, "not at the end yet");
    h.press("down");
    h.settle().await;
    let queries = h.fake.history_queries();
    assert_eq!(queries.len(), 2, "{queries:?}");
    assert_eq!(queries[1].cursor, Some(cursor("c")));
    assert_eq!(queries[1].search_text, "");
    let text = screen_text(&mut h);
    assert!(text.contains("select 'a'") && text.contains("select 'e'"), "{text}");
    h.press("down down");
    h.settle().await;
    assert_eq!(h.fake.history_queries().len(), 2, "the last page had no cursor: no more");
}

// ---- using an entry ----------------------------------------------------------------

#[tokio::test]
async fn enter_inserts_the_statement_at_the_caret_closes_the_overlay_and_one_undo_removes_it() {
    let mut h = history_open(vec![here("select 1 from film", "2026-11-25 14:03")]).await;
    h.press("enter");
    assert!(!open(&h));
    assert_eq!(h.app.ui.tab.editor.text(), "select 1 from film");
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    assert!(h.fake.calls().iter().all(|c| !matches!(c, crate::core::Call::Run(_))), "inserting runs nothing");
    h.press("ctrl+z");
    assert_eq!(h.app.ui.tab.editor.text(), "", "one undo removes it all");
}

#[tokio::test]
async fn an_inserted_statement_goes_on_its_own_lines_in_the_middle_of_a_line() {
    let mut h = on_pagila().await;
    h.type_text("foo bar");
    h.press("home right right right");
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 1\nfrom film", "2026-11-25 14:03")])));
    h.press("ctrl+alt+e");
    h.settle().await;
    h.press("enter");
    assert_eq!(h.app.ui.tab.editor.text(), "foo\nselect 1\nfrom film\n bar");
    h.press("ctrl+z");
    assert_eq!(h.app.ui.tab.editor.text(), "foo bar", "one step");
    assert_eq!(h.app.ui.tab.editor.cursor(), crate::core::Cursor { row: 0, col: 3 }, "the caret is where it was");
}

#[tokio::test]
async fn an_inserted_statement_after_text_on_the_line_starts_a_new_line_and_before_text_ends_its_own() {
    let mut h = on_pagila().await;
    h.type_text("select 0;");
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 1", "2026-11-25 14:03")])));
    h.press("ctrl+alt+e");
    h.settle().await;
    h.press("enter");
    assert_eq!(h.app.ui.tab.editor.text(), "select 0;\nselect 1");
}

#[tokio::test]
async fn control_characters_in_an_inserted_statement_are_replaced_as_a_paste_would() {
    let mut h = history_open(vec![here("select '\u{1b}[31m'", "2026-11-25 14:03")]).await;
    h.press("enter");
    assert!(!h.app.ui.tab.editor.text().chars().any(|c| c == '\u{1b}'), "{:?}", h.app.ui.tab.editor.text());
}

#[tokio::test]
async fn f5_on_an_entry_closes_the_overlay_and_emits_a_run_of_exactly_that_sql_in_the_active_tab() {
    for run_key in ["f5", "ctrl+enter"] {
        let sql = "select 'x'\nfrom film -- a comment";
        let mut h = history_open(vec![here(sql, "2026-11-25 14:03")]).await;
        let commands = h.press_fx(run_key);
        assert!(!open(&h), "{run_key}");
        let sent = runs(&commands);
        assert_eq!(sent.len(), 1, "{run_key}: {commands:?}");
        let request = &sent[0].2;
        assert_eq!(request.buffer, sql);
        let lines: Vec<&str> = sql.split('\n').collect();
        let end = crate::core::Cursor { row: lines.len() - 1, col: lines.last().unwrap().chars().count() };
        assert_eq!(request.selection, Some((crate::core::Cursor::default(), end)), "all of it, and only it");
        assert_eq!((request.connection.clone(), request.database.clone()), (cid("local pg"), Some("pagila".into())));
        assert_eq!(request.session, first_session());
        assert_eq!(h.app.ui.tab.editor.text(), "", "the editor is left alone");
        assert!(h.app.model.tabs[0].run.in_flight().is_some());
    }
}

#[tokio::test]
async fn f5_on_an_entry_that_ran_elsewhere_runs_it_on_this_tabs_target() {
    let mut h = history_open(vec![entry("select 2", "2026-11-25 14:03", "other", "edge", true)]).await;
    let sent = runs(&h.press_fx("f5"));
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].2.connection.clone(), sent[0].2.database.clone()), (cid("local pg"), Some("pagila".into())));
}

#[tokio::test]
async fn f5_on_an_entry_the_core_says_needs_confirmation_opens_the_confirmation() {
    let mut h = history_open(vec![here("delete from film", "2026-11-25 14:03")]).await;
    let commands = h.press_fx("f5");
    let request = runs(&commands)[0].2.clone();
    h.fake.script_run(Scripted::fail(needs(&request, true, &[Reason::Writes])));
    for _ in 0..50 {
        h.settle().await;
        if matches!(h.app.ui.overlay, Some(Overlay::ConfirmRun(_))) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let Some(Overlay::ConfirmRun(ask)) = &h.app.ui.overlay else { panic!("{:?}", h.app.ui.overlay) };
    assert_eq!(ask.assessment.confirmation.statement, "delete from film");
    assert!(h.fake.saved_history().is_empty(), "nothing ran yet");
}

#[tokio::test]
async fn a_run_from_the_history_is_itself_saved() {
    let mut h = history_open(vec![here("select 'again'", "2026-11-25 14:03")]).await;
    h.fake.script_run(Scripted::ok(ok_set("select 'again'")));
    h.press("f5");
    land(&mut h).await;
    assert_eq!(h.fake.saved_history().len(), 1);
    assert_eq!(h.fake.saved_history()[0].sql, "select 'again'");
}

#[tokio::test]
async fn f5_in_a_tab_that_cannot_run_closes_the_overlay_and_says_why() {
    let mut h = opened().await;
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 1", "2026-11-25 14:03")])));
    h.press("ctrl+alt+e");
    h.settle().await;
    let commands = h.press_fx("f5");
    assert!(runs(&commands).is_empty());
    assert!(!open(&h));
    assert!(notice_text(&h).contains("no database yet"), "{}", notice_text(&h));
}

#[tokio::test]
async fn esc_closes_the_overlay_and_the_editors_text_is_untouched() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 2", "2026-11-25 14:03")])));
    h.press("ctrl+alt+e");
    h.settle().await;
    h.type_text("typed in the search");
    h.press("esc");
    assert!(!open(&h));
    assert_eq!(h.app.ui.tab.editor.text(), "select 1");
}

#[tokio::test]
async fn the_buttons_carry_their_keys_and_a_click_does_what_the_key_does() {
    let mut h = history_open(vec![here("select 1", "2026-11-25 14:03"), here("select 2", "2026-11-25 14:02")]).await;
    let text = screen_text(&mut h);
    assert!(text.contains("↵ insert") && text.contains("f5 run") && text.contains("esc close"), "{text}");
    // A click on the second row highlights it; the button inserts it.
    let y = row_of(&mut h, "select 2") as u16;
    let rect = h.app.ui.overlay_buttons.iter().find(|b| b.rect.y == y).expect("a row to click").rect;
    click(&mut h.app, rect);
    let rows = frame_text(&mut h.app, 114, 30);
    assert!(rows.join("\n").contains("select 2"));
    let insert = h.app.ui.overlay_buttons.iter().find(|b| b.action == overlay::OverlayAction::HistoryInsert).expect("an insert button").rect;
    click(&mut h.app, insert);
    assert!(!open(&h));
    assert_eq!(h.app.ui.tab.editor.text(), "select 2");
}

#[tokio::test]
async fn the_overlay_takes_all_input_while_it_is_open() {
    let mut h = history_open(vec![here("select 1", "2026-11-25 14:03")]).await;
    h.press("ctrl+g");
    assert_eq!(h.app.ui.mode, Mode::Normal, "the prefix is not read");
    h.press("q");
    assert!(!h.quit && open(&h));
    assert_eq!(h.app.ui.tab.editor.text(), "");
}

// ---- review fixes ---------------------------------------------------------------------

#[tokio::test]
async fn enter_and_f5_do_nothing_while_a_search_is_pending() {
    for key in ["enter", "f5"] {
        let mut h = history_open(vec![here("select 'old'", "2026-11-25 14:03")]).await;
        h.type_text("f");
        let commands = h.press_fx(key);
        assert!(open(&h), "{key}: still open, waiting for the search");
        assert!(runs(&commands).is_empty(), "{key}: {commands:?}");
        assert_eq!(h.app.ui.tab.editor.text(), "", "{key}");
        // After a scope flip the search is in flight rather than due.
        let mut h = history_open(vec![here("select 'old'", "2026-11-25 14:03")]).await;
        h.fake.script_history(Scripted::ok(page_of(vec![])));
        h.press("tab");
        let commands = h.press_fx(key);
        assert!(open(&h) && runs(&commands).is_empty(), "{key} after tab: {commands:?}");
    }
}

#[tokio::test]
async fn a_search_reply_keeps_the_highlight_on_the_same_entry_when_it_is_still_there() {
    let mut h = history_open(vec![here("select 1", "2026-11-25 14:03"), here("select 2", "2026-11-25 14:02"), here("select 3", "2026-11-25 14:01")]).await;
    h.press("down");
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 0", "2026-11-25 14:04"), here("select 2", "2026-11-25 14:02")])));
    h.press("tab");
    h.settle().await;
    let Some(Overlay::History(o)) = &h.app.ui.overlay else { panic!("open") };
    assert_eq!(o.selected_entry().map(|e| e.sql.as_str()), Some("select 2"));
    // One that is gone: back to the first.
    h.fake.script_history(Scripted::ok(page_of(vec![here("select 9", "2026-11-25 14:05"), here("select 8", "2026-11-25 14:04")])));
    h.press("tab");
    h.settle().await;
    let Some(Overlay::History(o)) = &h.app.ui.overlay else { panic!("open") };
    assert_eq!(o.selected, 0);
}

#[tokio::test]
async fn what_runs_and_what_is_inserted_is_the_text_the_preview_showed() {
    let sql = "select '\u{202e}x\u{1b}[31m'\r\nfrom t";
    let clean = "select 'x\u{fffd}[31m'\nfrom t";
    let mut h = history_open(vec![here(sql, "2026-11-25 14:03")]).await;
    let sent = runs(&h.press_fx("f5"));
    assert_eq!(sent[0].2.buffer, clean);
    let mut h = history_open(vec![here(sql, "2026-11-25 14:03")]).await;
    h.press("enter");
    assert_eq!(h.app.ui.tab.editor.text(), clean);
}

#[tokio::test]
async fn the_saved_error_keeps_its_line_breaks_and_loses_control_characters() {
    let mut h = on_pagila().await;
    run_and_land(&mut h, "select 1", Scripted::fail(sql_error("first\nsecond\u{1b}[31m", "select 1"))).await;
    let error = h.fake.saved_history()[0].error.clone().unwrap();
    assert_eq!(error, "first\nsecond\u{fffd}[31m");
}
