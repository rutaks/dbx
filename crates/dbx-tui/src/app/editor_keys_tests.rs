//! Layer 2: the acceptance tests of slice 13 (editor keys), run on `App` with a
//! `FakeCore` behind the shell's command runner.
//!
//! A buffer, its caret and its selection are written as one string:
//! `‸` is the caret, `‹…›` a selection with the caret at `›`. Every action is
//! tried on each of its bindings (Cmd, Ctrl and the prefix fallback), and every
//! edit is undone with one `ctrl+z`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::complete_tests::{pagila, type_and_settle};
use super::keys::Action;
use super::overlay::Overlay;
use super::run_tests::{mode_bar, on_pagila, runs};
use super::settings::RunAtCaret;
use super::testing::{base, frame_rows, role_at};
use super::theme::Role;
use super::{Capabilities, Command, Event, PaneId};
use crate::core::{text_between, Cursor};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

async fn editing() -> Harness {
    let h = on_pagila().await;
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    h
}

/// Puts `marked` (see the module comment) in the editor.
fn load(h: &mut Harness, marked: &str) {
    let (mut text, mut row, mut col) = (String::new(), 0usize, 0usize);
    let (mut caret, mut anchor) = (None, None);
    for c in marked.chars() {
        let here = Cursor { row, col };
        match c {
            '‸' => caret = Some(here),
            '‹' => anchor = Some(here),
            '›' => caret = Some(here),
            '\n' => {
                text.push('\n');
                row += 1;
                col = 0;
            }
            c => {
                text.push(c);
                col += 1;
            }
        }
    }
    h.app.ui.editor.set_text(&text);
    match (anchor, caret) {
        (Some(a), Some(c)) => h.app.ui.editor.select_range(a, c),
        (_, Some(c)) => h.app.ui.editor.move_to(c),
        _ => h.app.ui.editor.move_to(Cursor::default()),
    }
}

/// The editor's text with its caret and selection marked.
fn dump(h: &Harness) -> String {
    let e = &h.app.ui.editor;
    let mut lines: Vec<Vec<char>> = e.lines().iter().map(|l| l.chars().collect()).collect();
    let mut marks: Vec<(Cursor, char)> = match e.selection() {
        Some((a, b)) => vec![(a, '‹'), (b, '›')],
        None => vec![(e.cursor(), '‸')],
    };
    marks.sort_by_key(|(at, _)| std::cmp::Reverse((at.row, at.col)));
    for (at, mark) in marks {
        lines[at.row].insert(at.col, mark);
    }
    lines.into_iter().map(|l| l.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn plain(marked: &str) -> String {
    marked.chars().filter(|c| !matches!(c, '‸' | '‹' | '›')).collect()
}

/// Loads `before`, presses `keys`, and checks the result. An edit is undone by
/// one `ctrl+z` (`undoes`).
fn act(h: &mut Harness, before: &str, keys: &str, after: &str) {
    load(h, before);
    h.press(keys);
    assert_eq!(dump(h), after, "{keys:?} on {before:?}");
}

fn act_undo(h: &mut Harness, before: &str, keys: &str, after: &str) {
    act(h, before, keys, after);
    h.press("ctrl+z");
    assert_eq!(h.app.ui.editor.text(), plain(before), "one ctrl+z undoes {keys:?} on {before:?}");
}

/// The clipboard texts among `commands`.
fn copied(commands: &[Command]) -> Vec<String> {
    commands.iter().filter_map(|c| if let Command::CopyToClipboard(t) = c { Some(t.clone()) } else { None }).collect()
}

fn screen(h: &mut Harness) -> String {
    frame_rows(&mut h.app, 114, 30).join("\n")
}

const T3: &str = "select a\nfrom t\nwhere x = 1";

fn key_event(code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) -> Event {
    let mut e = KeyEvent::new(code, mods);
    e.kind = kind;
    Event::Key(e)
}

// ---- running ----------------------------------------------------------------------

#[tokio::test]
async fn cmd_enter_ctrl_enter_f5_and_prefix_r_each_emit_one_run_command_and_alt_enter_none() {
    for keys in ["cmd+enter", "ctrl+enter", "f5", "ctrl+g r"] {
        let mut h = editing().await;
        h.type_text("select 1");
        let sent = runs(&h.press_fx(keys));
        assert_eq!(sent.len(), 1, "{keys}");
        assert_eq!(h.app.ui.editor.text(), "select 1", "{keys} typed nothing");
    }
    let mut h = editing().await;
    h.type_text("select 1");
    assert!(runs(&h.press_fx("alt+enter")).is_empty(), "alt+enter is the intention key, never run");
}

#[tokio::test]
async fn the_run_keys_work_from_any_pane_as_f5_does_and_ctrl_f2_and_prefix_c_cancel() {
    for keys in ["cmd+enter", "ctrl+enter"] {
        let mut h = editing().await;
        h.type_text("select 1");
        h.press("ctrl+g h");
        assert_eq!(h.app.ui.focus, PaneId::Sidebar);
        assert_eq!(runs(&h.press_fx(keys)).len(), 1, "{keys} from the sidebar");
    }
    for keys in ["ctrl+f2", "cmd+f2", "ctrl+g c", "ctrl+c"] {
        let mut h = editing().await;
        h.type_text("select 1");
        h.press("f5");
        let sent = h.press_fx(keys);
        assert!(sent.iter().any(|c| matches!(c, Command::Cancel(_))), "{keys}: {sent:?}");
    }
}

// ---- keys the editor must not turn into text -----------------------------------------

#[tokio::test]
async fn an_unbound_cmd_or_ctrl_key_types_nothing_and_ctrl_u_and_ctrl_r_are_no_longer_undo_and_redo() {
    let mut h = editing().await;
    for keys in ["cmd+k", "cmd+shift+x", "cmd+p", "ctrl+k", "ctrl+e", "alt+x", "cmd+r", "ctrl+r"] {
        act(&mut h, "abc‸", keys, "abc‸");
    }
    h.type_text("d");
    assert_eq!(h.app.ui.editor.text(), "abcd");
    h.press("ctrl+u");
    assert_eq!(h.app.ui.editor.text(), "abcd", "ctrl+u is not undo");
    h.press("ctrl+z ctrl+r");
    assert_eq!(h.app.ui.editor.text(), "abc", "ctrl+r is not redo");
    // A terminal that reports shift on a letter and a punctuation key.
    let fx = h.app.update(Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), base());
    h.run(fx);
    assert_eq!(h.app.ui.editor.text(), "abcd", "ctrl+shift+z as the protocol reports it redoes");
}

#[tokio::test]
async fn a_key_release_is_ignored_and_a_repeat_acts_as_a_press() {
    let mut h = editing().await;
    load(&mut h, "select a\n‸from t");
    let fx = h.app.update(key_event(KeyCode::Char('d'), KeyModifiers::SUPER, KeyEventKind::Release), base());
    h.run(fx);
    assert_eq!(h.app.ui.editor.lines().len(), 2, "a release does nothing");
    let fx = h.app.update(key_event(KeyCode::Char('d'), KeyModifiers::SUPER, KeyEventKind::Repeat), base());
    h.run(fx);
    assert_eq!(h.app.ui.editor.lines().len(), 3, "a repeat is another press");
    let fx = h.app.update(key_event(KeyCode::Char('d'), KeyModifiers::SUPER, KeyEventKind::Press), base());
    h.run(fx);
    assert_eq!(h.app.ui.editor.lines().len(), 4);
}

// ---- one test per editing action, on each of its bindings ------------------------------

#[tokio::test]
async fn duplicate_line_or_selection() {
    let mut h = editing().await;
    for keys in ["cmd+d", "ctrl+d", "ctrl+g d"] {
        act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "select a\nfrom t\nfr‸om t\nwhere x = 1");
        act_undo(&mut h, "select a\n‹from› t\nwhere x = 1", keys, "select a\nfrom‹from› t\nwhere x = 1");
        act_undo(&mut h, "select a\nfrom t\nwhere x = 1‸", keys, "select a\nfrom t\nwhere x = 1\nwhere x = 1‸");
    }
}

