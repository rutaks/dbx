//! The statement chooser and the key help, two overlays over the dimmed screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{panel, Drawn, Painter};
use crate::app::keys::{Binding, Keybinds};
use crate::app::overlay::{Chooser, HelpOverlay, Overlay, OverlayAction, OverlayButton};
use crate::app::theme::Role;
use crate::app::Capabilities;

/// "Run which?": one row per candidate, the highlighted one in the selection
/// colours, each with a one-line preview of exactly the text it runs.
pub(super) fn draw_chooser(c: &Chooser, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 80.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let rect = panel::centred(area, width, c.items.len() as u16 + 4);
    let inner = panel::panel(p, buf, rect, "Run which?", Role::Accent, Role::Accent);
    if inner.width < 8 || inner.height < 3 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pane = p.within(inner);
    let mut buttons = Vec::new();
    for (i, item) in c.items.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() - 2 {
            break;
        }
        let selected = i == c.selected;
        let base = if selected {
            Style::default().bg(p.theme.color(Role::SelectionBg)).add_modifier(p.theme.modifier(Role::SelectionBg))
        } else {
            Style::default().bg(bg)
        };
        pane.within(Rect { y, height: 1, ..inner }).fill(buf, base);
        let x = pane.put(buf, inner.x, y, item.label, base.fg(p.theme.color(Role::Accent)).add_modifier(ratatui::style::Modifier::BOLD));
        let at = inner.x + 12.max(x - inner.x + 2);
        pane.put(buf, at, y, &item.preview, base.fg(p.theme.color(Role::Text)));
        buttons.push(OverlayButton { rect: Rect { y, height: 1, ..inner }, action: OverlayAction::ChooserPick(i) });
    }
    let specs = Overlay::Chooser(c.clone()).buttons();
    buttons.extend(panel::buttons(p, buf, inner, inner.bottom() - 1, specs));
    Drawn { buttons, fields: Vec::new() }
}

/// A row of the help, as drawn.
enum Line<'a> {
    Title(&'static str),
    Row(&'static str, &'a [Binding]),
    Blank,
}

fn lines(sections: &[crate::app::keys::HelpSection]) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if i > 0 {
            out.push(Line::Blank);
        }
        out.push(Line::Title(s.title));
        for (label, bindings) in &s.rows {
            out.push(Line::Row(label, bindings));
        }
    }
    out
}

/// How wide the label column is.
const LABEL: u16 = 24;

/// Every action with all its keys, from the live bindings. A key this
/// terminal cannot deliver is in `[brackets]` and dimmed.
pub(super) fn draw_help(h: &mut HelpOverlay, keys: &Keybinds, caps: Capabilities, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 100.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let height = area.height.saturating_sub(4).max(8.min(area.height));
    let rect = panel::centred(area, width, height);
    let inner = panel::panel(p, buf, rect, "Keys", Role::Accent, Role::Accent);
    if inner.width < 12 || inner.height < 4 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pane = p.within(inner);
    let sections = keys.help();
    let all = lines(&sections);
    let rows = (inner.height - 2) as usize;
    h.scroll = h.scroll.min(all.len().saturating_sub(rows));
    for (i, line) in all.iter().skip(h.scroll).take(rows).enumerate() {
        let y = inner.y + i as u16;
        match line {
            Line::Title(t) => {
                pane.put(buf, inner.x, y, t, p.bold(Role::Accent).bg(bg));
            }
            Line::Blank => {}
            Line::Row(label, bindings) => {
                pane.put(buf, inner.x, y, label, p.fg(Role::Subtext0).bg(bg));
                let mut x = inner.x + LABEL;
                for b in bindings.iter() {
                    let missing = b.needs_protocol && !caps.keyboard_protocol;
                    let (text, style) = if missing { (format!("[{}]", b.text), p.fg(Role::Overlay0).bg(bg)) } else { (b.text.clone(), p.bold(Role::Text).bg(bg)) };
                    x = pane.put(buf, x, y, &text, style) + 2;
                }
            }
        }
    }
    // The foot: what the brackets mean, where in the list, and the keys.
    let foot = inner.bottom() - 1;
    let note = if caps.keyboard_protocol { "" } else { "[keys] need a terminal with the kitty keyboard protocol; this one has none" };
    pane.put(buf, inner.x, foot - 1, note, p.fg(Role::Overlay0).bg(bg));
    if all.len() > rows {
        let text = format!("{}-{} of {}", h.scroll + 1, (h.scroll + rows).min(all.len()), all.len());
        pane.put(buf, inner.x, foot, &text, p.fg(Role::Overlay0).bg(bg));
    }
    let buttons = panel::buttons(p, buf, inner, foot, Overlay::Help(h.clone()).buttons());
    Drawn { buttons, fields: Vec::new() }
}
