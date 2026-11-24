//! Layer 2 and 3: the acceptance tests of slice 15a (what belongs to one
//! query tab stays in it). Two tabs are opened and switched with the test
//! helpers `open_tab` and `activate_tab`; slice 15b adds the keys.

use std::time::Duration;

use super::complete_tests::{editing, pagila, type_and_settle, visible};
use super::grid_tests::{focused, text_set};
use super::query::{Outcome, RunState};
use super::run_tests::*;
use super::testing::*;
use super::*;
use crate::core::Scripted;
use crate::harness::Harness;

/// The tab with id `id`, from the model.
fn model_tab(h: &Harness, id: TabId) -> &QueryTab {
    h.app.model.tabs.iter().find(|t| t.id == id).expect("the tab exists")
}

/// Opens a second tab and makes it active.
fn second_tab(h: &mut Harness) -> TabId {
    let two = h.app.open_tab();
    let fx = h.app.activate_tab(two);
    h.run(fx);
    assert_eq!(h.app.active_tab(), two);
    two
}

fn activate(h: &mut Harness, id: TabId) {
    let fx = h.app.activate_tab(id);
    h.run(fx);
    assert_eq!(h.app.active_tab(), id);
}

/// Moves time on by `ms`, letting the replies due by then arrive.
async fn advance(h: &mut Harness, ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    h.settle().await;
    h.tick();
}

fn rows_shown(h: &Harness, id: TabId) -> Option<Vec<String>> {
    match &model_tab(h, id).outcome {
        Some(Outcome::Rows(set)) => Some(set.rows.iter().map(|r| format!("{:?}", r[0])).collect()),
        _ => None,
    }
}

// ---- layer 2: the editor, the grid, the popup, the find text ----------------------

#[tokio::test]
async fn text_typed_in_one_tab_is_absent_from_the_other_and_coming_back_restores_text_caret_selection_and_undo() {
    let mut h = on_pagila().await;
    let one = h.app.active_tab();
    h.type_text("select 1");
    h.app.update(Event::Paste(" from film".into()), base());
    h.press("shift+left shift+left");
    let (caret, selection) = (h.app.ui.tab.editor.cursor(), h.app.ui.tab.editor.selection());
    assert!(selection.is_some());
    let two = second_tab(&mut h);
    assert_eq!(h.app.ui.tab.editor.text(), "", "tab 2 starts empty");
    h.app.ui.focus = PaneId::Editor;
    h.type_text("abc");
    activate(&mut h, one);
    assert_eq!(h.app.ui.tab.editor.text(), "select 1 from film");
    assert_eq!(h.app.ui.tab.editor.cursor(), caret);
    assert_eq!(h.app.ui.tab.editor.selection(), selection);
    h.press("ctrl+z");
    assert_eq!(h.app.ui.tab.editor.text(), "select 1", "one undo removes only tab 1's last edit, the paste");
    activate(&mut h, two);
    assert_eq!(h.app.ui.tab.editor.text(), "abc", "tab 2's text is its own");
}

#[tokio::test]
async fn the_grid_cursor_and_scroll_of_a_tab_survive_a_switch_and_back() {
    let mut h = focused(text_set(200, 8)).await;
    let one = h.app.active_tab();
    for _ in 0..50 {
        h.press("j");
    }
    h.press("l l l");
    frame_rows(&mut h.app, 100, 30);
    let g = &h.app.ui.tab.grid;
    let before = (g.row, g.col, g.top, g.left);
    assert_eq!((before.0, before.1), (50, 3));
    assert!(before.2 > 0, "scrolled: {before:?}");
    second_tab(&mut h);
    frame_rows(&mut h.app, 100, 30);
    assert_eq!((h.app.ui.tab.grid.row, h.app.ui.tab.grid.col, h.app.ui.tab.grid.top), (0, 0, 0), "tab 2 has its own grid");
    activate(&mut h, one);
    frame_rows(&mut h.app, 100, 30);
    let g = &h.app.ui.tab.grid;
    assert_eq!((g.row, g.col, g.top, g.left), before);
}