#[tokio::test]
async fn delete_line() {
    let mut h = editing().await;
    for keys in ["cmd+backspace", "ctrl+y", "ctrl+g x"] {
        act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "select a\nwh‸ere x = 1");
        act_undo(&mut h, "select a\nfrom t\nwh‸ere x = 1", keys, "select a\nfr‸om t");
        act_undo(&mut h, "‹select a\nfr›om t\nwhere x = 1", keys, "wh‸ere x = 1");
        act_undo(&mut h, "on‸ly", keys, "‸");
    }
}

#[tokio::test]
async fn move_line_up_and_down() {
    let mut h = editing().await;
    act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", "alt+shift+up", "fr‸om t\nselect a\nwhere x = 1");
    act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", "alt+shift+down", "select a\nwhere x = 1\nfr‸om t");
    act_undo(&mut h, "select a\n‹from t›\nwhere x = 1", "alt+shift+up", "‹from t›\nselect a\nwhere x = 1");
    act_undo(&mut h, "‹select a\nfrom› t\nwhere x = 1", "alt+shift+down", "where x = 1\n‹select a\nfrom› t");
    // At the ends nothing moves, and there is nothing to undo.
    act(&mut h, "sel‸ect a\nfrom t", "alt+shift+up", "sel‸ect a\nfrom t");
    act(&mut h, "select a\nfr‸om t", "alt+shift+down", "select a\nfr‸om t");
}

#[tokio::test]
async fn move_statement_up_and_down() {
    let mut h = editing().await;
    for (up, down) in [("cmd+shift+up", "cmd+shift+down"), ("ctrl+shift+up", "ctrl+shift+down")] {
        act_undo(&mut h, "select 1;\nsel‸ect 2;\nselect 3;", up, "sel‸ect 2;\nselect 1;\nselect 3;");
        act_undo(&mut h, "select 1;\nsel‸ect 2;\nselect 3;", down, "select 1;\nselect 3;\nsel‸ect 2;");
        // A statement over two lines moves as a block, and the blank line between stays.
        act_undo(&mut h, "select a\nfr‸om t;\n\nselect 2;", down, "select 2;\n\nselect a\nfr‸om t;");
        act_undo(&mut h, "select 2;\n\nselect a\nfr‸om t;", up, "select a\nfr‸om t;\n\nselect 2;");
        // The first statement cannot go up and the last cannot go down.
        act(&mut h, "sel‸ect 1;\nselect 2;", up, "sel‸ect 1;\nselect 2;");
        act(&mut h, "select 1;\nsel‸ect 2;", down, "select 1;\nsel‸ect 2;");
    }
    // Two statements on one line cannot be moved as lines: nothing happens, and it says why.
    act(&mut h, "select 1; sel‸ect 2;\nselect 3;", "cmd+shift+down", "select 1; sel‸ect 2;\nselect 3;");
    assert!(h.app.ui.notice.as_ref().is_some_and(|n| n.text.contains("line")), "{:?}", h.app.ui.notice);
}

#[tokio::test]
async fn toggle_line_comment() {
    let mut h = editing().await;
    for keys in ["cmd+/", "ctrl+/", "ctrl+g /"] {
        act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "select a\n-- fr‸om t\nwhere x = 1");
        act(&mut h, "select a\n-- fr‸om t\nwhere x = 1", keys, "select a\nfr‸om t\nwhere x = 1");
        // Every selected line; a mix is commented, all commented is uncommented.
        act_undo(&mut h, "‹select a\nfrom› t\nwhere x = 1", keys, "‹-- select a\n-- from› t\nwhere x = 1");
        act(&mut h, "-- select a\n‹from t\n-- where› x = 1", keys, "-- select a\n‹-- from t\n-- -- where› x = 1");
        act(&mut h, "‹-- select a\n-- from› t\nwhere x = 1", keys, "‹select a\nfrom› t\nwhere x = 1");
        // The comment goes at the indent, and a blank line in the selection stays blank.
        act(&mut h, "  se‸lect a", keys, "  -- se‸lect a");
        act(&mut h, "‹  a\n\n    b›", keys, "‹  -- a\n\n    -- b›");
    }
}

