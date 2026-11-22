//! Layer 3: the acceptance tests of slice 06 (cancel and stale replies), on
//! `App` with a `FakeCore` behind the shell's own command runner, which
//! controls the order replies arrive in and when.

use std::time::Duration;

use super::query::{Outcome, RunState};
use super::run_tests::*;
use super::testing::*;
use super::*;
use crate::core::{Call, CoreErrorKind, ResultSet, RunId, Scripted};
use super::requests::RequestId;

/// A slow result for `select ...` that a cancel does not stop.
fn slow_and_deaf(secs: u64) -> Scripted<ResultSet> {
    Scripted::ok(rows_of(&[&["old"]])).after(Duration::from_secs(secs)).deaf()
}

/// Moves time on by `ms`: replies due by then arrive, and the app's timer
/// fires as the shell's would.
async fn advance(h: &mut crate::harness::Harness, ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    h.settle().await;
    h.tick();
}

// ---- the retarget race (carried over from slice 05) ---------------------------

#[tokio::test(start_paused = true)]
async fn a_tab_given_a_then_b_then_a_again_keeps_its_new_session_on_a_when_the_old_close_lands_late() {
    let mut h = on_pagila().await;
    // The first run winds down a second after its cancel, so the close of its
    // session (which waits for it) lands after the tab is back on pagila.
    h.fake.script_run(slow_and_deaf(1));
    h.fake.script_run(Scripted::ok(rows_of(&[&["new"]])).after(Duration::from_secs(3)));
    for _ in 0..3 {
        h.fake.script_close_session(Scripted::ok(()));
    }
    h.type_text("select ...");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+g h k u"); // edge: cancels the run, closes pagila's session once it ends
    h.press("j u"); // pagila again
    h.settle().await;
    let second = runs(&h.press_fx("ctrl+g l f5"));
    assert_eq!(second.len(), 1, "the tab is idle on pagila again");
    let pagila = dbp("local pg", "pagila");
    h.settle().await;
    assert!(h.fake.session_open(second[0].2.session, &pagila), "the new run opened its session");
    advance(&mut h, 1_100).await; // the old run ends; the late close of pagila's old session runs
    let closes: Vec<Call> = h.fake.calls().into_iter().filter(|c| matches!(c, Call::CloseSession(..))).collect();
    assert_eq!(closes.len(), 2, "{closes:?}");
    assert!(
        h.fake.session_open(second[0].2.session, &pagila),
        "the late close of the old session did not close the new one: {closes:?}"
    );
}

// ---- cancelling -----------------------------------------------------------------

/// `on_pagila` with one run done (a row `earlier`), and a second, slow run
/// of `secs` in flight. Returns the second run's id.
async fn second_run_in_flight(secs: u64) -> (crate::harness::Harness, RunId) {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(ResultSet { ran: "select 'earlier'".into(), ..rows_of(&[&["earlier"]]) }));
    h.fake.script_run(Scripted::ok(rows_of(&[&["late"]])).after(Duration::from_secs(secs)));
    h.type_text("select ...");
    h.press("f5");
    h.settle().await;
    let run = runs(&h.press_fx("f5"))[0].1;
    h.settle().await;
    (h, run)
}

#[tokio::test(start_paused = true)]
async fn cancel_during_a_run_emits_cancel_and_shows_cancelling_then_the_cancelled_reply_shows_cancelled_and_keeps_the_earlier_result() {
    let (mut h, run) = second_run_in_flight(30).await;
    let commands = h.press_fx("ctrl+c");
    assert_eq!(commands, vec![Command::Cancel(run)]);
    assert!(mode_bar(&mut h).contains("cancelling"), "{}", mode_bar(&mut h));
    let pane = results_rows(&mut h).join("\n");
    assert!(pane.contains("earlier"), "the earlier result is still shown: {pane}");
    assert!(pane.contains("cancelling"), "and the pane says the run is being cancelled: {pane}");

    // FakeCore ends the run with `Cancelled` at once: that reply is stale for
    // the result, but says the run is over.
    h.settle().await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { .. }), "{:?}", h.app.model.tabs[0].run);
    let bar = mode_bar(&mut h);
    assert!(bar.contains("cancelled") && !bar.contains("cancelling"), "{bar}");
    let pane = results_rows(&mut h);
    assert!(pane.join("\n").contains("earlier"), "{pane:#?}");
    let what_ran = pane.iter().find(|r| r.contains("select 'earlier'")).expect("the line naming what ran");
    assert!(what_ran.contains("earlier run"), "it says the rows are from the earlier run: {pane:#?}");
}

