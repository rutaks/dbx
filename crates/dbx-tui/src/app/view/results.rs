//! The results pane: the grid (a fixed header and row-number gutter, virtual
//! scrolling on both axes, numbers right-aligned, NULL as a dimmed glyph); the
//! affected count for a statement without a result set; a failed run's
//! message, position and statement.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    affected_text, draw_scrollbar, duration_text, panel, scrolled, thousands, width_of, Painter, MISSING_MARK, NEWLINE_MARK, NULL_MARK,
    TAB_MARK,
};
use crate::app::grid::{self, GridUi, Span, GAP, MAX_COLUMN};
use crate::app::keys::Action;
use crate::app::query::{CancelEnd, Outcome, RunState};
use crate::app::theme::Role;
use crate::app::App;
use crate::core::{Cell, CellKind, CoreError, CoreErrorKind, ResultSet};

/// Draws inside `area` (the pane below its title row): the run's state when
/// a run is in flight or was cancelled, then what the last run that gave a
/// result gave. The line naming the statement is drawn from that same result,
/// so it can never name another statement than the rows below it; while the
/// state line is shown, it says the rows are the earlier run's.
pub(super) fn draw(app: &mut App, p: &Painter, buf: &mut Buffer, area: Rect) {
    let p = p.within(area);
    if area.is_empty() {
        return;
    }
    let used = draw_state(app, &p, buf, area);
    let area = Rect { y: area.y + used, height: area.height.saturating_sub(used), ..area };
    if area.is_empty() {
        return;
    }
    let hint = match app.ui.keys.key_for(Action::RunDirect) {
        Some(key) => format!("Results appear here. {key} runs the statement under the cursor."),
        None => "Results appear here.".to_string(),
    };
    let focused = app.ui.focus == crate::app::PaneId::Results;
    let App { model, ui } = app;
    let tab = &model.tabs[0];
    // The outcome is the latest run's only while nothing came after it, and
    // the rows being loaded more of are the same statement's.
    let earlier = tab.run != RunState::Idle && tab.loading_more.is_none();
    match &tab.outcome {
        Some(Outcome::Rows(set)) => {
            // What ran heads the pane: the cursor's statement is not always
            // the one in view (dbx runs the one before a blank line).
            draw_ran(&p, buf, area, &set.ran, earlier);
            let below = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
            if set.columns.is_empty() {
                draw_affected(&p, buf, below, set)
            } else {
                draw_grid(&p, buf, below, set, &mut ui.grid, focused)
            }
        }
        Some(Outcome::Failed { error, .. } | Outcome::TimedOut { error, .. }) => {
            let area = if earlier {
                p.put(buf, area.x, area.y, EARLIER, p.fg(Role::Overlay0));
                Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area }
            } else {
                area
            };
            draw_error(&p, buf, area, error)
        }
        None if used > 0 => {}
        None => {
            p.put(buf, area.x, area.y, &hint, p.fg(Role::Overlay0));
        }
    }
}

/// How many rows at the top of the results pane (below its title row) say
/// what the pane shows: the run's state and the line naming what ran. Nothing
/// may be drawn over them (the notice keeps below them).
pub(super) fn head_rows(app: &App, width: u16) -> u16 {
    let tab = app.tab();
    let state = if tab.run == RunState::Idle && tab.abandoned.is_empty() {
        0
    } else {
        // As `draw_state` wraps it, give or take: up to three rows and a blank.
        let mut buf = Buffer::empty(Rect::new(0, 0, width.max(1), 5));
        let theme = app.ui.theme.clone();
        let p = Painter { theme: &theme, bounds: buf.area };
        draw_state(app, &p, &mut buf, Rect::new(0, 0, width.max(1), 5))
    };
    state + u16::from(tab.outcome.is_some())
}

/// What heads a result that is not the latest run's.
const EARLIER: &str = "earlier run:";

/// The run's state, when there is one to tell, then a line for each run the
/// tab let go of that may still be running, each wrapped over at most three
/// rows, then a blank row. Returns the rows used.
fn draw_state(app: &App, p: &Painter, buf: &mut Buffer, area: Rect) -> u16 {
    let tab = app.tab();
    let mut lines: Vec<(String, Role)> = Vec::new();
    lines.extend(run_line(app));
    for a in &tab.abandoned {
        lines.push((format!("an abandoned statement may still be running on {}", a.on), Role::Peach));
    }
    if lines.is_empty() {
        return 0;
    }
    let mut y = area.y;
    for (text, role) in lines {
        for (i, line) in panel::wrap(&text, area.width.saturating_sub(2) as usize).iter().take(3).enumerate() {
            if i == 0 {
                p.put(buf, area.x, y, "●", p.fg(role));
            }
            p.put(buf, area.x + 2, y, line, p.fg(role));
            y += 1;
        }
    }
    (y - area.y + 1).min(area.height)
}