#[tokio::test]
async fn toggle_block_comment() {
    let mut h = editing().await;
    for keys in ["cmd+alt+/", "ctrl+shift+/", "ctrl+g *"] {
        act_undo(&mut h, "select ‹a›\nfrom t", keys, "select /* ‹a› */\nfrom t");
        act(&mut h, "select /* ‹a› */\nfrom t", keys, "select ‹a›\nfrom t");
        act(&mut h, "select ‹/* a */›\nfrom t", keys, "select ‹a›\nfrom t");
        act_undo(&mut h, "select a\nfr‸om t", keys, "select a\n/* fr‸om t */");
        act(&mut h, "select a\n/* fr‸om t */", keys, "select a\nfr‸om t");
    }
}

#[tokio::test]
async fn extend_and_shrink_selection_walk_word_statement_everything_and_back() {
    let mut h = editing().await;
    let all = "select a from t;\nselect 2;\nselect 3";
    for (extend, shrink) in [("ctrl+w", "ctrl+shift+w"), ("alt+up", "alt+down"), ("ctrl+g w", "ctrl+g W")] {
        load(&mut h, "select a fr‸om t;\nselect 2;\nselect 3");
        let steps = ["select a ‹from› t;\nselect 2;\nselect 3", "‹select a from t›;\nselect 2;\nselect 3", &format!("‹{all}›")];
        for want in steps {
            h.press(extend);
            assert_eq!(dump(&h), want, "{extend}");
        }
        h.press(extend);
        assert_eq!(dump(&h), format!("‹{all}›"), "{extend}: everything is the end");
        for want in [steps[1], steps[0], "select a fr‸om t;\nselect 2;\nselect 3"] {
            h.press(shrink);
            assert_eq!(dump(&h), want, "{shrink}");
        }
    }
    // Not on a word: the first step is the statement.
    act(&mut h, "select 1;\n‸\nselect 3", "ctrl+w", "‹select 1;\n\nselect 3›");
}

#[tokio::test]
async fn select_all() {
    let mut h = editing().await;
    for keys in ["cmd+a", "ctrl+a", "ctrl+g a"] {
        act(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "‹select a\nfrom t\nwhere x = 1›");
    }
}

#[tokio::test]
async fn undo_and_redo_on_every_binding() {
    for (undo, redo) in [("cmd+z", "cmd+shift+z"), ("ctrl+z", "ctrl+shift+z"), ("ctrl+g u", "ctrl+g U")] {
        let mut h = editing().await;
        h.type_text("ab");
        h.press(undo);
        assert_eq!(h.app.ui.editor.text(), "a", "{undo}");
        h.press(redo);
        assert_eq!(h.app.ui.editor.text(), "ab", "{redo}");
        h.press(undo);
        h.press(undo);
        assert_eq!(h.app.ui.editor.text(), "", "{undo} twice");
        h.press(redo);
        h.press(redo);
        assert_eq!(h.app.ui.editor.text(), "ab", "{redo} twice");
    }
}

#[tokio::test]
async fn an_undo_is_one_step_for_a_compound_edit_and_a_new_edit_ends_the_redo() {
    let mut h = editing().await;
    load(&mut h, "select a\nfr‸om t");
    h.press("cmd+d");
    h.press("cmd+/");
    assert_eq!(h.app.ui.editor.text(), "select a\nfrom t\n-- from t");
    h.press("cmd+z");
    assert_eq!(h.app.ui.editor.text(), "select a\nfrom t\nfrom t");
    h.press("cmd+z");
    assert_eq!(h.app.ui.editor.text(), "select a\nfrom t");
    h.type_text("x");
    h.press("cmd+shift+z");
    assert_eq!(h.app.ui.editor.text(), "select a\nfrxom t", "typing ended the redo history");
}

#[tokio::test]
async fn copy_cut_and_paste_through_the_system_clipboard() {
    let mut h = editing().await;
    for keys in ["cmd+c", "ctrl+c"] {
        load(&mut h, "select a\n‹from t›\nwhere x = 1");
        let sent = h.press_fx(keys);
        assert_eq!(copied(&sent), ["from t"], "{keys}");
        assert_eq!(dump(&h), "select a\n‹from t›\nwhere x = 1", "copy changes nothing");
        // With nothing selected cmd+c copies the line, line break included
        // (ctrl+c does nothing then: it is the run's cancel).
        load(&mut h, "select a\nfr‸om t\nwhere x = 1");
        let want: &[&str] = if keys == "cmd+c" { &["from t\n"] } else { &[] };
        assert_eq!(copied(&h.press_fx(keys)), want, "{keys}");
    }
    for keys in ["cmd+x", "ctrl+x"] {
        load(&mut h, "select a\n‹from t›\nwhere x = 1");
        let sent = h.press_fx(keys);
        assert_eq!(copied(&sent), ["from t"], "{keys}");
        assert_eq!(dump(&h), "select a\n‸\nwhere x = 1", "{keys}");
        h.press("ctrl+z");
        assert_eq!(h.app.ui.editor.text(), T3, "one undo puts the cut text back");
        load(&mut h, "select a\nfr‸om t\nwhere x = 1");
        let sent = h.press_fx(keys);
        assert_eq!(copied(&sent), ["from t\n"], "{keys}: no selection cuts the line");
        assert_eq!(dump(&h), "select a\nwh‸ere x = 1", "{keys}");
    }
    // Over the terminal clipboard's size nothing is sent, and it says so.
    h.app.ui.editor.set_text(&"x".repeat(80_000));
    h.press("ctrl+a");
    assert!(copied(&h.press_fx("cmd+c")).is_empty());
    assert!(h.app.ui.notice.as_ref().is_some_and(|n| n.failure && n.text.contains("too large")), "{:?}", h.app.ui.notice);
}