// ---- timeouts -----------------------------------------------------------------------

#[tokio::test]
async fn a_timeout_reply_renders_as_timed_out_and_says_whose_limit_it_was_not_as_a_sql_error() {
    let mut e = crate::core::CoreError::new(
        CoreErrorKind::Timeout,
        "The statement ran longer than this connection's query timeout of 1 s and was stopped.",
    );
    e.statement = Some("select pg_sleep(30)".into());
    let mut h = ran(Err(e)).await;
    assert!(matches!(h.app.model.tabs[0].outcome, Some(Outcome::TimedOut { .. })), "{:?}", h.app.model.tabs[0].outcome);
    let bar = mode_bar(&mut h);
    assert!(bar.contains("timed out") && !bar.contains("failed"), "{bar}");
    let pane = results_rows(&mut h).join("\n");
    assert!(pane.contains("Timed out") && !pane.contains("SQL error"), "{pane}");
    assert!(pane.contains("query timeout of 1 s"), "whose limit: {pane}");
    let (x, y) = find_text(&mut h.app, 114, 30, "Timed out").unwrap();
    assert_ne!(role_at(&mut h.app, x, y).fg, Some(super::theme::Role::Red), "not drawn as an error");
}

// ---- stale run replies ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_successful_reply_that_arrives_after_the_cancel_is_dropped_and_the_tab_says_its_changes_stand() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(ResultSet { ran: "select 'earlier'".into(), ..rows_of(&[&["earlier"]]) }));
    h.fake.script_run(Scripted::ok(rows_of(&[&["finished-anyway"]])).after(Duration::from_secs(3)).deaf());
    h.type_text("select ...");
    h.press("f5");
    h.settle().await;
    h.press("f5");
    h.settle().await;
    h.press("ctrl+c");
    advance(&mut h, 3_000).await;
    let tab = &h.app.model.tabs[0];
    assert!(matches!(&tab.outcome, Some(Outcome::Rows(set)) if set.rows[0][0] == crate::core::Cell::Value("earlier".into())), "{:?}", tab.outcome);
    assert!(matches!(tab.run, RunState::Cancelled { how: super::query::CancelEnd::FinishedFirst, .. }), "{:?}", tab.run);
    let pane = results_rows(&mut h);
    assert!(!pane.join("\n").contains("finished-anyway"), "{pane:#?}");
    let state = pane[0].clone();
    assert!(state.contains("finished before the cancel"), "{pane:#?}");
    assert!(pane.join(" ").contains("any changes it made stand"), "{pane:#?}");
    assert!(!state.contains("cancelled"), "not headlined as cancelled: {state}");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("finished before the cancel") && !bar.contains("cancelled"), "{bar}");
}

/// `on_pagila` with a run on pagila in flight, then the tab moved to edge and
/// a second run started there. Returns both run requests, oldest first.
async fn two_generations_in_flight() -> (crate::harness::Harness, RequestId, RequestId) {
    let mut h = on_pagila().await;
    h.type_text("select ...");
    let old = runs(&h.press_fx("f5"))[0].0;
    h.press("ctrl+g h k u ctrl+g l");
    let new = runs(&h.press_fx("f5"))[0].0;
    (h, old, new)
}

#[tokio::test]
async fn two_replies_delivered_out_of_order_only_the_newer_generations_lands() {
    let reply = |name: &str| CoreReply::Ran(Ok(rows_of(&[&[name]])));
    let shown = |h: &crate::harness::Harness| match &h.app.model.tabs[0].outcome {
        Some(Outcome::Rows(set)) => set.rows[0][0].clone(),
        other => panic!("{other:?}"),
    };
    // The newer first, then the older.
    let (mut h, old, new) = two_generations_in_flight().await;
    assert!(h.app.update(Event::Core(new, reply("new")), base()).repaint);
    let late = h.app.update(Event::Core(old, reply("old")), base());
    assert!(late.commands.is_empty());
    assert_eq!(shown(&h), crate::core::Cell::Value("new".into()), "the older reply's rows never land");
    assert!(h.app.model.tabs[0].abandoned.is_empty(), "it only says the older run has ended");
    let again = h.app.update(Event::Core(old, reply("old")), base());
    assert!(!again.repaint, "and a reply already taken changes nothing at all");
    // The older first, then the newer.
    let (mut h, old, new) = two_generations_in_flight().await;
    let early = h.app.update(Event::Core(old, reply("old")), base());
    assert!(early.commands.is_empty() && h.app.model.tabs[0].outcome.is_none());
    assert!(matches!(h.app.model.tabs[0].run, RunState::Running { .. }), "the newer run is still running");
    assert!(h.app.update(Event::Core(new, reply("new")), base()).repaint);
    assert_eq!(shown(&h), crate::core::Cell::Value("new".into()));
}

