//! Layer 2: the editor as drawn (slice 10): syntax colours at the right
//! cells, line numbers, the current line, the matching bracket, the selection
//! over the colours, the outline of the statement that would run, and the
//! work all that costs.

use ratatui::style::Modifier;

use super::run_tests::on_pagila;
use super::testing::*;
use super::theme::{Palette, Role};
use crate::harness::Harness;

/// An editor on a Postgres database holding `text`, drawn once, with the
/// caret at the end of it.
async fn editor_with(text: &str) -> Harness {
    let mut h = on_pagila().await;
    h.app.ui.tab.editor.set_text(text);
    frame_rows(&mut h.app, 114, 30);
    h
}

/// The cell at character column `col` of text row `row`, assuming nothing
/// before it is wider than one cell.
fn at(h: &Harness, col: u16, row: u16) -> (u16, u16) {
    let t = h.app.ui.hits.editor_text;
    (t.x + col, t.y + row)
}

fn fg(h: &mut Harness, col: u16, row: u16) -> Option<Role> {
    let (x, y) = at(h, col, row);
    role_at(&mut h.app, x, y).fg
}

fn bg(h: &mut Harness, col: u16, row: u16) -> Option<Role> {
    let (x, y) = at(h, col, row);
    role_at(&mut h.app, x, y).bg
}

#[tokio::test]
async fn each_kind_of_token_is_drawn_in_its_role_at_its_cell() {
    let mut h = editor_with("select 'a', 12 from t -- note").await;
    for (col, role) in [
        (0, Role::SynKeyword),
        (5, Role::SynKeyword),
        (7, Role::SynString),
        (9, Role::SynString),
        (12, Role::SynNumber),
        (15, Role::SynKeyword),
        (20, Role::SynIdentifier),
        (22, Role::SynComment),
        (28, Role::SynComment),
    ] {
        assert_eq!(fg(&mut h, col, 0), Some(role), "column {col}");
    }
}

#[tokio::test]
async fn a_function_call_is_italic_and_types_come_from_the_core() {
    let mut h = editor_with("select count(*), x::varchar").await;
    assert_eq!(fg(&mut h, 7, 0), Some(Role::SynFunction));
    let (x, y) = at(&h, 7, 0);
    assert!(style_at(&mut h.app, x, y).2.contains(Modifier::ITALIC));
    assert_eq!(fg(&mut h, 20, 0), Some(Role::SynType), "varchar, from the fake Core's type names");
}

#[tokio::test]
async fn text_before_a_keyword_that_is_wide_does_not_move_its_colour() {
    let mut h = editor_with("select '日本語' from t").await;
    // The string is 5 characters but 8 cells: `from` starts at cell 16, not 13.
    assert_eq!(fg(&mut h, 16, 0), Some(Role::SynKeyword), "the f of from, at its display column");
    assert_eq!(fg(&mut h, 19, 0), Some(Role::SynKeyword));
    assert_eq!(fg(&mut h, 14, 0), Some(Role::SynString), "still inside the string's cells");
    assert_eq!(fg(&mut h, 21, 0), Some(Role::SynIdentifier), "the t");
}

#[tokio::test]
async fn an_unterminated_string_is_drawn_as_a_string_to_the_end() {
    let mut h = editor_with("select 1, 'abc\nstill in it").await;
    assert_eq!(fg(&mut h, 0, 0), Some(Role::SynKeyword));
    assert_eq!(fg(&mut h, 7, 0), Some(Role::SynNumber));
    assert_eq!(fg(&mut h, 11, 0), Some(Role::SynString));
    assert_eq!(fg(&mut h, 0, 1), Some(Role::SynString));
    assert_eq!(fg(&mut h, 9, 1), Some(Role::SynString));
}

#[tokio::test]
async fn the_bracket_beside_the_caret_and_its_partner_have_the_matched_brace_background() {
    let mut h = editor_with("select count(*) from t").await;
    h.press("home");
    for _ in 0..13 {
        h.press("right");
    }
    assert_eq!(bg(&mut h, 12, 0), Some(Role::MatchedBrace), "the ( before the caret");
    assert_eq!(bg(&mut h, 14, 0), Some(Role::MatchedBrace), "its partner");
    assert_ne!(bg(&mut h, 13, 0), Some(Role::MatchedBrace));
    assert_ne!(bg(&mut h, 20, 0), Some(Role::MatchedBrace));
    h.press("right right right");
    assert_ne!(bg(&mut h, 14, 0), Some(Role::MatchedBrace), "the caret left the bracket");
}

