//! Layer 2: the acceptance tests of slice 09 (mouse and resizing), run on
//! `App` with a `FakeCore` behind the shell's command runner. Clicks on rows,
//! cells and hints, and the wheel over the results, have their tests beside
//! the features (`tree_tests`, `grid_tests`, `connections_tests`); this file
//! has what slice 09 added: dividers, the resize mode, the preferences, the
//! editor's mouse, scrollbars, double-clicks and the overlay buttons.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::connections_tests::listed;
use super::grid_tests::{focused, text_set, H, W};
use super::layout::{HitMap, LayoutPrefs, EDITOR_LEAST, RESULTS_LEAST, RIGHT_LEAST, SIDEBAR_LEAST};
use super::overlay::{ConfirmDropSession, ConfirmRemove, ConfirmRerun, ErrorOverlay, FailureOverlay, Overlay};
use super::pointer::DOUBLE_CLICK;
use super::run_tests::{cid, mode_bar, started};
use super::testing::*;
use super::*;
use crate::core::Cursor;
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

fn send(h: &mut Harness, kind: MouseEventKind, x: u16, y: u16, mods: KeyModifiers, at: Instant) -> Effects {
    let fx = h.app.update(Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers: mods }), at);
    let seen = Effects { repaint: fx.repaint, commands: fx.commands.clone() };
    h.run(fx);
    seen
}

fn press_at(h: &mut Harness, x: u16, y: u16, at: Instant) -> Effects {
    send(h, MouseEventKind::Down(MouseButton::Left), x, y, KeyModifiers::NONE, at)
}

fn drag_to(h: &mut Harness, x: u16, y: u16) -> Effects {
    let now = h.now();
    send(h, MouseEventKind::Drag(MouseButton::Left), x, y, KeyModifiers::NONE, now)
}

fn release(h: &mut Harness, x: u16, y: u16) -> Effects {
    let now = h.now();
    send(h, MouseEventKind::Up(MouseButton::Left), x, y, KeyModifiers::NONE, now)
}

fn wheel(h: &mut Harness, kind: MouseEventKind, x: u16, y: u16, mods: KeyModifiers) -> Effects {
    let now = h.now();
    send(h, kind, x, y, mods, now)
}

/// An instant later than any this function gave before, by seconds: a press
/// made at it is never the second of a double-click with an earlier one.
fn later() -> Instant {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1000);
    base() + Duration::from_secs(N.fetch_add(10, Ordering::Relaxed))
}

/// A whole drag: press, move, release. The commands it asked for, all together.
fn drag(h: &mut Harness, from: (u16, u16), to: (u16, u16)) -> Vec<Command> {
    let mut all = press_at(h, from.0, from.1, later()).commands;
    all.extend(drag_to(h, to.0, to.1).commands);
    all.extend(release(h, to.0, to.1).commands);
    all
}

fn draw(h: &mut Harness) {
    frame_rows(&mut h.app, W, H);
}

/// The sidebar and its separator: what the README calls its width.
fn sidebar_total(h: &Harness) -> u16 {
    h.app.ui.hits.sidebar.width + h.app.ui.hits.sidebar_separator.width
}

fn t0() -> Instant {
    base()
}

async fn plain() -> Harness {
    let mut h = started(&["pg"]).await;
    draw(&mut h);
    h
}

// ---- dividers ------------------------------------------------------------------

#[tokio::test]
async fn dragging_the_sidebar_divider_resizes_the_sidebar_and_stops_at_the_minimum_and_the_maximum() {
    let mut h = plain().await;
    assert_eq!(sidebar_total(&h), 30, "about 30% of 100 columns");
    let sep = h.app.ui.hits.sidebar_separator;
    let sent = drag(&mut h, (sep.x, 5), (44, 5));
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 45, "the divider follows the pointer: the sidebar is 45 wide, separator included");
    assert_eq!(h.app.ui.hits.sidebar_separator.x, 44);
    assert_eq!(sent, vec![Command::SavePrefs(h.app.ui.layout)], "released: the layout is sent to be saved, once");
    assert_eq!(h.app.ui.layout.sidebar, Some(450));

    drag(&mut h, (44, 5), (2, 5));
    draw(&mut h);
    assert_eq!(sidebar_total(&h), SIDEBAR_LEAST, "dragged to column 2: it stops at the minimum");
    let sep = h.app.ui.hits.sidebar_separator;
    drag(&mut h, (sep.x, 5), (99, 5));
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 100 - RIGHT_LEAST, "dragged to the far edge: the editor keeps its least width");
    assert_eq!(h.app.ui.hits.editor.width, RIGHT_LEAST);
}

#[tokio::test]
async fn dragging_the_editor_results_divider_moves_it_and_each_pane_keeps_its_least_height() {
    let mut h = plain().await;
    let divider = h.app.ui.hits.divider;
    assert_eq!(divider, ratatui::layout::Rect::new(h.app.ui.hits.results.x, h.app.ui.hits.results.y, h.app.ui.hits.results.width, 1));
    let start = h.app.ui.hits.results.y;
    let sent = drag(&mut h, (divider.x + 10, divider.y), (divider.x + 10, divider.y + 4));
    draw(&mut h);
    assert_eq!(h.app.ui.hits.results.y, start + 4, "the divider row is where the pointer let go");
    assert_eq!(sent, vec![Command::SavePrefs(h.app.ui.layout)]);

    let divider = h.app.ui.hits.divider;
    drag(&mut h, (divider.x + 10, divider.y), (divider.x + 10, 0));
    draw(&mut h);
    assert_eq!(h.app.ui.hits.editor.height, EDITOR_LEAST);
    let divider = h.app.ui.hits.divider;
    drag(&mut h, (divider.x + 10, divider.y), (divider.x + 10, 29));
    draw(&mut h);
    assert_eq!(h.app.ui.hits.results.height, RESULTS_LEAST, "29 rows above the mode bar: the results keep their least");
}