#[tokio::test]
async fn switching_tabs_closes_the_popup_and_leaves_template_mode_in_the_tab_left_without_changing_its_buffer() {
    let mut h = editing(pagila()).await;
    let one = h.app.active_tab();
    type_and_settle(&mut h, "sele").await;
    assert!(visible(&h));
    second_tab(&mut h);
    assert!(!visible(&h), "no popup in tab 2");
    activate(&mut h, one);
    assert!(!visible(&h), "the popup of tab 1 was closed when it was left");
    assert_eq!(h.app.ui.tab.editor.text(), "sele");

    let mut h = editing(pagila()).await;
    let one = h.app.active_tab();
    type_and_settle(&mut h, "sel").await;
    h.press("tab");
    h.settle().await;
    assert!(h.app.ui.tab.template.active());
    let text = h.app.ui.tab.editor.text();
    second_tab(&mut h);
    assert!(!h.app.ui.tab.template.active());
    activate(&mut h, one);
    assert!(!h.app.ui.tab.template.active(), "template mode was left");
    assert_eq!(h.app.ui.tab.editor.text(), text, "and the buffer kept");
}

#[tokio::test]
async fn each_tab_keeps_its_own_find_text() {
    let mut h = on_pagila().await;
    let one = h.app.active_tab();
    h.type_text("select film from film");
    h.press("cmd+f");
    h.type_text("film");
    h.press("esc");
    second_tab(&mut h);
    h.app.ui.focus = PaneId::Editor;
    h.type_text("abc");
    h.press("cmd+f");
    assert_eq!(h.app.ui.tab.bar.as_ref().map(|b| b.field.text()), Some(""), "tab 2 has no find text yet");
    h.type_text("abc");
    h.press("esc");
    activate(&mut h, one);
    assert_eq!(h.app.ui.tab.last_find, "film");
    h.press("cmd+f");
    assert_eq!(h.app.ui.tab.bar.as_ref().map(|b| b.field.text()), Some("film"));
}

// ---- layer 3: replies land in their own tab -----------------------------------------

/// `on_pagila` with "select 1" running in tab 1 (reply after `one_ms`), and
/// tab 2 on pagila too, running "select 2" (reply after `two_ms`), tab 2
/// active.
async fn two_runs(one_ms: u64, two_ms: u64) -> (Harness, TabId, TabId) {
    let mut h = on_pagila().await;
    let one = h.app.active_tab();
    h.fake.script_run(Scripted::ok(rows_of(&[&["one"]])).after(Duration::from_millis(one_ms)));
    h.fake.script_run(Scripted::ok(rows_of(&[&["two"]])).after(Duration::from_millis(two_ms)));
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    let two = second_tab(&mut h);
    h.press("ctrl+g h u ctrl+g l");
    assert_eq!(model_tab(&h, two).target, Some(dbp("local pg", "pagila")));
    h.type_text("select 2");
    h.press("f5");
    h.settle().await;
    assert!(model_tab(&h, one).run.in_flight().is_some() && model_tab(&h, two).run.in_flight().is_some());
    (h, one, two)
}

#[tokio::test(start_paused = true)]
async fn a_slow_and_a_fast_run_in_two_tabs_each_land_in_their_own_tab_whichever_arrives_first() {
    for (one_ms, two_ms) in [(5_000, 1_000), (1_000, 5_000)] {
        let (mut h, one, two) = two_runs(one_ms, two_ms).await;
        let (first, second) = if one_ms < two_ms { (one, two) } else { (two, one) };
        advance(&mut h, 1_500).await;
        assert_eq!(model_tab(&h, first).run, RunState::Idle, "{one_ms}/{two_ms}");
        assert!(model_tab(&h, second).run.in_flight().is_some(), "the other tab is still running: {one_ms}/{two_ms}");
        assert!(rows_shown(&h, second).is_none(), "nothing landed in the other tab");
        advance(&mut h, 5_000).await;
        assert_eq!(rows_shown(&h, one), Some(vec![format!("{:?}", crate::core::Cell::Value("one".into()))]));
        assert_eq!(rows_shown(&h, two), Some(vec![format!("{:?}", crate::core::Cell::Value("two".into()))]));
        assert_eq!(model_tab(&h, one).run, RunState::Idle);
        assert_eq!(model_tab(&h, two).run, RunState::Idle);
    }
}

