//! Drawing. Every colour is a palette role; every width counts one column per
//! character, which holds for everything drawn so far (ASCII and box glyphs).

mod form;
mod panel;

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
    let hits = HitMap::compute(frame.area(), app.ui.focus, app.ui.zoomed);
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
    // The overlay comes last, over everything else dimmed.
    let drawn = match &mut app.ui.overlay {
        Some(overlay) => {
            panel::dim(buf, buf.area);
            draw_overlay(overlay, &p, buf)
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

fn draw_overlay(overlay: &mut Overlay, p: &Painter, buf: &mut Buffer) -> Drawn {
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
        Overlay::Form(form) => form::draw(form, buttons, p, buf),
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
        let total = UnicodeWidthStr::width(text.as_str());
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
fn width_of(s: &str) -> u16 {
    UnicodeWidthStr::width(display_safe(s).as_str()) as u16
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

/// Marks a primary key column.
pub const KEY_MARK: &str = "◆";
/// Marks a column that may be NULL.
pub const NULLABLE_MARK: &str = "?";

fn draw_editor(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.editor;
    if !r.is_empty() {
        // The title names where a statement would run: the connection and
        // database the sidebar's cursor is in.
        let mut title = "editor".to_string();
        if let (Some(active), Loadable::Ready(list)) = (&app.model.active, &app.model.connections) {
            if let Some(c) = list.iter().find(|c| c.id == active.connection) {
                title.push_str(&format!(" · {}", display_safe(&c.name)));
                if let Some(db) = &active.database {
                    title.push_str(&format!(" / {}", display_safe(db)));
                }
            }
        }
        p.within(r).title(buf, r.x + 1, r.y, &title, app.ui.focus == PaneId::Editor);
    }
}

/// The results title sits on the divider: `├─ results ────` in `surface_dim`.
fn draw_results(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
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
}

/// The hints for the mode bar in normal mode: only keys that work now.
fn normal_hints(app: &App) -> Vec<Hint> {
    let keys = &app.ui.keys;
    let one = |action: Action, desc: &'static str| keys.key_for(action).map(|k| Hint { keys: vec![(k, action)], desc });
    let mut hints = Vec::new();
    if app.ui.focus != PaneId::Sidebar || !matches!(&app.model.connections, Loadable::Ready(_)) {
        return vec![Hint { keys: vec![(keys.prefix.to_string(), Action::OpenPrefix)], desc: "prefix" }];
    }
    // Most relevant first, because a hint that does not fit is dropped and
    // the ones after it with it: what the row under the cursor can do, then
    // what is always possible.
    let Some(row) = app.cursor_row() else {
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

    let hints = match app.ui.mode {
        // An overlay takes all input and shows its own keys.
        _ if app.ui.overlay.is_some() => Vec::new(),
        Mode::Normal => normal_hints(app),
        Mode::Prefix { .. } => {
            x = p.put(buf, x + 1, r.y, " PREFIX ", label(Role::Yellow));
            let mut h = app.ui.keys.prefix_hints();
            h.push(Hint { keys: vec![("esc".to_string(), Action::OpenPrefix)], desc: "cancel" });
            h
        }
    };
    let mut hit_rects = Vec::new();
    for hint in hints {
        let start = x + 2;
        // A hint that does not fit whole is left out, not cut in half.
        let keys_w: u16 = hint.keys.iter().map(|(k, _)| width_of(k)).sum::<u16>() + hint.keys.len().saturating_sub(1) as u16;
        if start + keys_w + 1 + width_of(hint.desc) > r.right() {
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