#[tokio::test]
async fn pressing_a_divider_takes_no_focus_and_a_press_that_moves_nothing_changes_and_saves_nothing() {
    let mut h = plain().await;
    h.press("ctrl+g l");
    let sep = h.app.ui.hits.sidebar_separator;
    let fx = press_at(&mut h, sep.x, 6, t0());
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    assert!(!fx.repaint, "nothing changed, nothing is drawn");
    let fx = release(&mut h, sep.x, 6);
    assert!(fx.commands.is_empty() && !fx.repaint, "nothing to save: {:?}", fx.commands);
    assert_eq!(h.app.ui.layout, LayoutPrefs::default());
    // The same for the other divider.
    let d = h.app.ui.hits.divider;
    press_at(&mut h, d.x + 3, d.y, t0() + Duration::from_secs(5));
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    assert!(release(&mut h, d.x + 3, d.y).commands.is_empty());
}

#[tokio::test]
async fn a_double_click_on_a_divider_restores_its_default_and_two_slow_clicks_do_not() {
    let mut h = plain().await;
    let sep = h.app.ui.hits.sidebar_separator;
    drag(&mut h, (sep.x, 5), (50, 5));
    let d = h.app.ui.hits.divider;
    drag(&mut h, (d.x + 4, d.y), (d.x + 4, d.y + 3));
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 51);
    // Too slow: two presses, no reset.
    let sep = h.app.ui.hits.sidebar_separator;
    press_at(&mut h, sep.x, 5, t0());
    press_at(&mut h, sep.x, 5, t0() + DOUBLE_CLICK + Duration::from_millis(1));
    release(&mut h, sep.x, 5);
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 51, "the second press came too late");
    // In time: back to the automatic size, and saved.
    let start = t0() + Duration::from_secs(10);
    press_at(&mut h, sep.x, 5, start);
    let fx = press_at(&mut h, sep.x, 5, start + DOUBLE_CLICK);
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 30, "the default again");
    assert_eq!(h.app.ui.layout.sidebar, None);
    assert_eq!(fx.commands, vec![Command::SavePrefs(LayoutPrefs { sidebar: None, editor: h.app.ui.layout.editor })]);
    release(&mut h, sep.x, 5);
    assert_eq!(h.app.ui.drag, None);
    // The other divider the same way.
    let d = h.app.ui.hits.divider;
    let start = t0() + Duration::from_secs(20);
    press_at(&mut h, d.x + 4, d.y, start);
    press_at(&mut h, d.x + 4, d.y, start + Duration::from_millis(100));
    draw(&mut h);
    assert_eq!(h.app.ui.layout, LayoutPrefs::default());
    assert_eq!(h.app.ui.hits.results.y, 17, "three fifths of 29 rows");
}

#[tokio::test]
async fn a_zoomed_pane_has_no_dividers_to_drag() {
    let mut h = plain().await;
    h.press("ctrl+g z");
    draw(&mut h);
    assert!(h.app.ui.hits.divider.is_empty() && h.app.ui.hits.sidebar_separator.is_empty());
    let before = h.app.ui.layout;
    drag(&mut h, (29, 5), (60, 5));
    assert_eq!(h.app.ui.layout, before);
}

// ---- the resize mode -------------------------------------------------------------

#[tokio::test]
async fn prefix_r_then_l_three_times_widens_the_sidebar_by_three_columns_and_esc_leaves() {
    let mut h = plain().await;
    assert!(mode_bar(&mut h).contains("SIDEBAR"), "{}", mode_bar(&mut h));
    let bar = {
        h.press("ctrl+g");
        mode_bar(&mut h)
    };
    assert!(bar.contains("R resize"), "the prefix names the resize key: {bar}");
    h.press("R");
    assert_eq!(h.app.ui.mode, Mode::Resize);
    let bar = mode_bar(&mut h);
    assert!(bar.contains("RESIZE") && bar.contains("h/l sidebar") && bar.contains("j/k split") && bar.contains("esc done"), "{bar}");
    h.press("l l l");
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 33, "three columns wider");
    h.press("right");
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 34);
    h.press("h left h");
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 31);
    // Keys that are not the mode's do nothing, `q` included.
    h.press("q x ctrl+g");
    assert_eq!(h.app.ui.mode, Mode::Resize);
    assert!(!h.quit);
    let sent = h.press_fx("esc");
    assert_eq!(h.app.ui.mode, Mode::Normal);
    let bar = mode_bar(&mut h);
    assert!(bar.contains("SIDEBAR") && !bar.contains("RESIZE") && !bar.contains("esc done"), "the mode bar is back to normal: {bar}");
    assert_eq!(sent, vec![Command::SavePrefs(h.app.ui.layout)], "leaving saves the layout");
}

