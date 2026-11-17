//! Layer 2: the acceptance tests of slice 02 (store and connections), run on
//! `App` with a `FakeCore` behind the shell's command runner.

use crate::core::{Connection, ConnectionId, DbKind, FakeCore, Scripted};
use crate::harness::Harness;

use super::testing::*;

pub fn conn(name: &str, kind: DbKind) -> Connection {
    Connection {
        id: ConnectionId(format!("id-{name}")),
        name: name.into(),
        kind,
        host: "localhost".into(),
        port: Some(5432),
        database: None,
        username: "postgres".into(),
        tls: false,
        production: false,
    }
}

/// The sidebar's text, one string per screen row, without the separator.
pub(super) fn sidebar_rows(h: &mut Harness, w: u16, rows: u16) -> Vec<String> {
    let screen = frame_rows(&mut h.app, w, rows);
    let r = h.app.ui.hits.sidebar;
    screen
        .iter()
        .skip(r.y as usize)
        .take(r.height as usize)
        .map(|row| row.chars().skip(r.x as usize).take(r.width as usize).collect())
        .collect()
}

#[tokio::test]
async fn three_connections_from_the_core_render_as_three_rows_with_name_and_kind() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![
        conn("local pg", DbKind::Postgres),
        conn("shop", DbKind::MySql),
        conn("notes", DbKind::Sqlite),
    ]));
    let mut h = Harness::start(fake);
    h.settle().await;

    let rows = sidebar_rows(&mut h, 100, 30);
    let listed: Vec<&String> = rows.iter().filter(|r| r.contains('○')).collect();
    assert_eq!(listed.len(), 3, "{rows:#?}");
    for (row, (name, kind)) in listed.iter().zip([("local pg", "postgres"), ("shop", "mysql"), ("notes", "sqlite")]) {
        assert!(row.contains(name) && row.contains(kind), "{name} / {kind} in {row:?}");
        assert!(row.find(name) < row.find(kind), "name before kind: {row:?}");
    }
}

#[tokio::test]
async fn zero_connections_render_the_empty_state_saying_what_to_do_and_naming_the_key_that_does_it() {
    let mut h = Harness::start(FakeCore::empty());
    h.settle().await;
    let text = sidebar_rows(&mut h, 100, 30).join("\n");
    assert!(text.contains("No connections yet."), "{text}");
    // Adding a connection exists now (slice 03), so the sidebar names its key,
    // read from the live bindings (design sections 12 and 13).
    assert!(text.contains("Add one with ctrl+g n"), "{text}");
    assert!(!text.contains("dbx app"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn a_slow_reply_shows_the_loading_state_until_it_arrives() {
    use std::time::Duration;
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]).after(Duration::from_secs(5)));
    let mut h = Harness::start(fake);

    // At the start and just before the reply is due: loading, no rows.
    for step in [Duration::ZERO, Duration::from_millis(4_900)] {
        tokio::time::advance(step).await;
        h.settle().await;
        let text = sidebar_rows(&mut h, 100, 30).join("\n");
        assert!(text.contains("loading…"), "still loading: {text}");
        assert!(!text.contains("local pg"), "{text}");
    }
    tokio::time::advance(Duration::from_millis(100)).await;
    h.settle().await;
    let text = sidebar_rows(&mut h, 100, 30).join("\n");
    assert!(text.contains("local pg") && !text.contains("loading"), "{text}");
}

fn key_error_app() -> Harness {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::err(
        crate::core::CoreErrorKind::Key,
        "dbx-tui cannot read the key for this store (KEYRING_ACCESS_FAILED). Unlock your login keychain, then retry.",
    ));
    Harness::start(fake)
}