#[tokio::test(start_paused = true)]
async fn a_reply_for_the_tab_that_is_not_active_updates_only_it_and_repaints_for_its_glyph_in_the_bar() {
    // Tab 2's clock is shown: let it reach its next second first, so only
    // tab 1's reply arrives in the step that is counted.
    let (mut h, one, two) = two_runs(1_500, 60_000).await;
    advance(&mut h, 1_000).await;
    let (repaints, tab_two) = (h.repaints, model_tab(&h, two).clone());
    advance(&mut h, 500).await;
    assert!(rows_shown(&h, one).is_some());
    // Slice 15b: the bar shows tab 1's run glyph, which the reply changed.
    assert_eq!(h.repaints, repaints + 1, "tab 1's glyph changed in the bar");
    assert_eq!((model_tab(&h, two).run.clone(), model_tab(&h, two).outcome.clone()), (tab_two.run, tab_two.outcome), "tab 2 untouched");
}

#[tokio::test(start_paused = true)]
async fn a_completion_reply_for_tab_1_arriving_while_tab_2_is_active_opens_nothing_changes_nothing_and_does_not_repaint() {
    // Once with a Core that answers (after a second), once with one that
    // fails at once: neither reply may reach tab 2, open its popup, change
    // its text, show a notice or repaint.
    for answers in [true, false] {
        let mut h = if answers { editing(pagila()).await } else { on_pagila().await };
        h.fake.set_catalogue_delay(Duration::from_secs(1));
        // Asked, and not yet answered (no settle) when tab 2 becomes active.
        h.type_text("select * from fi");
        second_tab(&mut h);
        h.app.ui.focus = PaneId::Editor;
        h.type_text("1");
        let repaints = h.repaints;
        advance(&mut h, 3_000).await;
        let asked = h.fake.calls().iter().filter(|c| matches!(c, crate::core::Call::Complete(_) | crate::core::Call::CurrentSchema(_))).count();
        assert!(asked > 0, "tab 1 asked the Core");
        assert!(!visible(&h), "no popup in tab 2");
        assert_eq!(h.app.ui.tab.editor.text(), "1");
        assert!(h.app.ui.notice.is_none(), "{:?}", h.app.ui.notice);
        assert_eq!(h.repaints, repaints, "nothing on screen changed (answers: {answers})");
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_in_tab_1_leaves_an_idle_tab_2_untouched_and_tab_1s_late_reply_is_dropped() {
    let mut h = on_pagila().await;
    let one = h.app.active_tab();
    h.fake.script_run(Scripted::ok(rows_of(&[&["two"]])));
    h.fake.script_run(Scripted::ok(rows_of(&[&["late"]])).after(Duration::from_secs(1)).deaf());
    let two = second_tab(&mut h);
    h.press("ctrl+g h u ctrl+g l");
    h.type_text("select 2");
    h.press("f5");
    h.settle().await;
    let tab_two = model_tab(&h, two).clone();
    assert!(rows_shown(&h, two).is_some());
    activate(&mut h, one);
    h.app.ui.focus = PaneId::Editor;
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    let commands = h.press_fx("ctrl+c");
    assert!(matches!(commands.as_slice(), [Command::Cancel(_)]), "{commands:?}");
    let same = |a: &QueryTab, b: &QueryTab| a.run == b.run && a.outcome == b.outcome && a.session == b.session;
    assert!(same(model_tab(&h, two), &tab_two), "tab 2 untouched by the cancel");
    advance(&mut h, 1_500).await;
    assert!(matches!(model_tab(&h, one).run, RunState::Cancelled { .. }), "{:?}", model_tab(&h, one).run);
    assert!(rows_shown(&h, one).is_none(), "the late reply's rows were dropped");
    assert!(same(model_tab(&h, two), &tab_two), "tab 2 untouched by the late reply");
}

#[tokio::test(start_paused = true)]
async fn removing_a_running_tab_cancels_and_then_closes_its_session_and_its_later_reply_changes_nothing() {
    let (mut h, one, two) = two_runs(60_000, 5_200).await;
    activate(&mut h, one);
    let run = model_tab(&h, two).run.in_flight().expect("running").0;
    let session = model_tab(&h, two).session();
    let fx = h.app.remove_tab(two);
    let commands = fx.commands.clone();
    h.run(fx);
    assert_eq!(commands, vec![Command::Cancel(run), Command::CloseSession(session, dbp("local pg", "pagila"))]);
    assert!(h.app.model.tabs.iter().all(|t| t.id != two), "the tab is gone");
    // Tab 1's clock is shown: let it reach its sixth second first, so only
    // the removed tab's reply arrives in the step that is counted.
    advance(&mut h, 5_000).await;
    let before = (model_tab(&h, one).run.clone(), h.repaints);
    advance(&mut h, 500).await;
    assert!(h.fake.calls().iter().filter(|c| matches!(c, crate::core::Call::Run(..))).count() == 2);
    assert_eq!((model_tab(&h, one).run.clone(), h.repaints), before, "the removed tab's reply changed nothing and repainted nothing");
}

#[tokio::test(start_paused = true)]
async fn quit_with_two_tabs_running_cancels_both_and_closes_both_sessions() {
    let (mut h, one, two) = two_runs(60_000, 60_000).await;
    let runs = [model_tab(&h, one).run.in_flight().unwrap().0, model_tab(&h, two).run.in_flight().unwrap().0];
    let sessions = [model_tab(&h, one).session(), model_tab(&h, two).session()];
    let fx = h.app.quit_requested();
    for run in runs {
        assert!(fx.commands.contains(&Command::Cancel(run)), "{:?}", fx.commands);
    }
    for session in sessions {
        assert!(fx.commands.contains(&Command::CloseSession(session, dbp("local pg", "pagila"))), "{:?}", fx.commands);
    }
    let cancel = |r| fx.commands.iter().position(|c| *c == Command::Cancel(r)).unwrap();
    let close = |s| fx.commands.iter().position(|c| matches!(c, Command::CloseSession(x, _) if *x == s)).unwrap();
    assert!(cancel(runs[0]) < close(sessions[0]) && cancel(runs[1]) < close(sessions[1]), "each cancel goes before its close");
    assert_eq!(h.app.abandoned_on(), vec!["local pg / pagila".to_string(), "local pg / pagila".to_string()], "the stderr line names both");
}

#[tokio::test(start_paused = true)]
async fn quitting_after_removing_a_running_tab_still_names_its_place_until_its_late_reply_ends_it() {
    let (mut h, one, two) = two_runs(60_000, 5_200).await;
    activate(&mut h, one);
    let fx = h.app.remove_tab(two);
    h.run(fx);
    assert_eq!(h.app.abandoned_on(), vec!["local pg / pagila".to_string()], "the removed tab's run may still be running");
    h.app.quit_requested();
    assert_eq!(h.app.abandoned_on().len(), 2, "the stderr line names the removed tab's place and tab 1's");
    advance(&mut h, 5_500).await;
    assert_eq!(h.app.abandoned_on().len(), 1, "the late reply says the removed tab's run ended");
}

#[tokio::test(start_paused = true)]
async fn the_stop_waiting_question_closes_when_the_tab_changes_and_a_yes_to_it_acts_on_the_tab_it_was_asked_in() {
    let (mut h, one, two) = two_runs(60_000, 60_000).await;
    activate(&mut h, one);
    h.press("ctrl+c");
    h.press("ctrl+c");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmDropSession(_))), "{:?}", h.app.ui.overlay);
    activate(&mut h, two);
    assert!(h.app.ui.overlay.is_none(), "the question does not follow the user to another tab");
    // Were it answered there all the same, it is still tab 1's question.
    let session_two = model_tab(&h, two).session();
    h.app.ui.overlay = Some(overlay::Overlay::ConfirmDropSession(overlay::ConfirmDropSession { tab: one, target: "local pg / pagila".into(), stopped: false }));
    let mut fx = Effects::default();
    h.app.drop_session(&mut fx);
    assert!(matches!(model_tab(&h, one).run, RunState::Cancelled { .. }), "tab 1 stopped waiting: {:?}", model_tab(&h, one).run);
    assert_eq!(model_tab(&h, two).session(), session_two, "tab 2 keeps its session");
    assert!(model_tab(&h, two).run.in_flight().is_some(), "tab 2 still runs");
}

