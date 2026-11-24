//! Layer 2 and 3: the acceptance tests of slice 15b (the tab bar): opening,
//! switching and closing tabs by key and by mouse, what the bar draws, and
//! runs in tabs that are not on screen.

use std::time::Duration;

use super::query::{Outcome, RunState};
use super::run_tests::*;
use super::testing::*;
use super::theme::Role;
use super::*;
use crate::core::Scripted;
use crate::harness::Harness;

fn ids(h: &Harness) -> Vec<TabId> {
    h.app.model.tabs.iter().map(|t| t.id).collect()
}

fn model_tab(h: &Harness, id: TabId) -> &QueryTab {
    h.app.model.tabs.iter().find(|t| t.id == id).expect("the tab exists")
}

/// Moves time on by `ms`, letting the replies due by then arrive.
async fn advance(h: &mut Harness, ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    h.settle().await;
    h.tick();
}

/// The editor's top row, where the tab bar is, at 114 by 30.
fn bar_row(h: &mut Harness) -> String {
    let rows = frame_rows(&mut h.app, 114, 30);
    rows[h.app.ui.hits.editor.y as usize].clone()
}

/// The tab bar's hit for tab `id`: its label, or its `×`.
fn tab_hit(h: &mut Harness, id: TabId, close: bool) -> ratatui::layout::Rect {
    frame_rows(&mut h.app, 114, 30);
    h.app.ui.tab_hits.iter().find(|t| t.tab == id && t.close == close).map(|t| t.rect).unwrap_or_else(|| panic!("tab {id:?} (close: {close}) not drawn: {:?}", h.app.ui.tab_hits))
}

fn click_on(h: &mut Harness, rect: ratatui::layout::Rect) -> Vec<Command> {
    let fx = click(&mut h.app, rect);
    let commands = fx.commands.clone();
    h.run(fx);
    commands
}

/// The cancels and session closes among `commands` (a completion request
/// abandoned with its tab is beside the point).
fn sessions(commands: &[Command]) -> Vec<Command> {
    commands.iter().filter(|c| matches!(c, Command::Cancel(_) | Command::CloseSession(..))).cloned().collect()
}

/// The text of the open overlay as drawn, at 114 by 30.
fn screen(h: &mut Harness) -> String {
    frame_rows(&mut h.app, 114, 30).join("\n")
}

// ---- layer 2 --------------------------------------------------------------------

#[tokio::test]
async fn prefix_t_opens_tab_2_active_and_empty_on_tab_1s_target_in_its_own_session_and_the_bar_shows_both() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    h.press("ctrl+g t");
    assert_eq!(ids(&h), vec![TabId(1), TabId(2)]);
    assert_eq!(h.app.active_tab(), TabId(2));
    assert_eq!(h.app.ui.tab.editor.text(), "", "tab 2 starts empty");
    assert_eq!(model_tab(&h, TabId(2)).target, Some(dbp("local pg", "pagila")), "tab 1's target");
    // dbx's `client_session_id` is `dbx-tui-tab-{tab}-{serial}` (`core::dbx`).
    assert_eq!(model_tab(&h, TabId(2)).session(), crate::core::SessionId { tab: TabId(2), serial: 1 });
    let bar = bar_row(&mut h);
    assert!(bar.contains("1 local pg / pagila") && bar.contains("2 local pg / pagila"), "{bar:?}");
    let (x2, y2) = find_text(&mut h.app, 114, 30, "2 local pg / pagila").unwrap();
    assert_eq!(role_at(&mut h.app, x2, y2).fg, Some(Role::Accent), "the active tab is accent");
    assert_eq!(role_at(&mut h.app, x2 + 4, y2).fg, Some(Role::Accent));
    let (x1, y1) = find_text(&mut h.app, 114, 30, "1 local pg / pagila").unwrap();
    assert_eq!(role_at(&mut h.app, x1, y1).fg, Some(Role::Overlay0), "the others overlay0");
}

