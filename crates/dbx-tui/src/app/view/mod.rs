//! Drawing. Every colour is a palette role; every width counts one column per
//! character, which holds for everything drawn so far (ASCII and box glyphs).

mod dump;
mod editor;
mod export;
mod form;
mod history;
mod navigator;
mod overlays;
mod panel;
mod popup;
mod results;
mod tab_bar;
mod viewer;
mod welcome;

pub(crate) use results::cell_text;

/// Graphemes of cell values the grid has read on this thread (work, for tests).
#[cfg(test)]
pub(crate) fn cell_graphemes_read() -> usize {
    results::CELL_GRAPHEMES.with(|n| n.get())
}

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::layout::HitMap;
use super::theme::{Palette, Role};
use super::keys::{Action, Hint};
use super::layout::HintHit;
use super::overlay::{self, ErrorOverlay, FieldHit, Overlay, OverlayButton};
use super::sidebar::MARGIN;
use super::tree::{Health, Row, RowKind};
use super::{App, Loadable, Mode, PaneId};
use crate::core::{display_safe, ColumnInfo};

/// Word wrap by display width, for text the tree draws under a node.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    panel::wrap(text, width)
}

pub fn draw(app: &mut App, frame: &mut Frame) {
    let hits = HitMap::compute(frame.area(), app.ui.focus, app.ui.zoomed, &app.ui.layout);
    app.ui.hits = hits;
    // Rows are built for the width this frame gives the sidebar.
    app.ensure_rows();
    let theme = app.ui.theme.clone();
    let buf = frame.buffer_mut();
    let p = Painter { theme: &theme, bounds: buf.area };

    draw_sidebar(app, &p, buf, &hits);
    draw_editor(app, &p, buf, &hits);
    draw_results(app, &p, buf, &hits);
    app.ui.mode_hints = draw_mode_bar(app, &p, buf, &hits);
    // The completion popup is drawn over the panes, under any overlay.
    popup::draw(app, &p, buf, &hits);
    // (The viewer says what a copy did inside its own box.)
    if !matches!(app.ui.overlay, Some(Overlay::CellViewer(_))) {
        draw_notice(app, &p, buf);
    }
    // The overlay comes last, over everything else dimmed.
    // Where the active tab runs, for the history to say where an entry ran
    // somewhere else.
    let here = match (&app.ui.overlay, app.tab().target.clone()) {
        (Some(Overlay::History(_)), Some(path)) => {
            let name = app.connection(&path.connection).map(|c| c.name.clone()).unwrap_or_else(|| path.connection.0.clone());
            Some(history::Here { path, name })
        }
        _ => None,
    };
    // The dump's overlays say what the model holds.
    let dump_view = match &app.ui.overlay {
        Some(Overlay::DumpProgress(_)) => Some(app.dump_progress_lines()),
        Some(Overlay::ConfirmQuitDump) => Some(("Quit while dumping?".to_string(), vec![app.quit_dump_message()], true)),
        _ => None,
    };
    let drawn = match &mut app.ui.overlay {
        Some(overlay @ (Overlay::DumpProgress(_) | Overlay::ConfirmQuitDump)) => {
            panel::dim(buf, buf.area);
            let (title, lines, red) = dump_view.unwrap_or_default();
            dump::draw_progress(&title, &lines, red, matches!(overlay, Overlay::ConfirmQuitDump), overlay.buttons(), &p, buf)
        }
        Some(overlay) => {
            panel::dim(buf, buf.area);
            // The close-tab question says what is true now, not when it opened.
            let running = match overlay {
                Overlay::ConfirmCloseTab(ask) => app.model.tab(ask.tab).is_some_and(|t| t.run.in_flight().is_some()),
                _ => false,
            };
            draw_overlay(overlay, running, &app.ui.keys, app.ui.caps, app.ui.notice.as_ref(), here.as_ref(), &p, buf)
        }
        None => Drawn::default(),
    };
    app.ui.overlay_buttons = drawn.buttons;
    app.ui.overlay_fields = drawn.fields;
}

/// What an overlay drew that can be clicked.
#[derive(Default)]
pub(super) struct Drawn {
    pub buttons: Vec<OverlayButton>,
    pub fields: Vec<FieldHit>,
}