// ---- one run at a time ----------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn run_is_refused_while_running_or_cancelling_and_allowed_again_after_every_ending() {
    let refused = |h: &mut crate::harness::Harness, when: &str| {
        h.app.ui.notice = None;
        let sent = runs(&h.press_fx("f5"));
        assert!(sent.is_empty(), "{when}: {sent:?}");
        let said = notice_text(h);
        assert!(said.contains("still running") || said.contains("not stopped yet"), "{when}: {said:?}");
    };
    let allowed = |h: &mut crate::harness::Harness, when: &str| {
        assert_eq!(runs(&h.press_fx("f5")).len(), 1, "{when}");
    };
    let mut h = on_pagila().await;
    h.type_text("select ...");
    // Done, failed, timed out: each ends a run, after which the next may start.
    for (ending, script) in [
        ("done", Scripted::ok(rows_of(&[&["1"]]))),
        ("failed", Scripted::err(CoreErrorKind::Sql, "syntax error")),
        ("timed out", Scripted::err(CoreErrorKind::Timeout, "query timeout of 1 s")),
    ] {
        h.fake.script_run(script.after(Duration::from_secs(1)));
        allowed(&mut h, &format!("before {ending}"));
        h.settle().await;
        refused(&mut h, &format!("running, before {ending}"));
        advance(&mut h, 1_000).await;
        assert_eq!(h.app.model.tabs[0].run, RunState::Idle, "{ending}");
    }
    // Cancelling (also once the cancel is overdue), then cancelled.
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])).after(Duration::from_secs(10)).deaf());
    allowed(&mut h, "after timed out");
    h.settle().await;
    h.press("ctrl+c");
    refused(&mut h, "cancelling");
    advance(&mut h, 3_000).await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { stuck: true, .. }));
    refused(&mut h, "cancelling, overdue");
    advance(&mut h, 7_000).await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { .. }));
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])));
    allowed(&mut h, "after cancelled");
}

// ---- a cancel that does not land: the second cancel ---------------------------------

/// A run on pagila whose cancel does not land, cancelled once.
async fn cancelling_deaf() -> (crate::harness::Harness, RequestId, RunId) {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(rows_of(&[&["x"]])).after(Duration::from_secs(60)).deaf());
    h.type_text("select pg_sleep(60)");
    let sent = runs(&h.press_fx("f5"));
    h.settle().await;
    h.press("ctrl+c");
    h.settle().await;
    (h, sent[0].0, sent[0].1)
}

fn screen(h: &mut crate::harness::Harness) -> String {
    frame_rows(&mut h.app, 114, 30).join("\n")
}

#[tokio::test(start_paused = true)]
async fn a_second_cancel_while_cancelling_asks_before_closing_the_session_and_esc_keeps_waiting() {
    let (mut h, _, _) = cancelling_deaf().await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { .. }));
    let commands = h.press_fx("ctrl+c");
    assert!(commands.is_empty(), "nothing yet: {commands:?}");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmDropSession(_))), "{:?}", h.app.ui.overlay);
    let text = screen(&mut h);
    assert!(text.contains("Close this tab's session"), "{text}");
    assert!(text.contains("pagila"), "names what is affected: {text}");
    assert!(text.contains("↵ new session") && text.contains("esc keep waiting"), "{text}");
    assert!(text.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ").contains("committed when it ends"), "says what the server may do: {text}");
    // A destructive question: a red border.
    let (x, y) = find_text(&mut h.app, 114, 30, "Close this tab's session").unwrap();
    assert_eq!(role_at(&mut h.app, x.saturating_sub(2), y).fg, Some(super::theme::Role::Red));
    // Prefix c is the same action.
    let commands = h.press_fx("esc");
    assert!(commands.is_empty() && h.app.ui.overlay.is_none(), "{commands:?}");
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { .. }), "still waiting");
    assert!(mode_bar(&mut h).contains("ctrl+c stop waiting"), "{}", mode_bar(&mut h));
    h.press("ctrl+g c");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmDropSession(_))));
}

