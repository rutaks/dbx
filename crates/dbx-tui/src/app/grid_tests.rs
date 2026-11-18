//! Layer 2: the acceptance tests of slice 07 (the result grid), run on `App`
//! with a `FakeCore` behind the shell's command runner.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::run_tests::*;
use super::testing::*;
use super::theme::Role;
use super::view::{MISSING_MARK, NEWLINE_MARK, NULL_MARK, TAB_MARK};
use super::overlay;
use super::query::{self, Outcome};
use crate::core::{CoreErrorKind, Cursor};
use super::*;
use crate::core::{Cell, CellKind, ResultSet};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

/// `rows` by `cols` of text: column `c{i}`, cell `r{row}c{col}`.
pub(super) fn text_set(rows: usize, cols: usize) -> ResultSet {
    ResultSet {
        columns: (0..cols).map(|c| col(&format!("c{c}"), "text", CellKind::Text)).collect(),
        rows: (0..rows).map(|r| (0..cols).map(|c| Cell::Value(format!("r{r}c{c}"))).collect()).collect(),
        ..rows_of(&[])
    }
}

/// A result shown, with the results pane focused.
pub(super) async fn focused(set: ResultSet) -> Harness {
    let mut h = ran(Ok(set)).await;
    h.press("ctrl+g j");
    assert_eq!(h.app.ui.focus, PaneId::Results);
    h
}

pub(super) const W: u16 = 100;
pub(super) const H: u16 = 30;

pub(super) fn screen(h: &mut Harness) -> Vec<String> {
    frame_text(&mut h.app, W, H)
}

/// Where `text` is on screen, in cells (a wide character takes two).
pub(super) fn where_is(h: &mut Harness, text: &str) -> Option<(u16, u16)> {
    screen(h).iter().enumerate().find_map(|(y, row)| {
        let byte = row.find(text)?;
        Some((UnicodeWidthStr::width(&row[..byte]) as u16, y as u16))
    })
}

/// The results pane's rows, from the pane's left edge.
pub(super) fn pane(h: &mut Harness) -> Vec<String> {
    let rows = screen(h);
    let r = h.app.ui.hits.results;
    rows[r.y as usize..r.bottom() as usize]
        .iter()
        .map(|row| row.chars().skip(r.x as usize + 1).collect::<String>().trim_end().to_string())
        .collect()
}

pub(super) fn cursor(h: &Harness) -> (usize, usize) {
    (h.app.ui.grid.row, h.app.ui.grid.col)
}

// ---- the cursor -----------------------------------------------------------------

#[tokio::test]
async fn hjkl_and_the_arrows_move_the_cell_cursor_by_one_and_stop_at_the_edges() {
    let mut h = focused(text_set(5, 4)).await;
    screen(&mut h);
    assert_eq!(cursor(&h), (0, 0));
    h.press("l");
    assert_eq!(cursor(&h), (0, 1));
    h.press("j");
    assert_eq!(cursor(&h), (1, 1));
    h.press("right");
    assert_eq!(cursor(&h), (1, 2));
    h.press("down");
    assert_eq!(cursor(&h), (2, 2));
    h.press("h left k up");
    assert_eq!(cursor(&h), (0, 0));
    h.press("k h up left");
    assert_eq!(cursor(&h), (0, 0), "stops at the first row and column");
    h.press("j j j j j j l l l l l");
    assert_eq!(cursor(&h), (4, 3), "stops at the last row and column");
}

#[tokio::test]
async fn pages_the_ends_of_a_row_and_the_first_and_last_row_move_the_cursor() {
    let mut h = focused(text_set(200, 30)).await;
    screen(&mut h);
    let page = h.app.ui.grid.rows_visible;
    assert!(page >= 5, "a page is more than a few rows: {page}");
    h.press("pagedown");
    assert_eq!(cursor(&h), (page, 0));
    h.press("ctrl+d");
    assert_eq!(cursor(&h), (2 * page, 0));
    h.press("pageup ctrl+u");
    assert_eq!(cursor(&h), (0, 0));
    h.press("G");
    assert_eq!(cursor(&h), (199, 0));
    h.press("pagedown");
    assert_eq!(cursor(&h), (199, 0), "a page down at the end stays on the last row");
    h.press("g");
    assert_eq!(cursor(&h), (0, 0));
    h.press("end");
    assert_eq!(cursor(&h), (0, 29));
    h.press("home");
    assert_eq!(cursor(&h), (0, 0));
    h.press("$");
    assert_eq!(cursor(&h), (0, 29));
    h.press("0");
    assert_eq!(cursor(&h), (0, 0));
}

#[tokio::test]
async fn with_10_000_rows_and_120_columns_the_last_row_and_column_stay_on_screen_under_their_header() {
    let mut h = focused(text_set(10_000, 120)).await;
    screen(&mut h);
    h.press("G $");
    assert_eq!(cursor(&h), (9_999, 119));
    let (hx, hy) = where_is(&mut h, "c119").expect("the header of the last column");
    let (cx, cy) = where_is(&mut h, "r9999c119").expect("the last cell");
    assert_eq!(hx, cx, "the header is over its column");
    assert!(hy < cy, "and above it");
    assert_eq!(role_at(&mut h.app, cx, cy).bg, Some(Role::SelectionBg), "the cursor cell is the one drawn selected");
    assert!(cy < H - 1 && cx < W, "inside the screen");
}

#[tokio::test]
async fn the_header_and_the_row_number_gutter_stay_while_the_rows_scroll() {
    let mut h = focused(text_set(10_000, 6)).await;
    screen(&mut h);
    let header_y = where_is(&mut h, "c0").unwrap().1;
    h.press("G");
    let rows = pane(&mut h);
    assert!(rows.iter().any(|r| r.starts_with("10000 ") || r.trim() == "10000" || r.starts_with("10000")), "the last row's number in the gutter: {rows:#?}");
    assert_eq!(where_is(&mut h, "c0").unwrap().1, header_y, "the header did not move");
    let last = rows.iter().find(|r| r.contains("r9999c0")).expect("the last row");
    assert!(last.starts_with("10000"), "its number is at the gutter: {last:?}");
    assert!(!rows.iter().any(|r| r.contains("r0c0")), "the first row has scrolled away");
}

#[tokio::test]
async fn columns_scroll_whole_and_the_cursor_column_is_drawn_complete() {
    let mut h = focused(text_set(30, 40)).await;
    screen(&mut h);
    for _ in 0..20 {
        h.press("l");
        screen(&mut h);
    }
    let g = h.app.ui.grid.clone();
    assert!(g.left > 0, "scrolled right: {g:?}");
    assert!(g.left <= g.col && g.col < g.left + g.cols_visible, "the cursor column is among the drawn ones: {g:?}");
    let rows = pane(&mut h);
    let header = rows.iter().find(|r| r.contains("c20")).expect("the cursor column's header");
    assert!(header.contains(&format!("c{} ", g.left)), "the first drawn column is whole: {header:?}");
    assert!(rows.iter().any(|r| r.contains("r0c20")), "its cell is whole: {rows:#?}");
    assert!(!header.contains("c0 ") && !header.contains("c1 "), "the first columns are scrolled away: {header:?}");
}

#[tokio::test]
async fn a_column_wider_than_the_pane_shows_its_left_part() {
    let mut set = text_set(3, 2);
    set.rows[0][1] = Cell::Value("x".repeat(80));
    let mut h = focused(set).await;
    h.press("l");
    let screen = frame_rows(&mut h.app, 60, 20);
    let r = h.app.ui.hits.results;
    let row = screen.iter().find(|r| r.contains("xxxx")).unwrap_or_else(|| panic!("{screen:#?}"));
    assert!(row.trim_end().ends_with('…'), "cut with an ellipsis: {row:?}");
    assert!(row.chars().count() <= (r.x + r.width) as usize);
    assert_eq!(h.app.ui.grid.left, 1, "the wide column starts the window");
    assert_eq!(cursor(&h), (0, 1));
}

#[tokio::test]
async fn a_scrollbar_and_position_indicators_show_when_the_result_overflows() {
    let mut h = focused(text_set(10_000, 120)).await;
    let rows = screen(&mut h);
    let r = h.app.ui.hits.results;
    let bar_x = (r.right() - 1) as usize;
    assert!(rows[r.y as usize + 3..r.bottom() as usize].iter().any(|row| row.chars().nth(bar_x) == Some('▕')), "a ▕ scrollbar on the right: {rows:#?}");
    let title = &rows[r.y as usize];
    assert!(title.contains("r 1/10,000") && title.contains("c 1/120"), "{title:?}");
    h.press("G $");
    let title = screen(&mut h)[r.y as usize].clone();
    assert!(title.contains("r 10,000/10,000") && title.contains("c 120/120"), "{title:?}");

    let mut small = focused(text_set(3, 2)).await;
    let rows = screen(&mut small);
    let r = small.app.ui.hits.results;
    assert!(!rows[r.y as usize + 1..r.bottom() as usize].iter().any(|row| row.contains('▕')), "no scrollbar when everything fits");
}