#[tokio::test]
async fn ctrl_c_with_a_run_in_flight_cancels_and_does_not_copy() {
    let mut h = editing().await;
    h.type_text("select 1");
    h.press("f5");
    h.press("ctrl+a");
    let sent = h.press_fx("ctrl+c");
    assert!(sent.iter().any(|c| matches!(c, Command::Cancel(_))) && copied(&sent).is_empty(), "{sent:?}");
}

#[tokio::test]
async fn paste_arrives_as_the_terminals_paste_and_the_paste_keys_type_nothing() {
    let mut h = editing().await;
    load(&mut h, "select a\nfr‸om t");
    let fx = h.app.update(Event::Paste("1,\r\n2,\n3".into()), base());
    h.run(fx);
    assert_eq!(h.app.ui.editor.text(), "select a\nfr1,\n2,\n3om t", "multi-line text keeps its line breaks");
    h.press("ctrl+z");
    assert_eq!(h.app.ui.editor.text(), "select a\nfrom t", "a paste is one undo step");
    for keys in ["cmd+v", "ctrl+v"] {
        act(&mut h, "select a\nfr‸om t", keys, "select a\nfr‸om t");
        assert!(h.app.ui.notice.as_ref().is_some_and(|n| n.text.contains("paste")), "{keys} says what to do: {:?}", h.app.ui.notice);
    }
}

#[tokio::test]
async fn find_and_find_next_and_previous() {
    for open in ["cmd+f", "ctrl+f", "ctrl+g f"] {
        let mut h = editing().await;
        load(&mut h, T3);
        h.app.ui.editor.move_to(Cursor::default());
        h.press(open);
        assert!(screen(&mut h).contains("find"), "{open} opens the find bar");
        h.type_text("t");
        assert_eq!(dump(&h), "selec‹t› a\nfrom t\nwhere x = 1", "typing finds the first match after the caret");
        assert_eq!(h.app.ui.editor.text(), T3, "typing went to the bar");
        for (next, previous) in [("enter", "shift+f3"), ("f3", "cmd+shift+g"), ("cmd+g", "shift+f3")] {
            load(&mut h, T3);
            h.app.ui.editor.move_to(Cursor::default());
            h.press("esc");
            h.app.ui.last_find.clear();
            h.press(open);
            h.type_text("t");
            h.press(next);
            assert_eq!(dump(&h), "select a\nfrom ‹t›\nwhere x = 1", "{next}");
            h.press(next);
            assert_eq!(dump(&h), "selec‹t› a\nfrom t\nwhere x = 1", "{next} wraps");
            h.press(previous);
            assert_eq!(dump(&h), "select a\nfrom ‹t›\nwhere x = 1", "{previous} wraps back");
            h.press(previous);
            assert_eq!(dump(&h), "selec‹t› a\nfrom t\nwhere x = 1", "{previous}");
        }
        // esc closes the bar and keeps the text; the next esc is the editor's.
        h.press("esc");
        assert!(!screen(&mut h).contains("find"), "esc closes it");
        assert_eq!(h.app.ui.editor.text(), T3);
        // The bar is gone: typing is the editor's again.
        h.type_text("z");
        assert!(h.app.ui.editor.text().contains('z'));
    }
}

#[tokio::test]
async fn find_is_plain_text_and_case_insensitive_unless_the_query_has_a_capital() {
    let mut h = editing().await;
    load(&mut h, "‸Select a\nfrom (t) FROM t");
    h.press("cmd+f");
    h.type_text("from");
    assert_eq!(dump(&h), "Select a\n‹from› (t) FROM t", "no capital in the query: case does not matter");
    h.press("enter");
    assert_eq!(dump(&h), "Select a\nfrom (t) ‹FROM› t");
    h.press("esc");
    h.app.ui.last_find.clear();
    h.press("cmd+f");
    h.type_text("FROM");
    assert!(dump(&h).contains("‹FROM›") && !dump(&h).contains("‹from›"), "a capital makes it exact: {}", dump(&h));
    // Characters that mean something to a regular expression are only themselves.
    load(&mut h, "‸a(b)c a.c");
    h.press("esc");
    h.app.ui.last_find.clear();
    h.press("cmd+f");
    h.type_text("a.c");
    assert_eq!(dump(&h), "a(b)c ‹a.c›");
    h.press("esc");
    h.app.ui.last_find.clear();
    h.press("cmd+f");
    h.type_text("zzz");
    assert!(screen(&mut h).contains("no match"), "{}", screen(&mut h));
}

#[tokio::test]
async fn find_highlights_every_match_with_a_palette_role() {
    let mut h = editing().await;
    load(&mut h, "‸select a\nfrom t\nwhere x = 1");
    h.press("cmd+f");
    h.type_text("e");
    let text = h.app.ui.hits.editor_text;
    screen(&mut h);
    let text = if text.width == 0 { h.app.ui.hits.editor_text } else { text };
    // `e` at (0,1) is selected (the current match); (0,3) and (2,2) are the others.
    let at = |row: u16, col: u16| (text.x + col, text.y + row);
    for (row, col) in [(0u16, 3u16), (2, 2)] {
        let (x, y) = at(row, col);
        assert_eq!(role_at(&mut h.app, x, y).bg, Some(Role::FindMatch), "match at row {row} col {col}");
    }
    let (x, y) = at(0, 2);
    assert_ne!(role_at(&mut h.app, x, y).bg, Some(Role::FindMatch), "a non-match");
}