#[tokio::test]
async fn the_resize_mode_moves_the_editor_divider_with_j_and_k_and_enter_leaves() {
    let mut h = plain().await;
    let start = h.app.ui.hits.results.y;
    h.press("ctrl+g R j j j");
    draw(&mut h);
    assert_eq!(h.app.ui.hits.results.y, start + 3, "j moves the divider down");
    h.press("k up");
    draw(&mut h);
    assert_eq!(h.app.ui.hits.results.y, start + 1);
    h.press("down");
    draw(&mut h);
    assert_eq!(h.app.ui.hits.results.y, start + 2);
    for _ in 0..40 {
        h.press("k");
    }
    draw(&mut h);
    assert_eq!(h.app.ui.hits.editor.height, EDITOR_LEAST, "k stops at the editor's least height");
    let sent = h.press_fx("enter");
    assert_eq!(h.app.ui.mode, Mode::Normal);
    assert!(matches!(sent.as_slice(), [Command::SavePrefs(_)]));
    // And with nothing changed, leaving saves nothing.
    let mut h = plain().await;
    h.press("ctrl+g R");
    assert!(h.press_fx("esc").is_empty());
}

#[tokio::test]
async fn the_resize_hints_are_clickable_and_a_resize_works_on_any_pane_focus() {
    let mut h = plain().await;
    h.press("ctrl+g R");
    let bar = mode_bar(&mut h);
    let at = |text: &str| bar[..bar.find(text).unwrap()].chars().count() as u16;
    let y = h.app.ui.hits.mode_bar.y;
    // `h/l sidebar`: the `l` key, then the `h` key, then the words (the first key: `h`).
    press_at(&mut h, at("h/l sidebar") + 2, y, t0());
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 31, "a click on `l` is `l`");
    press_at(&mut h, at("h/l sidebar") + 5, y, t0() + Duration::from_secs(2));
    draw(&mut h);
    assert_eq!(sidebar_total(&h), 30, "a click on the words does what `h` does");
    press_at(&mut h, at("esc done"), y, t0() + Duration::from_secs(4));
    assert_eq!(h.app.ui.mode, Mode::Normal, "the `esc` hint leaves the mode");
}

#[tokio::test]
async fn quitting_with_a_layout_not_yet_saved_sends_it_before_the_quit() {
    let mut h = plain().await;
    h.press("ctrl+g R l l");
    let sent = h.press_fx("ctrl+g");
    assert!(sent.is_empty());
    // Still in the mode, the prefix key does nothing; a signal quits.
    let fx = h.app.quit_requested();
    assert!(matches!(fx.commands.as_slice(), [.., Command::SavePrefs(p), Command::Quit] if p.sidebar == Some(320)), "{:?}", fx.commands);
}

// ---- preferences ---------------------------------------------------------------

#[tokio::test]
async fn the_layout_written_on_quit_is_read_back_on_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = crate::shell::prefs::path_in(dir.path());
    let mut h = plain().await;
    h.tasks = h.tasks.clone().with_prefs(Some(path.clone()));
    assert!(!path.exists());
    h.press("ctrl+g R l l l l l");
    assert!(!path.exists(), "not written until the mode is left or the app quits");
    let fx = h.app.quit_requested();
    h.run(fx);
    assert!(h.quit);
    assert!(path.exists(), "written on quit");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("sidebar_permille") && text.contains("350"), "{text}");

    let mut next = App::new();
    next.set_layout(crate::shell::prefs::load(&path));
    assert_eq!(next.ui.layout, LayoutPrefs { sidebar: Some(350), editor: None });
    frame_rows(&mut next, W, H);
    assert_eq!(next.ui.hits.sidebar.width + next.ui.hits.sidebar_separator.width, 35, "the next run starts as this one ended");
    // A different terminal size keeps the proportion.
    frame_rows(&mut next, 200, H);
    assert_eq!(next.ui.hits.sidebar.width + next.ui.hits.sidebar_separator.width, 70);
}

#[tokio::test]
async fn a_corrupt_preferences_file_starts_with_the_defaults_and_shows_no_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = crate::shell::prefs::path_in(dir.path());
    std::fs::write(&path, "{ this is not json").unwrap();
    let mut app = App::new();
    app.set_layout(crate::shell::prefs::load(&path));
    assert_eq!(app.ui.layout, LayoutPrefs::default());
    assert!(app.ui.overlay.is_none());
    // A value that is not a usable proportion is not used even if it is set by hand.
    app.set_layout(LayoutPrefs { sidebar: Some(0), editor: Some(5000) });
    assert_eq!(app.ui.layout, LayoutPrefs::default());
}

#[tokio::test]
async fn without_a_preferences_file_a_save_goes_nowhere_and_a_failed_save_is_silent() {
    let mut h = plain().await;
    h.press("ctrl+g R l esc");
    assert!(h.tasks.is_empty());
    // A directory that does not exist: the write fails, the client goes on.
    let dir = tempfile::tempdir().unwrap();
    h.tasks = h.tasks.clone().with_prefs(Some(dir.path().join("gone").join("p.json")));
    h.press("ctrl+g R l esc ctrl+g q");
    assert!(h.quit);
}

// ---- small terminals ------------------------------------------------------------

