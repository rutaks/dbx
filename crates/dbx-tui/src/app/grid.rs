//! The result grid's state: the cell cursor, the window onto the result, and
//! the cached column widths. Drawing is `view/results.rs`; the keys that move
//! the cursor are `App::on_results_key` below.
//!
//! **Column widths** are measured over a bounded sample, never the whole
//! result. A result can hold a million rows, and a column's width should not
//! cost a scan of them (nor of a 10 MB cell: each cell is looked at only as
//! far as the widest column could show it). The sample is a window of rows
//! aligned to `CHUNK` rows and covering the viewport and the rows after it, so
//! a column keeps its width while the viewport stays inside the window (no
//! jitter on every scroll step) and a result of up to two chunks is measured
//! whole. A column is measured when it is first needed (it is on screen, or
//! the cursor needs it), and the measure is cached until the result changes or
//! the viewport leaves the window.

use std::time::Instant;

use ratatui::layout::Rect;

use super::keys::{Action, KeyCombo};
use super::overlay::{ConfirmRerun, Overlay, OverlayAction};
use super::query::{LoadMore, Outcome, RunState};
use super::requests::Target;
use super::viewer::{CellViewer, Scroll};
use super::{App, Command, CoreCall, Effects};
use crate::core::{row_ceiling, Cell, Cursor, Rerun, ResultSet, RunRequest, TabId};

/// The most text one copy sends. The clipboard travels as an OSC 52 escape
/// sequence (base64, so a third larger), and terminals cap those: xterm takes
/// 100,000 bytes by default, tmux and kitty more, some less. 75,000 bytes of
/// text is 100,000 of base64.
pub const CLIPBOARD_LIMIT: usize = 75_000;

/// Narrowest and widest a column is drawn, in display columns. Three keeps a
/// short name readable and a `…` meaningful; forty keeps one long value from
/// pushing every other column off the screen (the cell viewer shows it whole).
pub const MIN_COLUMN: u16 = 3;
pub const MAX_COLUMN: u16 = 40;
/// Columns between two columns.
pub const GAP: u16 = 2;
/// The sample window is aligned to this many rows.
pub const CHUNK: usize = 128;

/// One column as drawn: where it starts on screen, how wide, and which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub x: u16,
    pub width: u16,
    pub col: usize,
}

/// The grid's state. One per query tab (`tab::TabUi`).
#[derive(Debug, Clone, Default)]
pub struct GridUi {
    /// The cursor: a row and a column of the result.
    pub row: usize,
    pub col: usize,
    /// The first row and the first column drawn.
    pub top: usize,
    pub left: usize,
    /// How many rows fit, and how many whole columns were drawn, in the last
    /// frame. Page keys and the wheel move by these.
    pub rows_visible: usize,
    pub cols_visible: usize,
    /// Where the data rows were drawn (below the header, right of the gutter).
    pub data: Rect,
    /// The scrollbar's column as drawn, when the rows overflow.
    pub bar: Rect,
    /// The columns drawn in the last frame.
    pub spans: Vec<Span>,
    pub widths: WidthCache,
}

/// Column widths for the current sample window (see the module comment).
#[derive(Debug, Clone, Default)]
pub struct WidthCache {
    window: Option<(usize, usize)>,
    widths: Vec<Option<u16>>,
    /// How many cells were measured so far: tests use it to show that a move
    /// inside the window measures nothing and a jump measures a bounded amount.
    pub measured: usize,
}

impl WidthCache {
    /// Forgets everything (the result changed).
    pub fn clear(&mut self) {
        self.window = None;
        self.widths.clear();
    }

    /// Points the cache at the window for a viewport starting at row `top`
    /// and `rows_visible` high; forgets the widths if the window moved.
    pub fn at(&mut self, top: usize, rows_visible: usize, columns: usize) {
        let start = top / CHUNK * CHUNK;
        let window = (start, start + (rows_visible / CHUNK + 2) * CHUNK);
        if self.window != Some(window) || self.widths.len() != columns {
            self.window = Some(window);
            self.widths.clear();
            self.widths.resize(columns, None);
        }
    }