#[tokio::test]
async fn go_to_line() {
    for keys in ["cmd+l", "ctrl+g g"] {
        let mut h = editing().await;
        load(&mut h, T3);
        h.app.ui.editor.move_to(Cursor::default());
        h.press(keys);
        assert!(screen(&mut h).contains("line"), "{keys} opens the prompt");
        h.type_text("3");
        h.press("enter");
        assert_eq!(dump(&h), "select a\nfrom t\n‸where x = 1", "{keys}");
        h.press(keys);
        h.type_text("99");
        h.press("enter");
        assert_eq!(h.app.ui.editor.cursor().row, 2, "past the end goes to the last line");
        h.press(keys);
        h.type_text("1");
        h.press("esc");
        assert_eq!(h.app.ui.editor.cursor().row, 2, "esc cancels");
        assert_eq!(h.app.ui.editor.text(), T3);
        // Letters are not a line number.
        h.press(keys);
        h.type_text("ab");
        h.press("enter");
        assert_eq!(h.app.ui.editor.cursor().row, 2);
    }
}

#[tokio::test]
async fn start_new_line_after_and_before() {
    let mut h = editing().await;
    act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", "shift+enter", "select a\nfrom t\n‸\nwhere x = 1");
    act_undo(&mut h, "select a\n  fr‸om t\nwhere x = 1", "shift+enter", "select a\n  from t\n  ‸\nwhere x = 1");
    for keys in ["cmd+alt+enter", "ctrl+alt+enter"] {
        act_undo(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "select a\n‸\nfrom t\nwhere x = 1");
        act_undo(&mut h, "select a\n  fr‸om t", keys, "select a\n  ‸\n  from t");
        act_undo(&mut h, "se‸lect a", keys, "‸\nselect a");
    }
}

#[tokio::test]
async fn join_lines() {
    let mut h = editing().await;
    for keys in ["ctrl+shift+j", "ctrl+g J"] {
        act_undo(&mut h, "select a\nfr‸om t\n  where x = 1", keys, "select a\nfrom t‸ where x = 1");
        act_undo(&mut h, "‹select a\nfrom t\nwhere› x = 1", keys, "select a‸ from t where x = 1");
        act(&mut h, "select a\nfrom t\nwhere x = 1‸", keys, "select a\nfrom t\nwhere x = 1‸");
    }
}

#[tokio::test]
async fn toggle_case_of_the_selection_or_the_word() {
    let mut h = editing().await;
    for keys in ["cmd+shift+u", "ctrl+shift+u", "ctrl+g ~"] {
        act_undo(&mut h, "select a\n‹from› t", keys, "select a\n‹FROM› t");
        act(&mut h, "select a\n‹FROM› t", keys, "select a\n‹from› t");
        act(&mut h, "select a\n‹Fr›om t", keys, "select a\n‹FR›om t");
        act_undo(&mut h, "select a\nfr‸om t", keys, "select a\nFR‸OM t");
        act(&mut h, "select a\n‸ t", keys, "select a\n‸ t");
    }
}

#[tokio::test]
async fn indent_and_unindent() {
    let mut h = editing().await;
    act_undo(&mut h, "‹select a\nfrom› t\nwhere x = 1", "tab", "    ‹select a\n    from› t\nwhere x = 1");
    act_undo(&mut h, "    ‹select a\n  from› t\nwhere x = 1", "shift+tab", "‹select a\nfrom› t\nwhere x = 1");
    // With no selection tab inserts indentation to the next stop: never a tab character.
    act_undo(&mut h, "‸select a", "tab", "    ‸select a");
    act_undo(&mut h, "se‸lect a", "tab", "se  ‸lect a");
    act(&mut h, "   ‸x", "tab", "    ‸x");
    assert!(!h.app.ui.editor.text().contains('\t'));
    // A selection inside one line is replaced by the indentation.
    act(&mut h, "se‹lec›t a", "tab", "se  ‸t a");
    // shift+tab with no selection unindents the caret's line.
    act_undo(&mut h, "      fr‸om t", "shift+tab", "  fr‸om t");
    act(&mut h, "  fr‸om t", "shift+tab", "fr‸om t");
    act(&mut h, "fr‸om t", "shift+tab", "fr‸om t");
    // A setting asks for tab characters instead.
    h.app.model.settings.hard_tabs = true;
    act_undo(&mut h, "‸select a", "tab", "\t‸select a");
    act_undo(&mut h, "‹select a\nfrom› t", "tab", "\t‹select a\n\tfrom› t");
    act(&mut h, "\t‹select a\n  from› t", "shift+tab", "‹select a\nfrom› t");
}

#[tokio::test]
async fn tab_belongs_first_to_the_popup_then_to_a_template_then_to_a_multi_line_selection_and_last_to_indentation() {
    // The popup: tab accepts (here the word is typed in full: the same text, no indentation).
    let mut h = super::complete_tests::editing(pagila()).await;
    type_and_settle(&mut h, "select * from fil").await;
    assert!(super::complete_tests::visible(&h));
    h.press("tab");
    assert!(h.app.ui.editor.text().starts_with("select * from film") && !h.app.ui.editor.text().contains("  "), "the popup took tab: {:?}", h.app.ui.editor.text());
    // With a selection over several lines that is not in the way: indentation.
    let mut h = editing().await;
    act(&mut h, "‹select a\nfrom› t", "tab", "    ‹select a\n    from› t");
    // A template being filled in: tab goes to the next variable, and does not indent.
    let mut h = super::complete_tests::editing(pagila()).await;
    type_and_settle(&mut h, "sel").await;
    h.press("tab");
    h.settle().await;
    assert!(h.app.ui.template.active());
    let before = h.app.ui.editor.text();
    h.press("tab");
    assert!(!h.app.ui.editor.text().starts_with(' ') && h.app.ui.editor.text().len() == before.len(), "{:?}", h.app.ui.editor.text());
    // Nothing else owns it: indentation.
    let mut h = editing().await;
    act(&mut h, "‸", "tab", "    ‸");
}