#[tokio::test]
async fn the_cursor_cell_is_selection_bg_and_its_row_gets_the_active_row_background() {
    let mut h = focused(text_set(6, 3)).await;
    h.press("j l");
    let (x, y) = where_is(&mut h, "r1c1").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).bg, Some(Role::SelectionBg));
    let (x0, _) = where_is(&mut h, "r1c0").unwrap();
    assert_eq!(role_at(&mut h.app, x0, y).bg, Some(Role::ActiveRowBg), "the rest of the cursor's row");
    let (xo, yo) = where_is(&mut h, "r2c0").unwrap();
    assert_ne!(role_at(&mut h.app, xo, yo).bg, Some(Role::ActiveRowBg), "other rows are plain");
}


// ---- widths and glyphs ---------------------------------------------------------------

fn two_text_columns(a: &[&str], b: &[&str]) -> ResultSet {
    ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text)],
        rows: a.iter().zip(b).map(|(x, y)| vec![Cell::Value(x.to_string()), Cell::Value(y.to_string())]).collect(),
        ..rows_of(&[])
    }
}

#[tokio::test]
async fn a_column_of_wide_characters_is_as_wide_as_its_display_width_and_the_next_column_starts_right() {
    // `日本語 😀` is 6 + 1 + 2 = 9 cells wide in 5 characters.
    let mut h = focused(two_text_columns(&["日本語 😀", "x"], &["Z1", "Z2"])).await;
    let (ax, ay) = where_is(&mut h, "日本語 😀").expect("the wide cell");
    let (bx, by) = where_is(&mut h, "Z1").unwrap();
    assert_eq!(ay, by);
    assert_eq!(bx, ax + 9 + 2, "9 cells and a gap");
    let (hx, _) = where_is(&mut h, "b").unwrap();
    assert_eq!(hx, bx, "the header of the next column is over it");
    let (zx, _) = where_is(&mut h, "Z2").unwrap();
    assert_eq!(zx, bx, "and so is the second row");
}

#[tokio::test]
async fn combining_marks_and_zero_width_joiners_take_the_width_of_what_they_draw() {
    // `é` as e + U+0301 is one cell; the family emoji (a ZWJ sequence) is two.
    let family = "👨\u{200d}👩\u{200d}👧";
    let mut h = focused(two_text_columns(&["e\u{301}e\u{301}e\u{301}", family], &["B1", "B2"])).await;
    let (ax, _) = where_is(&mut h, "e\u{301}e\u{301}e\u{301}").unwrap();
    let (bx, _) = where_is(&mut h, "B1").unwrap();
    // The column is 3 wide (its minimum, and the text's width), then the gap.
    assert_eq!(bx, ax + 3 + 2, "three marks, three cells");
    // The ZWJ sequence is one two-cell character: drawn whole (not split into
    // its three members), and the column after it starts where it does on
    // the row above.
    assert_eq!(UnicodeWidthStr::width(family), 2, "the premise: a ZWJ sequence is two cells");
    let (fx, fy) = where_is(&mut h, family).expect("the family emoji is drawn whole");
    assert_eq!(fx, ax, "in its column");
    let (b2x, b2y) = where_is(&mut h, "B2").unwrap();
    assert_eq!(fy, b2y, "on its row");
    assert_eq!(b2x, bx, "the second column starts at the same place under the ZWJ sequence");
    assert!(!screen(&mut h).iter().any(|r| r.contains('\u{200d}') && !r.contains(family)), "never split");
}

#[tokio::test]
async fn a_cell_is_drawn_whole_or_ends_in_an_ellipsis_even_for_lam_alef_pairs() {
    // unicode-width counts the lam-alef pair as one cell, the painter draws
    // two graphemes of one cell each: the cut must follow what is drawn.
    let value = "لالالالا";
    let mut h = focused(two_text_columns(&[value], &["B1"])).await;
    let row = pane(&mut h).into_iter().find(|r| r.contains("B1")).expect("the row");
    assert!(row.contains(value) != row.contains('…'), "drawn whole, or cut with an ellipsis, not both or neither: {row:?}");
}

#[tokio::test]
async fn column_widths_stay_between_three_and_forty_cells() {
    let long = "x".repeat(100);
    let mut h = focused(two_text_columns(&["Q7", "Q8"], &[&long, &long])).await;
    let (ax, _) = where_is(&mut h, "Q7").unwrap();
    // The first column is `a`'s header and `Q7`: at most 2 wide, drawn 3 wide.
    let (bx, y) = where_is(&mut h, "xxx").unwrap();
    assert_eq!(bx, ax + 3 + 2, "the narrow column takes the minimum");
    let rows = screen(&mut h);
    let row = &rows[y as usize];
    let start = row.find("xxx").unwrap();
    let cut: String = row[start..].chars().take_while(|c| *c == 'x' || *c == '…').collect();
    assert_eq!(cut.chars().count(), 40, "{cut:?}");
    assert!(cut.ends_with('…'), "cut with an ellipsis: {cut:?}");
}

#[tokio::test]
async fn newlines_tabs_and_control_characters_never_reach_the_screen_raw() {
    let mut h = focused(two_text_columns(&["a\nb\r\nc", "t\tt"], &["\u{1b}[31mred", "bell\u{7}"])).await;
    let text = pane(&mut h).join("\n");
    assert!(text.contains(&format!("a{NEWLINE_MARK}b{NEWLINE_MARK}c")), "{text}");
    assert!(text.contains(&format!("t{TAB_MARK}t")), "{text}");
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'), "{text:?}");
}

#[tokio::test]
async fn zero_columns_and_zero_rows_does_not_panic_and_keys_do_nothing() {
    for set in [
        ResultSet { columns: vec![], rows: vec![], ..rows_of(&[]) },
        ResultSet { columns: vec![], rows: vec![vec![Cell::Value("stray".into())]], ..rows_of(&[]) },
    ] {
        let mut h = focused(set).await;
        screen(&mut h);
        h.press("j l k h G $ g 0 pagedown pageup enter y Y m end home");
        screen(&mut h);
        assert!(h.app.ui.overlay.is_none());
    }
}

#[tokio::test]
async fn an_empty_result_draws_its_header_and_0_rows_and_the_cursor_keys_do_nothing() {
    let mut h = focused(rows_of(&[])).await;
    let rows = pane(&mut h);
    assert!(rows.iter().any(|r| r.trim() == "v"), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("0 rows")), "{rows:#?}");
    h.press("j l G pagedown enter");
    screen(&mut h);
    assert_eq!(cursor(&h), (0, 0));
    assert!(h.app.ui.overlay.is_none(), "nothing to open");
}

#[tokio::test]
async fn duplicate_and_empty_column_names_are_drawn_as_given() {
    let mut set = text_set(2, 3);
    set.columns[0].name = "dup".into();
    set.columns[1].name = "dup".into();
    set.columns[2].name = String::new();
    let mut h = focused(set).await;
    let header = pane(&mut h).into_iter().find(|r| r.contains("dup")).unwrap();
    assert_eq!(header.matches("dup").count(), 2, "{header:?}");
    assert!(where_is(&mut h, "r0c2").is_some(), "the column with no name still draws its cells");
}

#[tokio::test]
async fn rows_shorter_or_longer_than_the_header_do_not_panic_and_a_missing_cell_has_its_own_glyph() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text)],
        rows: vec![vec![v("only")], vec![v("x"), Cell::Null, v("extra"), v("more")], vec![]],
        ..rows_of(&[])
    };
    let mut h = focused(set).await;
    let rows = pane(&mut h);
    assert!(rows.iter().any(|r| r.contains("only") && r.contains(MISSING_MARK)), "a short row: {rows:#?}");
    assert!(rows.iter().any(|r| r.contains('x') && r.contains(NULL_MARK) && !r.contains("extra")), "a long row, extras ignored: {rows:#?}");
    let (mx, my) = where_is(&mut h, MISSING_MARK).unwrap();
    assert_ne!(MISSING_MARK, NULL_MARK, "not the NULL glyph");
    assert_eq!(role_at(&mut h.app, mx, my).fg, Some(Role::Peach));
    h.press("j j l G $ y Y enter");
    screen(&mut h);
}

#[tokio::test]
async fn a_move_inside_the_sampled_rows_measures_nothing_and_a_jump_measures_a_bounded_amount() {
    let mut h = focused(text_set(10_000, 120)).await;
    screen(&mut h);
    let start = h.app.ui.grid.widths.measured;
    assert!(start > 0 && start < 20_000, "the first frame measured only the window: {start}");
    h.press("j j j l l");
    screen(&mut h);
    assert_eq!(h.app.ui.grid.widths.measured, start, "moving inside the window rescans nothing");
    h.press("G $");
    screen(&mut h);
    let after = h.app.ui.grid.widths.measured - start;
    assert!(after < 20_000, "jumping to the end measured {after} cells, not the 1,200,000 there are");
}

// ---- the cell viewer ------------------------------------------------------------------

const DOC: &str = r#"{"b":[1,2,{"c":null}],"a":"x","big":12345678901234567890.5}"#;

/// id, a JSON `doc` (a document, NULL, '', the text NULL, multi-line text).
fn doc_set() -> ResultSet {
    let v = |s: &str| Cell::Value(s.into());
    ResultSet {
        columns: vec![col("id", "int4", CellKind::Integer), col("doc", "jsonb", CellKind::Json), col("note", "text", CellKind::Text)],
        rows: vec![
            vec![v("1"), v(DOC), v("line1\nline2\n\tindented")],
            vec![v("2"), Cell::Null, v("")],
            vec![v("3"), v(""), v("NULL")],
            vec![v("4"), v("[1, 2"), Cell::Null],
        ],
        ..rows_of(&[])
    }
}