#[tokio::test]
async fn at_40_by_10_every_pane_keeps_its_least_size_whatever_the_stored_layout_and_nothing_panics() {
    for (sidebar, editor) in [(None, None), (Some(1), Some(1)), (Some(999), Some(999)), (Some(500), Some(500)), (Some(1), Some(999))] {
        let mut app = ready_app();
        app.set_layout(LayoutPrefs { sidebar, editor });
        frame_rows(&mut app, 40, 10);
        let hits = app.ui.hits;
        let total = hits.sidebar.width + hits.sidebar_separator.width;
        assert!((SIDEBAR_LEAST..=40 - 20).contains(&total), "{sidebar:?}: sidebar {total}");
        assert!(hits.editor.width >= 20, "{hits:?}");
        assert!(hits.editor.height >= EDITOR_LEAST, "{sidebar:?}/{editor:?}: {hits:?}");
        assert!(hits.results.height >= RESULTS_LEAST, "{sidebar:?}/{editor:?}: {hits:?}");
        assert_eq!(hits.editor.height + hits.results.height, 9, "nothing is lost or overlapped");
    }
    // Sweep: no size panics with an extreme layout, zoomed or not, dragging or not.
    for w in 0..=60u16 {
        for h in 0..=14u16 {
            for zoom in [false, true] {
                let mut app = ready_app();
                app.set_layout(LayoutPrefs { sidebar: Some(999), editor: Some(1) });
                app.ui.zoomed = zoom;
                frame_rows(&mut app, w, h);
                let down = |kind, x, y| Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
                for (x, y) in [(0, 0), (w / 2, h / 2), (w.saturating_sub(1), h.saturating_sub(1))] {
                    app.update(down(MouseEventKind::Down(MouseButton::Left), x, y), base());
                    app.update(down(MouseEventKind::Drag(MouseButton::Left), x / 2, y / 2), base());
                    app.update(down(MouseEventKind::Up(MouseButton::Left), x / 2, y / 2), base());
                }
                frame_rows(&mut app, w, h);
            }
        }
    }
}

#[tokio::test]
async fn dragging_at_40_by_10_stays_inside_the_limits() {
    let mut h = started(&["pg"]).await;
    frame_rows(&mut h.app, 40, 10);
    let sep = h.app.ui.hits.sidebar_separator;
    drag(&mut h, (sep.x, 3), (0, 3));
    frame_rows(&mut h.app, 40, 10);
    assert_eq!(h.app.ui.hits.sidebar.width + 1, SIDEBAR_LEAST);
    let d = h.app.ui.hits.divider;
    drag(&mut h, (d.x + 2, d.y), (d.x + 2, 8));
    frame_rows(&mut h.app, 40, 10);
    assert_eq!(h.app.ui.hits.results.height, RESULTS_LEAST);
    let d = h.app.ui.hits.divider;
    drag(&mut h, (d.x + 2, d.y), (d.x + 2, 0));
    frame_rows(&mut h.app, 40, 10);
    assert_eq!(h.app.ui.hits.editor.height, EDITOR_LEAST);
}

#[test]
fn the_proportions_round_trip_to_the_same_column_at_any_realistic_width() {
    for width in 20..=400u16 {
        for total in SIDEBAR_LEAST..=(width.saturating_sub(RIGHT_LEAST)).max(width / 2) {
            let prefs = LayoutPrefs { sidebar: Some(HitMap::sidebar_permille(width, total)), editor: None };
            assert_eq!(HitMap::sidebar_total(width, &prefs), total, "{total} of {width}");
        }
    }
}

#[tokio::test]
async fn a_divider_drag_whose_release_lands_on_an_overlay_is_still_saved() {
    let mut h = plain().await;
    let sep = h.app.ui.hits.sidebar_separator;
    press_at(&mut h, sep.x, 5, later());
    drag_to(&mut h, 44, 5);
    assert!(h.app.ui.layout_unsaved);
    // Something opens an overlay while the button is down.
    h.app.ui.overlay = Some(Overlay::Error(ErrorOverlay { title: "Store unavailable".into(), message: "Could not open it.".into() }));
    let fx = release(&mut h, 44, 5);
    assert_eq!(fx.commands, vec![Command::SavePrefs(h.app.ui.layout)], "the dragged layout is saved");
    assert!(h.app.ui.drag.is_none() && !h.app.ui.layout_unsaved);
}

// ---- overlays ------------------------------------------------------------------

/// A harness with overlay number `which` open, drawn.
async fn with_overlay(which: usize) -> Harness {
    let mut h = started(&["pg"]).await;
    match which {
        0 => h.app.ui.overlay = Some(Overlay::Error(ErrorOverlay { title: "Store unavailable".into(), message: "Could not open it.".into() })),
        1 => h.app.ui.overlay = Some(Overlay::Failure(FailureOverlay { title: "Failed".into(), message: "It did not work.".into() })),
        2 => h.app.ui.overlay = Some(Overlay::ConfirmRemove(ConfirmRemove { id: cid("pg"), name: "pg".into() })),
        3 => h.app.ui.overlay = Some(Overlay::ConfirmDropSession(ConfirmDropSession { target: "pg / edge".into(), stopped: false })),
        4 => h.app.ui.overlay = Some(Overlay::ConfirmDropSession(ConfirmDropSession { target: "pg / edge".into(), stopped: true })),
        5 => h.app.ui.overlay = Some(Overlay::ConfirmRerun(ConfirmRerun { statement: "select f()".into() })),
        6 => {
            h.press("ctrl+g n");
        }
        _ => {
            // The form's own question: discard what was typed?
            h.press("ctrl+g n x esc");
        }
    }
    assert!(h.app.ui.overlay.is_some(), "overlay {which}");
    draw(&mut h);
    h
}