#[tokio::test]
async fn a_selection_has_the_selection_background_and_keeps_the_syntax_colours() {
    let mut h = editor_with("select 1 from t").await;
    h.press("home shift+right shift+right shift+right");
    for col in 0..3 {
        assert_eq!(bg(&mut h, col, 0), Some(Role::SelectionBg), "column {col}");
        assert_eq!(fg(&mut h, col, 0), Some(Role::SynKeyword), "still readable as a keyword");
    }
    assert_ne!(bg(&mut h, 3, 0), Some(Role::SelectionBg));
    assert_ne!(bg(&mut h, 12, 0), Some(Role::SelectionBg));
}

#[tokio::test]
async fn line_numbers_fill_the_gutter_and_the_caret_rows_number_is_brighter() {
    let mut h = editor_with("select 1;\nselect 2;\nselect 3;").await;
    h.press("up");
    let rows = frame_rows(&mut h.app, 114, 30);
    let g = h.app.ui.hits.editor_gutter;
    let t = h.app.ui.hits.editor_text;
    assert!(g.width >= 3 && g.right() == t.x, "{g:?} {t:?}");
    for (i, n) in ["1", "2", "3"].iter().enumerate() {
        let row: String = rows[(t.y as usize) + i].chars().skip(g.x as usize).take(g.width as usize).collect();
        assert_eq!(row.trim().trim_start_matches('▎').trim(), *n, "gutter row {i}: {row:?}");
    }
    let number_x = g.right() - 2;
    assert_eq!(role_at(&mut h.app, number_x, t.y + 1).fg, Some(Role::LineNumberActive), "the caret row");
    assert_eq!(role_at(&mut h.app, number_x, t.y).fg, Some(Role::LineNumber));
    assert_eq!(role_at(&mut h.app, number_x, t.y + 2).fg, Some(Role::LineNumber));
}

#[tokio::test]
async fn the_caret_row_has_the_current_line_background_across_the_pane() {
    let mut h = editor_with("select 1;\nselect 2;").await;
    h.press("up");
    let t = h.app.ui.hits.editor_text;
    assert_eq!(bg(&mut h, 0, 0), Some(Role::CurrentLine));
    assert_eq!(bg(&mut h, t.width - 2, 0), Some(Role::CurrentLine), "to the far edge, past the text");
    assert_eq!(bg(&mut h, 0, 1), Some(Role::EditorBg));
}

// ---- the statement that would run ------------------------------------------------

const THREE: &str = "select 1;\nselect 2\n  from t;\nselect 3;";

/// The rows of the gutter that carry the range-to-execute bar.
fn bar_rows(h: &mut Harness) -> Vec<u16> {
    let rows = frame_rows(&mut h.app, 114, 30);
    let g = h.app.ui.hits.editor_gutter;
    let t = h.app.ui.hits.editor_text;
    (0..4)
        .filter(|&r| {
            let y = t.y + r;
            let shown = rows[y as usize].chars().skip(g.x as usize).take(g.width as usize).collect::<String>().contains('▎');
            let mut painted = false;
            for x in g.x..g.right() {
                painted |= role_at(&mut h.app, x, y).fg == Some(Role::RangeToExecute);
            }
            shown && painted
        })
        .collect()
}

#[tokio::test]
async fn only_the_statement_under_the_caret_has_the_bar_and_moving_the_caret_moves_it() {
    let mut h = editor_with(THREE).await;
    assert_eq!(bar_rows(&mut h), [3], "the caret is at the end, in the third statement");
    h.press("up up");
    assert_eq!(bar_rows(&mut h), [1, 2], "the second statement spans two rows");
    h.press("up");
    assert_eq!(bar_rows(&mut h), [0]);
    h.press("down down down");
    assert_eq!(bar_rows(&mut h), [3], "to the next statements, the outline follows");
}

#[tokio::test]
async fn a_selection_is_what_runs_so_no_statement_is_outlined_then() {
    let mut h = editor_with(THREE).await;
    h.press("up up shift+right");
    assert_eq!(bar_rows(&mut h), Vec::<u16>::new());
}

#[tokio::test]
async fn without_a_target_nothing_is_outlined_but_the_text_is_still_coloured() {
    let mut h = started_without_target().await;
    h.press("ctrl+g l");
    h.app.ui.tab.editor.set_text("select 1;");
    frame_rows(&mut h.app, 114, 30);
    assert_eq!(bar_rows(&mut h), Vec::<u16>::new());
    assert_eq!(fg(&mut h, 0, 0), Some(Role::SynKeyword));
}

async fn started_without_target() -> Harness {
    super::run_tests::started(&["local pg"]).await
}

// ---- themes -----------------------------------------------------------------------