#[tokio::test]
async fn word_wise_movement_and_deletion() {
    let mut h = editing().await;
    for (left, right) in [("alt+left", "alt+right"), ("ctrl+left", "ctrl+right")] {
        act(&mut h, "select a‸", left, "select ‸a");
        act(&mut h, "select ‸a", left, "‸select a");
        act(&mut h, "‸select a", right, "select‸ a");
        act(&mut h, "select‸ a", right, "select a‸");
        act(&mut h, "select a\n‸from t", left, "select ‸a\nfrom t");
        act(&mut h, "select a‸\nfrom t", right, "select a\nfrom‸ t");
        // With shift the selection grows.
        act(&mut h, "select a‸", &left.replace('+', "+shift+"), "select ‹a›");
        act(&mut h, "‸select a", &right.replace('+', "+shift+"), "‹select› a");
    }
    for keys in ["alt+backspace", "ctrl+backspace"] {
        act_undo(&mut h, "select a‸", keys, "select ‸");
        act_undo(&mut h, "select ‸", keys, "‸");
        act_undo(&mut h, "select a\n‸from t", keys, "select a‸from t");
    }
    for keys in ["alt+delete", "ctrl+delete"] {
        act_undo(&mut h, "‸select a", keys, "‸ a");
        act_undo(&mut h, "select‸ a", keys, "select‸");
        act_undo(&mut h, "select a‸\nfrom t", keys, "select a‸from t");
    }
}

#[tokio::test]
async fn line_start_and_end_and_text_start_and_end() {
    let mut h = editing().await;
    // `home` toggles between the first non-blank character and column 0.
    for keys in ["home", "cmd+left"] {
        act(&mut h, "  fr‸om t", keys, "  ‸from t");
        act(&mut h, "  ‸from t", keys, "‸  from t");
        act(&mut h, "‸  from t", keys, "  ‸from t");
        act(&mut h, "   ‸", keys, "‸   ");
        let select = keys.replace("home", "shift+home").replace("cmd+left", "cmd+shift+left");
        act(&mut h, "  fr‸om t", &select, "  ‹fr›om t");
    }
    for keys in ["end", "cmd+right"] {
        act(&mut h, "fr‸om t\nwhere", keys, "from t‸\nwhere");
        let select = keys.replace("end", "shift+end").replace("cmd+right", "cmd+shift+right");
        act(&mut h, "fr‸om t\nwhere", &select, "fr‹om t›\nwhere");
    }
    for keys in ["cmd+up", "cmd+home", "ctrl+home"] {
        act(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "‸select a\nfrom t\nwhere x = 1");
        let select = keys.replace("home", "shift+home").replace("cmd+up", "cmd+shift+up");
        if !select.contains("shift+up") {
            act(&mut h, "select a\nfr‸om t\nwhere x = 1", &select, "‹select a\nfr›om t\nwhere x = 1");
        }
    }
    for keys in ["cmd+down", "cmd+end", "ctrl+end"] {
        act(&mut h, "select a\nfr‸om t\nwhere x = 1", keys, "select a\nfrom t\nwhere x = 1‸");
        let select = keys.replace("end", "shift+end").replace("cmd+down", "cmd+shift+down");
        if !select.contains("shift+down") {
            act(&mut h, "select a\nfr‸om t\nwhere x = 1", &select, "select a\nfr‹om t\nwhere x = 1›");
        }
    }
}

#[tokio::test]
async fn matching_brace() {
    let mut h = editing().await;
    for keys in ["ctrl+m", "ctrl+shift+m", "ctrl+g %"] {
        act(&mut h, "select ‸(a + (b))", keys, "select (a + (b))‸");
        act(&mut h, "select (a + (b))‸", keys, "select ‸(a + (b))");
        act(&mut h, "select (a + ‸(b))", keys, "select (a + (b)‸)");
        // Brackets in strings and comments are not brackets.
        act(&mut h, "select ‸(')')", keys, "select (')')‸");
        act(&mut h, "select a‸", keys, "select a‸");
    }
}

// ---- smart keys ---------------------------------------------------------------------

#[tokio::test]
async fn typing_an_opening_bracket_or_quote_inserts_the_pair_with_the_caret_between() {
    let mut h = editing().await;
    for (typed, want) in [("(", "(‸)"), ("[", "[‸]"), ("'", "'‸'"), ("\"", "\"‸\"")] {
        act_undo(&mut h, "‸", typed, want);
    }
    act(&mut h, "select ‸", "(", "select (‸)");
    // Not when the next character is a word character.
    for typed in ["(", "[", "'", "\""] {
        act(&mut h, "‸abc", typed, &format!("{typed}‸abc"));
    }
    // A quote is not paired after a word character, nor inside a string or a comment.
    act(&mut h, "don‸", "'", "don'‸");
    act(&mut h, "select 'a‸b'", "'", "select 'a'‸b'");
    act(&mut h, "select a -- it‸", "'", "select a -- it'‸");
    act(&mut h, "select /* it‸ */", "'", "select /* it'‸ */");
    // A bracket inside a string is a bracket as far as pairs go: only the quote has the rule.
    act(&mut h, "select 'a‸'", "(", "select 'a(‸)'");
}

#[tokio::test]
async fn typing_the_closer_that_is_next_moves_over_it_and_tab_also_moves_past_it() {
    let mut h = editing().await;
    act(&mut h, "(‸)", ")", "()‸");
    act(&mut h, "[‸]", "]", "[]‸");
    act(&mut h, "'‸'", "'", "''‸");
    act(&mut h, "\"‸\"", "\"", "\"\"‸");
    act(&mut h, "(a‸)", ")", "(a)‸");
    act(&mut h, "(a‸) )", ")", "(a)‸ )");
    // `tab` is the other way out.
    act(&mut h, "(‸)", "tab", "()‸");
    act(&mut h, "(a, 'b‸')", "tab", "(a, 'b'‸)");
    act(&mut h, "(a, 'b'‸)", "tab", "(a, 'b')‸");
    // A closer with nothing to close is typed as it is.
    act(&mut h, "a‸)", ")", "a)‸)");
    // Typed in sequence: `(` `a` `)` leaves one pair.
    load(&mut h, "‸");
    h.type_text("count(*)");
    assert_eq!(h.app.ui.editor.text(), "count(*)");
    load(&mut h, "‸");
    h.type_text("f('a', [1])");
    assert_eq!(h.app.ui.editor.text(), "f('a', [1])");
}