#[tokio::test]
async fn a_click_on_an_overlays_primary_button_does_what_enter_does_for_every_overlay() {
    for which in 0..8 {
        // By key.
        let mut by_key = with_overlay(which).await;
        let primary = by_key.app.ui.overlay.as_ref().unwrap().buttons().iter().find(|b| b.key == "enter").expect("a primary button").action;
        let keyed = by_key.press_fx("enter");
        draw(&mut by_key);
        // By click, on the button drawn for that action.
        let mut by_click = with_overlay(which).await;
        let rect = by_click.app.ui.overlay_buttons.iter().find(|b| b.action == primary).unwrap_or_else(|| panic!("overlay {which}: no drawn button for {primary:?}")).rect;
        let fx = click(&mut by_click.app, rect);
        by_click.run(Effects { repaint: false, commands: fx.commands.clone() });
        draw(&mut by_click);
        assert_eq!(fx.commands, keyed, "overlay {which} ({primary:?}): the same commands");
        assert_eq!(
            format!("{:?}", by_click.app.ui.overlay),
            format!("{:?}", by_key.app.ui.overlay),
            "overlay {which} ({primary:?}): the same overlay after"
        );
        assert_eq!(by_click.quit, by_key.quit, "overlay {which}");
    }
}

#[tokio::test]
async fn a_click_a_drag_or_the_wheel_outside_an_open_overlay_reaches_nothing_behind_it() {
    let names: Vec<String> = (0..40).map(|i| format!("conn-{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut h = listed(&refs).await;
    h.press("ctrl+g l");
    h.app.ui.overlay = Some(Overlay::Failure(FailureOverlay { title: "Failed".into(), message: "Nothing to see.".into() }));
    draw(&mut h);
    let before = (h.app.ui.focus, h.app.ui.sidebar.cursor, h.app.ui.sidebar.scroll, h.app.ui.layout, h.app.ui.editor.cursor());
    let list = h.app.ui.hits.sidebar_list;
    let sep = h.app.ui.hits.sidebar_separator;
    let d = h.app.ui.hits.divider;
    let editor = h.app.ui.hits.editor_text;
    let mut asked = Vec::new();
    for (x, y) in [(list.x + 3, list.y + 3), (editor.x + 2, editor.y + 1), (d.x + 5, d.y + 3), (1, 1)] {
        asked.extend(press_at(&mut h, x, y, t0()).commands);
        asked.extend(release(&mut h, x, y).commands);
        asked.extend(wheel(&mut h, MouseEventKind::ScrollDown, x, y, KeyModifiers::NONE).commands);
    }
    asked.extend(drag(&mut h, (sep.x, 5), (50, 5)));
    asked.extend(drag(&mut h, (d.x + 5, d.y), (d.x + 5, d.y + 4)));
    let after = (h.app.ui.focus, h.app.ui.sidebar.cursor, h.app.ui.sidebar.scroll, h.app.ui.layout, h.app.ui.editor.cursor());
    assert_eq!(before, after, "nothing behind the overlay moved");
    assert!(asked.is_empty(), "no command: {asked:?}");
    assert!(h.app.ui.overlay.is_some(), "and the overlay is still open");
    assert_eq!(h.app.ui.drag, None);
}

#[tokio::test]
async fn a_drag_begun_before_an_overlay_opened_ends_when_the_button_goes_up_over_it() {
    let mut h = plain().await;
    let sep = h.app.ui.hits.sidebar_separator;
    press_at(&mut h, sep.x, 5, t0());
    assert!(h.app.ui.drag.is_some());
    h.app.ui.overlay = Some(Overlay::Failure(FailureOverlay { title: "Failed".into(), message: "x".into() }));
    release(&mut h, sep.x, 5);
    assert_eq!(h.app.ui.drag, None);
}

// ---- the editor --------------------------------------------------------------------

async fn editor_with(text: &str) -> Harness {
    let mut h = plain().await;
    h.app.ui.editor.set_text(text);
    draw(&mut h);
    h
}

#[tokio::test]
async fn a_click_in_the_editor_focuses_it_and_puts_the_caret_where_the_click_landed() {
    let mut h = editor_with("hello world\nselect 1").await;
    let text = h.app.ui.hits.editor_text;
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    press_at(&mut h, text.x + 3, text.y, t0());
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 3 });
    press_at(&mut h, text.x + 4, text.y + 1, t0() + Duration::from_secs(5));
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 1, col: 4 });
    // Past the end of a line and below the last one: the nearest place there is.
    press_at(&mut h, text.x + 40, text.y, t0() + Duration::from_secs(10));
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 11 });
    press_at(&mut h, text.x + 2, text.y + 9, t0() + Duration::from_secs(15));
    assert_eq!(h.app.ui.editor.cursor().row, 1);
    // What is typed goes where the caret is.
    h.press("X");
    assert!(h.app.ui.editor.text().contains("seXlect"), "{}", h.app.ui.editor.text());
}

#[tokio::test]
async fn the_editor_click_counts_wide_characters_by_their_cells() {
    let mut h = editor_with("日本語 text\n\tx").await;
    let text = h.app.ui.hits.editor_text;
    // 日 takes cells 0-1, 本 cells 2-3, 語 cells 4-5, the space is cell 6.
    for (cell, char_index) in [(0u16, 0usize), (2, 1), (4, 2), (6, 3), (7, 4)] {
        press_at(&mut h, text.x + cell, text.y, t0() + Duration::from_secs(10 * (u64::from(cell) + 1)));
        assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: char_index }, "cell {cell}");
    }
}