#[tokio::test(start_paused = true)]
async fn confirming_stops_waiting_closes_the_old_session_once_the_run_lets_go_and_the_next_run_starts_a_new_session() {
    let (mut h, _, _) = cancelling_deaf().await;
    let old = h.app.model.tabs[0].session();
    h.press("ctrl+c");
    let commands = h.press_fx("enter");
    assert_eq!(commands, vec![Command::CloseSession(old, dbp("local pg", "pagila"))]);
    assert!(h.app.ui.overlay.is_none());
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { how: super::query::CancelEnd::Abandoned, .. }));
    let text = screen(&mut h);
    assert!(text.contains("stopped waiting") && text.contains("may still be running"), "{text}");
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])));
    let next = runs(&h.press_fx("f5"));
    assert_eq!(next.len(), 1, "a run is allowed at once: it has a new session");
    assert_ne!(next[0].2.session, old);
}

#[tokio::test(start_paused = true)]
async fn when_the_run_ends_under_the_open_question_it_says_so_and_enter_only_dismisses_it() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(rows_of(&[&["x"]])).after(Duration::from_secs(5)).deaf());
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+c ctrl+c");
    assert!(h.app.ui.overlay.is_some());
    advance(&mut h, 5_000).await;
    assert!(h.app.ui.overlay.is_some(), "the box stays, so a key meant for it lands on it");
    assert!(screen(&mut h).contains("The statement has stopped"), "{}", screen(&mut h));
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { .. }));
    let session = h.app.model.tabs[0].session();
    let commands = h.press_fx("enter");
    assert!(commands.is_empty(), "nothing to stop any more: {commands:?}");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.editor.text(), "select 1", "the enter did not reach the editor");
    assert_eq!(h.app.model.tabs[0].session(), session, "no new session");
}

// ---- quitting -------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn quit_while_cancelling_or_with_the_question_open_cancels_again_closes_the_session_then_quits() {
    for question in [false, true] {
        let (mut h, _, run) = cancelling_deaf().await;
        let session = h.app.model.tabs[0].session();
        let commands = if question {
            h.press("ctrl+c");
            assert!(h.app.ui.overlay.is_some());
            h.app.quit_requested().commands
        } else {
            h.press_fx("ctrl+g q")
        };
        assert_eq!(
            commands,
            vec![Command::Cancel(run), Command::CloseSession(session, dbp("local pg", "pagila")), Command::Quit],
            "question open: {question}"
        );
    }
}

// ---- the keys and the hint ---------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn ctrl_c_with_nothing_running_does_nothing_at_all_in_any_pane_and_in_the_sidebar_never_cancels() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    let selection = h.app.ui.editor.selection();
    for (pane, keys) in [("editor", ""), ("results", "ctrl+g j"), ("sidebar", "ctrl+g h")] {
        if !keys.is_empty() {
            h.press(keys);
        }
        let now = h.now();
        let fx = press_at(&mut h.app, "ctrl+c", now);
        assert!(!fx.repaint && fx.commands.is_empty(), "{pane}: {fx:?}");
        assert!(h.app.ui.notice.is_none() && h.app.ui.overlay.is_none(), "{pane}");
    }
    assert_eq!(h.app.ui.editor.selection(), selection, "the widget's copy never ran");
    // With a run in flight, ctrl+c in the sidebar still does nothing.
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])).after(Duration::from_secs(5)));
    h.press("ctrl+g l f5");
    h.press("ctrl+g h");
    let now = h.now();
        let fx = press_at(&mut h.app, "ctrl+c", now);
    assert!(fx.commands.is_empty() && !fx.repaint, "{fx:?}");
    assert!(matches!(h.app.model.tabs[0].run, RunState::Running { .. }));
}