/// The viewer's box as text: the rows from its top border to its bottom, cut
/// to the box (the dimmed grid behind it is not part of it).
fn the_box(h: &mut Harness) -> String {
    let rows = screen(h);
    let top = rows.iter().position(|r| r.contains('┌')).expect("a box");
    let bottom = rows.iter().rposition(|r| r.contains('└')).expect("a box");
    let left = rows[top].chars().position(|c| c == '┌').unwrap();
    rows[top..=bottom].iter().map(|r| r.chars().skip(left).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn viewer(h: &Harness) -> &super::viewer::CellViewer {
    match &h.app.ui.overlay {
        Some(overlay::Overlay::CellViewer(v)) => v,
        other => panic!("no cell viewer open: {other:?}"),
    }
}

#[tokio::test]
async fn a_multi_line_value_shows_a_return_glyph_in_the_grid_and_its_real_lines_in_the_viewer() {
    let mut h = focused(doc_set()).await;
    h.press("l l");
    let grid = pane(&mut h).join("\n");
    assert!(grid.contains(&format!("line1{NEWLINE_MARK}line2{NEWLINE_MARK}{TAB_MARK}indented")), "{grid}");
    h.press("enter");
    let rows = screen(&mut h);
    let line = |t: &str| rows.iter().position(|r| r.contains(t)).unwrap_or_else(|| panic!("{t:?} in {rows:#?}"));
    assert_eq!((line("line1") + 1, line("line2") + 1), (line("line2"), line("indented")), "one line each");
    assert!(rows[line("indented")].contains(&format!("{TAB_MARK}indented")), "a tab is a glyph here too");
    assert!(!rows.join("\n").contains(NEWLINE_MARK), "no return glyph: these are real lines");
}

#[tokio::test]
async fn enter_on_a_json_cell_opens_the_viewer_with_pretty_json_and_esc_returns_to_the_same_cell() {
    let mut h = focused(doc_set()).await;
    h.press("l");
    assert!(h.app.ui.overlay.is_none());
    h.press("enter");
    let v = viewer(&h);
    assert_eq!((v.row, v.col), (0, 1));
    let rows = screen(&mut h);
    let title = rows.iter().find(|r| r.contains("doc") && r.contains("jsonb")).unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(title.contains("row 1"), "the column, its type and the row in the title: {title:?}");
    let text = rows.join("\n");
    for want in ["\"b\": [", "\"a\": \"x\",", "\"big\": 12345678901234567890.5", "\"c\": null"] {
        assert!(text.contains(want), "{want:?} in the pretty form: {text}");
    }
    let line = |t: &str| rows.iter().position(|r| r.contains(t)).unwrap();
    assert!(line("\"b\": [") < line("\"a\": \"x\""), "the keys keep their order");
    assert!(!text.contains(DOC), "not the raw single line");

    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.focus, PaneId::Results);
    assert_eq!(cursor(&h), (0, 1), "back on the same cell");
}

#[tokio::test]
async fn the_raw_key_shows_the_stored_json_and_back_and_copy_always_copies_the_stored_text() {
    let mut h = focused(doc_set()).await;
    h.press("l enter");
    let sent = h.press_fx("y");
    assert_eq!(sent, vec![Command::CopyToClipboard(DOC.to_string())], "the raw value, never the pretty form");
    h.press("r");
    assert!(viewer(&h).raw);
    assert!(screen(&mut h).iter().any(|r| r.contains(DOC)), "the stored text, as one line");
    let sent = h.press_fx("y");
    assert_eq!(sent, vec![Command::CopyToClipboard(DOC.to_string())]);
    h.press("r");
    assert!(!screen(&mut h).iter().any(|r| r.contains(DOC)));
    // A cell that is not JSON has nothing to toggle, and says it is not valid.
    h.press("esc j j j enter");
    h.app.ui.notice = None; // what the copies said has not expired yet
    assert_eq!(viewer(&h).value.as_deref(), Some("[1, 2"));
    assert!(!viewer(&h).can_toggle());
    let text = screen(&mut h).join("\n");
    assert!(text.contains("[1, 2") && text.contains("not valid JSON"), "{text}");
}

#[tokio::test]
async fn null_the_empty_string_and_the_text_null_look_different_in_the_viewer() {
    let mut h = focused(doc_set()).await;
    let shown = |h: &mut Harness| the_box(h);
    h.press("j l enter"); // the NULL doc
    let null = shown(&mut h);
    assert!(null.contains(NULL_MARK) && null.contains("NULL"), "NULL is its glyph and its name: {null}");
    assert!(!null.contains("empty string"));
    let (x, y) = where_is(&mut h, NULL_MARK).unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Overlay0), "dimmed");
    assert_eq!(h.press_fx("y"), vec![Command::CopyToClipboard(String::new())], "NULL copies as nothing");
    h.press("esc j enter"); // the empty doc
    let empty = shown(&mut h);
    assert!(empty.contains("empty string") && !empty.contains(NULL_MARK), "{empty}");
    h.app.ui.notice = None;
    h.press("esc l"); // note = the text NULL (row 3 is `j` away from row 2: here we are on row 2, col 1)
    h.press("l");
    h.press("enter");
    let text = shown(&mut h);
    assert!(text.contains("NULL") && !text.contains(NULL_MARK) && !text.contains("empty string"), "the text NULL is text: {text}");
    let (x, y) = where_is(&mut h, "NULL").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Text));
}

#[tokio::test]
async fn the_viewer_takes_all_input_dims_what_is_behind_and_the_keys_do_not_reach_the_grid() {
    let mut h = focused(doc_set()).await;
    screen(&mut h);
    let (hx, hy) = where_is(&mut h, "id").unwrap();
    assert!(!dimmed_at(&mut h.app, hx, hy));
    h.press("l enter");
    assert!(dimmed_at(&mut h.app, 0, 0), "the screen behind is dimmed");
    let before = cursor(&h);
    let sent = h.press_fx("q l G ctrl+g enter f5");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(h.app.ui.overlay.is_some(), "still the viewer");
    assert_eq!(cursor(&h), before, "the grid did not move");
    assert!(!h.quit);
}

fn long_lines(n: usize) -> ResultSet {
    let text: Vec<String> = (1..=n).map(|i| format!("line {i}")).collect();
    ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(text.join("\n"))]], ..rows_of(&[]) }
}

#[tokio::test]
async fn the_viewer_scrolls_by_line_and_page_and_to_the_ends() {
    let mut h = focused(long_lines(200)).await;
    h.press("enter");
    let rows = screen(&mut h);
    assert!(rows.iter().any(|r| r.contains("line 1↵")), "starts at the top");
    let page = viewer(&h).page;
    assert!(page >= 10, "{page}");
    h.press("j");
    assert_eq!(viewer(&h).scroll, 1);
    h.press("pagedown");
    assert_eq!(viewer(&h).scroll, 1 + page);
    h.press("k");
    h.press("ctrl+u");
    assert_eq!(viewer(&h).scroll, 0, "never above the top");
    h.press("G");
    let rows = screen(&mut h).join("\n");
    assert!(rows.contains("line 200") && !rows.contains("line 1\n"), "{rows}");
    assert_eq!(viewer(&h).scroll, 200 - viewer(&h).page, "the last line is at the bottom");
    h.press("j pagedown");
    assert_eq!(viewer(&h).scroll, 200 - viewer(&h).page, "and no further");
    h.press("g");
    assert_eq!(viewer(&h).scroll, 0);
    let text = screen(&mut h).join("\n");
    let page = viewer(&h).page;
    assert!(text.contains(&format!("lines 1-{page} of 200")), "the position names the lines shown and the total: {text}");
}

#[tokio::test]
async fn scrolling_down_past_the_end_of_a_short_value_keeps_the_last_line_in_view() {
    let mut h = focused(long_lines(3)).await;
    h.press("enter");
    h.press("j j j j j j");
    let text = the_box(&mut h);
    assert!(text.contains("line 3"), "the last line stays visible: {text}");
    assert!(text.contains("lines 1-3 of 3"), "and the counter stays: {text}");
    assert_eq!(viewer(&h).scroll, 0);
}

#[tokio::test]
async fn wrapping_is_by_display_width_and_loses_nothing() {
    let wide = "日本語".repeat(60); // 360 cells
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(wide.clone())]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    h.press("enter");
    let rows = screen(&mut h);
    let width = viewer(&h).width;
    let kept: String = rows.iter().flat_map(|r| r.chars()).filter(|c| "日本語".contains(*c)).collect();
    assert_eq!(kept, wide[..kept.len()], "the text in order, nothing dropped or doubled");
    for r in rows.iter().filter(|r| r.contains('日')) {
        let cells: String = r.chars().filter(|c| "日本語".contains(*c)).collect();
        assert!(UnicodeWidthStr::width(cells.as_str()) <= width, "{} cells in a {width}-cell text area", UnicodeWidthStr::width(cells.as_str()));
    }
    assert!(rows.iter().filter(|r| r.contains('日')).count() >= 4, "360 cells wrap over several lines: {rows:#?}");
}