#[tokio::test]
async fn the_editor_click_accounts_for_how_far_the_view_is_scrolled() {
    let lines: Vec<String> = (0..80).map(|i| format!("line {i}")).collect();
    let mut h = editor_with(&lines.join("\n")).await;
    let (top, _) = h.app.ui.editor.scroll_top();
    assert!(top > 20, "the caret is at the end, so the view is scrolled: {top}");
    let text = h.app.ui.hits.editor_text;
    press_at(&mut h, text.x + 2, text.y + 2, t0());
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: top + 2, col: 2 }, "the third row drawn is line {}", top + 2);
    // The wheel scrolls the text under the pointer, whichever pane has focus.
    h.press("ctrl+g h");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    draw(&mut h);
    let before = h.app.ui.editor.scroll_top().0;
    let fx = wheel(&mut h, MouseEventKind::ScrollUp, text.x + 2, text.y + 2, KeyModifiers::NONE);
    draw(&mut h);
    assert!(fx.repaint);
    assert_eq!(h.app.ui.editor.scroll_top().0, before - 3, "three lines a notch");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar, "the wheel takes no focus");
}

#[tokio::test]
async fn a_drag_in_the_editor_selects_text_and_a_click_ends_the_selection() {
    let mut h = editor_with("select * from film\nwhere id = 1").await;
    let text = h.app.ui.hits.editor_text;
    let now = h.now();
    press_at(&mut h, text.x + 7, text.y, now);
    assert_eq!(h.app.ui.editor.selection(), None, "a press alone selects nothing");
    let fx = drag_to(&mut h, text.x + 15, text.y);
    assert!(fx.repaint);
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 7 }, Cursor { row: 0, col: 15 })));
    // Backwards, and over two lines.
    drag_to(&mut h, text.x + 2, text.y + 1);
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 7 }, Cursor { row: 1, col: 2 })));
    drag_to(&mut h, text.x + 3, text.y);
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 3 }, Cursor { row: 0, col: 7 })));
    release(&mut h, text.x + 3, text.y);
    assert!(h.app.ui.editor.selection().is_some(), "the selection stays after the button goes up");
    press_at(&mut h, text.x + 1, text.y + 1, t0() + Duration::from_secs(9));
    assert_eq!(h.app.ui.editor.selection(), None, "a click ends it");
    // A press and release on the same place leaves no selection behind.
    let now = h.now() + Duration::from_secs(20);
    press_at(&mut h, text.x + 5, text.y, now);
    drag_to(&mut h, text.x + 5, text.y);
    release(&mut h, text.x + 5, text.y);
    assert_eq!(h.app.ui.editor.selection(), None);
    // Dragging past the bottom of the area keeps selecting (the view follows).
    press_at(&mut h, text.x, text.y, now + Duration::from_secs(5));
    drag_to(&mut h, text.x + 4, text.y + 30);
    assert_eq!(h.app.ui.editor.selection().map(|s| s.1.row), Some(1), "clamped to the last line");
}

#[tokio::test]
async fn a_double_click_in_the_editor_selects_the_word_and_two_slow_clicks_do_not() {
    let mut h = editor_with("select film_id, title from film").await;
    let text = h.app.ui.hits.editor_text;
    press_at(&mut h, text.x + 9, text.y, t0());
    press_at(&mut h, text.x + 9, text.y, t0() + DOUBLE_CLICK);
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 7 }, Cursor { row: 0, col: 14 })), "`film_id`: the underscore is part of the word");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 14 });
    release(&mut h, text.x + 9, text.y);
    // On the comma: the run of symbols.
    press_at(&mut h, text.x + 14, text.y, t0() + Duration::from_secs(5));
    press_at(&mut h, text.x + 14, text.y, t0() + Duration::from_secs(5) + Duration::from_millis(50));
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 14 }, Cursor { row: 0, col: 15 })));
    // Too slow.
    let later = t0() + Duration::from_secs(30);
    press_at(&mut h, text.x + 18, text.y, later);
    press_at(&mut h, text.x + 18, text.y, later + DOUBLE_CLICK + Duration::from_millis(1));
    assert_eq!(h.app.ui.editor.selection(), None);
    // Then a third click, after a double-click, starts over.
    let later = t0() + Duration::from_secs(60);
    press_at(&mut h, text.x + 1, text.y, later);
    press_at(&mut h, text.x + 1, text.y, later + Duration::from_millis(10));
    assert!(h.app.ui.editor.selection().is_some());
    press_at(&mut h, text.x + 1, text.y, later + Duration::from_millis(20));
    assert_eq!(h.app.ui.editor.selection(), None, "a third quick click is a first click");
}

// ---- the wheel -------------------------------------------------------------------

#[tokio::test]
async fn the_wheel_over_the_results_scrolls_them_while_the_editor_keeps_focus() {
    let mut h = focused(text_set(100, 3)).await;
    h.press("ctrl+g k");
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    draw(&mut h);
    let r = h.app.ui.hits.results;
    let fx = wheel(&mut h, MouseEventKind::ScrollDown, r.x + 4, r.y + 3, KeyModifiers::NONE);
    wheel(&mut h, MouseEventKind::ScrollDown, r.x + 4, r.y + 3, KeyModifiers::NONE);
    assert!(fx.repaint);
    assert_eq!(h.app.ui.grid.top, 6, "three rows a notch");
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    // Over the editor it scrolls the editor, not the results.
    let e = h.app.ui.hits.editor_text;
    wheel(&mut h, MouseEventKind::ScrollDown, e.x + 2, e.y + 2, KeyModifiers::NONE);
    assert_eq!(h.app.ui.grid.top, 6);
    // Over the divider, the mode bar, or the sidebar's separator: nothing to scroll, nothing drawn.
    let bar = h.app.ui.hits.mode_bar;
    let sep = h.app.ui.hits.sidebar_separator;
    for (x, y) in [(bar.x + 3, bar.y), (sep.x, sep.y + 4)] {
        let fx = wheel(&mut h, MouseEventKind::ScrollDown, x, y, KeyModifiers::NONE);
        assert!(!fx.repaint && fx.commands.is_empty());
    }
}

