//! Drawing. Every colour is a palette role; every width counts one column per
//! character, which holds for everything drawn so far (ASCII and box glyphs).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use super::layout::HitMap;
use super::theme::{Palette, Role};
use super::{App, Loadable, Mode, PaneId};

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

    /// Writes `text` from (x, y) up to the right edge of the bounds. Text that
    /// does not fit ends in `…`. Returns the column after the last one written.
    fn put(&self, buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) -> u16 {
        if y < self.bounds.top() || y >= self.bounds.bottom() || x < self.bounds.left() {
            return x;
        }
        let room = self.bounds.right().saturating_sub(x) as usize;
        let count = text.chars().count();
        let mut cx = x;
        for (i, ch) in text.chars().enumerate() {
            if i >= room {
                break;
            }
            let ch = if count > room && i + 1 == room { '…' } else { ch };
            buf[(cx, y)].set_char(ch).set_style(style);
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

fn draw_sidebar(app: &App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
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
            p.put(buf, x, y, &e.message, p.fg(Role::Red));
        }
        Loadable::Ready(list) if list.is_empty() => {
            p.put(buf, x, y, "No connections yet.", note);
        }
        Loadable::Ready(list) => {
            let room = r.bottom().saturating_sub(y) as usize;
            for (row, c) in (y..).zip(list.iter().take(room)) {
                p.put(buf, x, row, &format!("○ {}", c.name), p.fg(Role::Text));
            }
        }
    }

    let sep = hits.sidebar_separator;
    let ps = frame.within(sep);
    for y in sep.top()..sep.bottom() {
        ps.put(buf, sep.x, y, "│", ps.fg(Role::SurfaceDim));
    }
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
        Mode::Normal => vec![(app.ui.keys.prefix.to_string(), "prefix")],
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