#[tokio::test]
async fn a_ten_megabyte_value_opens_and_scrolls_by_wrapping_only_what_is_on_screen() {
    let big = "é".repeat(5 * 1024 * 1024);
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(big)]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    let t = std::time::Instant::now();
    h.press("enter");
    screen(&mut h);
    let open = t.elapsed();
    let t = std::time::Instant::now();
    h.press("pagedown pagedown j");
    screen(&mut h);
    let scroll = t.elapsed();
    let worked = viewer(&h).work;
    h.press("G");
    let text = screen(&mut h).join("\n");
    let at_end = viewer(&h).work - worked;
    eprintln!("OBSERVED 10 MB viewer: open {open:?}, scroll {scroll:?}, G visited {at_end} graphemes");
    assert!(open < std::time::Duration::from_millis(250), "{open:?}");
    assert!(scroll < std::time::Duration::from_millis(100), "{scroll:?}");
    // G wraps the tail only (counted, not timed), and says `end` for the line
    // numbers it does not know.
    assert!(viewer(&h).anchored() && viewer(&h).total().is_none());
    assert!(at_end < 20_000, "G visited {at_end} graphemes of 5,242,880");
    assert!(text.contains("end"), "{text}");
    assert!(!text.contains("lines 1"), "no line numbers: {text}");
}

// ---- the viewer: the tail, the mouse, repaints ------------------------------------

fn viewer_of(text: &str, kind: CellKind) -> super::viewer::CellViewer {
    let set = ResultSet { columns: vec![col("t", "text", kind)], rows: vec![vec![Cell::Value(text.to_string())]], ..rows_of(&[]) };
    let mut v = super::viewer::CellViewer::open(&set, 0, 0).unwrap();
    v.set_area(78, 30);
    v
}

/// Every wrapped line of `text` at `width`, from the top: what a full wrap gives.
fn full_wrap(text: &str, width: usize) -> Vec<String> {
    let (mut out, mut at) = (Vec::new(), 0);
    while at < text.len() {
        let (end, next) = super::viewer::next_line(text, at, width);
        out.push(text[at..end].to_string());
        at = next;
    }
    out
}

#[test]
fn g_on_a_single_huge_line_wraps_only_about_a_page_of_bytes_before_the_end() {
    let mut text = "abcdefghij".repeat(1_000_000); // one 10 MB line
    text.push_str("THE-END");
    let mut v = viewer_of(&text, CellKind::Text);
    v.lines(0, 30);
    let before = v.work;
    assert!(before < 30_000, "showing the top visited {before} graphemes");
    v.scroll_to_end();
    let visited = v.work - before;
    assert!(visited < 3 * 31 * 78, "jumping to the end visited {visited} graphemes of 10,000,007");
    assert!(v.anchored() && v.total().is_none(), "the line numbers are not known");
    assert_eq!(v.position(30), "end");
    let last = v.lines(v.scroll, 30).pop().unwrap();
    assert!(last.body.ends_with("THE-END"), "the last line is the end of the value: {:?}", last.body);
    // Drawing the page after the jump costs a page, not the value.
    let drawn = v.work;
    v.lines(v.scroll, 30);
    assert!(v.work - drawn < 4 * 30 * 78);
    // Another G, and a key that moves nothing, visit nothing more.
    let again = v.work;
    v.scroll_to_end();
    assert!(v.work == again, "already at the end");
    // A different width at the end stays at the end without wrapping the value.
    v.set_area(60, 20);
    let lines = v.lines(v.scroll, 20);
    assert!(lines.last().unwrap().body.ends_with("THE-END"));
    assert!(v.work - again < 20_000, "{}", v.work - again);
    // `g` is the top again, with its numbers.
    v.scroll_to_top();
    assert!(!v.anchored() && v.scroll == 0);
    assert_eq!(v.position(30), "lines 1-30", "the numbers are back");
}

#[test]
fn g_on_a_value_with_line_breaks_shows_the_same_last_page_a_full_wrap_does() {
    let text: String = (0..200_000).map(|i| format!("row {i} of the value\n")).collect();
    let reference = full_wrap(&text, 78);
    let mut v = viewer_of(&text, CellKind::Text);
    v.scroll_to_end();
    assert!(v.work < 20_000, "{}", v.work);
    assert!(v.anchored());
    let shown: Vec<String> = v.lines(v.scroll, 30).into_iter().map(|l| l.body).collect();
    assert_eq!(shown, reference[reference.len() - 30..], "line-aligned: the page is the one a full wrap ends with");
    // Up a page, then up past the top of the wrapped tail: the window grows and what is shown stays right.
    let page_up = |v: &mut super::viewer::CellViewer, n: usize| {
        for _ in 0..n {
            v.scroll_by(-30);
        }
    };
    page_up(&mut v, 3);
    let shown: Vec<String> = v.lines(v.scroll, 30).into_iter().map(|l| l.body).collect();
    assert_eq!(shown, reference[reference.len() - 120..reference.len() - 90]);
    let worked = v.work;
    page_up(&mut v, 100); // 3,000 lines up: past the tail that was wrapped first
    assert!(v.work - worked < 500_000, "{} graphemes for 3,000 lines of 200,000", v.work - worked);
    let shown: Vec<String> = v.lines(v.scroll, 30).into_iter().map(|l| l.body).collect();
    assert_eq!(shown, reference[reference.len() - 3_120..reference.len() - 3_090], "{}", v.position(30));
    // And down again to the end.
    for _ in 0..200 {
        v.scroll_by(30);
    }
    assert_eq!(v.lines(v.scroll, 30).into_iter().map(|l| l.body).collect::<Vec<_>>(), reference[reference.len() - 30..]);
    assert_eq!(v.position(30), "end");
    // `g` and the numbers are back.
    v.scroll_to_top();
    assert_eq!(v.lines(0, 1)[0].body, "row 0 of the value");
    assert_eq!(v.position(30), "lines 1-30");
}

#[test]
fn g_on_a_small_value_is_exact_and_keeps_its_line_numbers() {
    let text: String = (0..100).map(|i| format!("l{i}\n")).collect();
    let mut v = viewer_of(&text, CellKind::Text);
    v.scroll_to_end();
    assert!(!v.anchored(), "a value that fits the budget is wrapped whole");
    assert_eq!(v.total(), Some(100));
    assert_eq!(v.position(30), "lines 71-100 of 100");
}

#[tokio::test]
async fn the_viewers_scroll_hints_are_clickable_and_the_wheel_scrolls_it() {
    let long: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(long)]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    h.press("enter");
    let text = screen(&mut h).join("\n");
    assert!(text.contains("j/k scroll") && text.contains("pagedown/pageup page") && text.contains("g/G ends"), "{text}");
    let page = viewer(&h).page;
    let click_on = |h: &mut Harness, t: &str, dx: u16| {
        let (x, y) = where_is(h, t).unwrap_or_else(|| panic!("{t:?}"));
        click(&mut h.app, Rect::new(x + dx, y, 1, 1));
    };
    click_on(&mut h, "j/k scroll", 0);
    assert_eq!(viewer(&h).scroll, 1, "the `j` key");
    screen(&mut h);
    click_on(&mut h, "j/k scroll", 2);
    assert_eq!(viewer(&h).scroll, 0, "the `k` key");
    screen(&mut h);
    click_on(&mut h, "j/k scroll", 4);
    assert_eq!(viewer(&h).scroll, 1, "the words do what the first key does");
    screen(&mut h);
    click_on(&mut h, "pagedown/pageup page", 0);
    assert_eq!(viewer(&h).scroll, 1 + page, "a page down");
    screen(&mut h);
    click_on(&mut h, "pagedown/pageup page", 9);
    assert_eq!(viewer(&h).scroll, 1, "a page up");
    screen(&mut h);
    click_on(&mut h, "g/G ends", 2);
    assert!(viewer(&h).scroll > 300, "G: the end ({} of 401 lines)", viewer(&h).scroll);
    screen(&mut h);
    click_on(&mut h, "g/G ends", 0);
    assert_eq!(viewer(&h).scroll, 0, "g: the top");
    screen(&mut h);
    // The wheel scrolls three lines a notch, wherever the pointer is over the screen.
    mouse(&mut h, MouseEventKind::ScrollDown, 40, 12, KeyModifiers::NONE);
    mouse(&mut h, MouseEventKind::ScrollDown, 3, 3, KeyModifiers::NONE);
    assert_eq!(viewer(&h).scroll, 6);
    mouse(&mut h, MouseEventKind::ScrollUp, 40, 12, KeyModifiers::NONE);
    assert_eq!(viewer(&h).scroll, 3);
    assert!(h.app.ui.overlay.is_some());
}