#[tokio::test(start_paused = true)]
async fn the_mode_bar_offers_cancel_only_while_a_run_is_in_flight_and_a_click_on_it_cancels() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    assert!(!mode_bar(&mut h).contains("cancel"), "{}", mode_bar(&mut h));
    h.press("ctrl+g");
    assert!(!mode_bar(&mut h).contains("cancel"), "not after the prefix either: {}", mode_bar(&mut h));
    h.press("esc");
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])).after(Duration::from_secs(5)));
    let run = runs(&h.press_fx("f5"))[0].1;
    h.settle().await;
    assert!(mode_bar(&mut h).contains("ctrl+c cancel"), "{}", mode_bar(&mut h));
    h.press("ctrl+g");
    assert!(mode_bar(&mut h).contains("c cancel"), "after the prefix: {}", mode_bar(&mut h));
    h.press("esc ctrl+g h");
    assert!(mode_bar(&mut h).contains("ctrl+g c cancel"), "from the sidebar, the prefix key: {}", mode_bar(&mut h));
    let commands = click_hint(&mut h, "ctrl+g c cancel");
    assert_eq!(commands, vec![Command::Cancel(run)]);
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { .. }));
    h.settle().await;
    assert!(!mode_bar(&mut h).contains("c cancel"), "over: {}", mode_bar(&mut h));
}

#[tokio::test(start_paused = true)]
async fn typing_moving_and_the_sidebar_stay_live_while_a_run_is_running_or_being_cancelled() {
    let (mut h, _, _) = cancelling_deaf().await;
    advance(&mut h, 3_000).await; // overdue
    h.type_text(" -- more");
    assert!(h.app.ui.editor.text().ends_with(" -- more"));
    h.press("ctrl+g h");
    let before = h.app.ui.sidebar.cursor;
    h.press("k");
    assert_ne!(h.app.ui.sidebar.cursor, before, "the sidebar moves");
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { stuck: true, .. }));
}

// ---- the run's connection or target goes away -----------------------------------------

#[tokio::test(start_paused = true)]
async fn removing_the_connection_a_run_is_on_cancels_the_run_before_the_removal_and_leaves_the_tab_idle() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])).after(Duration::from_secs(30)));
    h.fake.script_remove_connection(Scripted::ok(()));
    h.type_text("select pg_sleep(30)");
    let run = runs(&h.press_fx("f5"))[0].1;
    h.settle().await;
    let commands = h.press_fx("ctrl+g h g d enter");
    let cancel = commands.iter().position(|c| *c == Command::Cancel(run));
    let remove = commands.iter().position(|c| matches!(c, Command::Core(_, CoreCall::RemoveConnection(_))));
    assert!(matches!((cancel, remove), (Some(c), Some(r)) if c < r), "the run is cancelled first: {commands:?}");
    h.settle().await;
    let tab = &h.app.model.tabs[0];
    assert_eq!((tab.target.clone(), tab.run.clone()), (None, RunState::Idle));
}

#[tokio::test(start_paused = true)]
async fn giving_the_tab_another_database_while_cancelling_cancels_again_closes_the_old_session_and_starts_a_new_one() {
    let (mut h, request, run) = cancelling_deaf().await;
    let old = h.app.model.tabs[0].session();
    let commands = h.press_fx("ctrl+g h k u");
    assert_eq!(commands, vec![Command::Cancel(run), Command::CloseSession(old, dbp("local pg", "pagila"))]);
    let tab = &h.app.model.tabs[0];
    assert_eq!(tab.run, RunState::Idle);
    assert_ne!(tab.session(), old);
    // The old run's reply, whenever it comes, shows nothing: it only clears
    // the marker saying it may still be running.
    let fx = h.app.update(Event::Core(request, CoreReply::Ran(Ok(rows_of(&[&["x"]])))), base());
    assert!(fx.commands.is_empty() && h.app.model.tabs[0].outcome.is_none());
    assert!(h.app.model.tabs[0].abandoned.is_empty());
}

// ---- also fixed in slice 06 ----------------------------------------------------------------

#[tokio::test]
async fn a_key_the_editor_takes_without_changing_anything_does_not_repaint() {
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    for key in ["left", "up", "home", "backspace"] {
        assert!(!h.press(key), "{key} on an empty editor changes nothing");
    }
    assert!(h.press("x"), "typing repaints");
    assert!(h.press("left"), "moving the cursor repaints");
    assert!(!h.press("left"), "at the start already");
    assert!(!h.press("down"), "nowhere to go");
    assert!(h.press("shift+right"), "selecting repaints");
}