#[tokio::test]
async fn prefix_brackets_cycle_and_wrap_and_prefix_digits_go_to_a_tab_that_exists() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t ctrl+g t");
    let [one, two, three] = ids(&h)[..] else { panic!("{:?}", ids(&h)) };
    assert_eq!(h.app.active_tab(), three);
    h.press("ctrl+g ]");
    assert_eq!(h.app.active_tab(), one, "next wraps to the first");
    h.press("ctrl+g [");
    assert_eq!(h.app.active_tab(), three, "previous wraps to the last");
    h.press("ctrl+g [");
    assert_eq!(h.app.active_tab(), two);
    h.press("ctrl+g ]");
    assert_eq!(h.app.active_tab(), three);
    h.press("ctrl+g 1");
    assert_eq!(h.app.active_tab(), one);
    h.press("ctrl+g 3");
    assert_eq!(h.app.active_tab(), three);
    h.press("ctrl+g 2");
    let before = screen(&mut h);
    let fx = press(&mut h.app, "ctrl+g");
    h.run(fx);
    let fx = press(&mut h.app, "9");
    assert!(fx.commands.is_empty());
    assert_eq!(h.app.active_tab(), two, "no tab 9: nothing changes");
    assert_eq!(screen(&mut h), before);
}

#[tokio::test]
async fn closing_an_empty_idle_tab_asks_nothing_closes_its_session_and_makes_the_tab_to_its_left_active() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t ctrl+g t ctrl+g 2");
    let [one, two, three] = ids(&h)[..] else { panic!() };
    let session = model_tab(&h, two).session();
    let commands = sessions(&h.press_fx("ctrl+g T"));
    assert!(h.app.ui.overlay.is_none(), "{:?}", h.app.ui.overlay);
    assert_eq!(commands, vec![Command::CloseSession(session, dbp("local pg", "pagila"))]);
    assert_eq!(ids(&h), vec![one, three]);
    assert_eq!(h.app.active_tab(), one, "the tab to its left");
    // The first tab has nothing to its left: the one to its right.
    h.press("ctrl+g T");
    assert_eq!(ids(&h), vec![three]);
    assert_eq!(h.app.active_tab(), three);
}

#[tokio::test]
async fn closing_a_tab_with_text_asks_first_naming_the_tab_and_esc_keeps_it_and_its_text() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    let two = h.app.active_tab();
    h.type_text("select 2");
    let commands = sessions(&h.press_fx("ctrl+g T"));
    assert!(commands.is_empty(), "{commands:?}");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmCloseTab(_))), "{:?}", h.app.ui.overlay);
    let shown = screen(&mut h);
    assert!(shown.contains("tab 2") && shown.contains("text will be lost"), "{shown}");
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!((h.app.active_tab(), h.app.ui.tab.editor.text()), (two, "select 2".to_string()));
    let commands = sessions(&h.press_fx("ctrl+g T enter"));
    assert!(matches!(commands.as_slice(), [Command::CloseSession(..)]), "{commands:?}");
    assert_eq!(ids(&h), vec![TabId(1)]);
}

#[tokio::test(start_paused = true)]
async fn closing_a_running_tab_says_its_statement_will_be_cancelled_and_cancels_before_closing_its_session() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    let two = h.app.active_tab();
    h.fake.script_run(Scripted::ok(rows_of(&[&["late"]])).after(Duration::from_secs(60)).deaf());
    h.fake.script_close_session(Scripted::ok(()));
    h.type_text("select pg_sleep(60)");
    h.press("f5");
    h.settle().await;
    let run = model_tab(&h, two).run.in_flight().expect("running").0;
    let session = model_tab(&h, two).session();
    h.press("ctrl+g T");
    let shown = screen(&mut h);
    assert!(shown.contains("tab 2") && shown.contains("will be cancelled"), "{shown}");
    let commands = sessions(&h.press_fx("enter"));
    h.settle().await;
    assert_eq!(commands, vec![Command::Cancel(run), Command::CloseSession(session, dbp("local pg", "pagila"))]);
    let closed = |h: &Harness| h.fake.calls().contains(&crate::core::Call::CloseSession(session, dbp("local pg", "pagila")));
    advance(&mut h, 1_900).await;
    assert!(!closed(&h), "the shell waits for the run first");
    advance(&mut h, 200).await;
    assert!(closed(&h), "then closes the session, the run having had 2 s");
    assert_eq!(h.app.abandoned_on(), vec!["local pg / pagila".to_string()], "quit still names it");
}