#[tokio::test]
async fn shift_wheel_and_a_horizontal_wheel_scroll_the_grid_by_columns_over_the_pointer() {
    let mut h = focused(text_set(10, 60)).await;
    h.press("ctrl+g k");
    draw(&mut h);
    let r = h.app.ui.hits.results;
    wheel(&mut h, MouseEventKind::ScrollDown, r.x + 4, r.y + 3, KeyModifiers::SHIFT);
    assert_eq!(h.app.ui.grid.left, 1);
    wheel(&mut h, MouseEventKind::ScrollRight, r.x + 4, r.y + 3, KeyModifiers::NONE);
    assert_eq!(h.app.ui.grid.left, 2);
    wheel(&mut h, MouseEventKind::ScrollLeft, r.x + 4, r.y + 3, KeyModifiers::NONE);
    wheel(&mut h, MouseEventKind::ScrollUp, r.x + 4, r.y + 3, KeyModifiers::SHIFT);
    assert_eq!(h.app.ui.grid.left, 0);
    assert_eq!(h.app.ui.grid.top, 0, "no row moved");
}

// ---- scrollbars --------------------------------------------------------------------

#[tokio::test]
async fn a_click_in_the_sidebars_scrollbar_jumps_there_and_dragging_the_thumb_scrolls() {
    let names: Vec<String> = (0..60).map(|i| format!("conn-{i:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut h = listed(&refs).await;
    draw(&mut h);
    let bar = h.app.ui.hits.sidebar_bar;
    assert!(!bar.is_empty() && bar.x == h.app.ui.hits.sidebar.right() - 1, "{bar:?}");
    let (total, room) = (60usize, bar.height as usize);
    let max = total - room;
    // Bottom, then top, then the middle.
    press_at(&mut h, bar.x, bar.bottom() - 1, t0());
    assert_eq!(h.app.ui.sidebar.scroll, max, "the bottom of the track is the end of the list");
    release(&mut h, bar.x, bar.bottom() - 1);
    press_at(&mut h, bar.x, bar.y, t0() + Duration::from_secs(5));
    assert_eq!(h.app.ui.sidebar.scroll, 0);
    release(&mut h, bar.x, bar.y);
    press_at(&mut h, bar.x, bar.y + room as u16 / 2, t0() + Duration::from_secs(10));
    let mid = h.app.ui.sidebar.scroll;
    assert!((max / 2).abs_diff(mid) <= 3, "the middle of the track is about the middle of the list: {mid} of {max}");
    release(&mut h, bar.x, bar.y + room as u16 / 2);
    assert_eq!(h.app.ui.sidebar.rows[h.app.ui.sidebar.cursor].node, h.app.ui.sidebar.rows[h.app.ui.sidebar.cursor].node);
    assert!(h.app.ui.sidebar.cursor >= mid && h.app.ui.sidebar.cursor < mid + room, "the cursor is kept on screen, and nothing was selected by the click");

    // Dragging the thumb: press on it, move down, move up.
    draw(&mut h);
    press_at(&mut h, bar.x, bar.y, t0() + Duration::from_secs(20));
    press_at(&mut h, bar.x, bar.y, t0() + Duration::from_secs(40));
    release(&mut h, bar.x, bar.y);
    assert_eq!(h.app.ui.sidebar.scroll, 0);
    draw(&mut h);
    let (top, len) = crate::app::view::scrollbar_thumb(total, room, 0).unwrap();
    assert_eq!(top, 0);
    press_at(&mut h, bar.x, bar.y + 1, t0() + Duration::from_secs(60));
    assert!(h.app.ui.drag.is_some());
    assert_eq!(h.app.ui.sidebar.scroll, 0, "pressing the thumb does not move it");
    drag_to(&mut h, bar.x, bar.y + 6);
    let s1 = h.app.ui.sidebar.scroll;
    drag_to(&mut h, bar.x, bar.y + 12);
    let s2 = h.app.ui.sidebar.scroll;
    assert!(0 < s1 && s1 < s2, "{s1} {s2}");
    assert_eq!(s2, super::pointer::scroll_for_thumb(total, room, len, 11), "the thumb is where the pointer is, less where it was grabbed");
    drag_to(&mut h, bar.x + 20, bar.bottom() + 20);
    assert_eq!(h.app.ui.sidebar.scroll, max, "dragging off the track keeps the thumb at its end");
    drag_to(&mut h, bar.x, 0);
    assert_eq!(h.app.ui.sidebar.scroll, 0);
    release(&mut h, bar.x, 0);
    assert_eq!(h.app.ui.drag, None);
}

#[tokio::test]
async fn a_click_in_the_results_scrollbar_jumps_there_and_dragging_the_thumb_scrolls() {
    let mut h = focused(text_set(1000, 3)).await;
    h.press("ctrl+g k");
    draw(&mut h);
    let bar = h.app.ui.grid.bar;
    assert!(!bar.is_empty() && bar.x == h.app.ui.hits.results.right() - 1, "{bar:?}");
    let (total, room) = (1000usize, bar.height as usize);
    let cursor = h.app.ui.grid.row;
    press_at(&mut h, bar.x, bar.bottom() - 1, t0());
    assert_eq!(h.app.ui.grid.top, total - room, "the end");
    assert_eq!(h.app.ui.focus, PaneId::Results, "a press on the scrollbar focuses the pane");
    assert!(h.app.ui.grid.row >= h.app.ui.grid.top, "the cursor came along: it was {cursor}, now {}", h.app.ui.grid.row);
    release(&mut h, bar.x, bar.bottom() - 1);
    draw(&mut h);
    // Drag the thumb from the bottom up to the middle.
    let (top, len) = crate::app::view::scrollbar_thumb(total, room, h.app.ui.grid.top).unwrap();
    let grab_row = bar.y + top as u16 + (len as u16) / 2;
    press_at(&mut h, bar.x, grab_row, t0() + Duration::from_secs(5));
    drag_to(&mut h, bar.x, bar.y + room as u16 / 2);
    let at = h.app.ui.grid.top;
    assert!((total - room) / 2 > at.saturating_sub(60) && at > 100 && at < total - room, "about the middle: {at}");
    release(&mut h, bar.x, bar.y);
    // A click in the column does not pick a cell: the cell cursor is where the window kept it.
    let rows = h.app.ui.grid.rows_visible;
    assert!((h.app.ui.grid.top..h.app.ui.grid.top + rows).contains(&h.app.ui.grid.row));
}

// ---- double-clicks -----------------------------------------------------------------

#[tokio::test]
async fn a_double_click_on_a_cell_opens_the_viewer_and_a_slow_second_click_does_not() {
    let mut h = focused(text_set(10, 4)).await;
    draw(&mut h);
    let (x, y) = super::grid_tests::where_is(&mut h, "r3c2").unwrap();
    press_at(&mut h, x + 1, y, t0());
    assert!(h.app.ui.overlay.is_none());
    assert_eq!((h.app.ui.grid.row, h.app.ui.grid.col), (3, 2));
    press_at(&mut h, x + 1, y, t0() + DOUBLE_CLICK + Duration::from_millis(1));
    assert!(h.app.ui.overlay.is_none(), "too slow");
    // Two quick presses on another cell: it opens that one.
    let (x, y) = super::grid_tests::where_is(&mut h, "r5c1").unwrap();
    let start = t0() + Duration::from_secs(10);
    press_at(&mut h, x, y, start);
    let fx = press_at(&mut h, x, y, start + Duration::from_millis(349));
    assert!(fx.repaint);
    match &h.app.ui.overlay {
        Some(Overlay::CellViewer(v)) => assert_eq!((v.row, v.col), (5, 1)),
        other => panic!("{other:?}"),
    }
    // Two clicks on two different cells are not a double-click.
    h.press("esc");
    draw(&mut h);
    let (x1, y1) = super::grid_tests::where_is(&mut h, "r2c0").unwrap();
    let (x2, y2) = super::grid_tests::where_is(&mut h, "r4c0").unwrap();
    let start = t0() + Duration::from_secs(30);
    press_at(&mut h, x1, y1, start);
    press_at(&mut h, x2, y2, start + Duration::from_millis(100));
    assert!(h.app.ui.overlay.is_none());
}

#[tokio::test]
async fn a_double_click_in_the_sidebar_toggles_a_node_within_350_ms_and_not_after() {
    let mut h = listed(&["alpha", "beta"]).await;
    draw(&mut h);
    let list = h.app.ui.hits.sidebar_list;
    let name = (list.x + 6, list.y);
    press_at(&mut h, name.0, name.1, t0());
    assert!(!h.app.ui.sidebar.expanded.iter().any(|_| true), "one click only selects");
    press_at(&mut h, name.0, name.1, t0() + Duration::from_millis(349));
    assert_eq!(h.app.ui.sidebar.expanded.len(), 1, "two clicks within 350 ms open the connection");
    press_at(&mut h, name.0, name.1, t0() + Duration::from_secs(10));
    press_at(&mut h, name.0, name.1, t0() + Duration::from_secs(10) + Duration::from_millis(351));
    assert_eq!(h.app.ui.sidebar.expanded.len(), 1, "351 ms is two clicks");
}

#[tokio::test]
async fn a_new_result_forgets_a_pending_click_so_the_second_press_is_not_a_double_click() {
    let mut h = focused(text_set(10, 4)).await;
    draw(&mut h);
    let (x, y) = super::grid_tests::where_is(&mut h, "r3c2").unwrap();
    let start = t0() + Duration::from_secs(100);
    press_at(&mut h, x + 1, y, start);
    // A new result lands (a run finished) with a cell of the same place.
    let mut fx = Effects::default();
    let Some(query::Outcome::Rows(set)) = h.app.model.tabs[0].outcome.clone() else { panic!("a result is shown") };
    h.app.model.tabs[0].run = query::RunState::Idle;
    h.app.on_ran(Ok(set), &mut fx);
    draw(&mut h);
    press_at(&mut h, x + 1, y, start + Duration::from_millis(100));
    assert!(h.app.ui.overlay.is_none(), "the first press was on the old result");
}

#[tokio::test]
async fn rebuilt_tree_rows_forget_a_pending_click_so_the_second_press_is_not_a_double_click() {
    let mut h = listed(&["alpha", "beta"]).await;
    draw(&mut h);
    let list = h.app.ui.hits.sidebar_list;
    let name = (list.x + 6, list.y);
    let start = t0() + Duration::from_secs(200);
    press_at(&mut h, name.0, name.1, start);
    // The rows are replaced (a listing arrived, the connection list reloaded).
    h.app.ui.sidebar.dirty = true;
    draw(&mut h);
    press_at(&mut h, name.0, name.1, start + Duration::from_millis(100));
    assert!(h.app.ui.sidebar.expanded.is_empty(), "the first press was on rows that are gone");
}