#[tokio::test(start_paused = true)]
async fn the_notice_never_covers_the_run_state_or_the_line_naming_what_ran() {
    let (mut h, _) = second_run_in_flight(30).await;
    h.press("f5"); // refused, with a notice
    assert!(h.app.ui.notice.is_some());
    for (w, ht) in [(114, 30), (100, 16), (80, 12), (70, 11)] {
        let rows = frame_rows(&mut h.app, w, ht);
        let r = h.app.ui.hits.results;
        assert!(rows.last().unwrap().contains("running"), "{w}x{ht}: the mode bar's run state: {rows:#?}");
        let pane: Vec<&String> = rows[(r.y + 1) as usize..r.bottom() as usize].iter().collect();
        assert!(pane.iter().any(|row| row.contains("● running")), "{w}x{ht}: the state line: {rows:#?}");
        assert!(pane.iter().any(|row| row.contains("earlier run: select 'earlier'")), "{w}x{ht}: what ran: {rows:#?}");
        assert!(rows.iter().any(|row| row.contains("still running in this tab")), "{w}x{ht}: the notice is shown: {rows:#?}");
    }
}

// ---- the stale-reply audit: every Core reply path -----------------------------------------
//
// The rule: a reply whose generation is older than its target's changes
// nothing and repaints nothing. Already tested elsewhere: the connection list
// (`tests::a_reply_with_a_stale_generation_changes_no_state`), a tree node's
// children (`tree_tests::a_reply_that_was_superseded_by_a_refresh_or_a_collapse_changes_nothing`),
// and runs (above). A save is never superseded: the form is inert while it
// saves, and what it saved is real, so its reply always lands.

fn first_core_call(commands: &[Command]) -> RequestId {
    commands.iter().find_map(|c| match c {
        Command::Core(id, _) => Some(*id),
        _ => None,
    }).unwrap_or_else(|| panic!("no Core call in {commands:?}"))
}

#[tokio::test]
async fn a_connection_test_superseded_by_an_edit_changes_nothing_and_repaints_nothing_when_it_replies() {
    let mut h = crate::harness::Harness::start(crate::core::FakeCore::empty());
    h.settle().await;
    h.press("ctrl+g n");
    h.type_text("n");
    h.press("tab");
    h.type_text("h");
    let id = first_core_call(&h.press_fx("ctrl+t"));
    h.type_text("x"); // an edit: the test's result would no longer be true
    let before = format!("{:?}", h.app.ui.overlay);
    let fx = h.app.update(Event::Core(id, CoreReply::Tested(Ok(()))), base());
    assert!(!fx.repaint && fx.commands.is_empty(), "{fx:?}");
    assert_eq!(format!("{:?}", h.app.ui.overlay), before, "the form is untouched");
}