#[tokio::test(start_paused = true)]
async fn the_close_question_stops_promising_a_cancel_once_the_run_has_ended() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    h.fake.script_run(Scripted::ok(rows_of(&[&["done"]])).after(Duration::from_secs(5)));
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+g T");
    assert!(screen(&mut h).contains("will be cancelled"));
    advance(&mut h, 6_000).await;
    assert!(h.app.model.tab(h.app.active_tab()).unwrap().run.in_flight().is_none(), "the run landed");
    let shown = screen(&mut h);
    assert!(!shown.contains("will be cancelled") && shown.contains("has ended"), "{shown}");
}

#[tokio::test]
async fn when_the_active_tab_alone_is_wider_than_the_row_its_cross_still_sits_inside_it() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    h.fake.script_run(Scripted::ok(rows_of(&[&["x"]])).after(Duration::from_secs(60)).deaf());
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    let mut narrow = 0;
    for width in 30u16..60 {
        let rows = frame_rows(&mut h.app, width, 24);
        let y = h.app.ui.hits.editor.y as usize;
        let bar = &rows[y];
        let editor = h.app.ui.hits.editor;
        if editor.width < 8 {
            continue;
        }
        narrow += usize::from(editor.width < 24);
        let close = h.app.ui.tab_hits.iter().find(|t| t.close).unwrap_or_else(|| panic!("{width}: no cross: {bar:?}"));
        let edge = editor.x + editor.width;
        assert!(close.rect.x + close.rect.width <= edge, "{width}: cross {:?} past {edge}", close.rect);
        let cell: String = bar.chars().skip(close.rect.x as usize).take(1).collect();
        assert_eq!(cell, "×", "{width}: {bar:?}");
        assert!(bar.contains('●'), "{width}: the glyph shows: {bar:?}");
    }
    assert!(narrow > 0, "some editor was narrower than the tab");
}

#[tokio::test]
async fn closing_the_only_tab_leaves_one_empty_tab_on_the_same_target() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    h.press("ctrl+g T enter");
    assert_eq!(h.app.model.tabs.len(), 1);
    let only = h.app.active_tab();
    assert_ne!(only, TabId(1), "a new tab");
    assert_eq!(h.app.ui.tab.editor.text(), "");
    assert_eq!(model_tab(&h, only).target, Some(dbp("local pg", "pagila")));
}

#[tokio::test]
async fn a_click_on_a_tab_activates_it_and_a_click_on_the_active_tabs_cross_closes_it_by_the_same_rules() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    let [one, two] = ids(&h)[..] else { panic!() };
    h.type_text("select 2");
    let at = tab_hit(&mut h, one, false);
    click_on(&mut h, at);
    assert_eq!(h.app.active_tab(), one);
    let at = tab_hit(&mut h, two, false);
    click_on(&mut h, at);
    assert_eq!(h.app.active_tab(), two);
    frame_rows(&mut h.app, 114, 30);
    assert!(h.app.ui.tab_hits.iter().all(|t| !(t.tab == one && t.close)), "only the active tab has a cross");
    let at = tab_hit(&mut h, two, true);
    let commands = sessions(&click_on(&mut h, at));
    assert!(commands.is_empty() && matches!(h.app.ui.overlay, Some(overlay::Overlay::ConfirmCloseTab(_))), "tab 2 has text: asked first");
    h.press("enter");
    assert_eq!(ids(&h), vec![one]);
    // An empty tab closes at once.
    h.press("ctrl+g t");
    let three = h.app.active_tab();
    let at = tab_hit(&mut h, three, true);
    let commands = sessions(&click_on(&mut h, at));
    assert!(matches!(commands.as_slice(), [Command::CloseSession(..)]), "{commands:?}");
    assert_eq!(ids(&h), vec![one]);
}

#[tokio::test]
async fn with_twelve_tabs_at_80_columns_the_active_tab_shows_dots_mark_the_hidden_and_the_bar_is_one_row() {
    let mut h = on_pagila().await;
    for _ in 0..11 {
        h.press("ctrl+g t");
    }
    assert_eq!(h.app.model.tabs.len(), 12);
    for go in ["", "ctrl+g 1", "ctrl+g 6"] {
        h.press(go);
        let n = h.app.model.tabs.iter().position(|t| t.id == h.app.active_tab()).unwrap() + 1;
        let rows = frame_rows(&mut h.app, 80, 24);
        let y = h.app.ui.hits.editor.y as usize;
        let bar = &rows[y];
        assert!(bar.contains(&format!("{n} local pg / pagila")), "tab {n} shows: {bar:?}");
        assert!(bar.contains('…'), "hidden tabs are marked: {bar:?}");
        assert!(!rows[y + 1].contains("local pg /"), "nothing wraps: {:?}", rows[y + 1]);
        let drawn = h.app.ui.tab_hits.iter().filter(|t| !t.close).count();
        assert!((1..12).contains(&drawn), "{drawn}");
        assert!(h.app.ui.tab_hits.iter().all(|t| t.rect.height == 1 && t.rect.y as usize == y));
    }
}