#[tokio::test]
async fn backspace_between_an_empty_pair_deletes_both() {
    let mut h = editing().await;
    for pair in ["()", "[]", "''", "\"\""] {
        let (open, close) = (&pair[..1], &pair[1..]);
        act_undo(&mut h, &format!("a {open}‸{close}"), "backspace", "a ‸");
    }
    act(&mut h, "(a‸)", "backspace", "(‸)");
    act(&mut h, "(‸) ", "backspace", "‸ ");
    act(&mut h, "(‸x)", "backspace", "‸x)");
}

#[tokio::test]
async fn an_opening_character_typed_over_a_selection_surrounds_it() {
    let mut h = editing().await;
    act_undo(&mut h, "‹abc›", "'", "'‹abc›'");
    act_undo(&mut h, "select ‹abc› d", "(", "select (‹abc›) d");
    act(&mut h, "‹abc›", "[", "[‹abc›]");
    act(&mut h, "‹abc›", "\"", "\"‹abc›\"");
    act(&mut h, "‹ab\ncd›", "(", "(‹ab\ncd›)");
    // Any other character replaces it, as ever.
    act(&mut h, "‹abc›", "x", "x‸");
}

#[tokio::test]
async fn smart_keys_each_have_a_setting_that_turns_them_off() {
    let mut h = editing().await;
    h.app.model.settings.auto_pair = false;
    act(&mut h, "‸", "(", "(‸");
    act(&mut h, "(‸)", ")", "()‸)");
    act(&mut h, "‹abc›", "'", "'‸");
    act(&mut h, "(‸)", "backspace", "‸)");
    h.app.model.settings.smart_enter = false;
    act(&mut h, "  a‸", "enter", "  a\n‸");
    h.app.model.settings.smart_home = false;
    act(&mut h, "  fr‸om", "home", "‸  from");
}

#[tokio::test]
async fn enter_keeps_the_indent_and_between_brackets_puts_the_closer_on_its_own_line() {
    let mut h = editing().await;
    act_undo(&mut h, "  select a‸", "enter", "  select a\n  ‸");
    act(&mut h, "    a‸ b", "enter", "    a\n    ‸ b");
    // In the middle of the indentation the new line takes only what is before the caret.
    act(&mut h, "  ‸  a", "enter", "  \n  ‸  a");
    act(&mut h, "a‸", "enter", "a\n‸");
    act_undo(&mut h, "f(‸)", "enter", "f(\n    ‸\n)");
    act(&mut h, "  f(‸)", "enter", "  f(\n      ‸\n  )");
    act(&mut h, "f(a‸)", "enter", "f(a\n‸)");
}

#[tokio::test]
async fn enter_after_an_opening_block_comment_closes_it() {
    let mut h = editing().await;
    act_undo(&mut h, "select 1 /*‸", "enter", "select 1 /*\n * ‸\n */");
    act(&mut h, "  /*‸", "enter", "  /*\n   * ‸\n   */");
    // Already closed on the row, or text after the caret: only a new line.
    act(&mut h, "/* a */‸", "enter", "/* a */\n‸");
    act(&mut h, "/*‸ a", "enter", "/*\n‸ a");
}

#[tokio::test]
async fn the_prefix_has_keys_for_a_new_line_after_and_before_and_the_mode_bar_shows_them() {
    let mut h = editing().await;
    act_undo(&mut h, "select ‸a\nfrom t", "ctrl+g o", "select a\n‸\nfrom t");
    act_undo(&mut h, "select ‸a\nfrom t", "ctrl+g O", "‸\nselect a\nfrom t");
    h.press("ctrl+g");
    let bar = frame_rows(&mut h.app, 400, 30).pop().unwrap();
    assert!(bar.contains("o new line after") && bar.contains("new line before"), "{bar}");
}

#[tokio::test]
async fn find_starts_with_the_last_search_when_nothing_is_selected() {
    let mut h = editing().await;
    load(&mut h, T3);
    h.app.ui.editor.move_to(Cursor::default());
    h.press("ctrl+f");
    h.type_text("from");
    h.press("esc");
    h.app.ui.editor.move_to(Cursor::default());
    h.press("ctrl+f");
    assert!(screen(&mut h).contains("from"), "the bar starts with the last search");
    h.press("enter");
    assert_eq!(dump(&h), "select a\n‹from› t\nwhere x = 1");
}

// ---- the statement chooser ----------------------------------------------------------------

const SUB: &str = "select * from (select id from t‸) x";

#[tokio::test]
async fn inside_a_subquery_run_opens_a_chooser_and_enter_runs_exactly_the_listed_text() {
    for keys in ["cmd+enter", "ctrl+enter", "f5", "ctrl+g r"] {
        let mut h = editing().await;
        load(&mut h, SUB);
        let sent = h.press_fx(keys);
        assert!(runs(&sent).is_empty(), "{keys}: the chooser asks first");
        assert!(matches!(h.app.ui.overlay, Some(Overlay::Chooser(_))), "{keys}");
        let shown = screen(&mut h);
        assert!(shown.contains("subquery") && shown.contains("statement") && shown.contains("all"), "{shown}");
        let Some(Overlay::Chooser(c)) = &h.app.ui.overlay else { panic!("a chooser") };
        assert_eq!(c.items.iter().map(|i| i.label).collect::<Vec<_>>(), ["subquery", "statement", "all"], "{keys}");
        assert!(shown.contains("select id from t") && shown.contains("select * from (select id from t) x"), "a preview of each: {shown}");
        let sent = runs(&h.press_fx("enter"));
        assert_eq!(sent.len(), 1, "{keys}");
        let request = &sent[0].2;
        let (a, b) = request.selection.expect("what runs is the listed range");
        assert_eq!(text_between(&request.buffer, a, b), "select id from t", "{keys}: the first entry is the innermost");
        assert!(h.app.ui.overlay.is_none());
    }
}

