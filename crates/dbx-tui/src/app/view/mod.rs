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
use super::keys::Action;
use super::overlay::{self, ErrorOverlay, FieldHit, Overlay, OverlayButton};
use super::{App, Loadable, Mode, PaneId};
use crate::core::{display_safe, Connection, DbKind};

pub fn draw(app: &mut App, frame: &mut Frame) {
    let hits = HitMap::compute(frame.area(), app.ui.focus, app.ui.zoomed);
    app.ui.hits = hits;
    let theme = app.ui.theme.clone();
    let buf = frame.buffer_mut();
    let p = Painter { theme: &theme, bounds: buf.area };

    draw_sidebar(app, &p, buf, &hits);
    draw_editor(app, &p, buf, &hits);
    draw_results(app, &p, buf, &hits);
    draw_mode_bar(app, &p, buf, &hits);
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
        Loadable::Ready(list) => {
            let room = r.bottom().saturating_sub(y) as usize;
            let sb = &mut app.ui.sidebar;
            sb.cursor = sb.cursor.min(list.len().saturating_sub(1));
            sb.scroll = scrolled(sb.scroll, sb.cursor, room);
            let (cursor, scroll) = (sb.cursor, sb.scroll);
            let shown = (list.len() - scroll).min(room) as u16;
            app.ui.hits.sidebar_list = Rect::new(r.x, y, r.width, shown).intersection(r);
            for (row, (i, c)) in (y..).zip(list.iter().enumerate().skip(scroll).take(room)) {
                draw_connection_row(&p, buf, Rect::new(r.x, row, r.width, 1), c, i == cursor);
            }
        }
    }

    if let Some(note) = &app.ui.note {
        let role = if note.failure { Role::Red } else { Role::Yellow };
        let lines = panel::wrap(&note.text, r.width.saturating_sub(2) as usize);
        let lines = &lines[..lines.len().min(4)];
        for (i, line) in lines.iter().enumerate() {
            let row = r.bottom().saturating_sub((lines.len() - i) as u16);
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

/// One connection: state glyph, name, and its kind right-aligned and muted.
/// The name gives way (ending in `…`) before the kind does.
fn draw_connection_row(p: &Painter, buf: &mut Buffer, row: Rect, c: &Connection, cursor: bool) {
    let p = p.within(row);
    if cursor {
        p.fill(buf, Style::default().bg(p.theme.color(Role::SelectionBg)));
    }
    let kind = c.kind.label();
    let kind_w = kind.chars().count() as u16;
    // " ○ " then the name, then one space, the kind and one space of margin.
    let name_x = row.x + 3;
    let kind_x = row.right().saturating_sub(kind_w + 1).max(name_x + 2);
    p.put(buf, row.x + 1, row.y, "○", p.fg(Role::Overlay0));
    p.within(Rect::new(name_x, row.y, kind_x.saturating_sub(name_x + 1), 1)).put(buf, name_x, row.y, &c.name, p.fg(Role::Text));
    p.put(buf, kind_x, row.y, kind, p.fg(Role::Overlay0));
}

fn draw_editor(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.editor;
    if !r.is_empty() {
        p.within(r).title(buf, r.x + 1, r.y, "editor", app.ui.focus == PaneId::Editor);
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

fn draw_mode_bar(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    let r = hits.mode_bar;
    if r.is_empty() {
        return;
    }
    let p = p.within(r);
    p.fill(buf, Style::default());
    let label = |role| p.bold(role).add_modifier(Modifier::REVERSED);
    let pane = format!(" {}{} ", app.ui.focus.name(), if app.ui.zoomed { " (zoom)" } else { "" });
    let mut x = p.put(buf, r.x, r.y, &pane, label(Role::Accent));

    let hints = match app.ui.mode {
        // An overlay takes all input and shows its own keys.
        _ if app.ui.overlay.is_some() => Vec::new(),
        Mode::Normal => {
            let mut h = vec![(app.ui.keys.prefix.to_string(), "prefix")];
            let has_rows = matches!(&app.model.connections, Loadable::Ready(list) if !list.is_empty());
            if app.ui.focus == PaneId::Sidebar && has_rows {
                h.push(("j/k".to_string(), "move"));
                if app.selected_connection().is_some_and(|c| c.kind == DbKind::Postgres) {
                    h.push(("e".to_string(), "edit"));
                }
                h.push(("d".to_string(), "remove"));
            }
            h
        }
        Mode::Prefix { .. } => {
            x = p.put(buf, x + 1, r.y, " PREFIX ", label(Role::Yellow));
            let mut h = app.ui.keys.hints();
            h.push(("esc".to_string(), "cancel"));
            h
        }
    };
    for (key, desc) in hints {
        x = p.put(buf, x + 2, r.y, &key, p.bold(Role::Accent));
        x = p.put(buf, x + 1, r.y, desc, p.fg(Role::Overlay0));
    }
}