    /// The width of column `col`: its header and the cells of the window,
    /// between `MIN_COLUMN` and `MAX_COLUMN`.
    pub fn width(&mut self, set: &ResultSet, col: usize) -> u16 {
        if let Some(Some(w)) = self.widths.get(col) {
            return *w;
        }
        let (from, to) = self.window.unwrap_or((0, CHUNK * 2));
        let mut w = super::view::width_of(&set.columns[col].name);
        for row in set.rows.iter().skip(from).take(to.saturating_sub(from)) {
            self.measured += 1;
            w = w.max(super::view::width_of(&super::view::cell_text(row.get(col))));
            if w >= MAX_COLUMN {
                break;
            }
        }
        let w = w.clamp(MIN_COLUMN, MAX_COLUMN);
        if let Some(slot) = self.widths.get_mut(col) {
            *slot = Some(w);
        }
        w
    }
}

impl GridUi {
    /// Points the width cache at the window for the current viewport.
    pub fn cache_at(&mut self, rows_visible: usize, columns: usize) {
        self.widths.at(self.top, rows_visible, columns);
    }

    /// A new result: back to the corner, nothing measured.
    pub fn reset(&mut self) {
        *self = GridUi::default();
    }
}

/// The result the grid shows, if the last run gave rows and columns.
pub fn shown(outcome: &Option<Outcome>) -> Option<&ResultSet> {
    match outcome {
        Some(Outcome::Rows(set)) if !set.columns.is_empty() => Some(set),
        _ => None,
    }
}

/// The cell under (row, column), or `None` where the row is shorter than the
/// header. NULL is `Some(&Cell::Null)`.
pub fn cell_at(set: &ResultSet, row: usize, col: usize) -> Option<&Cell> {
    set.rows.get(row)?.get(col)
}

impl App {
    /// A key pressed while the results pane has focus.
    pub(super) fn on_results_key(&mut self, combo: super::keys::KeyCombo, now: Instant, fx: &mut Effects) {
        if let Some(action) = self.ui.keys.lookup_results(&combo) {
            self.perform_results(action, now, fx);
        }
    }

    /// What a results key (or a click on its hint) does.
    pub(super) fn perform_results(&mut self, action: Action, _now: Instant, fx: &mut Effects) {
        // The paging keys work on a page with no rows too (the one past the end).
        match action {
            Action::NextPage => return self.next_page(_now, fx),
            Action::PrevPage => return self.prev_page(_now, fx),
            Action::CountRows => return self.count_rows(_now, fx),
            _ => {}
        }
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (rows, cols) = (set.rows.len(), set.columns.len());
        let page = self.ui.tab.grid.rows_visible.max(1);
        let (row, col) = (self.ui.tab.grid.row, self.ui.tab.grid.col);
        match action {
            Action::OpenCell => {
                if let Some(viewer) = CellViewer::open(set, row, col) {
                    self.ui.overlay = Some(Overlay::CellViewer(Box::new(viewer)));
                    fx.repaint = true;
                }
                return;
            }
            Action::CopyCell => return self.copy_cell(fx),
            Action::CopyRow => return self.copy_row(fx),
            Action::Export => return self.open_export(fx),
            Action::CopyAs => return self.open_copy_as(fx),
            Action::LoadMore => return self.load_more(_now, fx),
            _ => {}
        }
        let g = &mut self.ui.tab.grid;
        let last_row = rows.saturating_sub(1);
        match action {
            Action::GridLeft => g.col = col.saturating_sub(1),
            Action::GridRight => g.col = (col + 1).min(cols - 1),
            Action::GridUp => g.row = row.saturating_sub(1),
            Action::GridDown => g.row = (row + 1).min(last_row),
            Action::GridPageUp => g.row = row.saturating_sub(page),
            Action::GridPageDown => g.row = (row + page).min(last_row),
            Action::GridFirstCol => g.col = 0,
            Action::GridLastCol => g.col = cols - 1,
            Action::GridFirstRow => g.row = 0,
            Action::GridLastRow => g.row = last_row,
            _ => return,
        }
        if (g.row, g.col) != (row, col) {
            fx.repaint = true;
        }
    }

