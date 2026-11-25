//! Drawing the export overlay and the copy-as list (slice 19): rows of fields
//! with the keys that work along the bottom, what the format loses written out
//! before anything is written, and buttons that carry their keys.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::form::draw_input;
use super::{message_box, panel, Drawn, Painter};
use crate::app::export::{CopyAsOverlay, ExportField, ExportOverlay, Stage};
use crate::app::grid::{size_text, CLIPBOARD_LIMIT};
use crate::app::overlay::{ButtonSpec, OverlayAction, OverlayButton};
use crate::app::theme::Role;
use crate::core::ExportFormat;

/// Columns for a row's label, including the gap after it.
const LABEL_W: u16 = 9;
/// The most rows a failure message takes.
const STATUS_ROWS: usize = 4;

/// `text` wrapped to `width` after a leading mark, the other lines indented
/// under it.
fn bulleted(text: &str, width: usize) -> Vec<String> {
    panel::wrap(text, width.saturating_sub(2))
        .into_iter()
        .enumerate()
        .map(|(i, line)| if i == 0 { format!("· {line}") } else { format!("  {line}") })
        .collect()
}

pub(super) fn draw(o: &mut ExportOverlay, buttons: &'static [ButtonSpec], load_more: Option<&str>, p: &Painter, buf: &mut Buffer) -> Drawn {
    let width = 78.min(buf.area.width.saturating_sub(4)).max(24);
    let inner_w = width.saturating_sub(4) as usize;
    let rows = panel::wrap(&o.rows_line(load_more), inner_w);
    let losses: Vec<String> = o.losses().iter().flat_map(|l| bulleted(l, inner_w)).collect();
    let status: Vec<String> = match &o.stage {
        Stage::Writing(_) => vec!["writing…".to_string()],
        Stage::Failed(message) => {
            let mut lines = panel::wrap(message, inner_w.saturating_sub(2));
            lines.truncate(STATUS_ROWS);
            lines
        }
        _ => Vec::new(),
    };
    let fields = o.fields();
    // Border, the rows, a blank, what is written, a blank, the losses, a
    // blank and the status when there is one, the buttons, border.
    let height = 2 + fields.len() + 1 + rows.len() + 1 + losses.len() + if status.is_empty() { 0 } else { status.len() + 1 } + 1 + 1;
    let rect = panel::centred(buf.area, width, height as u16);
    let inner = panel::panel(p, buf, rect, "Export result", Role::Accent, Role::Accent);
    if inner.is_empty() {
        return Drawn::default();
    }
    let pi = p.within(inner);
    let bg = p.theme.color(Role::PanelBg);
    let input_x = inner.x + LABEL_W.min(inner.width);
    let input_w = inner.width.saturating_sub(LABEL_W);
    let locked = o.locked();
    let mut drawn = Drawn::default();

    let mut y = inner.y;
    for field in fields {
        let focused = o.focus == field && !locked;
        let label = if focused { pi.bold(Role::Accent) } else { pi.fg(Role::Subtext0) }.bg(bg);
        pi.put(buf, inner.x, y, field.label(), label);
        match field {
            ExportField::Format => {
                let mut x = input_x;
                for (i, format) in ExportFormat::ALL.iter().enumerate() {
                    let on = *format == o.format;
                    let style = if on && focused { pi.bold(Role::Accent) } else if on { pi.fg(Role::Text) } else { pi.fg(Role::Overlay0) }.bg(bg);
                    let from = x;
                    x = pi.put(buf, x, y, if on { "●" } else { "○" }, if on { pi.fg(Role::Accent).bg(bg) } else { style });
                    x = pi.put(buf, x + 1, y, format.label(), style);
                    drawn.buttons.push(OverlayButton { rect: Rect::new(from, y, x - from, 1).intersection(inner), action: OverlayAction::ExportFormat(i) });
                    x += 2;
                }
            }
            ExportField::Null => {
                let mut x = input_x;
                for (empty, text) in [(false, "\\N"), (true, "empty")] {
                    let on = o.null_empty == empty;
                    let style = if on && focused { pi.bold(Role::Accent) } else if on { pi.fg(Role::Text) } else { pi.fg(Role::Overlay0) }.bg(bg);
                    let from = x;
                    x = pi.put(buf, x, y, if on { "●" } else { "○" }, if on { pi.fg(Role::Accent).bg(bg) } else { style });
                    x = pi.put(buf, x + 1, y, text, style);
                    drawn.buttons.push(OverlayButton { rect: Rect::new(from, y, x - from, 1).intersection(inner), action: OverlayAction::ExportNull(empty) });
                    x += 2;
                }
            }
            text_field => {
                let rect = Rect::new(input_x, y, input_w, 1);
                if let Some(input) = o.text_mut(text_field) {
                    draw_input(&pi, buf, rect, input, focused, None);
                }
                drawn.buttons.push(OverlayButton { rect: rect.intersection(inner), action: OverlayAction::ExportFocus(text_field) });
            }
        }
        y += 1;
    }

    y += 1;
    for line in &rows {
        pi.put(buf, inner.x, y, line, pi.fg(Role::Text).bg(bg));
        y += 1;
    }
    y += 1;
    for line in &losses {
        pi.put(buf, inner.x, y, line, pi.fg(Role::Subtext0).bg(bg));
        y += 1;
    }
    if !status.is_empty() {
        y += 1;
        let (glyph, text) = if matches!(o.stage, Stage::Failed(_)) { (Role::Red, Role::Red) } else { (Role::Yellow, Role::Overlay0) };
        for (i, line) in status.iter().enumerate() {
            pi.put(buf, inner.x, y, if i == 0 { "●" } else { " " }, pi.fg(glyph).bg(bg));
            pi.put(buf, inner.x + 2, y, line, pi.fg(text).bg(bg));
            y += 1;
        }
    }

    let by = inner.bottom() - 1;
    let spec_w: usize =
        buttons.iter().map(|b| b.key_label.chars().count() + 1 + b.label.chars().count()).sum::<usize>() + 3 * buttons.len().saturating_sub(1);
    let hint = "tab next field";
    if !locked && inner.width as usize >= hint.len() + 2 + spec_w {
        pi.put(buf, inner.x, by, hint, pi.fg(Role::Overlay0).bg(bg));
    }
    if let Stage::Replace(path) = &o.stage {
        // The question goes over the overlay, which is dimmed behind it, and
        // takes the clicks.
        panel::dim(buf, rect);
        drawn.buttons.clear();
        let message = format!("{path} is there already. Replacing it puts the new file in its place, and cannot be undone.");
        drawn.buttons = message_box(p, buf, "Replace the file?", &message, buttons, Role::Red, Role::Red);
    } else {
        drawn.buttons.extend(panel::buttons(p, buf, inner, by, buttons));
    }
    drawn
}