// ---- layer 3 --------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_slow_run_in_tab_1_keeps_its_yellow_glyph_while_tab_2_runs_and_its_reply_repaints_without_touching_tab_2() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(rows_of(&[&["a"]])).after(Duration::from_millis(3_500)));
    h.fake.script_run(Scripted::ok(rows_of(&[&["b"]])).after(Duration::from_millis(100)));
    h.type_text("select pg_sleep(3), 'a'");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+g t");
    h.type_text("select 'b'");
    h.press("f5");
    h.settle().await;
    advance(&mut h, 200).await;
    let two = h.app.active_tab();
    assert!(matches!(&model_tab(&h, two).outcome, Some(Outcome::Rows(set)) if set.rows.len() == 1), "tab 2's result shows");
    assert!(results_rows(&mut h).join("\n").contains('b'));
    let glyph = |h: &mut Harness| {
        let (x, y) = find_text(&mut h.app, 114, 30, "1 local pg / pagila").unwrap();
        let x = x + "1 local pg / pagila ".len() as u16;
        let rows = frame_rows(&mut h.app, 114, 30);
        let ch = rows[y as usize].chars().nth(x as usize);
        (ch, role_at(&mut h.app, x, y).fg)
    };
    assert_eq!(glyph(&mut h), (Some('●'), Some(Role::Yellow)), "tab 1 is running");
    let grid = model_tab(&h, two).outcome.clone();
    // Tab 1's clock is not on screen: let tab 2's state settle, then count.
    advance(&mut h, 2_000).await;
    let repaints = h.repaints;
    advance(&mut h, 1_500).await;
    assert_eq!(model_tab(&h, TabId(1)).run, RunState::Idle, "tab 1's reply arrived");
    assert!(h.repaints > repaints, "and repainted: its glyph changed");
    assert_ne!(glyph(&mut h).0, Some('●'), "tab 1 is done: no glyph");
    assert_eq!(model_tab(&h, two).outcome, grid, "tab 2's grid is unchanged");
    assert_eq!(h.app.active_tab(), two);
}

#[tokio::test(start_paused = true)]
async fn a_tab_closed_while_running_drops_its_late_reply_and_nothing_repaints() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    // Deaf: the cancel does not end it; the session's close (after 2 s) does.
    h.fake.script_run(Scripted::ok(rows_of(&[&["late"]])).after(Duration::from_secs(5)).deaf());
    h.fake.script_close_session(Scripted::ok(()));
    h.type_text("select pg_sleep(5)");
    h.press("f5");
    h.settle().await;
    h.press("ctrl+g T enter");
    assert_eq!(ids(&h), vec![TabId(1)]);
    advance(&mut h, 1_000).await;
    assert_eq!(h.app.abandoned_on().len(), 1, "the closed tab's run has not replied yet");
    // The FakeCore has no catalogue: its completion notice, expiring on its
    // own, would repaint whatever event came next.
    h.app.ui.notice = None;
    let (repaints, tab) = (h.repaints, model_tab(&h, TabId(1)).clone());
    advance(&mut h, 5_000).await;
    assert_eq!(h.repaints, repaints, "the late reply repainted nothing");
    assert_eq!((model_tab(&h, TabId(1)).run.clone(), model_tab(&h, TabId(1)).outcome.clone()), (tab.run, tab.outcome));
    assert!(h.app.abandoned_on().is_empty(), "and it says the run ended");
}

#[tokio::test]
async fn with_another_pane_zoomed_the_bar_is_not_drawn_and_no_tab_is_clickable() {
    let mut h = on_pagila().await;
    h.press("ctrl+g t");
    frame_rows(&mut h.app, 114, 30);
    assert!(!h.app.ui.tab_hits.is_empty());
    h.press("ctrl+g j ctrl+g z");
    frame_rows(&mut h.app, 114, 30);
    assert!(h.app.ui.tab_hits.is_empty(), "{:?}", h.app.ui.tab_hits);
}