    /// Why "load more" would be refused now, in the words of the notice, or
    /// `None` when it can go ahead (maybe after a question). The `m more` hint
    /// is shown only when this is `None`.
    pub(super) fn load_more_refusal(&self) -> Option<String> {
        self.load_more_refusal_of(self.ui.tab.id)
    }

    fn load_more_refusal_of(&self, id: TabId) -> Option<String> {
        let tab = self.model.tab(id)?;
        // A browse tab pages instead.
        if tab.browse.is_some() {
            return Some("Load more is not offered while browsing a table: it shows one page at a time.".into());
        }
        let set = shown(&tab.outcome).filter(|set| set.truncated)?;
        if tab.run.in_flight().is_some() {
            return Some("A statement is still running in this tab: load more once it has ended.".into());
        }
        if set.rerun == Rerun::Write {
            return Some("Load more runs the statement again, and this one changes data, so it is not repeated. Run it from the editor if you mean to.".into());
        }
        if tab.session != tab.result_session {
            return Some("This tab has a new session since this result: its SET values and temporary tables are gone, so running the statement again may give other rows. Run it from the editor.".into());
        }
        let ceiling = row_ceiling(set.columns.len());
        if set.rows.len() >= ceiling {
            return Some(format!(
                "Load more stops at {} rows for a result this wide: narrow the query with WHERE or LIMIT.",
                super::view::thousands(ceiling)
            ));
        }
        None
    }