#[tokio::test]
async fn a_key_error_at_startup_opens_the_error_overlay_and_no_pane_takes_input() {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut h = key_error_app();
    h.settle().await;
    let screen = frame_rows(&mut h.app, 100, 30).join("\n");
    assert!(screen.contains("KEYRING_ACCESS_FAILED"), "dbx's message is shown: {screen}");
    assert!(screen.contains("Unlock your login keychain"), "and what to do: {screen}");

    // Keys and clicks that would move focus, enter prefix mode or move the
    // sidebar cursor change nothing behind the overlay.
    let before = (h.app.ui.focus, h.app.ui.mode, h.app.ui.zoomed);
    for keys in ["ctrl+g l", "ctrl+g z", "j", "down", "ctrl+g"] {
        h.press(keys);
        assert_eq!((h.app.ui.focus, h.app.ui.mode, h.app.ui.zoomed), before, "after {keys}");
    }
    let editor = h.app.ui.hits.editor;
    let at = |kind| MouseEvent { kind, column: editor.x + 2, row: editor.y + 2, modifiers: KeyModifiers::NONE };
    h.app.update(super::Event::Mouse(at(MouseEventKind::Down(MouseButton::Left))), base());
    assert_eq!(h.app.ui.focus, before.0, "a click behind the overlay does not focus a pane");
    assert!(h.fake.calls().len() == 1, "nothing was asked of the Core: {:?}", h.fake.calls());
    assert!(frame_rows(&mut h.app, 100, 30).join("\n").contains("KEYRING_ACCESS_FAILED"), "still open");
}

#[tokio::test]
async fn the_error_overlay_is_a_panel_bg_box_with_an_accent_border_over_a_dimmed_screen() {
    use ratatui::style::Modifier;
    use super::theme::Role;

    let mut h = key_error_app();
    h.settle().await;
    let rows = frame_rows(&mut h.app, 100, 30);
    let top = rows.iter().position(|r| r.contains("┌")).expect("a box");
    let left = rows[top].chars().position(|c| c == '┌').unwrap() as u16;
    let top = top as u16;
    assert_eq!(role_at(&mut h.app, left, top).fg, Some(Role::Accent), "accent border");
    assert_eq!(role_at(&mut h.app, left + 1, top + 1).bg, Some(Role::PanelBg), "panel_bg inside");
    let title_at = rows[top as usize].find("Store key unavailable").expect("title on the top edge");
    let (tx, ty) = (rows[top as usize][..title_at].chars().count() as u16, top);
    assert_eq!(role_at(&mut h.app, tx, ty).fg, Some(Role::Red), "the title says it failed");

    // Buttons carry their key: key in accent, label in overlay0.
    let (bx, by) = find_text(&mut h.app, 100, 30, "↵ retry").expect("retry button");
    assert_eq!(role_at(&mut h.app, bx, by).fg, Some(Role::Accent));
    assert_eq!(role_at(&mut h.app, bx + 2, by).fg, Some(Role::Overlay0));
    assert!(find_text(&mut h.app, 100, 30, "q quit").is_some());

    // Everything behind is dimmed; the box itself is not.
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    let buf = term.backend().buffer();
    let (sx, sy) = (1, 0); // the sidebar title
    assert!(buf[(sx, sy)].modifier.contains(Modifier::DIM), "behind is dimmed");
    assert!(!buf[(bx, by)].modifier.contains(Modifier::DIM), "the box is not");
}

#[tokio::test]
async fn retry_by_key_or_by_click_asks_the_core_again_and_closes_the_overlay() {
    for how in ["key", "click"] {
        let mut h = key_error_app();
        h.fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]));
        h.settle().await;
        frame_rows(&mut h.app, 100, 30);
        if how == "key" {
            h.press("enter");
        } else {
            let button = h.app.ui.overlay_buttons[0].rect;
            let fx = click(&mut h.app, button);
            h.run(fx);
        }
        assert!(h.app.ui.overlay.is_none(), "{how}: closed");
        h.settle().await;
        assert_eq!(h.fake.calls().len(), 2, "{how}: asked again");
        let text = sidebar_rows(&mut h, 100, 30).join("\n");
        assert!(text.contains("local pg"), "{how}: {text}");
    }
}

#[tokio::test]
async fn q_in_the_error_overlay_quits_and_the_prefix_does_not_reach_behind_it() {
    let mut h = key_error_app();
    h.settle().await;
    let focus = h.app.ui.focus;
    h.press("ctrl+g");
    assert_eq!(h.app.ui.mode, super::Mode::Normal, "the prefix is not taken while the overlay is open");
    assert_eq!(h.app.next_deadline(), None);
    h.press("l");
    assert_eq!(h.app.ui.focus, focus, "prefix then l moves no focus");
    assert!(!h.quit);
    h.press("q");
    assert!(h.quit, "q is the overlay's quit button");
}