#[tokio::test]
async fn the_chooser_moves_runs_the_statement_and_esc_cancels_without_losing_text() {
    let mut h = editing().await;
    load(&mut h, SUB);
    h.press("cmd+enter");
    h.press("down");
    let sent = runs(&h.press_fx("enter"));
    let request = &sent[0].2;
    let (a, b) = request.selection.expect("a range");
    assert_eq!(text_between(&request.buffer, a, b), "select * from (select id from t) x");
    load(&mut h, SUB);
    h.press("cmd+enter");
    assert!(runs(&h.press_fx("esc")).is_empty());
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.editor.text(), plain(SUB));
    // Not inside a subquery: no chooser, as before.
    let mut h = editing().await;
    load(&mut h, "select * from t‸ where a = 1");
    let sent = runs(&h.press_fx("cmd+enter"));
    assert_eq!(sent.len(), 1);
    assert!(h.app.ui.overlay.is_none());
    // A selection runs as it is.
    let mut h = editing().await;
    load(&mut h, "select * from (‹select id› from t) x");
    let sent = runs(&h.press_fx("cmd+enter"));
    assert_eq!(sent.len(), 1);
}

#[tokio::test]
async fn the_run_at_caret_setting_picks_the_smallest_the_statement_or_asks() {
    let mut h = editing().await;
    h.app.model.settings.run_at_caret = RunAtCaret::Smallest;
    load(&mut h, SUB);
    let sent = runs(&h.press_fx("cmd+enter"));
    assert_eq!(sent.len(), 1, "no chooser");
    let (a, b) = sent[0].2.selection.expect("the subquery as a range");
    assert_eq!(text_between(&sent[0].2.buffer, a, b), "select id from t");
    let mut h = editing().await;
    h.app.model.settings.run_at_caret = RunAtCaret::Statement;
    load(&mut h, SUB);
    let sent = runs(&h.press_fx("cmd+enter"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2.selection, None, "the whole statement, as the Core picks it");
    assert!(h.app.ui.overlay.is_none());
}

// ---- help and hints -------------------------------------------------------------------------

#[tokio::test]
async fn the_help_overlay_lists_every_binding_and_marks_those_this_terminal_cannot_deliver() {
    let mut h = editing().await;
    h.app.ui.caps = Capabilities { keyboard_protocol: false };
    h.press("ctrl+g ?");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Help(_))));
    let shown = screen(&mut h);
    let run_row = shown.lines().find(|l| l.contains("ctrl+g r")).unwrap_or_else(|| panic!("{shown}"));
    assert!(run_row.contains("f5") && !run_row.contains("[f5]"), "f5 is deliverable: {run_row}");
    assert!(run_row.contains("[cmd+enter]") && run_row.contains("[ctrl+enter]"), "no protocol: {run_row}");
    assert!(run_row.contains("ctrl+g r") && !run_row.contains("[ctrl+g r]"), "the prefix is deliverable: {run_row}");
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    // With the protocol nothing is marked.
    h.app.ui.caps = Capabilities { keyboard_protocol: true };
    h.press("ctrl+g ?");
    let shown = screen(&mut h);
    let run_row = shown.lines().find(|l| l.contains("ctrl+g r")).unwrap_or_else(|| panic!("{shown}"));
    assert!(run_row.contains("cmd+enter") && !run_row.contains('['), "{run_row}");
}

#[tokio::test]
async fn the_help_overlay_scrolls_and_lists_the_editing_actions_with_all_their_bindings() {
    let mut h = editing().await;
    h.app.ui.caps = Capabilities { keyboard_protocol: true };
    h.press("ctrl+g ?");
    let mut found = String::new();
    for _ in 0..12 {
        let shown = screen(&mut h);
        if let Some(row) = shown.lines().find(|l| l.contains("duplicate")) {
            found = row.to_string();
            break;
        }
        h.press("pagedown");
    }
    for key in ["cmd+d", "ctrl+d", "ctrl+g d"] {
        assert!(found.contains(key), "{key} in {found:?}");
    }
}

#[tokio::test]
async fn the_mode_bar_still_shows_f5_first_and_the_prefix_bar_offers_help() {
    let mut h = editing().await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("f5 run"), "{bar}");
    h.press("ctrl+g");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("? help") && bar.contains("esc back"), "{bar}");
}

#[tokio::test]
async fn every_edit_binding_is_in_the_live_keybindings_by_action() {
    let h = editing().await;
    let keys = &h.app.ui.keys;
    let all = |a: Action| keys.all_keys(a);
    assert_eq!(all(Action::Duplicate), ["cmd+d", "ctrl+d", "ctrl+g d"]);
    assert_eq!(all(Action::DeleteLine), ["cmd+backspace", "ctrl+y", "ctrl+g x"]);
    assert_eq!(all(Action::MoveLineUp), ["alt+shift+up"]);
    assert_eq!(all(Action::MoveStatementDown), ["cmd+shift+down", "ctrl+shift+down"]);
    assert_eq!(all(Action::LineComment), ["cmd+/", "ctrl+/", "ctrl+g /"]);
    assert_eq!(all(Action::GotoLine), ["cmd+l", "ctrl+g g"]);
    assert_eq!(all(Action::RunDirect), ["f5", "cmd+enter", "ctrl+enter"]);
    assert_eq!(all(Action::Undo), ["cmd+z", "ctrl+z", "ctrl+g u"]);
    assert_eq!(all(Action::Redo), ["cmd+shift+z", "ctrl+shift+z", "ctrl+g U"]);
    // Rebinding an action replaces its keys.
    let mut keys = keys.clone();
    assert!(keys.rebind(Action::Duplicate, "ctrl+j"));
    assert_eq!(keys.all_keys(Action::Duplicate), ["ctrl+j", "ctrl+g d"]);
}