#[tokio::test]
async fn switching_to_intellij_dark_changes_the_keyword_cell_to_its_colour() {
    let mut h = editor_with("select 1").await;
    let (x, y) = at(&h, 0, 0);
    let (before, ..) = style_at(&mut h.app, x, y);
    assert_eq!(before, Palette::default_theme().color(Role::Peach));
    h.app.ui.theme = Palette::by_name("intellij-dark").unwrap();
    let (after, ..) = style_at(&mut h.app, x, y);
    let dark = Palette::by_name("intellij-dark").unwrap();
    assert_eq!(after, dark.color(Role::SynKeyword));
    assert_ne!(after, before, "the keyword cell changed colour");
    // The editor's background is IntelliJ's too.
    let (_, back, _) = style_at(&mut h.app, x + 30, y + 3);
    assert_eq!(back, dark.color(Role::EditorBg));
}

// ---- work done ----------------------------------------------------------------------

#[tokio::test]
async fn type_names_are_asked_once_per_kind_and_the_range_once_per_text_and_caret() {
    let mut h = editor_with("select 1;\nselect 2;").await;
    for _ in 0..5 {
        frame_rows(&mut h.app, 114, 30);
    }
    let calls = h.fake.syntax_calls();
    assert_eq!(calls.type_names.len(), 1, "{calls:?}");
    assert_eq!(calls.statement_ranges, 1, "five frames, one question: {calls:?}");
    h.press("up");
    frame_rows(&mut h.app, 114, 30);
    frame_rows(&mut h.app, 114, 30);
    assert_eq!(h.fake.syntax_calls().statement_ranges, 2, "the caret moved");
    h.type_text("x");
    frame_rows(&mut h.app, 114, 30);
    assert_eq!(h.fake.syntax_calls().statement_ranges, 3, "the text changed");
    assert_eq!(h.fake.syntax_calls().type_names.len(), 1, "still the one kind");
}

#[tokio::test]
async fn a_5000_line_buffer_is_tokenized_once_per_edit_and_not_per_redraw_or_caret_move() {
    let text: String = (0..5000).map(|i| format!("select {i}, 'x' from t;")).collect::<Vec<_>>().join("\n");
    let mut h = editor_with(&text).await;
    frame_rows(&mut h.app, 114, 30);
    let first = h.app.ui.tab.syntax.stats();
    assert_eq!(first.lines_tokenized, 5000 * first.runs, "{first:?}");
    frame_rows(&mut h.app, 114, 30);
    h.press("up up left home");
    frame_rows(&mut h.app, 114, 30);
    h.press("pageup");
    frame_rows(&mut h.app, 114, 30);
    assert_eq!(h.app.ui.tab.syntax.stats(), first, "redraws and caret moves tokenize nothing");
    h.type_text("x");
    frame_rows(&mut h.app, 114, 30);
    let after = h.app.ui.tab.syntax.stats();
    assert_eq!(after.runs, first.runs + 1, "{first:?} -> {after:?}");
    // The view is far from the top, and the colours are right where it is.
    let t = h.app.ui.hits.editor_text;
    assert_eq!(role_at(&mut h.app, t.x, t.y).fg, Some(Role::SynKeyword));
}

#[tokio::test]
async fn a_comment_opened_far_above_the_view_colours_what_is_in_view_as_comment() {
    let mut lines: Vec<String> = (0..1000).map(|i| format!("select {i};")).collect();
    lines[100] = "/* it's".to_string();
    lines[990] = "*/".to_string();
    let mut h = editor_with(&lines.join("\n")).await;
    frame_rows(&mut h.app, 114, 30);
    frame_rows(&mut h.app, 114, 30);
    assert!(h.app.ui.tab.editor.scroll_top().0 > 900, "the view is far below the comment's start");
    let t = h.app.ui.hits.editor_text;
    assert_eq!(role_at(&mut h.app, t.x + 2, t.y + 3).fg, Some(Role::SynComment));
}

#[tokio::test]
async fn painting_a_very_long_line_walks_only_what_fits_on_screen_and_still_draws_the_caret() {
    let long = "select 1, 'ab' from t where x = 12; ".repeat(450); // 16 KiB
    assert!(long.len() > 16_000);
    let mut h = editor_with(&format!("{long}\nselect 2")).await;
    h.press("up home");
    frame_rows(&mut h.app, 114, 30);
    let t = h.app.ui.hits.editor_text;
    let walked = h.app.ui.tab.syntax.graphemes_painted;
    assert!(walked < 2 * t.width as usize + 20, "walked {walked} graphemes for a {} column pane", t.width);
    // The caret (reversed) is at the start of the long line.
    let (x, y) = at(&h, 0, 0);
    assert!(style_at(&mut h.app, x, y).2.contains(Modifier::REVERSED));
    assert_eq!(fg(&mut h, 40, 0), Some(Role::SynKeyword), "the visible part is coloured");
    // A selection running past the line's end must not make it walk it all.
    h.press("shift+down");
    frame_rows(&mut h.app, 114, 30);
    let walked = h.app.ui.tab.syntax.graphemes_painted;
    assert!(walked < 2 * t.width as usize + 40, "walked {walked} with a selection");
}