#[tokio::test]
async fn keys_and_wheel_turns_that_change_nothing_in_the_viewer_repaint_nothing() {
    let long: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(long)]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    h.press("enter");
    screen(&mut h);
    // At the top: nothing moves.
    for keys in ["k", "g", "pageup", "ctrl+u", "x", "h", "tab", "r"] {
        assert!(!h.press(keys), "{keys:?} at the top repainted");
    }
    let up = mouse(&mut h, MouseEventKind::ScrollUp, 40, 12, KeyModifiers::NONE);
    assert!(up.is_empty());
    assert!(!h.app.update(Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 40, row: 12, modifiers: KeyModifiers::NONE }), base()).repaint);
    // One that moves does.
    assert!(h.press("j"));
    screen(&mut h);
    assert!(h.press("G"));
    screen(&mut h);
    // At the end: nothing moves either, and a second G finds nothing new.
    for keys in ["j", "G", "pagedown", "ctrl+d"] {
        assert!(!h.press(keys), "{keys:?} at the end repainted");
    }
    assert!(!h.app.update(Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 40, row: 12, modifiers: KeyModifiers::NONE }), base()).repaint);
    assert!(h.press("g"));
    assert!(h.app.ui.overlay.is_some(), "none of that closed it");
}

#[tokio::test]
async fn the_viewers_buttons_work_by_click_and_name_their_keys() {
    let mut h = focused(doc_set()).await;
    h.press("l enter");
    let text = screen(&mut h).join("\n");
    for hint in ["esc close", "y copy", "r raw"] {
        assert!(text.contains(hint), "{hint:?} in {text}");
    }
    let find = |h: &mut Harness, t: &str| {
        let (x, y) = where_is(h, t).unwrap();
        Rect::new(x, y, 1, 1)
    };
    let at = find(&mut h, "y copy");
    let sent = click(&mut h.app, at).commands;
    assert_eq!(sent, vec![Command::CopyToClipboard(DOC.to_string())]);
    let at = find(&mut h, "r raw");
    click(&mut h.app, at);
    assert!(viewer(&h).raw);
    assert!(screen(&mut h).join("\n").contains("r pretty"), "the button says what it does next");
    let at = find(&mut h, "esc close");
    click(&mut h.app, at);
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(cursor(&h), (0, 1));
}

// ---- copy ----------------------------------------------------------------------------

fn copied(commands: &[Command]) -> Vec<String> {
    commands.iter().filter_map(|c| if let Command::CopyToClipboard(t) = c { Some(t.clone()) } else { None }).collect()
}

fn notice_of(h: &Harness) -> String {
    h.app.ui.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default()
}

#[tokio::test]
async fn copy_on_a_null_cell_copies_an_empty_string_and_says_so() {
    let mut h = focused(doc_set()).await;
    h.press("j l"); // row 2, doc: NULL
    let sent = h.press_fx("y");
    assert_eq!(sent, vec![Command::CopyToClipboard(String::new())]);
    assert!(notice_of(&h) == "Sent an empty string to the terminal clipboard (the cell is NULL)", "{:?}", notice_of(&h));
}

#[tokio::test]
async fn copy_copies_the_exact_text_of_the_cell_and_names_its_size() {
    let exact = "a\tb\nc \u{1b}[31m\u{7} é😀  ";
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(exact.into())]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    assert_eq!(copied(&h.press_fx("y")), vec![exact.to_string()], "nothing filtered, nothing cut");
    assert_eq!(notice_of(&h), format!("Sent cell to the terminal clipboard ({} B)", exact.len()));
    let kb = "k".repeat(1_234);
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(kb)]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    h.press("y");
    assert_eq!(notice_of(&h), "Sent cell to the terminal clipboard (1.2 kB)");
}

#[tokio::test]
async fn copy_row_joins_the_exact_cell_texts_with_tabs_and_writes_null_as_an_empty_field() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text), col("c", "text", CellKind::Text), col("d", "text", CellKind::Text)],
        rows: vec![
            vec![v("1"), Cell::Null, v(""), v("x\ty\nz")],
            vec![v("short")],
            vec![v("1"), v("2"), v("3"), v("4"), v("extra")],
        ],
        ..rows_of(&[])
    };
    let mut h = focused(set).await;
    assert_eq!(copied(&h.press_fx("Y")), vec!["1\t\t\tx\ty\nz".to_string()], "NULL and '' are both empty; a cell's own tab and newline stay");
    assert!(notice_of(&h).starts_with("Sent row to the terminal clipboard"), "{:?}", notice_of(&h));
    h.press("j");
    assert_eq!(copied(&h.press_fx("Y")), vec!["short\t\t\t".to_string()], "missing cells are empty fields");
    h.press("j");
    assert_eq!(copied(&h.press_fx("Y")), vec!["1\t2\t3\t4".to_string()], "cells past the header are not part of the row");
}

#[tokio::test]
async fn a_cell_a_short_row_lacks_is_not_null_in_the_viewer_and_has_nothing_to_copy() {
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text)],
        rows: vec![vec![Cell::Value("only".into())]],
        ..rows_of(&[])
    };
    let mut h = focused(set).await;
    h.press("l");
    // Copying it from the grid sends nothing and says so.
    let sent = h.press_fx("y");
    assert!(copied(&sent).is_empty(), "{sent:?}");
    assert!(notice_of(&h).contains("Nothing to copy") && !notice_of(&h).contains("NULL"), "{:?}", notice_of(&h));
    h.app.ui.notice = None;
    h.press("enter");
    let text = the_box(&mut h);
    assert!(text.contains("no cell: this row is shorter than the header"), "{text}");
    assert!(!text.contains("NULL"), "{text}");
    let sent = h.press_fx("y");
    assert!(copied(&sent).is_empty(), "{sent:?}");
    assert!(notice_of(&h).contains("Nothing to copy"), "{:?}", notice_of(&h));
}

/// The viewer's box for a single text cell holding `value`.
async fn viewed(value: &str) -> (Harness, String) {
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value(value.into())]], ..rows_of(&[]) };
    let mut h = focused(set).await;
    h.press("enter");
    let text = the_box(&mut h);
    (h, text)
}

fn title_of(text: &str) -> &str {
    text.lines().next().unwrap()
}

#[tokio::test]
async fn the_viewer_title_gives_the_length_in_characters_and_bytes() {
    let (_, a) = viewed("a").await;
    let (_, a_space) = viewed("a ").await;
    let (_, accent) = viewed("é😀").await;
    assert!(title_of(&a).contains("1 char, 1 B"), "{}", title_of(&a));
    assert!(title_of(&a_space).contains("2 chars, 2 B"), "{}", title_of(&a_space));
    assert!(title_of(&accent).contains("2 chars, 6 B"), "{}", title_of(&accent));
}

#[tokio::test]
async fn trailing_spaces_and_tabs_are_drawn_as_a_dimmed_glyph_and_copy_still_copies_the_text() {
    let (mut h, spaced) = viewed("a ").await;
    let (_, plain) = viewed("a").await;
    // (The title has a `·` of its own: look below it.)
    let body = |t: &str| t.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert!(body(&spaced).contains("a·") && !body(&plain).contains('·'), "'a ' and 'a' differ:\n{spaced}\n{plain}");
    let (x, y) = where_is(&mut h, "a·").unwrap();
    let x = x + 1;
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Overlay0), "dimmed");
    assert_eq!(copied(&h.press_fx("y")), vec!["a ".to_string()], "the stored text, not the glyph");
    let (mut h, tabbed) = viewed("a\t").await;
    assert!(tabbed.contains(&format!("a{TAB_MARK}")), "{tabbed}");
    let (x, y) = where_is(&mut h, TAB_MARK).unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Overlay0), "a trailing tab is dimmed");
    // Spaces inside a line stay spaces.
    let (_, inner) = viewed("a b").await;
    assert!(inner.contains("a b") && !body(&inner).contains('·'), "{inner}");
}

#[tokio::test]
async fn every_hard_line_break_ends_in_a_dimmed_glyph_that_differs_for_lf_cr_and_crlf() {
    let (mut h, lf_end) = viewed("a\n").await;
    let (_, plain) = viewed("a").await;
    assert!(lf_end.contains("a↵") && !plain.contains('↵'), "a trailing newline shows:\n{lf_end}\n{plain}");
    let (x, y) = where_is(&mut h, "↵").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Overlay0), "dimmed");
    assert_eq!(copied(&h.press_fx("y")), vec!["a\n".to_string()]);
    let (_, lf) = viewed("a\nb").await;
    let (_, crlf) = viewed("a\r\nb").await;
    let (_, cr) = viewed("x\ry").await;
    assert!(lf.contains("a↵") && !lf.contains('←'), "{lf}");
    assert!(crlf.contains("a←↵"), "{crlf}");
    assert!(cr.contains("x←") && !cr.contains('↵'), "{cr}");
    assert_ne!(lf, crlf);
}

#[tokio::test]
async fn a_copy_over_the_clipboard_limit_is_refused_with_a_notice_that_says_why() {
    use super::grid::CLIPBOARD_LIMIT;
    let at = |n: usize| ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value("x".repeat(n))]], ..rows_of(&[]) };
    let mut h = focused(at(CLIPBOARD_LIMIT)).await;
    assert_eq!(copied(&h.press_fx("y")).len(), 1, "exactly at the limit is sent");
    let mut h = focused(at(CLIPBOARD_LIMIT + 1)).await;
    let sent = h.press_fx("y");
    assert!(sent.is_empty(), "nothing is sent over the limit");
    let said = notice_of(&h);
    assert!(said.contains("Not copied") && said.contains("too large") && said.contains("75.0 kB"), "{said:?}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    // A row of cells that together pass it, the same.
    let half = "y".repeat(CLIPBOARD_LIMIT / 2);
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text)],
        rows: vec![vec![Cell::Value(half.clone()), Cell::Value(half)]],
        ..rows_of(&[])
    };
    let mut h = focused(set).await;
    assert!(h.press_fx("Y").is_empty());
    assert!(notice_of(&h).contains("Not copied"), "{:?}", notice_of(&h));
    // And from the viewer.
    let mut h = focused(at(CLIPBOARD_LIMIT + 1)).await;
    h.press("enter");
    assert!(h.press_fx("y").is_empty());
}