#[tokio::test]
async fn a_removal_superseded_by_a_second_removal_of_the_same_connection_changes_nothing_when_it_replies() {
    let mut h = started(&["local pg"]).await;
    let first = first_core_call(&h.press_fx("d enter"));
    let second = first_core_call(&h.press_fx("d enter"));
    let fx = h.app.update(Event::Core(first, CoreReply::Removed(Err(crate::core::CoreError::new(CoreErrorKind::Store, "locked")))), base());
    assert!(!fx.repaint && fx.commands.is_empty(), "{fx:?}");
    assert!(h.app.ui.overlay.is_none(), "no failure shown for a superseded removal");
    assert!(h.app.update(Event::Core(second, CoreReply::Removed(Ok(()))), base()).repaint);
    assert!(matches!(&h.app.model.connections, Loadable::Ready(list) if list.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_superseded_runs_reply_arriving_while_another_run_is_being_cancelled_does_not_end_the_cancel() {
    let (mut h, old, _) = two_generations_in_flight().await;
    h.press("ctrl+c"); // the run on edge is being cancelled; the one on pagila was superseded earlier
    let state = h.app.model.tabs[0].run.clone();
    assert!(matches!(state, RunState::Cancelling { .. }));
    let fx = h.app.update(Event::Core(old, CoreReply::Ran(Err(crate::core::CoreError::new(CoreErrorKind::Cancelled, "c")))), base());
    assert!(fx.commands.is_empty(), "{fx:?}");
    assert_eq!(h.app.model.tabs[0].run, state, "still cancelling: that was not the run being cancelled");
    assert!(h.app.model.tabs[0].abandoned.is_empty(), "it only cleared its own marker");
}

#[tokio::test]
async fn a_current_cancelled_reply_the_tab_did_not_ask_for_ends_the_run_as_cancelled_and_keeps_the_earlier_result() {
    let mut h = on_pagila().await;
    h.type_text("select ...");
    let first = runs(&h.press_fx("f5"))[0].0;
    h.app.update(Event::Core(first, CoreReply::Ran(Ok(rows_of(&[&["earlier"]])))), base());
    let second = runs(&h.press_fx("f5"))[0].0;
    let mut e = crate::core::CoreError::new(CoreErrorKind::Cancelled, "Query canceled");
    e.statement = Some("select ...".into());
    assert!(h.app.update(Event::Core(second, CoreReply::Ran(Err(e))), base()).repaint);
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { .. }), "{:?}", h.app.model.tabs[0].run);
    assert!(results_rows(&mut h).join("\n").contains("earlier run:"));
}


// ---- slice 06 review --------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_question_says_a_write_may_still_commit_and_hold_locks() {
    let (mut h, _, _) = cancelling_deaf().await;
    h.press("ctrl+c");
    let text = screen(&mut h).replace('\n', " ");
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("committed when it ends"), "{text}");
    assert!(flat.contains("locks"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn after_stopping_waiting_the_tab_says_an_abandoned_statement_may_still_be_running_until_that_run_ends() {
    let (mut h, request, _) = cancelling_deaf().await;
    h.press("ctrl+c enter");
    let marker = "an abandoned statement may still be running on local pg / pagila";
    assert!(results_rows(&mut h).join(" ").contains(marker), "{:#?}", results_rows(&mut h));
    // A new run does not hide it.
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])));
    h.press("f5");
    h.settle().await;
    assert!(results_rows(&mut h).join(" ").contains(marker), "{:#?}", results_rows(&mut h));
    // The abandoned run's last word clears it.
    let fx = h.app.update(Event::Core(request, CoreReply::Ran(Err(crate::core::CoreError::new(CoreErrorKind::Cancelled, "c")))), base());
    assert!(fx.repaint);
    assert!(!results_rows(&mut h).join(" ").contains("abandoned"), "{:#?}", results_rows(&mut h));
}

#[tokio::test(start_paused = true)]
async fn giving_the_tab_another_database_or_removing_its_connection_while_a_cancel_is_overdue_leaves_the_marker() {
    for how in ["retarget", "remove"] {
        let (mut h, _, _) = cancelling_deaf().await;
        advance(&mut h, 3_000).await;
        assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelling { stuck: true, .. }));
        if how == "retarget" {
            h.press("ctrl+g h k u");
        } else {
            h.fake.script_remove_connection(Scripted::ok(()));
            h.press("ctrl+g h g d enter");
            h.settle().await;
            advance(&mut h, 2_100).await; // the removal waits for the run, at most RUN_WAIT
            assert_eq!(h.app.model.tabs[0].target, None, "removed: {:?}", h.fake.calls());
        }
        let pane = results_rows(&mut h).join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(pane.contains("an abandoned statement may still be running on local pg / pagila"), "{how}: {pane}");
    }
}

#[tokio::test(start_paused = true)]
async fn giving_the_tab_another_database_after_a_cancel_starts_it_idle() {
    let (mut h, _) = second_run_in_flight(30).await;
    h.press("ctrl+c");
    h.settle().await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Cancelled { .. }));
    h.press("ctrl+g h k u");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    assert!(!mode_bar(&mut h).contains("cancelled"), "{}", mode_bar(&mut h));
}

#[tokio::test(start_paused = true)]
async fn stopping_waiting_clears_the_notice_that_said_to_wait() {
    let (mut h, _, _) = cancelling_deaf().await;
    h.press("f5");
    assert!(h.app.ui.notice.is_some(), "refused, with a notice");
    h.press("ctrl+c enter");
    assert!(h.app.ui.notice.is_none(), "{:?}", h.app.ui.notice);
}