/// What the run's state line says, if anything. It never claims more than
/// the client knows: Postgres does not confirm a cancel, so a statement is
/// called stopped only once the run has ended.
fn run_line(app: &App) -> Option<(String, Role)> {
    let tab = app.tab();
    let secs = |started: std::time::Instant| app.ui.now.map_or(0, |now| now.saturating_duration_since(started).as_secs());
    let cancel_key = app.ui.keys.key_for(Action::CancelDirect).unwrap_or_default();
    Some(match &tab.run {
        RunState::Idle => return None,
        RunState::Running { started, .. } if tab.loading_more.is_some() => (
            format!("loading more… {}s: the rows so far stay below", secs(*started)),
            Role::Yellow,
        ),
        RunState::Running { started, .. } => match super::timeout_phase(app, *started) {
            Some(false) => (format!("timed out: stopping… {}s", secs(*started)), Role::Yellow),
            Some(true) => (
                format!("timed out, and not stopped yet after {}s: the client cannot tell whether the cancel reached the server", secs(*started)),
                Role::Red,
            ),
            None => (format!("running {}s", secs(*started)), Role::Yellow),
        },
        RunState::Cancelling { stuck: false, started, .. } => {
            (format!("cancelling… {}s: waiting for the statement to stop", secs(*started)), Role::Yellow)
        }
        RunState::Cancelling { stuck: true, started, .. } => (
            format!(
                "not stopped yet after {}s: the client cannot tell whether the cancel reached the server. {cancel_key} again offers to stop waiting.",
                secs(*started)
            ),
            Role::Red,
        ),
        RunState::Cancelled { after, statement, how } => {
            let what = statement.as_deref().map(first_line).map(|s| format!(": {s}")).unwrap_or_default();
            match how {
                CancelEnd::Stopped => (format!("cancelled after {}{what}", duration_text(*after)), Role::Subtext0),
                CancelEnd::FinishedFirst => (
                    format!(
                        "finished before the cancel, after {}: any changes it made stand; its result is not shown{what}",
                        duration_text(*after)
                    ),
                    Role::Peach,
                ),
                CancelEnd::Failed(why) => (format!("failed after the cancel, after {}: {why}", duration_text(*after)), Role::Red),
                CancelEnd::Abandoned => (
                    format!(
                        "stopped waiting after {}: not confirmed stopped. This tab runs in a new session now.",
                        duration_text(*after)
                    ),
                    Role::Peach,
                ),
            }
        }
    })
}