#[tokio::test]
async fn the_re_run_question_and_the_cell_viewer_close_when_the_tab_changes_and_a_yes_acts_on_the_tab_it_was_asked_in() {
    let mut h = super::grid_tests::focused(super::grid_tests::unproven_set(1_000)).await;
    let one = h.app.active_tab();
    h.press("m");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmRerun(_))));
    let two = second_tab(&mut h);
    assert!(h.app.ui.overlay.is_none(), "the question does not follow the user to another tab");
    h.app.ui.overlay = Some(overlay::Overlay::ConfirmRerun(overlay::ConfirmRerun { tab: one, statement: "select my_func(id) as v".into() }));
    let mut fx = Effects::default();
    h.app.confirm_rerun(&mut fx);
    assert!(model_tab(&h, one).loading_more.is_some(), "tab 1 loads more");
    assert!(model_tab(&h, two).loading_more.is_none() && model_tab(&h, two).run.in_flight().is_none(), "tab 2 untouched");
    assert!(matches!(fx.commands.as_slice(), [Command::Core(_, CoreCall::Run(..))]), "{:?}", fx.commands);

    // The cell viewer closes too.
    let mut h = super::grid_tests::focused(super::grid_tests::truncated_set(10)).await;
    h.press("enter");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::CellViewer(_))), "{:?}", h.app.ui.overlay);
    second_tab(&mut h);
    assert!(h.app.ui.overlay.is_none(), "a cell of tab 1 is not shown over tab 2");
}