#[tokio::test(start_paused = true)]
async fn the_states_after_a_cancel_never_claim_more_than_the_client_knows() {
    let (mut h, _, _) = cancelling_deaf().await;
    advance(&mut h, 3_000).await;
    let words = |h: &mut crate::harness::Harness| format!("{}\n{}", results_rows(h).join("\n"), mode_bar(h));
    let overdue = words(&mut h);
    assert!(overdue.contains("not stopped yet"), "{overdue}");
    for claim in ["still running on the server", "has not landed", "never confirmed"] {
        assert!(!overdue.contains(claim), "{claim}: {overdue}");
    }
    h.press("ctrl+c");
    let question = screen(&mut h);
    assert!(!question.contains("never confirmed") && !question.contains("has not confirmed"), "{question}");
    h.press("enter");
    let after = words(&mut h);
    assert!(after.contains("not confirmed stopped"), "{after}");
    assert!(!after.contains("never confirmed"), "{after}");
}

#[tokio::test(start_paused = true)]
async fn an_error_other_than_cancelled_after_a_cancel_is_named_and_not_called_cancelled() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::err(CoreErrorKind::Connect, "server closed the connection unexpectedly").after(Duration::from_secs(2)).deaf());
    h.type_text("select pg_sleep(9)");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+c");
    advance(&mut h, 2_000).await;
    let pane = results_rows(&mut h).join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let bar = mode_bar(&mut h);
    assert!(pane.contains("server closed the connection unexpectedly"), "{pane}");
    assert!(!pane.contains("cancelled after") && !bar.contains("cancelled after"), "{pane}\n{bar}");
    assert!(bar.contains("failed"), "{bar}");
}

#[tokio::test(start_paused = true)]
async fn confirming_stop_waiting_closes_the_old_session_only_once_the_run_ends_or_the_wait_runs_out() {
    let (mut h, _, _) = cancelling_deaf().await;
    let old = h.app.model.tabs[0].session();
    h.fake.script_close_session(Scripted::ok(()));
    h.press("ctrl+c enter");
    h.settle().await;
    let closed = |h: &crate::harness::Harness| h.fake.calls().contains(&Call::CloseSession(old, dbp("local pg", "pagila")));
    advance(&mut h, 1_900).await;
    assert!(!closed(&h), "the shell waits for the run first");
    advance(&mut h, 200).await;
    assert!(closed(&h), "then closes it, the run still going (RUN_WAIT)");
}

#[tokio::test]
async fn prefix_then_c_with_nothing_running_emits_nothing_and_leaves_the_screen_as_esc_would() {
    let mut h = on_pagila().await;
    h.press("ctrl+g");
    let commands = h.press_fx("c");
    assert!(commands.is_empty() && h.app.ui.notice.is_none() && h.app.ui.overlay.is_none(), "{commands:?}");
    let after_c = screen(&mut h);
    h.press("ctrl+g esc");
    assert_eq!(after_c, screen(&mut h));
}

/// `on_pagila` where the connection has a query timeout of `secs`.
async fn on_pagila_with_timeout(secs: u64) -> crate::harness::Harness {
    let fake = crate::core::FakeCore::new();
    let mut c = super::connections_tests::conn("local pg", crate::core::DbKind::Postgres);
    c.query_timeout = Some(secs);
    fake.script_connections(Scripted::ok(vec![c]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    let mut h = crate::harness::Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("j j u ctrl+g l");
    h
}

#[tokio::test(start_paused = true)]
async fn past_the_connections_timeout_the_tab_says_stopping_then_not_stopped_yet_then_timed_out() {
    let mut h = on_pagila_with_timeout(1).await;
    let mut e = crate::core::CoreError::new(CoreErrorKind::Timeout, "query timeout of 1 s and was stopped");
    e.statement = Some("select pg_sleep(9)".into());
    h.fake.script_run(Scripted::err(CoreErrorKind::Timeout, "query timeout of 1 s and was stopped").after(Duration::from_secs(5)).deaf());
    h.type_text("select pg_sleep(9)");
    h.press("f5");
    h.settle().await;
    advance(&mut h, 1_000).await;
    assert!(mode_bar(&mut h).contains("timed out: stopping"), "{}", mode_bar(&mut h));
    advance(&mut h, 2_100).await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("not stopped yet") && !bar.contains("still running on the server"), "{bar}");
    advance(&mut h, 2_000).await;
    assert!(mode_bar(&mut h).contains("timed out after"), "{}", mode_bar(&mut h));
    let _ = e;
}