pub(super) fn draw_copy_as(c: &CopyAsOverlay, buttons: &'static [ButtonSpec], p: &Painter, buf: &mut Buffer) -> Drawn {
    let width = 60.min(buf.area.width.saturating_sub(4)).max(24);
    let inner_w = width.saturating_sub(4) as usize;
    let mut text = format!(
        "Sends the {} held to the terminal clipboard, up to {}.",
        if c.truncated { format!("first {} rows", c.rows) } else { format!("{} {}", c.rows, if c.rows == 1 { "row" } else { "rows" }) },
        size_text(CLIPBOARD_LIMIT)
    );
    if c.truncated {
        text.push_str(" The result was cut at the row cap.");
    }
    let note = panel::wrap(&text, inner_w);
    // Border, the formats, a blank, the note, a blank, the buttons, border.
    let height = 2 + ExportFormat::COPYABLE.len() + 1 + note.len() + 1 + 1;
    let rect = panel::centred(buf.area, width, height as u16);
    let inner = panel::panel(p, buf, rect, "Copy as", Role::Accent, Role::Accent);
    if inner.is_empty() {
        return Drawn::default();
    }
    let pi = p.within(inner);
    let bg = p.theme.color(Role::PanelBg);
    let mut drawn = Drawn::default();
    let mut y = inner.y;
    for (i, format) in ExportFormat::COPYABLE.iter().enumerate() {
        let on = c.selected == i;
        let style = if on { pi.bold(Role::Accent) } else { pi.fg(Role::Text) }.bg(bg);
        pi.put(buf, inner.x, y, if on { "▸" } else { " " }, style);
        let x = pi.put(buf, inner.x + 2, y, &(i + 1).to_string(), pi.bold(Role::Accent).bg(bg));
        pi.put(buf, x + 1, y, format.label(), style);
        drawn.buttons.push(OverlayButton { rect: Rect::new(inner.x, y, inner.width, 1).intersection(inner), action: OverlayAction::CopyAsPick(i) });
        y += 1;
    }
    y += 1;
    for line in &note {
        pi.put(buf, inner.x, y, line, pi.fg(Role::Subtext0).bg(bg));
        y += 1;
    }
    drawn.buttons.extend(panel::buttons(p, buf, inner, inner.bottom() - 1, buttons));
    drawn
}
