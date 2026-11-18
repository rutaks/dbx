//! The results pane for slice 05: a header, the rows that fit, numbers
//! right-aligned, NULL as a dimmed glyph; the affected count for a statement
//! without a result set; a failed run's message, position and statement.
//! Scrolling and the full grid are slice 07.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{affected_text, duration_text, panel, width_of, Painter, NEWLINE_MARK, NULL_MARK};
use crate::app::keys::Action;
use crate::app::query::{CancelEnd, Outcome, RunState};
use crate::app::theme::Role;
use crate::app::App;
use crate::core::{Cell, CellKind, CoreError, CoreErrorKind, ResultSet};

/// The widest a column is drawn; longer cells end in `…`.
const MAX_COLUMN: u16 = 40;
/// Columns between two columns.
const GAP: u16 = 2;

/// Draws inside `area` (the pane below its title row): the run's state when
/// a run is in flight or was cancelled, then what the last run that gave a
/// result gave. The line naming the statement is drawn from that same result,
/// so it can never name another statement than the rows below it; while the
/// state line is shown, it says the rows are the earlier run's.
pub(super) fn draw(app: &App, p: &Painter, buf: &mut Buffer, area: Rect) {
    let p = p.within(area);
    if area.is_empty() {
        return;
    }
    let tab = app.tab();
    let used = draw_state(app, &p, buf, area);
    let area = Rect { y: area.y + used, height: area.height.saturating_sub(used), ..area };
    if area.is_empty() {
        return;
    }
    // The outcome is the latest run's only while nothing came after it.
    let earlier = tab.run != RunState::Idle;
    match &tab.outcome {
        Some(Outcome::Rows(set)) => {
            // What ran heads the pane: the cursor's statement is not always
            // the one in view (dbx runs the one before a blank line).
            draw_ran(&p, buf, area, &set.ran, earlier);
            let below = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
            if set.columns.is_empty() {
                draw_affected(&p, buf, below, set)
            } else {
                draw_rows(&p, buf, below, set)
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
            let hint = match app.ui.keys.key_for(Action::RunDirect) {
                Some(key) => format!("Results appear here. {key} runs the statement under the cursor."),
                None => "Results appear here.".to_string(),
            };
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
/// break inside it is a glyph. Only the start of a long value is looked at
/// (enough to fill the widest column and know it goes on), so a 10 MB cell
/// costs no more to draw than a short one. The stored cell stays whole.
fn cell_text(cell: &Cell) -> Option<String> {
    let Cell::Value(v) = cell else { return None };
    let (mut out, mut width, mut chars) = (String::new(), 0usize, 0usize);
    for g in v.graphemes(true) {
        // Past the widest column, or (for text of zero-width characters)
        // past what any column could hold, the rest cannot be seen.
        if width > MAX_COLUMN as usize || chars > 8 * MAX_COLUMN as usize {
            break;
        }
        let g = if g == "\r\n" || g == "\n" || g == "\r" { NEWLINE_MARK } else { g };
        out.push_str(g);
        width += UnicodeWidthStr::width(g);
        chars += 1;
    }
    Some(out)
}

fn right_aligned(kind: CellKind) -> bool {
    matches!(kind, CellKind::Integer | CellKind::Decimal | CellKind::Float)
}

fn draw_rows(p: &Painter, buf: &mut Buffer, area: Rect, set: &ResultSet) {
    // Room for the header and as many rows as fit.
    let shown = set.rows.iter().take(area.height.saturating_sub(1) as usize);
    // At most as many columns as could start on screen (one column each).
    let most = area.width.div_ceil(1 + GAP) as usize;
    let texts: Vec<Vec<Option<String>>> = shown.map(|row| row.iter().take(most).map(cell_text).collect()).collect();
    // Only the columns that start on screen are measured.
    let mut widths: Vec<u16> = Vec::new();
    let mut x = area.x;
    for (i, c) in set.columns.iter().enumerate() {
        if x >= area.right() {
            break;
        }
        let cells = texts.iter().map(|r| r.get(i).cloned().flatten().map_or(1, |t| width_of(&t)));
        let w = cells.chain([width_of(&c.name)]).max().unwrap_or(1).clamp(1, MAX_COLUMN);
        widths.push(w);
        x = x.saturating_add(w + GAP);
    }
    let header = p.bold(Role::Subtext0);
    let mut x = area.x;
    for (c, w) in set.columns.iter().zip(&widths) {
        if x >= area.right() {
            break;
        }
        put_cell(p, buf, x, area.y, *w, &c.name, right_aligned(c.kind), header);
        x += w + GAP;
    }
    for (row_y, row) in (area.y + 1..area.bottom()).zip(&texts) {
        let mut x = area.x;
        for ((cell, c), w) in row.iter().zip(&set.columns).zip(&widths) {
            if x >= area.right() {
                break;
            }
            match cell {
                Some(text) => put_cell(p, buf, x, row_y, *w, text, right_aligned(c.kind), p.fg(Role::Text)),
                None => put_cell(p, buf, x, row_y, *w, NULL_MARK, right_aligned(c.kind), p.fg(Role::Overlay0)),
            }
            x += w + GAP;
        }
    }
    if set.rows.is_empty() && area.height > 1 {
        p.put(buf, area.x, area.y + 1, "0 rows", p.fg(Role::Overlay0));
    }
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