// ---- load more ------------------------------------------------------------------------

/// `n` rows of `id`, `v`; cut by the row cap (`truncated`).
fn truncated_set(n: usize) -> ResultSet {
    ResultSet {
        columns: vec![col("id", "int4", CellKind::Integer), col("v", "text", CellKind::Text)],
        rows: (0..n).map(|r| vec![Cell::Value(r.to_string()), Cell::Value(format!("v{r}"))]).collect(),
        truncated: true,
        ran: "select id, v from t order by id".into(),
        ..rows_of(&[])
    }
}

/// Feeds `reply` to the one run in `commands`, as the shell would.
fn answer(h: &mut Harness, commands: &[Command], reply: Result<ResultSet, crate::core::CoreError>) {
    let sent = runs(commands);
    assert_eq!(sent.len(), 1, "one run to answer: {commands:?}");
    let fx = h.app.update(Event::Core(sent[0].0, CoreReply::Ran(reply)), base() + std::time::Duration::from_millis(80));
    h.run(fx);
}

#[tokio::test]
async fn a_truncated_result_shows_1000_plus_and_m_emits_a_run_of_the_same_statement_with_a_larger_cap() {
    let mut h = focused(truncated_set(1_000)).await;
    assert!(mode_bar(&mut h).contains("1,000+ rows"), "{}", mode_bar(&mut h));
    h.app.model.tabs[0].target.clone().expect("a target");
    let sent = runs(&h.press_fx("m"));
    assert_eq!(sent.len(), 1, "one run");
    let request = &sent[0].2;
    assert_eq!(request.row_cap, 2_000, "double");
    assert_eq!(request.buffer, "select id, v from t order by id", "the statement that ran, not the one under the editor's cursor");
    assert_eq!(request.selection, Some((Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 31 })), "all of it selected, so the Core runs exactly it");
    assert_eq!(request.session, h.app.model.tabs[0].session());
    assert_eq!(request.database.as_deref(), Some("pagila"));
}

#[tokio::test]
async fn while_more_loads_the_pane_says_so_and_the_rows_so_far_stay_navigable_then_the_cursor_is_where_it_was() {
    let mut h = focused(truncated_set(1_000)).await;
    screen(&mut h); // a page is as many rows as were drawn
    h.press("pagedown pagedown l");
    h.press("j j j");
    let at = cursor(&h);
    assert!(at.0 > 20 && at.1 == 1, "{at:?}");
    let commands = h.press_fx("m");
    let pane_text = pane(&mut h).join("\n");
    assert!(pane_text.contains("loading more"), "{pane_text}");
    assert!(!pane_text.contains("earlier run"), "it is the same statement, not an earlier run: {pane_text}");
    assert!(pane_text.contains(&format!("v{}", at.0)), "the rows so far are still drawn: {pane_text}");
    assert!(mode_bar(&mut h).contains("running"), "{}", mode_bar(&mut h));
    h.press("j h");
    assert_eq!(cursor(&h), (at.0 + 1, 0), "and the cursor still moves");
    h.press("k l");
    assert_eq!(cursor(&h), at);

    answer(&mut h, &commands, Ok(truncated_set(2_000)));
    assert_eq!(cursor(&h), at, "the same row and column when the rows land");
    let pane_text = pane(&mut h).join("\n");
    assert!(!pane_text.contains("loading more"), "{pane_text}");
    assert!(pane_text.contains(&format!("v{}", at.0)), "{pane_text}");
    assert!(mode_bar(&mut h).contains("2,000+ rows"), "{}", mode_bar(&mut h));
    let title_row = screen(&mut h)[h.app.ui.hits.results.y as usize].clone();
    assert!(title_row.contains(&format!("r {}/2,000+", at.0 + 1)), "the position names the row and the new total: {title_row:?}");
    assert!(h.app.model.tabs[0].loading_more.is_none());
    assert_eq!(h.app.model.tabs[0].run, query::RunState::Idle);
}

#[tokio::test]
async fn load_more_on_a_result_that_is_not_truncated_does_nothing() {
    let mut h = focused(ResultSet { truncated: false, ..truncated_set(50) }).await;
    let sent = h.press_fx("m");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(h.app.ui.notice.is_none(), "not even a notice");
    assert_eq!(h.app.model.tabs[0].run, query::RunState::Idle);
}

