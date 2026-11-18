//! The cell viewer overlay: a box over the dimmed screen with the cell's
//! column, type and row in its title, the value wrapped and scrollable, the
//! keys that work along the bottom, and buttons that carry them (design
//! section 12, "Overlays").

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{panel, scrollbar_thumb, thousands, Drawn, Painter, MISSING_MARK, NULL_MARK};
use crate::app::keys::{Action, Keybinds};
use crate::app::overlay::{OverlayAction, OverlayButton};
use crate::app::query::Notice;
use crate::app::theme::Role;
use crate::app::viewer::CellViewer;

/// The widest the box is, however wide the terminal.
const MAX_WIDTH: u16 = 100;

pub(super) fn draw(v: &mut CellViewer, keys: &Keybinds, notice: Option<&Notice>, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = MAX_WIDTH.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let height = area.height.saturating_sub(4).max(8.min(area.height));
    let rect = panel::centred(area, width, height);
    let mut title = v.column.clone();
    if !v.db_type.is_empty() {
        title = format!("{title} · {}", v.db_type);
    }
    title = format!("{title} · row {}", v.row + 1);
    if v.value.is_some() {
        let chars = v.chars;
        title = format!("{title} · {} {}, {} B", thousands(chars), if chars == 1 { "char" } else { "chars" }, thousands(v.bytes()));
    }
    if v.can_toggle() {
        title.push_str(if v.raw { " · stored text" } else { " · pretty" });
    }
    let inner = panel::panel(p, buf, rect, &title, Role::Accent, Role::Accent);
    if inner.width < 6 || inner.height < 4 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let p = p.within(inner);
    // Text, then a row for the keys (or a note), then the buttons; one column
    // on the right for the scrollbar.
    let text = Rect::new(inner.x, inner.y, inner.width - 1, inner.height - 2);
    let (note_y, button_y) = (inner.bottom() - 2, inner.bottom() - 1);
    // Two columns stay free at the end of a line for its break glyph.
    v.set_area((text.width as usize).saturating_sub(2), text.height as usize);
    v.scroll_by(0);

    let mut shown = 0;
    match &v.value {
        None if v.missing => {
            p.put(buf, text.x, text.y, &format!("{MISSING_MARK}  no cell: this row is shorter than the header"), p.fg(Role::Overlay0).bg(bg));
        }
        None => {
            p.put(buf, text.x, text.y, &format!("{NULL_MARK}  NULL: no value"), p.fg(Role::Overlay0).bg(bg));
        }
        Some(s) if s.is_empty() => {
            p.put(buf, text.x, text.y, "empty string", p.fg(Role::Overlay0).bg(bg));
        }
        Some(_) => {
            let lines = v.lines(v.scroll, text.height as usize);
            shown = lines.len();
            let pt = p.within(text);
            for (i, line) in lines.iter().enumerate() {
                let y = text.y + i as u16;
                let dim = p.fg(Role::Overlay0).bg(bg);
                let mut x = pt.put(buf, text.x, y, &line.body, p.fg(Role::Text).bg(bg));
                x = pt.put(buf, x, y, &line.trailing, dim);
                pt.put(buf, x, y, line.end, dim);
            }
        }
    }
    if let Some(total) = v.total().filter(|t| *t > text.height as usize) {
        if let Some((top, len)) = scrollbar_thumb(total, text.height as usize, v.scroll) {
            for i in top..top + len {
                p.put(buf, inner.right() - 1, text.y + i as u16, "▐", p.fg(Role::Overlay0).bg(bg));
            }
        }
    }

    // What a copy just said (the notice card would sit over the buttons), or
    // the note, or the keys that scroll.
    match v.note {
        _ if notice.is_some() => {
            let n = notice.expect("checked");
            let x = p.put(buf, inner.x, note_y, "●", p.fg(if n.failure { Role::Red } else { Role::Yellow }).bg(bg));
            p.put(buf, x + 1, note_y, &n.text, p.fg(Role::Text).bg(bg));
        }
        Some(note) => {
            p.put(buf, inner.x, note_y, note, p.fg(Role::Peach).bg(bg));
        }
        None if v.value.as_deref().is_some_and(|s| !s.is_empty()) => {
            let key = |a| keys.key_for(a).unwrap_or_default();
            let hints = [
                (format!("{}/{}", key(Action::GridDown), key(Action::GridUp)), "scroll"),
                (format!("{}/{}", key(Action::GridPageDown), key(Action::GridPageUp)), "page"),
                (format!("{}/{}", key(Action::GridFirstRow), key(Action::GridLastRow)), "ends"),
            ];
            let mut x = inner.x;
            for (k, desc) in hints {
                x = p.put(buf, x, note_y, &k, p.bold(Role::Accent).bg(bg));
                x = p.put(buf, x + 1, note_y, desc, p.fg(Role::Overlay0).bg(bg)) + 2;
            }
        }
        None => {}
    }

    // Where in the value, on the left of the buttons.
    if shown > 0 {
        let first = v.scroll + 1;
        let place = match v.total() {
            Some(total) => format!("lines {first}-{} of {total}", v.scroll + shown),
            None => format!("lines {first}-{}", v.scroll + shown),
        };
        p.put(buf, inner.x, button_y, &place, p.fg(Role::Overlay0).bg(bg));
    }

    let mut specs: Vec<(String, &str, OverlayAction)> = Vec::new();
    if v.can_toggle() {
        if let Some(k) = keys.key_for(Action::ToggleRaw) {
            specs.push((k, if v.raw { "pretty" } else { "raw" }, OverlayAction::ViewerToggleRaw));
        }
    }
    if let Some(k) = keys.key_for(Action::CopyCell) {
        specs.push((k, "copy", OverlayAction::ViewerCopy));
    }
    specs.push(("esc".to_string(), "close", OverlayAction::Dismiss));
    let total: usize = specs.iter().map(|(k, l, _)| k.chars().count() + 1 + l.chars().count()).sum::<usize>() + 3 * (specs.len() - 1);
    let mut x = inner.right().saturating_sub(total as u16).max(inner.x);
    let mut buttons = Vec::new();
    for (k, label, action) in specs {
        let start = x;
        x = p.put(buf, x, button_y, &k, p.bold(Role::Accent).bg(bg));
        x = p.put(buf, x + 1, button_y, label, p.fg(Role::Overlay0).bg(bg));
        buttons.push(OverlayButton { rect: Rect::new(start, button_y, x - start, 1).intersection(inner), action });
        x += 3;
    }
    Drawn { buttons, fields: Vec::new() }
}