#[tokio::test]
async fn a_store_error_opens_the_overlay_but_other_failures_stay_in_the_sidebar() {
    use crate::core::CoreErrorKind;
    for (kind, overlay) in [(CoreErrorKind::Store, true), (CoreErrorKind::Other, false), (CoreErrorKind::Timeout, false)] {
        let fake = FakeCore::new();
        fake.script_connections(Scripted::err(kind, "it failed"));
        let mut h = Harness::start(fake);
        h.settle().await;
        assert_eq!(h.app.ui.overlay.is_some(), overlay, "{kind:?}");
        assert!(frame_rows(&mut h.app, 100, 30).join("\n").contains("it failed"), "{kind:?}");
    }
}

pub(super) async fn listed(names: &[&str]) -> Harness {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(names.iter().map(|n| conn(n, DbKind::Postgres)).collect()));
    let mut h = Harness::start(fake);
    h.settle().await;
    h
}

/// The screen row of the sidebar's cursor (the row drawn on `selection_bg`).
pub(super) fn cursor_row(h: &mut Harness) -> Option<String> {
    use super::theme::Role;
    let rows = frame_rows(&mut h.app, 100, 30);
    let r = h.app.ui.hits.sidebar;
    (r.y..r.bottom())
        .find(|&y| role_at(&mut h.app, r.x + 1, y).bg == Some(Role::SelectionBg))
        .map(|y| rows[y as usize].chars().take(r.width as usize).collect())
}

#[tokio::test]
async fn the_cursor_row_is_selection_bg_and_j_k_and_the_arrows_move_it() {
    let mut h = listed(&["alpha", "beta", "gamma"]).await;
    let at = |h: &mut Harness| cursor_row(h).unwrap_or_default();
    assert!(at(&mut h).contains("alpha"), "starts on the first row");
    for (keys, want) in [("j", "beta"), ("down", "gamma"), ("j", "gamma"), ("k", "beta"), ("up", "alpha"), ("k", "alpha")] {
        h.press(keys);
        assert!(at(&mut h).contains(want), "after {keys}: {:?}", at(&mut h));
    }
}

#[tokio::test]
async fn movement_keys_reach_the_sidebar_only_when_it_has_focus() {
    let mut h = listed(&["alpha", "beta"]).await;
    h.press("ctrl+g l j");
    h.press("ctrl+g h");
    assert!(cursor_row(&mut h).unwrap_or_default().contains("alpha"));
}