#[allow(clippy::too_many_arguments)]
fn draw_overlay(
    overlay: &mut Overlay,
    closing_tab_running: bool,
    keys: &super::keys::Keybinds,
    caps: super::Capabilities,
    notice: Option<&super::query::Notice>,
    here: Option<&history::Here>,
    p: &Painter,
    buf: &mut Buffer,
) -> Drawn {
    let buttons = overlay.buttons();
    match overlay {
        Overlay::Error(e) => {
            let buttons = message_box(p, buf, &e.title, &e.message, buttons, Role::Accent, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::Failure(f) => {
            let buttons = message_box(p, buf, &f.title, &f.message, buttons, Role::Red, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::ConfirmRemove(ask) => {
            let message = format!("Remove \"{}\"? Its saved password goes with it. This cannot be undone.", ask.name);
            let buttons = message_box(p, buf, "Remove connection", &message, buttons, Role::Red, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::ConfirmDropSession(ask) if ask.stopped => {
            let buttons = message_box(p, buf, "Statement stopped", "The statement has stopped. There is nothing left to stop waiting for.", buttons, Role::Accent, Role::Accent);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::ConfirmDropSession(ask) => {
            let message = format!(
                "The statement has not stopped yet. This tab can stop waiting and run again at once, in a new session \
                 on {}. The current session is closed once dbx lets go of the statement: its SET values and temporary \
                 tables go with it, and an open transaction is rolled back. The statement itself may keep running: a \
                 write outside a transaction is committed when it ends, and until then it may hold locks that make this \
                 tab's statements wait.",
                ask.target
            );
            let buttons = message_box(p, buf, "Close this tab's session?", &message, buttons, Role::Red, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::ConfirmRerun(ask) => {
            let message = format!(
                "Load more runs the statement again: {}. It calls functions that may change data, and dbx cannot \
                 tell that it is safe to repeat. Run it again?",
                ask.statement
            );
            let buttons = message_box(p, buf, "Run the statement again?", &message, buttons, Role::Red, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::ConfirmCloseTab(ask) => {
            let mut message = format!("Close {}?", ask.name);
            if closing_tab_running {
                message.push_str(" The statement running in it will be cancelled, and its session closed.");
            } else if ask.running {
                message.push_str(" The statement that was running in it has ended.");
            }
            if ask.text {
                message.push_str(" Its text will be lost.");
            }
            let buttons = message_box(p, buf, "Close tab", &message, buttons, Role::Red, Role::Red);
            Drawn { buttons, fields: Vec::new() }
        }
        Overlay::Form(form) => form::draw(form, buttons, p, buf),
        Overlay::CellViewer(v) => viewer::draw(v, keys, notice, p, buf),
        Overlay::Chooser(c) => overlays::draw_chooser(c, p, buf),
        Overlay::ConfirmRun(c) => overlays::draw_confirm_run(c, p, buf),
        Overlay::Help(h) => overlays::draw_help(h, keys, caps, p, buf),
        Overlay::History(h) => history::draw(h, keys, here, p, buf),
        Overlay::Navigator(n) => navigator::draw(n, p, buf),
        Overlay::Export(o) => export::draw(o, buttons, keys.key_for(Action::LoadMore).as_deref(), p, buf),
        Overlay::CopyAs(c) => export::draw_copy_as(c, buttons, p, buf),
        Overlay::Dump(o) => dump::draw(o, buttons, p, buf),
        // Drawn by `draw`, which has the model.
        Overlay::DumpProgress(_) | Overlay::ConfirmQuitDump => Drawn::default(),
    }
}

/// A box with a title, a wrapped message and buttons along the bottom.
/// `border` and `title_role` say what kind of question it is (design section
/// 12: `red` for anything destructive).
fn message_box(
    p: &Painter,
    buf: &mut Buffer,
    title: &str,
    message: &str,
    buttons: &[overlay::ButtonSpec],
    border: Role,
    title_role: Role,
) -> Vec<OverlayButton> {
    let width = 64.min(buf.area.width.saturating_sub(4)).max(24);
    let text_rows = panel::wrap(message, width.saturating_sub(4) as usize).len() as u16;
    // Border, message, a blank row, the buttons, border.
    let rect = panel::centred(buf.area, width, text_rows + 4);
    let inner = panel::panel(p, buf, rect, title, border, title_role);
    if inner.is_empty() {
        return Vec::new();
    }
    let text_area = Rect { height: inner.height.saturating_sub(2), ..inner };
    panel::paragraph(p, buf, text_area, message, Role::Text);
    panel::buttons(p, buf, inner, inner.bottom() - 1, buttons)
}

/// Draws within `bounds`: the frame at first, then one pane. Nothing written
/// through it can land outside.
#[derive(Clone, Copy)]
struct Painter<'a> {
    theme: &'a Palette,
    bounds: Rect,
}

impl<'a> Painter<'a> {
    fn within(&self, rect: Rect) -> Painter<'a> {
        Painter { theme: self.theme, bounds: self.bounds.intersection(rect) }
    }

    fn fg(&self, role: Role) -> Style {
        Style::default().fg(self.theme.color(role))
    }

    fn bold(&self, role: Role) -> Style {
        self.fg(role).add_modifier(Modifier::BOLD)
    }

    fn fill(&self, buf: &mut Buffer, style: Style) {
        for y in self.bounds.top()..self.bounds.bottom() {
            for x in self.bounds.left()..self.bounds.right() {
                buf[(x, y)].set_symbol(" ").set_style(style);
            }
        }
    }

    /// Writes `text` from (x, y) up to the right edge of the bounds, one
    /// grapheme at a time by display width (a wide character takes two cells
    /// and is never drawn half). Control characters never reach a cell: they
    /// are replaced (`display_safe`), because the backend would print them
    /// raw and a terminal acts on escape sequences. Text that does not fit
    /// ends in `…`. Returns the column after the last one written.
    fn put(&self, buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) -> u16 {
        if y < self.bounds.top() || y >= self.bounds.bottom() || x < self.bounds.left() {
            return x;
        }
        let room = self.bounds.right().saturating_sub(x) as usize;
        let text = display_safe(text);
        let total = drawn_width(&text);
        let limit = if total > room { room.saturating_sub(1) } else { room };
        let (mut cx, mut used) = (x, 0usize);
        for g in text.graphemes(true) {
            let w = UnicodeWidthStr::width(g);
            if w == 0 {
                continue;
            }
            if used + w > limit {
                break;
            }
            buf[(cx, y)].set_symbol(g).set_style(style);
            for extra in 1..w as u16 {
                // The cells a wide character covers hold nothing of their own.
                buf[(cx + extra, y)].set_symbol("").set_style(style);
            }
            cx += w as u16;
            used += w;
        }
        if total > room && room > 0 {
            buf[(cx, y)].set_char('…').set_style(style);
            cx += 1;
        }
        cx
    }

    /// One grapheme (`g`, already `display_safe`, `width` cells wide) at
    /// (x, y), in `style`; the cells a wide one covers hold nothing of their
    /// own. Nothing outside the bounds is touched, and a wide grapheme that
    /// would not fit is not drawn half.
    fn glyph(&self, buf: &mut Buffer, x: u16, y: u16, g: &str, width: usize, style: Style) {
        let b = self.bounds;
        if y < b.top() || y >= b.bottom() || x < b.left() || x as usize + width > b.right() as usize || width == 0 {
            return;
        }
        buf[(x, y)].set_symbol(g).set_style(style);
        for extra in 1..width as u16 {
            buf[(x + extra, y)].set_symbol("").set_style(style);
        }
    }

    /// A pane title on its top edge: bold accent when focused, overlay0 if not.
    fn title(&self, buf: &mut Buffer, x: u16, y: u16, text: &str, focused: bool) {
        let style = self.bold(if focused { Role::Accent } else { Role::Overlay0 });
        self.put(buf, x, y, text, style);
    }
}

fn draw_sidebar(app: &mut App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.sidebar;
    if r.is_empty() {
        return;
    }
    let frame = p;
    let p = p.within(r);
    p.fill(buf, Style::default().bg(p.theme.color(Role::SidebarBg)));
    p.title(buf, r.x + 1, r.y, "connections", app.ui.focus == PaneId::Sidebar);

    let (x, y) = (r.x + 1, r.y + 2);
    // The note at the foot takes its rows from the list, so no row is ever
    // under it (the cursor cannot hide there, and a click there hits nothing).
    let note_lines: Vec<String> = match &app.ui.note {
        Some(note) => {
            let mut lines = panel::wrap(&note.text, r.width.saturating_sub(2) as usize);
            lines.truncate(4);
            lines
        }
        None => Vec::new(),
    };
    let note = p.fg(Role::Subtext0);
    match &app.model.connections {
        Loadable::Loading => {
            p.put(buf, x, y, "loading…", p.fg(Role::Overlay0));
        }
        Loadable::Failed(e) => {
            // A store or key failure is explained in the overlay; here it
            // only needs naming. Anything else is explained here.
            let brief = ErrorOverlay::for_startup(e).map(|o| format!("{}.", o.title));
            p.put(buf, x, y, brief.as_deref().unwrap_or(&e.message), p.fg(Role::Red));
        }
        Loadable::Ready(list) if list.is_empty() => {
            p.put(buf, x, y, "No connections yet.", note);
            // The key comes from the live bindings: only keys that work are shown.
            if let Some(key) = app.ui.keys.key_for(Action::NewConnection) {
                let hint = format!("Add one with {} {key}", app.ui.keys.prefix);
                p.put(buf, x, y + 1, &hint, p.fg(Role::Overlay0));
            }
        }
        Loadable::Ready(_) => {
            let room = r.bottom().saturating_sub(y).saturating_sub(note_lines.len() as u16) as usize;
            let total = app.ui.sidebar.rows.len();
            let sb = &mut app.ui.sidebar;
            sb.cursor = sb.cursor.min(total.saturating_sub(1));
            sb.scroll = scrolled(sb.scroll.min(total.saturating_sub(room)), sb.cursor, room);
            let (cursor, scroll) = (sb.cursor, sb.scroll);
            let shown = (total - scroll).min(room) as u16;
            app.ui.hits.sidebar_list = Rect::new(r.x, y, r.width, shown).intersection(r);
            // A scrollbar takes the last column when the list does not fit.
            let overflow = total > room && r.width > 4;
            let width = if overflow { r.width - 1 } else { r.width };
            for (row_y, (i, row)) in (y..).zip(app.ui.sidebar.rows.iter().enumerate().skip(scroll).take(room)) {
                let rect = Rect::new(r.x, row_y, width, 1);
                draw_row(&p, buf, rect, row, i == cursor, &app.model.schema);
            }
            if overflow {
                let track = Rect::new(r.right() - 1, y, 1, room as u16);
                app.ui.hits.sidebar_bar = track;
                draw_scrollbar(&p, buf, track, total, scroll);
            }
        }
    }

    if let Some(note) = &app.ui.note {
        let role = if note.failure { Role::Red } else { Role::Yellow };
        for (i, line) in note_lines.iter().enumerate() {
            let row = r.bottom().saturating_sub((note_lines.len() - i) as u16);
            p.put(buf, x, row, line, p.fg(role));
        }
    }

    let sep = hits.sidebar_separator;
    let ps = frame.within(sep);
    for y in sep.top()..sep.bottom() {
        ps.put(buf, sep.x, y, "│", ps.fg(Role::SurfaceDim));
    }
}

/// The first visible row of a list scrolled as little as possible from
/// `scroll` to keep `cursor` inside a window of `room` rows.
fn scrolled(scroll: usize, cursor: usize, room: usize) -> usize {
    if room == 0 || cursor < scroll {
        cursor
    } else if cursor >= scroll + room {
        cursor + 1 - room
    } else {
        scroll
    }
}

/// The thumb of a scrollbar over a track of `room` rows, for a list of `total`
/// rows scrolled to `scroll`: where it starts and how long it is (herdr's
/// `scrollbar_thumb`, from the top).
pub(super) fn scrollbar_thumb(total: usize, room: usize, scroll: usize) -> Option<(usize, usize)> {
    if room == 0 || total <= room {
        return None;
    }
    let len = ((room * room) as f32 / total as f32).round().clamp(1.0, room as f32) as usize;
    let max_top = room - len;
    let max_scroll = total - room;
    let top = ((scroll.min(max_scroll) * max_top) as f32 / max_scroll as f32).round() as usize;
    Some((top.min(max_top), len))
}

/// A thin `▕` scrollbar: the track in `surface_dim`, the thumb in `overlay0`.
fn draw_scrollbar(p: &Painter, buf: &mut Buffer, track: Rect, total: usize, scroll: usize) {
    let Some((top, len)) = scrollbar_thumb(total, track.height as usize, scroll) else { return };
    let p = p.within(track);
    for i in 0..track.height as usize {
        let role = if (top..top + len).contains(&i) { Role::Overlay0 } else { Role::SurfaceDim };
        p.put(buf, track.x, track.y + i as u16, "▕", p.fg(role));
    }
}

/// The text `s` takes on screen, as drawn (control characters replaced).
pub(crate) fn width_of(s: &str) -> u16 {
    drawn_width(&display_safe(s)).min(u16::MAX as usize) as u16
}

/// The cells `safe` (already `display_safe`) takes as `Painter::put` draws it:
/// the sum of its graphemes' widths, zero-width ones skipped. The width of the
/// whole string can differ (a lam-alef pair counts one cell as a string and
/// two as the graphemes it is drawn as), so every measure of text that is
/// then drawn goes through this one function.
pub(crate) fn drawn_width(safe: &str) -> usize {
    safe.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// Writes `right` against the right edge of `row` (one column of margin) and
/// returns the column it starts at, never further left than `min_x`.
fn put_right(p: &Painter, buf: &mut Buffer, row: Rect, min_x: u16, right: &str, style: Style) -> u16 {
    let x = row.right().saturating_sub(width_of(right) + 1).max(min_x);
    p.put(buf, x, row.y, right, style);
    x
}

/// One row of the tree: arrow, then by kind the state glyph and name (a
/// connection), the name and its kind (a view), the name and type (a column),
/// or a line of text under a node. The name gives way, ending in `…`, before
/// what is right-aligned does.
fn draw_row(p: &Painter, buf: &mut Buffer, row: Rect, r: &Row, cursor: bool, cache: &super::tree::SchemaCache) {
    let p = p.within(row);
    if cursor {
        p.fill(buf, Style::default().bg(p.theme.color(Role::SelectionBg)));
    }
    let base = row.x + MARGIN + 2 * r.depth;
    let text = p.fg(Role::Text);
    let muted = p.fg(Role::Overlay0);
    if r.is_node() {
        // Work in progress shows on the arrow (and, for a connection, its glyph).
        let arrow = if cache.state(&r.node).is_working() { p.fg(Role::Yellow) } else { muted };
        p.put(buf, base, row.y, if r.expanded { "▾" } else { "▸" }, arrow);
    }
    let label_x = base + 2;
    // The name's room ends one column before whatever is on the right.
    fn name(p: &Painter, buf: &mut Buffer, y: u16, x: u16, end: u16, label: &str, style: Style) {
        p.within(Rect::new(x, y, end.saturating_sub(x), 1)).put(buf, x, y, label, style);
    }
    match &r.kind {
        RowKind::Connection { name: label, kind, .. } => {
            // A call in flight wins over what the last one said: a retry after a
            // failure is work in progress, not a failure.
            let (glyph, role) = match cache.health(r.node.connection()) {
                _ if cache.state(&r.node).is_working() => ("●", Role::Yellow),
                Some(Health::Ok) => ("●", Role::Green),
                Some(Health::Failed) => ("●", Role::Red),
                None => ("○", Role::Overlay0),
            };
            p.put(buf, label_x, row.y, glyph, p.fg(role));
            let kind_x = put_right(&p, buf, row, label_x + 4, kind, muted);
            name(&p, buf, row.y, label_x + 2, kind_x.saturating_sub(1), label, text);
        }
        RowKind::Database(label) | RowKind::Schema(label) => {
            name(&p, buf, row.y, label_x, row.right(), label, text);
        }
        RowKind::Table { name: label, kind } => match kind.label() {
            Some(tag) => {
                let tag_x = put_right(&p, buf, row, label_x + 2, tag, muted);
                name(&p, buf, row.y, label_x, tag_x.saturating_sub(1), label, text);
            }
            None => name(&p, buf, row.y, label_x, row.right(), label, text),
        },
        RowKind::Column(c) => draw_column(&p, buf, row, base, label_x, c),
        RowKind::Note { text: line, error } => {
            let style = if *error { p.fg(Role::Red) } else { muted };
            name(&p, buf, row.y, label_x, row.right(), line, style);
        }
    }
}

/// A column: a key marker where the arrow would be for a primary key, the
/// name, and on the right its type and a `?` if it may be NULL. When both do
/// not fit, the name keeps up to two fifths of the room and the type, cut with
/// `…`, the rest: a long type (`timestamp with time zone`) must not leave a
/// column nameless.
fn draw_column(p: &Painter, buf: &mut Buffer, row: Rect, base: u16, label_x: u16, c: &ColumnInfo) {
    if c.primary_key {
        p.put(buf, base, row.y, KEY_MARK, p.fg(Role::Yellow));
    }
    let muted = p.fg(Role::Overlay0);
    // The nullable slot is always two columns wide, so types line up.
    let null = if c.nullable { NULLABLE_MARK } else { " " };
    let right = format!("{} {}", display_safe(&c.db_type), null);
    let end = row.right().saturating_sub(1);
    let room = end.saturating_sub(label_x);
    let (name_w, right_w) = (width_of(&c.name), width_of(&right));
    let name_least = name_w.min((room * 2 / 5).max(4)).min(room.saturating_sub(2));
    let (name_room, type_x) = if name_w + 1 + right_w <= room {
        (name_w, end - right_w)
    } else if name_least + 1 + right_w <= room {
        // The type fits whole, at the right edge; the name gives way.
        (room - right_w - 1, end - right_w)
    } else {
        (name_least, label_x + name_least + 1)
    };
    p.within(Rect::new(label_x, row.y, name_room, 1)).put(buf, label_x, row.y, &c.name, p.fg(Role::Text));
    p.within(Rect::new(type_x, row.y, end.saturating_sub(type_x), 1)).put(buf, type_x, row.y, &right, muted);
}

/// A NULL cell: dimmed, and not text a cell could hold by accident.
pub const NULL_MARK: &str = "∅";
/// A line break inside a cell.
pub const NEWLINE_MARK: &str = "⏎";
/// A tab inside a cell.
pub const TAB_MARK: &str = "⇥";
/// A cell the row does not have (it is shorter than the header).
pub const MISSING_MARK: &str = "⊘";

/// Marks a primary key column.
pub const KEY_MARK: &str = "◆";
/// Marks a column that may be NULL.
pub const NULLABLE_MARK: &str = "?";

fn draw_editor(app: &mut App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.editor;
    // No tab is clickable where the bar is not drawn (another pane zoomed).
    app.ui.tab_hits.clear();
    if r.is_empty() {
        return;
    }
    let focused = app.ui.focus == PaneId::Editor;
    // The tab bar is the editor's title: each tab names where its statements
    // run, the active one first among them.
    let bar = Rect::new(r.x + 1, r.y, r.width.saturating_sub(2), 1).intersection(p.bounds);
    app.ui.tab_hits = tab_bar::draw(app, p, buf, bar, focused);
    let area = Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(1), r.height.saturating_sub(1)).intersection(p.bounds);
    if area.is_empty() {
        return;
    }
    let placeholder = editor_placeholder(app);
    let (kind, dialect) = (app.active_kind(), app.active_dialect());
    // The find or go-to-line bar takes the editor's last row.
    app.refresh_bar();
    let (area, bar_row) = match (&app.ui.tab.bar, area.height >= 3) {
        (Some(_), true) => (Rect { height: area.height - 1, ..area }, Some(Rect { y: area.bottom() - 1, height: 1, ..area })),
        _ => (area, None),
    };
    editor::draw(app, p, buf, area, focused, kind.as_ref(), dialect, &placeholder);
    if let Some(row) = bar_row {
        editor::draw_bar(app, p, buf, row, focused);
    }
}

/// What the empty editor says: what to do next, with the keys that do it.
fn editor_placeholder(app: &App) -> String {
    let keys = &app.ui.keys;
    if app.tab().target.is_none() {
        let use_key = keys.key_for(Action::UseTarget).unwrap_or_default();
        return format!("No database yet: in the sidebar, press {use_key} on one. Then type SQL here.");
    }
    match keys.key_for(Action::RunDirect) {
        Some(run) => format!("Type SQL, then {run} runs the statement under the cursor."),
        None => "Type SQL.".to_string(),
    }
}

/// The results title sits on the divider: `├─ results ────` in `surface_dim`.
fn draw_results(app: &mut App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.results;
    if r.is_empty() {
        return;
    }
    let pr = p.within(r);
    let line = pr.fg(Role::SurfaceDim);
    for x in r.left()..r.right() {
        pr.put(buf, x, r.y, "─", line);
    }
    let sep = hits.sidebar_separator;
    if !sep.is_empty() {
        p.within(sep).put(buf, sep.x, r.y, "├", line);
    }
    pr.title(buf, r.x + 1, r.y, " results ", app.ui.focus == PaneId::Results);
    let area = Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(1), r.height.saturating_sub(1));
    results::draw(app, p, buf, area.intersection(r));
    // Where the cursor is, on the divider row, against the right edge.
    if let Some(text) = results::position(app) {
        let x = r.right().saturating_sub(width_of(&text) + 1);
        if x > r.x + 12 {
            pr.put(buf, x, r.y, &text, pr.fg(Role::Overlay0));
        }
    }
}

/// The hints for the mode bar in normal mode: only keys that work now.
fn normal_hints(app: &App) -> Vec<Hint> {
    let keys = &app.ui.keys;
    // While the completion popup is open the bar shows its keys.
    if app.completion_visible() {
        return keys.complete_hints(app.ui.tab.complete.popup.selected_item().is_some_and(|i| i.kind == super::complete_popup::ItemKind::Template));
    }
    // Filling in a template: its keys.
    if app.ui.tab.template.active() && app.ui.focus == PaneId::Editor {
        return keys.template_hints();
    }
    let one = |action: Action, desc: &'static str| keys.key_for(action).map(|k| Hint { keys: vec![(k, action)], desc });
    let mut hints = Vec::new();
    let prefix = Hint { keys: vec![(keys.prefix.to_string(), Action::OpenPrefix)], desc: "prefix" };
    // While a run is in flight, cancelling it comes first: ctrl+c where it
    // works (the editor and the results), the prefix command elsewhere.
    // While the cancel waits to land, pressing it again offers to stop waiting.
    if let Some(desc) = match app.tab().run {
        super::query::RunState::Running { .. } => Some("cancel"),
        super::query::RunState::Cancelling { .. } => Some("stop waiting"),
        _ => None,
    } {
        match app.ui.focus {
            PaneId::Editor | PaneId::Results => hints.extend(one(Action::CancelDirect, desc)),
            PaneId::Sidebar => {
                if let Some(k) = keys.key_for(Action::Cancel) {
                    hints.push(Hint { keys: vec![(format!("{} {k}", keys.prefix), Action::Cancel)], desc });
                }
            }
        }
    }
    if app.ui.focus == PaneId::Editor {
        if app.star_site().is_some() {
            hints.extend(one(Action::ExpandStar, "expand *"));
        }
        hints.extend(one(Action::RunDirect, "run"));
        hints.extend(one(Action::CompleteOpen, "complete"));
        hints.extend(one(Action::HistoryDirect, "history"));
        hints.push(prefix);
        // Only while there is a name to go to.
        if app.ui.tab.marks.unresolved_count() > 0 {
            hints.extend(one(Action::NextUnresolved, "next error"));
            hints.extend(one(Action::PrevUnresolved, "prev error"));
        }
        return hints;
    }
    if app.ui.focus == PaneId::Results {
        return results_hints(app, hints, prefix);
    }
    if app.ui.focus != PaneId::Sidebar || !matches!(&app.model.connections, Loadable::Ready(_)) {
        hints.push(prefix);
        return hints;
    }
    // Most relevant first, because a hint that does not fit is dropped and
    // the ones after it with it: what the row under the cursor can do, then
    // what is always possible.
    let Some(row) = app.cursor_row() else {
        // Nothing saved yet (the welcome is showing): the three things worth
        // doing, each clickable, before the prefix.
        if matches!(&app.model.connections, Loadable::Ready(list) if list.is_empty()) {
            for (action, desc) in [(Action::NewConnection, "new"), (Action::Help, "help"), (Action::Quit, "quit")] {
                if let Some(k) = keys.key_for(action) {
                    hints.push(Hint { keys: vec![(format!("{} {k}", keys.prefix), action)], desc });
                }
            }
            hints.push(prefix);
            return hints;
        }
        return general_hints(app, hints);
    };
    if row.is_node() && !row.expanded {
        hints.extend(one(Action::Expand, "expand"));
    }
    if row.is_node() && row.expanded {
        hints.extend(one(Action::Collapse, "collapse"));
    } else if row.node.parent().is_some() || !row.is_node() {
        hints.extend(one(Action::Collapse, "parent"));
    }
    if app.cursor_can_browse() {
        hints.extend(one(Action::OpenTable, "browse"));
    }
    if app.cursor_can_be_used() {
        hints.extend(one(Action::UseTarget, "use"));
    }
    if app.cursor_can_dump() {
        hints.extend(one(Action::Dump, "dump"));
    }
    if app.cursor_on_editable() {
        hints.extend(one(Action::EditConnection, "edit"));
    }
    if app.selected_connection().is_some() {
        hints.extend(one(Action::RemoveConnection, "remove"));
    }
    hints.extend(one(Action::Refresh, "refresh"));
    let mut moves = Vec::new();
    for action in [Action::MoveDown, Action::MoveUp] {
        moves.extend(keys.key_for(action).map(|k| (k, action)));
    }
    hints.push(Hint { keys: moves, desc: "move" });
    general_hints(app, hints)
}

/// The results pane's hints, from the live bindings: what the cursor's cell
/// can do (view, copy, load more when the result was cut), then moving. Only
/// while the pane shows rows; otherwise the prefix alone. A hint that does not
/// fit is dropped, and the ones after it, so the most useful come first.
fn results_hints(app: &App, mut hints: Vec<Hint>, prefix: Hint) -> Vec<Hint> {
    let Some(set) = super::grid::shown(&app.tab().outcome) else {
        hints.push(prefix);
        return hints;
    };
    let keys = &app.ui.keys;
    let one = |action: Action, desc: &'static str| keys.key_for(action).map(|k| Hint { keys: vec![(k, action)], desc });
    let several = |actions: &[Action], desc: &'static str| Hint {
        keys: actions.iter().filter_map(|a| keys.key_for(*a).map(|k| (k, *a))).collect(),
        desc,
    };
    if !set.rows.is_empty() {
        hints.extend(one(Action::OpenCell, "view"));
        hints.extend(one(Action::CopyCell, "copy"));
        hints.extend(one(Action::CopyRow, "copy row"));
        // Not offered where the key could only be refused (a run in flight, a
        // write, another session, the ceiling).
        if set.truncated && app.load_more_refusal().is_none() {
            hints.extend(one(Action::LoadMore, "more"));
        }
        hints.push(several(&[Action::GridLeft, Action::GridDown, Action::GridUp, Action::GridRight], "move"));
        hints.extend(one(Action::Export, "export"));
        hints.extend(one(Action::CopyAs, "copy as"));
    }
    // A browse tab pages and counts; the keys are offered where they work.
    if app.can_page_next() {
        hints.extend(one(Action::NextPage, "next page"));
    }
    if app.can_page_prev() {
        hints.extend(one(Action::PrevPage, "prev page"));
    }
    if app.can_count() {
        hints.extend(one(Action::CountRows, "count"));
    }
    hints.push(prefix);
    if !set.rows.is_empty() {
        hints.push(several(&[Action::GridPageDown, Action::GridPageUp], "page"));
        hints.push(several(&[Action::GridFirstRow, Action::GridLastRow], "top/bottom"));
    }
    hints.retain(|h| !h.keys.is_empty());
    hints
}

/// The hints that apply wherever the cursor is, after `hints`: adding a
/// connection (a prefix command, so the hint spells out both keys) and the
/// prefix itself.
fn general_hints(app: &App, mut hints: Vec<Hint>) -> Vec<Hint> {
    let keys = &app.ui.keys;
    if let Some(k) = keys.key_for(Action::NewConnection) {
        hints.push(Hint { keys: vec![(format!("{} {k}", keys.prefix), Action::NewConnection)], desc: "new" });
    }
    hints.push(Hint { keys: vec![(keys.prefix.to_string(), Action::OpenPrefix)], desc: "prefix" });
    hints
}

/// Draws the mode bar and returns what each hint's keys, and the hint as a
/// whole, do when clicked.
fn draw_mode_bar(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) -> Vec<HintHit> {
    let r = hits.mode_bar;
    if r.is_empty() {
        return Vec::new();
    }
    let p = p.within(r);
    p.fill(buf, Style::default());
    let label = |role| p.bold(role).add_modifier(Modifier::REVERSED);
    let pane = format!(" {}{} ", app.ui.focus.name(), if app.ui.zoomed { " (zoom)" } else { "" });
    let mut x = p.put(buf, r.x, r.y, &pane, label(Role::Accent));
    // Before anything runs: this tab's statements reach production.
    if app.tab_is_production(app.ui.tab.id) {
        x = p.put(buf, x + 1, r.y, " PROD ", label(Role::Red));
    }

    let hints = match app.ui.mode {
        // An overlay takes all input and shows its own keys.
        _ if app.ui.overlay.is_some() => Vec::new(),
        Mode::Normal => {
            if app.ui.tab.template.active() && app.ui.focus == PaneId::Editor {
                x = p.put(buf, x + 1, r.y, " TEMPLATE ", label(Role::Teal));
            }
            normal_hints(app)
        }
        Mode::Resize => {
            x = p.put(buf, x + 1, r.y, " RESIZE ", label(Role::Peach));
            app.ui.keys.resize_hints()
        }
        Mode::Prefix { .. } => {
            x = p.put(buf, x + 1, r.y, " PREFIX ", label(Role::Yellow));
            let mut h = app.ui.keys.prefix_hints(app.ui.focus == PaneId::Editor);
            // Cancel is offered only while there is something to cancel.
            if app.tab().run.in_flight().is_none() {
                h.retain(|hint| hint.keys.iter().all(|(_, a)| *a != Action::Cancel));
            }
            // Going to a numbered tab is in the help, not the bar; moving
            // along the bar only with another tab to move to.
            let one_tab = app.model.tabs.len() < 2;
            h.retain(|hint| {
                hint.keys.iter().all(|(_, a)| !matches!(a, Action::GoToTab(_)) && !(one_tab && matches!(a, Action::NextTab | Action::PrevTab)))
            });
            // First, so it is the one a narrow bar keeps: backing out always works.
            h.insert(0, Hint { keys: vec![("esc".to_string(), Action::OpenPrefix)], desc: "back" });
            h
        }
    };
    // The run state sits on the right; hints stop short of it.
    let mut right_edge = r.right();
    if let Some((text, role)) = run_state(app) {
        let at = r.right().saturating_sub(width_of(&text) + 1).max(x);
        p.put(buf, at, r.y, &text, p.fg(role));
        right_edge = at.saturating_sub(1);
    }
    let mut hit_rects = Vec::new();
    // A dump's status, in every pane, left of the run state: a click shows
    // its progress (and the key that cancels it).
    if let Some((text, working)) = app.dump_status() {
        let at = right_edge.saturating_sub(width_of(&text) + 1).max(x);
        let end = p.put(buf, at, r.y, &text, p.fg(if working { Role::Teal } else { Role::Yellow }));
        hit_rects.push(HintHit { rect: Rect::new(at, r.y, end.saturating_sub(at), 1).intersection(r), action: Action::Dump });
        right_edge = at.saturating_sub(1);
    }
    for hint in hints {
        let start = x + 2;
        // A hint that does not fit whole is left out, not cut in half.
        let keys_w: u16 = hint.keys.iter().map(|(k, _)| width_of(k)).sum::<u16>() + hint.keys.len().saturating_sub(1) as u16;
        if start + keys_w + 1 + width_of(hint.desc) > right_edge {
            break;
        }
        x = start;
        for (i, (key, action)) in hint.keys.iter().enumerate() {
            if i > 0 {
                x = p.put(buf, x, r.y, "/", p.fg(Role::Overlay0));
            }
            let from = x;
            x = p.put(buf, x, r.y, key, p.bold(Role::Accent));
            // `esc` in prefix mode is not a button: it only names the key.
            if !(*action == Action::OpenPrefix && key == "esc") {
                hit_rects.push(HintHit { rect: Rect::new(from, r.y, x - from, 1).intersection(r), action: *action });
            }
        }
        x = p.put(buf, x + 1, r.y, hint.desc, p.fg(Role::Overlay0));
        // The description counts as the first key.
        if let Some((_, action)) = hint.keys.first().filter(|(k, _)| k != "esc") {
            hit_rects.push(HintHit { rect: Rect::new(start, r.y, x.saturating_sub(start), 1).intersection(r), action: *action });
        }
    }
    hit_rects
}

/// The tab's run state for the mode bar: running or cancelling with its
/// elapsed whole seconds, cancelled and after how long, then for a run that
/// gave a result the row count (`+` when the row cap cut it) and duration,
/// the failure, or the timeout.
fn run_state(app: &App) -> Option<(String, Role)> {
    use super::query::{CancelEnd, Outcome, RunState};
    let tab = app.tab();
    let secs = |started: std::time::Instant| app.ui.now.map_or(0, |now| now.saturating_duration_since(started).as_secs());
    match &tab.run {
        RunState::Running { started, .. } => {
            return Some(match timeout_phase(app, *started) {
                Some(false) => (format!("timed out: stopping… {}s", secs(*started)), Role::Yellow),
                Some(true) => (format!("timed out, not stopped yet {}s", secs(*started)), Role::Red),
                None => (format!("running {}s", secs(*started)), Role::Yellow),
            })
        }
        RunState::Cancelling { started, stuck: false, .. } => return Some((format!("cancelling… {}s", secs(*started)), Role::Yellow)),
        RunState::Cancelling { started, stuck: true, .. } => {
            return Some((format!("not stopped yet {}s", secs(*started)), Role::Red))
        }
        RunState::Cancelled { after, how, .. } => {
            return Some(match how {
                CancelEnd::Stopped => (format!("cancelled after {}", duration_text(*after)), Role::Subtext0),
                CancelEnd::FinishedFirst => ("finished before the cancel".to_string(), Role::Peach),
                CancelEnd::Failed(_) => (format!("failed after the cancel, {}", duration_text(*after)), Role::Red),
                CancelEnd::Abandoned => ("stopped waiting: not confirmed stopped".to_string(), Role::Peach),
            });
        }
        RunState::Idle => {}
    }
    match tab.outcome.as_ref()? {
        Outcome::Rows(set) if set.columns.is_empty() => {
            Some((format!("{}  {}", affected_text(set.affected), duration_text(set.elapsed)), Role::Subtext0))
        }
        Outcome::Rows(set) => {
            let plus = if set.truncated { "+" } else { "" };
            let noun = if set.rows.len() == 1 && !set.truncated { "row" } else { "rows" };
            Some((format!("{}{plus} {noun}  {}", thousands(set.rows.len()), duration_text(set.elapsed)), Role::Subtext0))
        }
        Outcome::TimedOut { elapsed, .. } => Some((format!("timed out after {}", duration_text(*elapsed)), Role::Peach)),
        Outcome::Failed { elapsed, .. } => Some((format!("failed  {}", duration_text(*elapsed)), Role::Red)),
    }
}

/// Where a run started at `started` is against its connection's query
/// timeout: `None` before it (or with no timeout), `Some(false)` once it has
/// passed and the run is being cancelled for it, `Some(true)` once the cancel
/// has not stopped it within `CANCEL_BOUND` either.
pub(super) fn timeout_phase(app: &App, started: std::time::Instant) -> Option<bool> {
    let limit = app.tab_timeout()?;
    let ran = app.ui.now?.saturating_duration_since(started);
    (ran >= limit).then_some(ran >= limit + super::query::CANCEL_BOUND)
}

/// `3 rows affected`, `1 row affected`.
pub(super) fn affected_text(n: u64) -> String {
    format!("{} {} affected", thousands(n as usize), if n == 1 { "row" } else { "rows" })
}

/// A count with thousands separators: `1,000`.
pub(crate) fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `12ms`, `<1ms`, `3.4s`.
pub(super) fn duration_text(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    match ms {
        0 => "<1ms".to_string(),
        1..=999 => format!("{ms}ms"),
        _ => format!("{:.1}s", d.as_secs_f64()),
    }
}

/// The notice: a small bordered card in the bottom right corner, above the
/// mode bar, with a coloured `●`. It takes no input and expires on its own.
/// It never covers the mode bar or the head of the results pane (the run's
/// state and what ran): when the results pane has no room below its head,
/// the card sits just above the pane instead, over the editor.
fn draw_notice(app: &App, p: &Painter, buf: &mut Buffer) {
    let Some(notice) = &app.ui.notice else { return };
    let area = buf.area;
    if area.width < 12 || area.height < 5 {
        return;
    }
    let width = (width_of(&notice.text) + 6).min(48).min(area.width - 2);
    let mut lines = panel::wrap(&notice.text, width.saturating_sub(6) as usize);
    let hits = &app.ui.hits;
    let results = hits.results;
    let floor = if hits.mode_bar.is_empty() { area.bottom() } else { hits.mode_bar.y };
    let (top_limit, bottom) = if results.is_empty() {
        (area.y, floor)
    } else {
        let head = results.y + 1 + results::head_rows(app, results.width.saturating_sub(1));
        if floor.saturating_sub(head) >= 3 {
            (head, floor)
        } else {
            (area.y, results.y)
        }
    };
    let room = bottom.saturating_sub(top_limit);
    if room < 3 {
        return;
    }
    lines.truncate((room - 2) as usize);
    let height = lines.len() as u16 + 2;
    let rect = Rect::new(area.right() - width - 1, bottom - height, width, height).intersection(area);
    let inner = panel::panel(p, buf, rect, "notice", Role::SurfaceDim, Role::Overlay0);
    if inner.is_empty() {
        return;
    }
    let bg = p.theme.color(Role::PanelBg);
    let pi = p.within(inner);
    pi.put(buf, inner.x, inner.y, "●", p.fg(if notice.failure { Role::Red } else { Role::Yellow }).bg(bg));
    for (i, line) in lines.iter().enumerate() {
        pi.put(buf, inner.x + 2, inner.y + i as u16, line, p.fg(Role::Text).bg(bg));
    }
}