    /// "Load more": runs `ResultSet.ran` again with twice the rows (design
    /// section 18, ADR 0004). The result on screen stays, with its cursor,
    /// until the new one lands. Does nothing for a result the cap did not cut;
    /// refused, with a notice, while the tab runs or cancels, for a statement
    /// that changes data, once the tab's session is another one, and at the
    /// ceiling. A statement that is read-only but that dbx cannot prove
    /// repeatable (it calls a function) is run only after the user says yes.
    pub(super) fn load_more(&mut self, now: Instant, fx: &mut Effects) {
        if self.is_browsing() {
            return;
        }
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome).filter(|set| set.truncated) else { return };
        if let Some(text) = self.load_more_refusal() {
            return self.notify(text, true, fx);
        }
        if set.rerun == Rerun::ReadOnlyUnproven {
            let first = set.ran.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
            self.ui.overlay = Some(Overlay::ConfirmRerun(ConfirmRerun { tab: self.ui.tab.id, statement: first }));
            fx.repaint = true;
            return;
        }
        self.begin_load_more(self.ui.tab.id, now, fx);
    }

    /// The answer yes to the re-run question.
    pub(super) fn confirm_rerun(&mut self, fx: &mut Effects) {
        // The tab the question was asked in, whichever is active now.
        let asked = match &self.ui.overlay {
            Some(Overlay::ConfirmRerun(ask)) => Some(ask.tab),
            _ => None,
        };
        self.ui.overlay = None;
        fx.repaint = true;
        let id = asked.unwrap_or(self.ui.tab.id);
        let now = self.ui.now.unwrap_or_else(Instant::now);
        // The state may have moved while the question was open.
        if let Some(text) = self.load_more_refusal_of(id) {
            return self.notify(text, true, fx);
        }
        self.begin_load_more(id, now, fx);
    }

    fn begin_load_more(&mut self, id: TabId, now: Instant, fx: &mut Effects) {
        let Some(tab) = self.model.tab(id) else { return };
        let Some(set) = shown(&tab.outcome) else { return };
        let Some(target) = tab.target.clone() else { return };
        let fetched = set.rows.len();
        let ceiling = row_ceiling(set.columns.len());
        let cap = (fetched * 2).clamp(1, ceiling);
        // The whole statement is the selection, so the Core runs exactly it.
        let lines: Vec<&str> = set.ran.split('\n').collect();
        let end = Cursor { row: lines.len() - 1, col: lines.last().map_or(0, |l| l.chars().count()) };
        let (ran, session) = (set.ran.clone(), tab.session());
        let run = crate::core::RunId(self.model.next_run);
        self.model.next_run += 1;
        let request_id = self.model.requests.begin(Target::Run(id));
        let request = RunRequest {
            connection: target.connection,
            database: Some(target.database),
            session,
            buffer: ran,
            cursor: Cursor::default(),
            selection: Some((Cursor::default(), end)),
            row_cap: cap,
            confirm_dangerous: self.model.settings.confirm_dangerous,
            confirmation: None,
        };
        let tab = self.model.active_mut(id);
        tab.run = RunState::Running { run, request: request_id, started: now };
        tab.loading_more = Some(LoadMore { cap });
        // The statement is already in the history from the run that made the result.
        tab.history = false;
        fx.commands.push(Command::Core(request_id, CoreCall::Run(run, request)));
        fx.repaint = true;
    }

    /// The wheel over the results pane: `rows` rows and `cols` columns. The
    /// window moves; the cursor moves only as far as it must to stay in it.
    pub(super) fn scroll_grid(&mut self, rows: isize, cols: isize, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (nrows, ncols) = (set.rows.len(), set.columns.len());
        let g = &mut self.ui.tab.grid;
        let before = (g.top, g.left, g.row, g.col);
        let room = g.rows_visible.max(1);
        g.top = g.top.saturating_add_signed(rows).min(nrows.saturating_sub(room));
        g.row = g.row.clamp(g.top, (g.top + room - 1).min(nrows.saturating_sub(1)));
        // Columns: the window moves by whole columns, and the cursor stays among them.
        let shown_cols = g.cols_visible.max(1);
        let old_left = g.left;
        g.left = g.left.saturating_add_signed(cols).min(ncols.saturating_sub(1));
        let last = (old_left + shown_cols - 1).max(g.left).min(ncols.saturating_sub(1));
        g.col = g.col.clamp(g.left, last);
        if (g.top, g.left, g.row, g.col) != before {
            fx.repaint = true;
        }
    }

    /// A left click on the grid: the cursor goes to the cell under it. The
    /// row-number gutter picks a row, a gap between columns the column before
    /// it, and the header and the empty rows below the last one nothing.
    /// Returns the cell the click landed on, when it landed on one (the gutter
    /// is not a cell), for double-clicks.
    pub(super) fn click_grid(&mut self, x: u16, y: u16, fx: &mut Effects) -> Option<(usize, usize)> {
        let set = shown(&self.model.active(self.ui.tab.id).outcome)?;
        let g = &mut self.ui.tab.grid;
        if y < g.data.y || y >= g.data.bottom() {
            return None;
        }
        let row = g.top + (y - g.data.y) as usize;
        if row >= set.rows.len() {
            return None;
        }
        let before = (g.row, g.col);
        g.row = row;
        let mut hit = None;
        if x >= g.data.x {
            if let Some(span) = g.spans.iter().rev().find(|s| s.x <= x) {
                g.col = span.col;
                hit = Some((row, span.col));
            }
        }
        if (g.row, g.col) != before {
            fx.repaint = true;
        }
        hit
    }

    /// A press on the results' scrollbar column at screen row `y`: the list
    /// jumps there (the thumb is centred on the press), or, on the thumb, it
    /// is grabbed where it was pressed. Returns the drag it begins.
    pub(super) fn press_grid_bar(&mut self, y: u16, fx: &mut Effects) -> Option<super::pointer::Drag> {
        let set = shown(&self.model.active(self.ui.tab.id).outcome)?;
        let (bar, top) = (self.ui.tab.grid.bar, self.ui.tab.grid.top);
        let thumb = super::view::scrollbar_thumb(set.rows.len(), bar.height as usize, top)?;
        let (grab, _) = super::pointer::grab(thumb, y.saturating_sub(bar.y) as usize);
        self.drag_grid_bar(y, grab as u16, fx);
        Some(super::pointer::Drag::GridBar { grab: grab as u16 })
    }

    /// The pointer is at screen row `y` with the thumb grabbed `grab` rows
    /// below its top: scroll so the thumb is there.
    pub(super) fn drag_grid_bar(&mut self, y: u16, grab: u16, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (bar, top) = (self.ui.tab.grid.bar, self.ui.tab.grid.top);
        let (total, room) = (set.rows.len(), bar.height as usize);
        let Some((_, len)) = super::view::scrollbar_thumb(total, room, top) else { return };
        let thumb_top = (y.saturating_sub(bar.y) as usize).saturating_sub(grab as usize);
        let scroll = super::pointer::scroll_for_thumb(total, room, len, thumb_top);
        self.scroll_grid(scroll as isize - top as isize, 0, fx);
    }

    /// A key pressed while the cell viewer is open.
    pub(super) fn on_viewer_key(&mut self, combo: KeyCombo, fx: &mut Effects) {
        if combo.code == crossterm::event::KeyCode::Esc {
            return self.overlay_action(OverlayAction::Dismiss, fx);
        }
        let Some(action) = self.ui.keys.lookup_results(&combo) else { return };
        match action {
            Action::GridDown => self.scroll_viewer_by(Scroll::Line(1), fx),
            Action::GridUp => self.scroll_viewer_by(Scroll::Line(-1), fx),
            Action::GridPageDown => self.scroll_viewer_by(Scroll::Page(1), fx),
            Action::GridPageUp => self.scroll_viewer_by(Scroll::Page(-1), fx),
            Action::GridFirstRow => self.scroll_viewer_by(Scroll::Top, fx),
            Action::GridLastRow => self.scroll_viewer_by(Scroll::Bottom, fx),
            Action::ToggleRaw => self.toggle_viewer_raw(fx),
            Action::CopyCell => self.copy_viewer(fx),
            _ => {}
        }
    }

    /// The wheel over the viewer scrolls its text.
    pub(super) fn scroll_viewer(&mut self, lines: isize, fx: &mut Effects) {
        self.scroll_viewer_by(Scroll::Line(lines), fx);
    }

    /// Moves the viewer's window, repainting only if it moved.
    pub(super) fn scroll_viewer_by(&mut self, how: Scroll, fx: &mut Effects) {
        if let Some(Overlay::CellViewer(v)) = &mut self.ui.overlay {
            fx.repaint |= v.scroll(how);
        }
    }

    /// Pretty-printed or stored text: repaints only where there is a pretty form.
    pub(super) fn toggle_viewer_raw(&mut self, fx: &mut Effects) {
        if let Some(Overlay::CellViewer(v)) = &mut self.ui.overlay {
            if v.can_toggle() {
                v.toggle_raw();
                fx.repaint = true;
            }
        }
    }

    /// The viewer's copy key or button: the stored value, never the
    /// pretty-printed form.
    pub(super) fn copy_viewer(&mut self, fx: &mut Effects) {
        let Some(Overlay::CellViewer(v)) = &self.ui.overlay else { return };
        if v.missing {
            return self.nothing_to_copy(fx);
        }
        match v.value.clone() {
            Some(value) => self.copy_text(value, "cell", fx),
            None => self.copy_text(String::new(), "NULL", fx),
        }
    }

    fn copy_cell(&mut self, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        match cell_at(set, self.ui.tab.grid.row, self.ui.tab.grid.col) {
            Some(Cell::Value(v)) => {
                let v = v.clone();
                self.copy_text(v, "cell", fx)
            }
            Some(Cell::Null) => self.copy_text(String::new(), "NULL", fx),
            None => self.nothing_to_copy(fx),
        }
    }

    /// A cell the row does not have: nothing is sent.
    fn nothing_to_copy(&mut self, fx: &mut Effects) {
        self.notify("Nothing to copy: this row has no cell here (it is shorter than the header).".into(), true, fx);
    }

    /// The row's exact cell texts, one tab between each, NULL as an empty
    /// field. A cell that holds a tab or a line break is copied as it is, so
    /// pasting it into a spreadsheet splits it: the text is exact, and the
    /// separators are the format's.
    fn copy_row(&mut self, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let Some(row) = set.rows.get(self.ui.tab.grid.row) else { return };
        let cells = (0..set.columns.len()).map(|c| match row.get(c) {
            Some(Cell::Value(v)) => v.as_str(),
            _ => "",
        });
        // Counted first, so a row of huge cells is refused before it is built.
        let size: usize = cells.clone().map(str::len).sum::<usize>() + set.columns.len().saturating_sub(1);
        if size > CLIPBOARD_LIMIT {
            return self.refuse_copy("row", size, fx);
        }
        let text = cells.collect::<Vec<_>>().join("\t");
        let lossy = lossy_row(set, row);
        self.copy_text(text, "row", fx);
        // Said after what was sent, in the same notice: the text is exact, but
        // pasting it may not give the row back.
        if let (Some(why), Some(notice)) = (lossy, &mut self.ui.notice) {
            notice.text = format!("{}. Lossy: {why}.", notice.text);
        }
    }

    /// Sends `text` to the clipboard and says what was copied, or why not.
    /// Returns whether it was sent.
    pub(super) fn send_to_clipboard(&mut self, text: String, what: &str, fx: &mut Effects) -> bool {
        let sent = text.len() <= CLIPBOARD_LIMIT;
        self.copy_text(text, what, fx);
        sent
    }

    pub(super) fn copy_text(&mut self, text: String, what: &str, fx: &mut Effects) {
        if text.len() > CLIPBOARD_LIMIT {
            return self.refuse_copy(what, text.len(), fx);
        }
        let said = if what == "NULL" {
            "Sent an empty string to the terminal clipboard (the cell is NULL)".to_string()
        } else {
            // Only "sent": OSC 52 is silently dropped by tmux's default
            // `set-clipboard external`, macOS Terminal.app and iTerm2's
            // default, and nothing tells us.
            format!("Sent {what} to the terminal clipboard ({})", size_text(text.len()))
        };
        fx.commands.push(Command::CopyToClipboard(text));
        self.notify(said, false, fx);
    }

    fn refuse_copy(&mut self, what: &str, size: usize, fx: &mut Effects) {
        let text = format!(
            "Not copied: the {what} is {}, too large for the terminal clipboard (limit {}).",
            size_text(size),
            size_text(CLIPBOARD_LIMIT)
        );
        self.notify(text, true, fx);
    }
}