#[tokio::test]
async fn a_list_longer_than_the_pane_scrolls_to_keep_the_cursor_on_screen() {
    let names: Vec<String> = (0..40).map(|i| format!("conn-{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut h = listed(&refs).await;
    for _ in 0..30 {
        h.press("j");
    }
    let row = cursor_row(&mut h).expect("the cursor row is on screen");
    assert!(row.contains("conn-30"), "{row:?}");
    let text = sidebar_rows(&mut h, 100, 30).join("\n");
    assert!(!text.contains("conn-00"), "scrolled past the top: {text}");
    for _ in 0..30 {
        h.press("k");
    }
    assert!(cursor_row(&mut h).expect("on screen").contains("conn-00"));
}

#[tokio::test]
async fn a_shorter_list_after_a_reload_keeps_the_cursor_on_a_row() {
    let mut h = listed(&["alpha", "beta", "gamma"]).await;
    h.press("j j");
    h.fake.script_connections(Scripted::ok(vec![conn("only", DbKind::Sqlite)]));
    let fx = h.app.start();
    h.run(fx);
    h.settle().await;
    assert!(cursor_row(&mut h).unwrap_or_default().contains("only"));
}

#[tokio::test]
async fn the_mode_bar_shows_the_sidebar_movement_keys_only_where_they_work() {
    let mut h = listed(&["alpha", "beta"]).await;
    let bar = |h: &mut Harness| frame_rows(&mut h.app, 100, 30)[29].clone();
    assert!(bar(&mut h).contains("j/k move"), "{:?}", bar(&mut h));
    h.press("ctrl+g l");
    assert!(!bar(&mut h).contains("move"), "the editor does not move a list: {:?}", bar(&mut h));

    let mut empty = Harness::start(FakeCore::empty());
    empty.settle().await;
    assert!(!bar(&mut empty).contains("move"), "nothing to move through: {:?}", bar(&mut empty));
}

#[tokio::test]
async fn while_the_error_overlay_is_open_the_mode_bar_offers_no_key_it_would_ignore() {
    let mut h = key_error_app();
    h.settle().await;
    let bar = frame_rows(&mut h.app, 100, 30)[29].clone();
    assert!(!bar.contains("ctrl+g"), "the prefix does nothing behind an overlay: {bar:?}");
}

#[tokio::test]
async fn behind_the_overlay_the_sidebar_names_the_failure_briefly() {
    let mut h = key_error_app();
    h.settle().await;
    let text = sidebar_rows(&mut h, 100, 30).join("\n");
    assert!(text.contains("Store key unavailable"), "{text}");
    assert!(!text.contains('…'), "no sentence cut short: {text}");
}

fn mouse(h: &mut Harness, kind: crossterm::event::MouseEventKind, column: u16, row: u16) {
    let ev = crossterm::event::MouseEvent { kind, column, row, modifiers: crossterm::event::KeyModifiers::NONE };
    let fx = h.app.update(super::Event::Mouse(ev), base());
    h.run(fx);
}

#[tokio::test]
async fn clicking_a_connection_row_moves_the_cursor_there_and_focuses_the_sidebar() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut h = listed(&["alpha", "beta", "gamma"]).await;
    h.press("ctrl+g l");
    let (x, y) = find_text(&mut h.app, 100, 30, "gamma").expect("gamma on screen");
    mouse(&mut h, MouseEventKind::Down(MouseButton::Left), x, y);
    assert_eq!(h.app.ui.focus, super::PaneId::Sidebar);
    assert!(cursor_row(&mut h).unwrap_or_default().contains("gamma"));

    // A click below the last row focuses the sidebar but moves no cursor.
    let below = h.app.ui.hits.sidebar.bottom() - 2;
    mouse(&mut h, MouseEventKind::Down(MouseButton::Left), x, below);
    assert!(cursor_row(&mut h).unwrap_or_default().contains("gamma"));
}

#[tokio::test]
async fn the_wheel_over_the_sidebar_scrolls_the_list_even_when_another_pane_has_focus() {
    use crossterm::event::MouseEventKind;
    let names: Vec<String> = (0..40).map(|i| format!("conn-{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut h = listed(&refs).await;
    h.press("ctrl+g l");
    frame_rows(&mut h.app, 100, 30);
    let r = h.app.ui.hits.sidebar;
    for _ in 0..5 {
        mouse(&mut h, MouseEventKind::ScrollDown, r.x + 2, r.y + 5);
    }
    let text = sidebar_rows(&mut h, 100, 30).join("\n");
    assert!(!text.contains("conn-00"), "scrolled down: {text}");
    assert_eq!(h.app.ui.focus, super::PaneId::Editor, "the wheel does not take focus");
    assert!(cursor_row(&mut h).is_some(), "the cursor stays on a visible row");
    for _ in 0..10 {
        mouse(&mut h, MouseEventKind::ScrollUp, r.x + 2, r.y + 5);
    }
    assert!(sidebar_rows(&mut h, 100, 30).join("\n").contains("conn-00"), "back at the top");
}

#[tokio::test(start_paused = true)]
async fn while_the_first_core_call_is_pending_the_prefix_times_out_and_quit_works() {
    use std::time::Duration;
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![]).after(Duration::from_secs(600)));
    let mut h = Harness::start(fake);
    h.settle().await;
    press_at(&mut h.app, "ctrl+g", base());
    let deadline = h.app.next_deadline().expect("prefix deadline");
    h.app.update(super::Event::Timer, deadline);
    assert_eq!(h.app.ui.mode, super::Mode::Normal, "the prefix expired");
    assert!(matches!(h.app.model.connections, super::Loadable::Loading), "still waiting on the Core");
    h.press("ctrl+g q");
    assert!(h.quit);
}