#[tokio::test]
async fn load_more_is_refused_with_a_notice_for_a_statement_that_is_not_safe_to_repeat() {
    let mut h = focused(ResultSet { rerun: crate::core::Rerun::Write, ran: "update t set a = 1 returning id, v".into(), ..truncated_set(1_000) }).await;
    let sent = h.press_fx("m");
    assert!(sent.is_empty(), "{sent:?}");
    let said = notice_of(&h);
    assert!(said.contains("changes data") || said.contains("again"), "{said:?}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    assert_eq!(h.app.model.tabs[0].run, query::RunState::Idle);
}

fn unproven_set(n: usize) -> ResultSet {
    ResultSet { rerun: crate::core::Rerun::ReadOnlyUnproven, ran: "\n  select my_func(id) as v\nfrom t".into(), ..truncated_set(n) }
}

#[tokio::test]
async fn load_more_on_a_read_only_but_unproven_statement_asks_first_and_enter_re_runs_it() {
    let mut h = focused(unproven_set(1_000)).await;
    assert!(mode_bar(&mut h).contains("m more"), "the question can be answered yes: {}", mode_bar(&mut h));
    let sent = h.press_fx("m");
    assert!(sent.is_empty(), "nothing runs before the answer: {sent:?}");
    assert!(h.app.model.tabs[0].loading_more.is_none());
    let text = the_box(&mut h);
    assert!(text.contains("select my_func(id) as v"), "names the statement's first line: {text}");
    assert!(!text.contains("from t"), "only the first line: {text}");
    assert!(text.contains("may change data") && text.contains("functions"), "{text}");
    assert!(text.contains("re-run") && text.contains("cancel"), "{text}");
    let sent = h.press_fx("enter");
    let sent = runs(&sent);
    assert_eq!(sent.len(), 1, "enter re-runs");
    assert_eq!(sent[0].2.row_cap, 2_000);
    assert!(h.app.ui.overlay.is_none());
    assert!(h.app.model.tabs[0].loading_more.is_some());
}

#[tokio::test]
async fn esc_on_the_re_run_question_runs_nothing_and_leaves_the_result_as_it_was() {
    let mut h = focused(unproven_set(1_000)).await;
    h.press("j j");
    h.press("m");
    assert!(h.app.ui.overlay.is_some());
    let sent = h.press_fx("esc");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(h.app.ui.overlay.is_none());
    assert!(h.app.model.tabs[0].loading_more.is_none());
    assert_eq!(h.app.model.tabs[0].run, query::RunState::Idle);
    assert_eq!(cursor(&h), (2, 0));
    // Other keys do not reach the grid while it is asked.
    h.press("m");
    let before = cursor(&h);
    assert!(h.press_fx("j l q").is_empty());
    assert_eq!(cursor(&h), before);
    assert!(h.app.ui.overlay.is_some() && !h.quit);
}

#[tokio::test]
async fn the_more_hint_is_not_offered_where_load_more_can_only_be_refused() {
    use crate::core::{Rerun, MAX_ROW_CAP};
    // Offered: proven, and read-only but unproven (it asks).
    let mut h = focused(truncated_set(1_000)).await;
    assert!(mode_bar(&mut h).contains("m more"), "{}", mode_bar(&mut h));
    // A write.
    let mut h = focused(ResultSet { rerun: Rerun::Write, ..truncated_set(1_000) }).await;
    assert!(!mode_bar(&mut h).contains("more"), "{}", mode_bar(&mut h));
    // The tab has a new session.
    let mut h = focused(truncated_set(1_000)).await;
    h.app.model.tabs[0].session += 1;
    assert!(!mode_bar(&mut h).contains("more"), "{}", mode_bar(&mut h));
    // At the ceiling.
    let mut h = focused(truncated_set(MAX_ROW_CAP)).await;
    assert!(!mode_bar(&mut h).contains("more"), "{}", mode_bar(&mut h));
}

#[tokio::test]
async fn load_more_is_refused_while_a_run_is_in_flight_or_being_cancelled() {
    let mut h = focused(truncated_set(1_000)).await;
    // A run in the editor, in flight.
    h.press("ctrl+g k");
    h.type_text(" ");
    assert_eq!(runs(&h.press_fx("f5")).len(), 1);
    h.press("ctrl+g j");
    h.app.ui.notice = None;
    let sent = h.press_fx("m");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(notice_of(&h).contains("running"), "{:?}", notice_of(&h));
    // Cancelling.
    h.press("ctrl+c");
    assert!(matches!(h.app.model.tabs[0].run, query::RunState::Cancelling { .. }));
    h.app.ui.notice = None;
    assert!(h.press_fx("m").is_empty());
    assert!(notice_of(&h).contains("cancel") || notice_of(&h).contains("running"), "{:?}", notice_of(&h));
}

#[tokio::test]
async fn a_second_load_more_while_the_first_runs_is_refused() {
    let mut h = focused(truncated_set(1_000)).await;
    assert_eq!(runs(&h.press_fx("m")).len(), 1);
    assert!(h.press_fx("m").is_empty());
    assert!(notice_of(&h).contains("running"), "{:?}", notice_of(&h));
}

#[tokio::test]
async fn load_more_is_refused_after_the_tab_was_given_a_new_session() {
    let mut h = focused(truncated_set(1_000)).await;
    h.app.model.tabs[0].session += 1; // as stopping waiting for a run does
    let sent = h.press_fx("m");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(notice_of(&h).contains("session"), "{:?}", notice_of(&h));
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
}

#[tokio::test]
async fn the_cap_doubles_up_to_a_ceiling_and_at_the_ceiling_it_says_so() {
    use crate::core::MAX_ROW_CAP;
    let mut h = focused(truncated_set(3_000)).await;
    assert_eq!(runs(&h.press_fx("m"))[0].2.row_cap, 6_000, "doubles what was fetched");
    let near = MAX_ROW_CAP * 2 / 3;
    let mut h = focused(truncated_set(near)).await;
    assert_eq!(runs(&h.press_fx("m"))[0].2.row_cap, MAX_ROW_CAP, "never past the ceiling");
    let mut h = focused(truncated_set(MAX_ROW_CAP)).await;
    let sent = h.press_fx("m");
    assert!(sent.is_empty());
    assert!(notice_of(&h).contains("100,000"), "{:?}", notice_of(&h));
}

#[tokio::test]
async fn the_ceiling_is_also_a_budget_of_cells_so_a_wide_result_stops_earlier() {
    use crate::core::MAX_CELLS;
    // 100 columns: the budget allows MAX_CELLS / 100 rows, well under the row ceiling.
    let mut wide = text_set(15_000, 100);
    wide.truncated = true;
    let allowed = MAX_CELLS / 100;
    assert!(allowed < 30_000 && allowed > 15_000, "the test's numbers: {allowed}");
    let mut h = focused(wide.clone()).await;
    assert_eq!(runs(&h.press_fx("m"))[0].2.row_cap, allowed, "doubling would pass the budget");
    let mut full = text_set(allowed, 100);
    full.truncated = true;
    let mut h = focused(full).await;
    assert!(h.press_fx("m").is_empty());
    let said = notice_of(&h);
    assert!(said.contains(&super::view::thousands(allowed)), "the notice names the rows it stops at: {said:?}");
}

#[tokio::test]
async fn if_the_new_result_has_fewer_rows_than_the_cursors_row_the_cursor_clamps_and_a_notice_says_it_changed() {
    let mut h = focused(truncated_set(1_000)).await;
    h.press("G l");
    assert_eq!(cursor(&h), (999, 1));
    let commands = h.press_fx("m");
    h.app.ui.notice = None;
    answer(&mut h, &commands, Ok(ResultSet { truncated: false, ..truncated_set(600) }));
    assert_eq!(cursor(&h), (599, 1), "on the last row there is now");
    let said = notice_of(&h);
    assert!(said.contains("changed") && said.contains("600"), "{said:?}");
    screen(&mut h);
    assert!(h.app.ui.grid.top <= 599);
}

#[tokio::test]
async fn a_load_more_that_lands_on_changed_rows_says_so_and_one_that_does_not_stays_quiet() {
    // Unchanged: the first 1,000 rows are the same, 1,000 more follow.
    let mut h = focused(truncated_set(1_000)).await;
    let commands = h.press_fx("m");
    answer(&mut h, &commands, Ok(truncated_set(2_000)));
    let said = notice_of(&h);
    assert!(said.contains("Loaded 2,000 rows") && !said.contains("changed"), "{said:?}");
    // Changed: row 5 is not what it was, and the new result is longer.
    let mut h = focused(truncated_set(1_000)).await;
    h.press("j j");
    let commands = h.press_fx("m");
    let mut set = truncated_set(2_000);
    set.rows[5][1] = Cell::Value("someone updated this".into());
    answer(&mut h, &commands, Ok(set));
    let said = notice_of(&h);
    assert!(said.contains("The rows changed since the last run."), "{said:?}");
    assert!(!said.contains("cursor moved"), "the cursor did not move: {said:?}");
    assert_eq!(cursor(&h), (2, 0));
    // Fewer columns: the column is clamped, the row is not, and the cursor is not said to have moved.
    let mut h = focused(truncated_set(1_000)).await;
    h.press("j l");
    let commands = h.press_fx("m");
    let one_column = ResultSet {
        columns: vec![col("id", "int4", CellKind::Integer)],
        rows: (0..2_000).map(|r| vec![Cell::Value(r.to_string())]).collect(),
        ..truncated_set(2_000)
    };
    answer(&mut h, &commands, Ok(one_column));
    assert_eq!(cursor(&h), (1, 0));
    let said = notice_of(&h);
    assert!(said.contains("The rows changed since the last run."), "{said:?}");
    assert!(!said.contains("cursor moved"), "only the column was clamped: {said:?}");
}

#[tokio::test]
async fn a_failed_load_more_keeps_the_rows_and_says_what_failed() {
    let mut h = focused(truncated_set(1_000)).await;
    h.press("j j");
    let commands = h.press_fx("m");
    answer(&mut h, &commands, Err(crate::core::CoreError::new(CoreErrorKind::Sql, "ERROR: canceling statement")));
    assert_eq!(h.app.model.tabs[0].run, query::RunState::Idle);
    assert!(matches!(&h.app.model.tabs[0].outcome, Some(Outcome::Rows(set)) if set.rows.len() == 1_000), "the result is still the one on screen");
    assert_eq!(cursor(&h), (2, 0));
    let said = notice_of(&h);
    assert!(said.contains("canceling statement"), "{said:?}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    assert!(mode_bar(&mut h).contains("1,000+ rows"), "and it can be tried again");
}

#[tokio::test]
async fn cancelling_a_load_more_keeps_the_rows_and_ends_the_loading_state() {
    let mut h = focused(truncated_set(1_000)).await;
    let run = runs(&h.press_fx("m"))[0].1;
    assert_eq!(h.press_fx("ctrl+c"), vec![Command::Cancel(run)]);
    assert!(h.app.model.tabs[0].loading_more.is_none());
    let text = pane(&mut h).join("\n");
    assert!(!text.contains("loading more") && text.contains("cancelling"), "{text}");
    assert!(text.contains("v0"), "the rows stay");
}

#[tokio::test]
async fn retargeting_the_tab_mid_load_more_drops_the_loading_state_and_the_late_rows() {
    let mut h = focused(truncated_set(1_000)).await;
    let commands = h.press_fx("m");
    h.press("ctrl+g h k u"); // the tab moves to `edge`
    assert!(h.app.model.tabs[0].loading_more.is_none());
    answer(&mut h, &commands, Ok(truncated_set(2_000)));
    assert_eq!(h.app.model.tabs[0].outcome, None, "rows from another target never land");
}

#[tokio::test]
async fn a_new_result_starts_the_cursor_in_the_corner_and_a_load_more_keeps_it() {
    let mut h = focused(truncated_set(1_000)).await;
    h.press("G l");
    // A new run (not load more) with a result: back to the corner.
    h.press("ctrl+g k");
    let commands = h.press_fx("f5");
    answer(&mut h, &commands, Ok(truncated_set(40)));
    assert_eq!(cursor(&h), (0, 0));
    assert_eq!((h.app.ui.grid.top, h.app.ui.grid.left), (0, 0));
}

// ---- the mouse and the mode bar -------------------------------------------------------

fn mouse(h: &mut Harness, kind: MouseEventKind, x: u16, y: u16, mods: KeyModifiers) -> Vec<Command> {
    let fx = h.app.update(Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers: mods }), base());
    let commands = fx.commands.clone();
    h.run(fx);
    commands
}

#[tokio::test]
async fn clicking_a_cell_moves_the_cursor_there_and_focuses_the_results() {
    let mut h = focused(text_set(40, 10)).await;
    h.press("ctrl+g k"); // focus the editor
    let (x, y) = where_is(&mut h, "r3c2").unwrap();
    mouse(&mut h, MouseEventKind::Down(MouseButton::Left), x + 1, y, KeyModifiers::NONE);
    assert_eq!(cursor(&h), (3, 2));
    assert_eq!(h.app.ui.focus, PaneId::Results);
    // A click on the row-number gutter picks the row; the header picks nothing.
    let (gx, gy) = where_is(&mut h, "r5c0").map(|(x, y)| (x - 2, y)).unwrap();
    mouse(&mut h, MouseEventKind::Down(MouseButton::Left), gx, gy, KeyModifiers::NONE);
    assert_eq!(cursor(&h).0, 5);
    let (hx, hy) = where_is(&mut h, "c4").unwrap();
    mouse(&mut h, MouseEventKind::Down(MouseButton::Left), hx, hy, KeyModifiers::NONE);
    assert_eq!(cursor(&h).0, 5, "the header is not a row");
    // Below the last row there is nothing to click.
    let mut short = focused(text_set(2, 3)).await;
    let (_, y) = where_is(&mut short, "r1c0").unwrap();
    let x = where_is(&mut short, "r1c0").unwrap().0;
    mouse(&mut short, MouseEventKind::Down(MouseButton::Left), x, y + 3, KeyModifiers::NONE);
    assert_eq!(cursor(&short), (0, 0));
}

