//! The editor as drawn: a gutter (the statement bar and line numbers) and the
//! text, painted a grapheme at a time from the widget's lines, the syntax
//! spans, the selection and the matching bracket.
//!
//! Layers, lowest first: the editor background, the caret row's background,
//! the matched-bracket background, the selection background (over the syntax
//! colours, which stay as the text colour), the caret (reversed, while the
//! editor has focus).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::Painter;
use crate::app::highlight::{matching_bracket, Span};
use crate::app::marks::Mark;
use crate::app::theme::Role;
use crate::app::App;
use crate::core::{display_safe, DbKind};

/// The gutter's columns: the statement bar, the number (at least two digits),
/// and a space.
fn gutter_width(lines: usize) -> u16 {
    (lines.to_string().len().max(2) + 2) as u16
}

/// The width of a tab stop, as the widget expands tabs.
const TAB: usize = 4;

/// Where character `at` of the buffer is on screen, as the last frame drew
/// the editor: its cell, when it is inside the text area.
pub(super) fn cell_of(app: &App, at: crate::core::Cursor) -> Option<(u16, u16)> {
    let text = app.ui.hits.editor_text;
    let (top, left) = app.ui.editor.scroll_top();
    let line = app.ui.editor.lines().get(at.row)?;
    if at.row < top {
        return None;
    }
    let y = text.y.checked_add(u16::try_from(at.row - top).ok()?).filter(|y| *y < text.bottom())?;
    let (mut col, mut chars) = (0usize, 0usize);
    for g in line.graphemes(true) {
        if chars >= at.col {
            break;
        }
        chars += g.chars().count();
        col += if g == "\t" { TAB - col % TAB } else { UnicodeWidthStr::width(display_safe(g).as_str()) };
    }
    let x = text.x + col.saturating_sub(left) as u16;
    (x < text.right()).then_some((x, y))
}

/// Marks the rows of the statement that would run, in the gutter.
const BAR: &str = "▎";

pub(super) fn draw(app: &mut App, p: &Painter, buf: &mut Buffer, area: Rect, focused: bool, kind: Option<&DbKind>, placeholder: &str) {
    let lines = app.ui.editor.lines().len();
    let gutter = gutter_width(lines).min(area.width.saturating_sub(8));
    let gutter_area = Rect { width: gutter, ..area };
    let text = Rect { x: area.x + gutter, width: area.width - gutter, ..area };
    // Where a click places the caret.
    app.ui.hits.editor_text = text;
    app.ui.hits.editor_gutter = gutter_area;

    app.ui.editor.layout(text);
    let (top, left) = app.ui.editor.scroll_top();
    let ui = &mut app.ui;
    let range = ui.syntax.prepare(&ui.editor, kind);

    let theme = p.theme;
    let pane = p.within(area);
    pane.fill(buf, Style::default().bg(theme.color(Role::EditorBg)).fg(theme.color(Role::Text)));
    let on = |role: Role| Style::default().fg(theme.color(role)).add_modifier(theme.modifier(role));
    let caret = ui.editor.cursor();

    if ui.editor.is_empty() {
        let row = Style::default().bg(theme.color(Role::CurrentLine));
        pane.within(text.intersection(Rect { height: 1, ..text })).fill(buf, row);
        let number = on(Role::LineNumberActive);
        gutter_text(&pane, buf, gutter_area, text.y, "1", number);
        let cursor = if focused { Style::default().fg(theme.color(Role::Text)).add_modifier(Modifier::REVERSED) } else { row };
        p.within(text).glyph(buf, text.x, text.y, " ", 1, cursor.bg(theme.color(Role::CurrentLine)));
        let hint = on(Role::Overlay0).bg(theme.color(Role::CurrentLine));
        for (i, line) in placeholder.split('\n').enumerate() {
            if i as u16 >= text.height {
                break;
            }
            p.within(text).put(buf, text.x + 1, text.y + i as u16, line, hint);
        }
        return;
    }

    let selection = ui.editor.selection();
    let mut painted = 0;
    let braces = matching_bracket(ui.syntax.colours(), caret);
    let bar_rows = range.map(|(a, b)| a.row..=if b.col == 0 && b.row > a.row { b.row - 1 } else { b.row });
    for y in 0..text.height {
        let row = top + y as usize;
        let Some(line) = ui.editor.lines().get(row) else { break };
        let at_caret = row == caret.row;
        let base_bg = theme.color(if at_caret { Role::CurrentLine } else { Role::EditorBg });
        let sy = text.y + y;
        if at_caret {
            pane.within(Rect { y: sy, height: 1, ..text }).fill(buf, Style::default().bg(base_bg));
        }
        // The gutter.
        if bar_rows.as_ref().is_some_and(|r| r.contains(&row)) {
            pane.glyph(buf, gutter_area.x, sy, BAR, 1, on(Role::RangeToExecute).bg(theme.color(Role::EditorBg)));
        }
        let number = on(if at_caret { Role::LineNumberActive } else { Role::LineNumber }).bg(theme.color(Role::EditorBg));
        gutter_text(&pane, buf, gutter_area, sy, &(row + 1).to_string(), number);

        let spans = ui.syntax.colours().line(row).unwrap_or(&[]);
        let selected = selection.and_then(|(a, b)| {
            (a.row <= row && row <= b.row).then_some((if row == a.row { a.col } else { 0 }, if row == b.row { b.col } else { usize::MAX }))
        });
        let marked: Vec<usize> = braces.iter().flatten().filter(|(l, _)| *l == row).map(|&(_, c)| c).collect();
        let look = Look {
            theme,
            spans,
            marks: ui.marks.row(row),
            selected,
            marked: &marked,
            caret: (focused && at_caret).then_some(caret.col),
            base_bg,
        };
        let done = paint_line(&p.within(text), buf, text, sy, line, left, &look);
        painted += done.graphemes;
        // A selection that goes on past this row's end shows the line break.
        if selected.is_some_and(|(_, to)| to == usize::MAX) {
            // `end` is known only when the walk reached the end of the line.
            if let Some(end) = done.end.filter(|&end| end >= left && end - left < text.width as usize) {
                p.within(text).glyph(buf, text.x + (end - left) as u16, sy, " ", 1, Style::default().bg(theme.color(Role::SelectionBg)).add_modifier(theme.modifier(Role::SelectionBg)));
            }
        }
    }
    ui.syntax.graphemes_painted = painted;
}