#[tokio::test]
async fn a_completion_failure_in_a_parked_tab_is_said_when_the_tab_is_shown_again() {
    let mut h = super::complete_tests::editing_on(crate::core::FakeCore::new()).await; // every completion call fails
    let one = h.app.active_tab();
    h.type_text("select * from film where ti");
    second_tab(&mut h);
    h.settle().await;
    assert!(h.app.ui.notice.is_none(), "tab 2 is not told: {:?}", h.app.ui.notice);
    activate(&mut h, one);
    let said = h.app.ui.notice.as_ref().map(|n| (n.text.clone(), n.failure)).expect("tab 1 says what failed once it is shown");
    assert!(said.0.starts_with("Completion for ") && said.0.contains(" failed: ") && said.1, "{said:?}");
    assert!(h.app.ui.tab.held_notice.is_none(), "said once: the tab keeps nothing");
}

#[tokio::test]
async fn a_load_more_failure_in_a_parked_tab_is_said_when_the_tab_is_shown_again() {
    let mut h = super::grid_tests::focused(super::grid_tests::truncated_set(1_000)).await;
    let one = h.app.active_tab();
    let sent = runs(&h.press_fx("m"));
    assert_eq!(sent.len(), 1);
    second_tab(&mut h);
    let error = crate::core::CoreError::new(crate::core::CoreErrorKind::Sql, "boom");
    let fx = h.app.update(Event::Core(sent[0].0, CoreReply::Ran(Err(error))), base());
    h.run(fx);
    assert!(h.app.ui.notice.is_none(), "tab 2 is not told: {:?}", h.app.ui.notice);
    assert!(model_tab(&h, one).run.in_flight().is_none(), "the run ended");
    activate(&mut h, one);
    let said = h.app.ui.notice.as_ref().map(|n| (n.text.clone(), n.failure)).expect("a notice");
    assert!(said.0.starts_with("Load more failed") && said.0.contains("boom") && said.1, "{said:?}");
}