/// Why pasting a row's copy may not give the row back, or `None`: NULL and the
/// empty string both come out as an empty field, and a tab or a line break
/// inside a cell is read as a separator by whatever it is pasted into.
fn lossy_row(set: &ResultSet, row: &[Cell]) -> Option<String> {
    let cells = || (0..set.columns.len()).filter_map(|c| row.get(c));
    let mut why = Vec::new();
    if cells().any(|c| matches!(c, Cell::Null)) && cells().any(|c| matches!(c, Cell::Value(v) if v.is_empty())) {
        why.push("NULL and an empty string both paste as an empty field");
    }
    if cells().any(|c| matches!(c, Cell::Value(v) if v.contains('\t'))) {
        why.push("a tab inside a cell splits it when pasted into a spreadsheet");
    }
    if cells().any(|c| matches!(c, Cell::Value(v) if v.contains(['\n', '\r']))) {
        why.push("a line break inside a cell splits it when pasted into a spreadsheet");
    }
    (!why.is_empty()).then(|| why.join("; "))
}

/// `812 B`, `1.2 kB`, `3.4 MB`.
pub fn size_text(bytes: usize) -> String {
    match bytes {
        0..=999 => format!("{bytes} B"),
        1_000..=999_999 => format!("{:.1} kB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}