/// Writes `n` right-aligned in the gutter, leaving the bar column and one
/// space.
fn gutter_text(p: &Painter, buf: &mut Buffer, gutter: Rect, y: u16, n: &str, style: Style) {
    let width = gutter.width.saturating_sub(2) as usize;
    let x = gutter.x + 1 + width.saturating_sub(n.len()) as u16;
    p.within(gutter).put(buf, x, y, n, style);
}

struct Look<'a> {
    theme: &'a crate::app::theme::Palette,
    spans: &'a [Span],
    /// Names the loaded schema resolves (or does not), over plain identifiers.
    marks: &'a [Mark],
    /// Selected characters of the row, `usize::MAX` meaning to its end.
    selected: Option<(usize, usize)>,
    /// Characters with the matched-bracket background.
    marked: &'a [usize],
    /// The caret's character, when it is drawn.
    caret: Option<usize>,
    base_bg: ratatui::style::Color,
}

/// What painting a row did.
struct Painted {
    /// Graphemes walked.
    graphemes: usize,
    /// The display column the line ends at, if the walk got to its end (it
    /// stops at the right edge of the pane).
    end: Option<usize>,
}

/// Paints one row of text from display column `left`, and stops at the right
/// edge: a very long line costs what is on screen, not its length.
fn paint_line(p: &Painter, buf: &mut Buffer, text: Rect, y: u16, line: &str, left: usize, look: &Look) -> Painted {
    let theme = look.theme;
    let right = left + text.width as usize;
    let (mut col, mut ch, mut si) = (0usize, 0usize, 0usize);
    let place = |buf: &mut Buffer, sym: &str, width: usize, from: usize, style: Style| {
        // Cells outside the window are skipped; a wide character cut by an
        // edge is blank cells, so the others keep their columns.
        if from + width <= left || from >= right {
            return;
        }
        if from >= left && from + width <= right {
            p.glyph(buf, text.x + (from - left) as u16, y, sym, width, style);
        } else {
            for c in from.max(left)..(from + width).min(right) {
                p.glyph(buf, text.x + (c - left) as u16, y, " ", 1, style);
            }
        }
    };
    let mut graphemes = 0;
    let mut finished = true;
    for g in line.graphemes(true) {
        if col >= right {
            finished = false;
            break;
        }
        graphemes += 1;
        let chars = g.chars().count();
        while look.spans.get(si).is_some_and(|s| s.end <= ch) {
            si += 1;
        }
        let mut role = look.spans.get(si).filter(|s| s.start <= ch).map_or(Role::Text, |s| s.role);
        if role == Role::SynIdentifier {
            if let Some(m) = look.marks.iter().find(|m| m.start <= ch && ch < m.end) {
                role = m.role;
            }
        }
        let mut style = Style::default().fg(theme.color(role)).add_modifier(theme.modifier(role)).bg(look.base_bg);
        if look.marked.iter().any(|&m| (ch..ch + chars).contains(&m)) {
            style = style.bg(theme.color(Role::MatchedBrace)).add_modifier(theme.modifier(Role::MatchedBrace));
        }
        if look.selected.is_some_and(|(a, b)| ch < b && ch + chars > a) {
            style = style.bg(theme.color(Role::SelectionBg)).add_modifier(theme.modifier(Role::SelectionBg));
        }
        let caret = look.caret.is_some_and(|c| (ch..ch + chars).contains(&c));
        let here = |style: Style| if caret { style.add_modifier(Modifier::REVERSED) } else { style };
        if g == "\t" {
            let pad = TAB - col % TAB;
            for i in 0..pad {
                place(buf, " ", 1, col + i, if i == 0 { here(style) } else { style });
            }
            col += pad;
        } else {
            // What a terminal would act on never reaches a cell.
            let safe = display_safe(g);
            let width = UnicodeWidthStr::width(safe.as_str());
            if width > 0 {
                place(buf, &safe, width, col, here(style));
                col += width;
            }
        }
        ch += chars;
    }
    // The caret at the end of the line is a reversed space after it.
    if finished && look.caret.is_some_and(|c| c >= ch) {
        let style = Style::default().fg(theme.color(Role::Text)).bg(look.base_bg).add_modifier(Modifier::REVERSED);
        place(buf, " ", 1, col, style);
    }
    Painted { graphemes, end: finished.then_some(col) }
}