#[tokio::test]
async fn the_wheel_scrolls_rows_the_shift_wheel_and_the_horizontal_wheel_scroll_columns_and_the_cursor_stays_in_view() {
    let mut h = focused(text_set(200, 40)).await;
    let (x, y) = where_is(&mut h, "r0c0").unwrap();
    mouse(&mut h, MouseEventKind::ScrollDown, x, y, KeyModifiers::NONE);
    screen(&mut h);
    assert_eq!(h.app.ui.grid.top, 3, "a notch is three rows");
    assert!(h.app.ui.grid.row >= 3, "the cursor came with the window: {:?}", cursor(&h));
    mouse(&mut h, MouseEventKind::ScrollUp, x, y, KeyModifiers::NONE);
    screen(&mut h);
    assert_eq!(h.app.ui.grid.top, 0);
    // The wheel over the pane scrolls it even when another pane has focus.
    h.press("ctrl+g k");
    mouse(&mut h, MouseEventKind::ScrollDown, x, y, KeyModifiers::NONE);
    screen(&mut h);
    assert_eq!((h.app.ui.grid.top, h.app.ui.focus), (3, PaneId::Editor));

    mouse(&mut h, MouseEventKind::ScrollDown, x, y, KeyModifiers::SHIFT);
    screen(&mut h);
    assert_eq!(h.app.ui.grid.left, 1, "shift+wheel scrolls a column");
    assert!(h.app.ui.grid.col >= 1, "{:?}", cursor(&h));
    mouse(&mut h, MouseEventKind::ScrollRight, x, y, KeyModifiers::NONE);
    screen(&mut h);
    assert_eq!(h.app.ui.grid.left, 2, "so does a horizontal wheel");
    mouse(&mut h, MouseEventKind::ScrollLeft, x, y, KeyModifiers::NONE);
    mouse(&mut h, MouseEventKind::ScrollUp, x, y, KeyModifiers::SHIFT);
    screen(&mut h);
    assert_eq!(h.app.ui.grid.left, 0);
    let g = h.app.ui.grid.clone();
    assert!(g.left <= g.col && g.col < g.left + g.cols_visible.max(1), "{g:?}");
}

#[tokio::test]
async fn the_mode_bar_shows_the_results_keys_that_work_from_the_live_bindings_and_only_then() {
    let mut h = focused(truncated_set(1_000)).await;
    let bar = mode_bar(&mut h);
    for hint in ["enter view", "y copy", "Y copy row", "m more", "h/j/k/l move"] {
        assert!(bar.contains(hint), "{hint:?} in {bar:?}");
    }
    // `more` only for a truncated result.
    let mut h2 = focused(ResultSet { truncated: false, ..truncated_set(30) }).await;
    let bar = mode_bar(&mut h2);
    assert!(bar.contains("enter view") && !bar.contains("more"), "{bar:?}");
    // Not in the editor.
    h.press("ctrl+g k");
    let bar = mode_bar(&mut h);
    assert!(!bar.contains("enter view") && !bar.contains("copy") && !bar.contains("more"), "{bar:?}");
    // A rebound key is the one shown, and the one that works.
    let mut h = focused(truncated_set(30)).await;
    assert!(h.app.ui.keys.rebind(Action::CopyCell, "c"));
    assert!(mode_bar(&mut h).contains("c copy") && !mode_bar(&mut h).contains("y copy"), "{}", mode_bar(&mut h));
    assert_eq!(copied(&h.press_fx("c")).len(), 1);
    assert!(h.press_fx("y").is_empty());
    // With no rows to move in, no results keys.
    let mut none = ran(Ok(ResultSet { columns: vec![], ..rows_of(&[]) })).await;
    none.press("ctrl+g j");
    assert!(!mode_bar(&mut none).contains("enter view"), "{}", mode_bar(&mut none));
}

#[tokio::test]
async fn clicking_a_results_hint_does_what_its_key_does() {
    let mut h = focused(truncated_set(1_000)).await;
    click_hint(&mut h, "y copy");
    assert_eq!(h.app.ui.notice.as_ref().map(|n| n.text.clone()), Some("Sent cell to the terminal clipboard (1 B)".to_string()));
    let sent = click_hint(&mut h, "m more");
    assert_eq!(runs(&sent).len(), 1);
    h.press("ctrl+c");
    click_hint(&mut h, "enter view");
    assert!(matches!(h.app.ui.overlay, Some(overlay::Overlay::CellViewer(_))));
    h.press("esc");
    click_hint(&mut h, "h/j/k/l move");
    // The first key of the hint is the one the click does: `h`, at the edge.
    assert_eq!(cursor(&h), (0, 0));
}

// ---- performance budgets ---------------------------------------------------------------

fn frame_time(h: &mut Harness) -> std::time::Duration {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    let started = std::time::Instant::now();
    term.draw(|f| h.app.view(f)).unwrap();
    started.elapsed()
}

/// The median of five frames.
fn warm_frame(h: &mut Harness) -> std::time::Duration {
    let mut times: Vec<_> = (0..5).map(|_| frame_time(h)).collect();
    times.sort();
    times[2]
}

#[tokio::test]
async fn a_10_000_by_120_result_draws_in_a_few_milliseconds_and_moving_rescans_nothing() {
    let mut h = focused(text_set(10_000, 120)).await;
    let first = frame_time(&mut h);
    let blank = {
        // The same frame with nothing in the results pane, for what the rest of the screen costs.
        let mut empty = ran(Ok(rows_of(&[]))).await;
        empty.press("ctrl+g j");
        warm_frame(&mut empty)
    };
    let warm = warm_frame(&mut h);
    let measured = h.app.ui.grid.widths.measured;
    h.press("j j l pagedown");
    let moved = warm_frame(&mut h);
    assert_eq!(h.app.ui.grid.widths.measured, measured, "moving the cursor measured no cells");
    h.press("G $");
    let end = warm_frame(&mut h);
    eprintln!("OBSERVED 10,000x120: first frame {first:?}, warm {warm:?}, after moves {moved:?}, at the end {end:?}, an empty pane {blank:?}");
    let budget = std::time::Duration::from_millis(10);
    assert!(warm < budget && moved < budget && end < budget, "{warm:?} {moved:?} {end:?}");
    assert!(first < std::time::Duration::from_millis(60), "the first frame measures the window once: {first:?}");
}

#[tokio::test]
async fn a_result_with_one_ten_megabyte_cell_draws_and_moves_in_a_few_milliseconds() {
    let big = "é".repeat(5 * 1024 * 1024);
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("big", "text", CellKind::Text), col("z", "text", CellKind::Text)],
        rows: vec![vec![Cell::Value("one".into()), Cell::Value(big), Cell::Value("two".into())], vec![Cell::Value("x".into()), Cell::Null, Cell::Value("y".into())]],
        ..rows_of(&[])
    };
    let mut h = focused(set).await;
    let first = frame_time(&mut h);
    h.press("l");
    let on_it = warm_frame(&mut h);
    let measured = h.app.ui.grid.widths.measured;
    h.press("j k l h"); // and back on the big cell
    let moved = warm_frame(&mut h);
    assert_eq!(h.app.ui.grid.widths.measured, measured, "no rescan");
    let t = std::time::Instant::now();
    let sent = h.press_fx("y");
    let copy = t.elapsed();
    eprintln!("OBSERVED one 10 MB cell: first {first:?}, cursor on it {on_it:?}, after moves {moved:?}, a refused copy {copy:?}");
    assert!(sent.is_empty(), "too large to copy");
    let budget = std::time::Duration::from_millis(10);
    assert!(on_it < budget && moved < budget, "{on_it:?} {moved:?}");
    assert!(first < std::time::Duration::from_millis(60), "{first:?}");
}

#[tokio::test]
async fn the_more_hint_is_hidden_while_a_run_is_in_flight_because_the_key_would_only_be_refused() {
    let mut h = focused(truncated_set(1_000)).await;
    assert!(mode_bar(&mut h).contains("m more"));
    h.press("m");
    let bar = mode_bar(&mut h);
    assert!(!bar.contains("m more") && bar.contains("ctrl+c cancel"), "{bar}");
}

#[tokio::test]
async fn a_copy_from_the_viewer_says_so_inside_the_box_and_draws_no_notice_card_behind_it() {
    let mut h = focused(doc_set()).await;
    h.press("l enter y");
    // On a wide screen the box leaves room at the sides for a card.
    let rows = frame_text(&mut h.app, 160, 40);
    assert!(!rows.iter().any(|r| r.contains("┌─ notice")), "no card: {rows:#?}");
    assert!(the_box(&mut h).contains("Sent cell to the terminal clipboard"), "{}", the_box(&mut h));
}