/// The first line of a statement, with `…` when there is more.
fn first_line(statement: &str) -> String {
    let mut lines = statement.trim().lines();
    let first = lines.next().unwrap_or("").trim_end();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

/// The first line of the statement that ran, in `overlay0`, with `…` when
/// there is more of it than is shown; after `earlier run:` when it is not the
/// latest run's.
fn draw_ran(p: &Painter, buf: &mut Buffer, area: Rect, ran: &str, earlier: bool) {
    let text = if earlier { format!("{EARLIER} {}", first_line(ran)) } else { first_line(ran) };
    p.put(buf, area.x, area.y, &text, p.fg(Role::Overlay0));
}

/// A cell as one line of text, as much of it as a column can show: a line
/// break or a tab inside it is a glyph, NULL is its glyph, and a cell the row
/// does not have (a row shorter than the header) has its own. Only the start
/// of a long value is looked at (enough to fill the widest column and know it
/// goes on), so a 10 MB cell costs no more to draw than a short one. The
/// stored cell stays whole.
pub(crate) fn cell_text(cell: Option<&Cell>) -> String {
    let v = match cell {
        None => return MISSING_MARK.to_string(),
        Some(Cell::Null) => return NULL_MARK.to_string(),
        Some(Cell::Value(v)) => v,
    };
    let (mut out, mut width, mut chars) = (String::new(), 0usize, 0usize);
    for g in v.graphemes(true) {
        // Past the widest column, or (for text of zero-width characters)
        // past what any column could hold, the rest cannot be seen.
        if width > MAX_COLUMN as usize || chars > 8 * MAX_COLUMN as usize {
            break;
        }
        let g = match g {
            "\r\n" | "\n" | "\r" => NEWLINE_MARK,
            "\t" => TAB_MARK,
            g => g,
        };
        out.push_str(g);
        width += UnicodeWidthStr::width(g);
        chars += 1;
    }
    out
}

fn right_aligned(kind: CellKind) -> bool {
    matches!(kind, CellKind::Integer | CellKind::Decimal | CellKind::Float)
}

/// The digits of `n` (at least one).
fn digits(n: usize) -> usize {
    n.max(1).to_string().len()
}

/// Draws the grid in `area`: the header row, then the rows from `g.top`, the
/// row-number gutter on the left and a `▕` scrollbar on the right when the
/// rows overflow. Work is proportional to what is on screen: the rows and
/// columns in the window, each cell read only as far as a column shows it.
/// Moves the window so the cursor cell is on screen (whole columns at a time,
/// and a column wider than the pane by its left part).
fn draw_grid(p: &Painter, buf: &mut Buffer, area: Rect, set: &ResultSet, g: &mut GridUi, focused: bool) {
    let (rows, cols) = (set.rows.len(), set.columns.len());
    let rows_h = area.height.saturating_sub(1) as usize;
    g.rows_visible = rows_h;
    g.cols_visible = 0;
    g.spans.clear();
    g.data = Rect::default();
    g.bar = Rect::default();
    if area.width == 0 || area.height == 0 || cols == 0 {
        return;
    }
    g.row = g.row.min(rows.saturating_sub(1));
    g.col = g.col.min(cols - 1);
    let overflow = rows > rows_h && area.width > 6;
    let content_w = area.width - u16::from(overflow);
    let gutter = if content_w >= 8 { (digits(rows) + 1) as u16 } else { 0 };
    let avail = content_w - gutter;
    g.top = scrolled(g.top.min(rows.saturating_sub(rows_h)), g.row, rows_h);
    g.cache_at(rows_h, cols);

    // Horizontal: the first column drawn is `left`, moved by whole columns
    // only as far as the cursor column needs.
    g.left = g.left.min(g.col);
    let width = |g: &mut GridUi, c: usize| g.widths.width(set, c).min(avail.max(1));
    let (mut used, mut fits) = (0u16, true);
    for c in g.left..=g.col {
        used = used.saturating_add(width(g, c) + if c > g.left { GAP } else { 0 });
        if used > avail {
            fits = false;
            break;
        }
    }
    if !fits {
        // Walk back from the cursor column while columns still fit beside it.
        g.left = g.col;
        let mut used = width(g, g.col);
        while g.left > 0 {
            let w = width(g, g.left - 1);
            if used + GAP + w > avail {
                break;
            }
            used += GAP + w;
            g.left -= 1;
        }
    }
    let mut at = 0u16;
    for c in g.left..cols {
        let w = width(g, c);
        // A column that does not fit whole is not drawn, except the first,
        // which shows its left part.
        if at + w > avail && c > g.left {
            break;
        }
        g.spans.push(Span { x: area.x + gutter + at, width: w, col: c });
        at += w + GAP;
    }
    g.cols_visible = g.spans.len();
    g.data = Rect::new(area.x + gutter, area.y + 1, avail, rows_h as u16);

    let row_bg = Style::default().bg(p.theme.color(Role::ActiveRowBg));
    let cursor_bg = Style::default().bg(p.theme.color(Role::SelectionBg));
    let header = p.bold(Role::Subtext0);
    for span in &g.spans {
        let c = &set.columns[span.col];
        let style = if span.col == g.col && focused { p.bold(Role::Accent) } else { header };
        put_cell(p, buf, span.x, area.y, span.width, &c.name, right_aligned(c.kind), style);
    }
    if rows == 0 {
        if area.height > 1 {
            p.put(buf, area.x, area.y + 1, "0 rows", p.fg(Role::Overlay0));
        }
        return;
    }
    for (i, y) in (area.y + 1..area.bottom()).enumerate() {
        let r = g.top + i;
        let Some(row) = set.rows.get(r) else { break };
        let here = r == g.row;
        if here {
            p.within(Rect::new(area.x, y, content_w, 1)).fill(buf, row_bg);
        }
        if gutter > 0 {
            let number = format!("{:>w$}", r + 1, w = gutter as usize - 1);
            p.within(Rect::new(area.x, y, gutter, 1)).put(buf, area.x, y, &number, p.fg(if here { Role::Subtext0 } else { Role::Overlay0 }));
        }
        for span in &g.spans {
            let cell = row.get(span.col);
            if here && span.col == g.col {
                p.within(Rect::new(span.x, y, span.width, 1)).fill(buf, cursor_bg);
            }
            let style = match cell {
                Some(Cell::Value(_)) => p.fg(Role::Text),
                Some(Cell::Null) => p.fg(Role::Overlay0),
                None => p.fg(Role::Peach),
            };
            let numeric = right_aligned(set.columns[span.col].kind) && matches!(cell, Some(Cell::Value(_)));
            put_cell(p, buf, span.x, y, span.width, &cell_text(cell), numeric, style);
        }
    }
    if overflow {
        g.bar = Rect::new(area.right() - 1, area.y + 1, 1, rows_h as u16);
        draw_scrollbar(p, buf, g.bar, rows, g.top);
    }
}

/// `r 501/10,000  c 12/120`: where the cursor is, for the pane's title row;
/// `+` after the row count when the result is truncated.
pub(super) fn position(app: &App) -> Option<String> {
    let set = grid::shown(&app.tab().outcome)?;
    if set.rows.is_empty() {
        return None;
    }
    let g = &app.ui.grid;
    let plus = if set.truncated { "+" } else { "" };
    Some(format!("r {}/{}{plus}  c {}/{}", thousands(g.row + 1), thousands(set.rows.len()), g.col + 1, set.columns.len()))
}

/// `text` in a column `width` wide at (x, y), against its right edge when
/// `right`; cut with `…` when it does not fit. Through the painter, so no
/// control character reaches the terminal.
#[allow(clippy::too_many_arguments)]
fn put_cell(p: &Painter, buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, right: bool, style: ratatui::style::Style) {
    let cell = p.within(Rect::new(x, y, width, 1));
    let w = width_of(text);
    let at = if right && w < width { x + width - w } else { x };
    cell.put(buf, at, y, text, style);
}

fn draw_affected(p: &Painter, buf: &mut Buffer, area: Rect, set: &ResultSet) {
    p.put(buf, area.x, area.y, &affected_text(set.affected), p.fg(Role::Text));
    for (y, message) in (area.y + 1..area.bottom()).zip(&set.messages) {
        p.put(buf, area.x, y, message, p.fg(Role::Overlay0));
    }
}

/// A failed run: what kind of failure and where, dbx's message, then the
/// statement that ran with a caret under the position.
fn draw_error(p: &Painter, buf: &mut Buffer, area: Rect, e: &CoreError) {
    let label = match e.kind {
        CoreErrorKind::Sql => "SQL error",
        CoreErrorKind::Connect => "Could not connect",
        CoreErrorKind::Timeout => "Timed out",
        CoreErrorKind::Cancelled => "Cancelled",
        _ => "Failed",
    };
    // A time limit is not a failure of the SQL: it is told in its own colour.
    let (role, text) = if e.kind == CoreErrorKind::Timeout { (Role::Peach, Role::Text) } else { (Role::Red, Role::Red) };
    let mut x = p.put(buf, area.x, area.y, "●", p.fg(role));
    x = p.put(buf, x + 1, area.y, label, p.bold(role));
    if let Some(at) = e.position {
        p.put(buf, x + 2, area.y, &format!("line {}, column {}", at.line, at.column), p.fg(Role::Overlay0));
    }
    let mut y = area.y + 1;
    for line in panel::wrap(&e.message, area.width as usize) {
        if y >= area.bottom() {
            return;
        }
        p.put(buf, area.x, y, &line, p.fg(text));
        y += 1;
    }
    let Some(statement) = &e.statement else { return };
    y += 1;
    for (i, line) in statement.split('\n').enumerate() {
        if y >= area.bottom() {
            return;
        }
        p.put(buf, area.x, y, line, p.fg(Role::Subtext0));
        y += 1;
        // The caret under the column the server named, on the line it named.
        if let Some(at) = e.position.filter(|at| at.line as usize == i + 1 && at.column > 0) {
            if y >= area.bottom() {
                return;
            }
            let before: String = line.chars().take(at.column as usize - 1).collect();
            p.put(buf, area.x + width_of(&before), y, "^", p.bold(Role::Red));
            y += 1;
        }
    }
}
