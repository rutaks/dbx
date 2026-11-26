//! Drawing the connection form: a panel of labelled rows, a status line and
//! buttons that carry their keys (design section 12, "Overlays").

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::panel;
use super::{Drawn, Painter};
use crate::app::form::{ConnectionForm, Field, FormStatus};
use crate::app::overlay::{ButtonSpec, FieldHit};
use crate::app::text_field::TextField;
use crate::app::theme::Role;

/// Columns for a row's label, including the gap after it.
const LABEL_W: u16 = 11;
/// The most rows a long message may take in the status area.
const STATUS_ROWS: usize = 4;

pub(super) fn draw(form: &mut ConnectionForm, buttons: &'static [ButtonSpec], p: &Painter, buf: &mut Buffer) -> Drawn {
    let width = 64.min(buf.area.width.saturating_sub(4)).max(24);
    let inner_w = width.saturating_sub(4) as usize;
    let status = status_lines(&form.status, inner_w.saturating_sub(2));
    let notes = form.fields().iter().filter(|f| form.error(**f).is_none() && form.note(**f).is_some()).count();
    let error_rows = (form.errors.len() + notes) as u16;
    // Border, the rows (and their errors), a blank row, the status, a blank row, the buttons, border.
    let height = 2 + form.fields().len() as u16 + error_rows + 1 + status.len().max(1) as u16 + 1 + 1;
    let rect = panel::centred(buf.area, width, height);
    let inner = panel::panel(p, buf, rect, form.title(), Role::Accent, Role::Accent);
    if inner.is_empty() {
        return Drawn::default();
    }
    let pi = p.within(inner);
    let bg = p.theme.color(Role::PanelBg);
    let input_x = inner.x + LABEL_W.min(inner.width);
    let input_w = inner.width.saturating_sub(LABEL_W);

    let mut fields = Vec::new();
    let mut y = inner.y;
    for &field in form.fields() {
        let focused = form.focus == field && !form.confirm_discard;
        let label_style = if focused { pi.bold(Role::Accent) } else { pi.fg(Role::Subtext0) }.bg(bg);
        pi.put(buf, inner.x, y, form.label(field), label_style);
        let rect = Rect::new(input_x, y, input_w, 1);
        if field == Field::Kind {
            // The types on offer, the chosen one filled; space or a click on
            // the choices (not the label, not past them) picks the next.
            let mut x = input_x;
            let mut end = input_x;
            for choice in &form.choices {
                let chosen = choice.kind == form.kind;
                let style = match (chosen, focused) {
                    (true, true) => pi.bold(Role::Accent),
                    (true, false) => pi.fg(Role::Text),
                    (false, _) => pi.fg(Role::Overlay0),
                }
                .bg(bg);
                let text = format!("{} {}", if chosen { "●" } else { "○" }, choice.kind.display_name());
                end = pi.put(buf, x, y, &text, style);
                x = end + 2;
            }
            fields.push(FieldHit { rect: Rect::new(input_x, y, end.saturating_sub(input_x), 1).intersection(inner), field });
        } else if field.is_text() {
            let placeholder = if field == Field::Password && form.editing.is_some() {
                Some("(unchanged)")
            } else if field == Field::Host && form.file {
                Some("path of a SQLite file, e.g. ~/data/app.db")
            } else {
                None
            };
            if let Some(text) = form.text_field_mut(field) {
                draw_input(&pi, buf, rect, text, focused, placeholder);
            }
            fields.push(FieldHit { rect: rect.intersection(inner), field });
        } else {
            let (on, what) = match field {
                Field::Tls => (form.tls, "require TLS (certificate not verified)"),
                _ => (form.production, "confirm before writes"),
            };
            let mark = if on { "[x]" } else { "[ ]" };
            let mark_style = if focused { pi.bold(Role::Accent) } else { pi.fg(Role::Text) }.bg(bg);
            let after = pi.put(buf, input_x, y, mark, mark_style);
            pi.put(buf, after + 1, y, what, pi.fg(Role::Overlay0).bg(bg));
            fields.push(FieldHit { rect: Rect::new(inner.x, y, inner.width, 1).intersection(inner), field });
        }
        y += 1;
        if let Some(message) = form.error(field) {
            pi.put(buf, input_x, y, message, pi.fg(Role::Red).bg(bg));
            y += 1;
        } else if let Some(note) = form.note(field) {
            pi.put(buf, input_x, y, note, pi.fg(Role::Yellow).bg(bg));
            y += 1;
        }
    }

    // The status, below a blank row.
    y += 1;
    for (i, line) in status.iter().enumerate() {
        if let Some((glyph_role, text_role)) = glyph_roles(&form.status) {
            let g = if i == 0 { "●" } else { " " };
            pi.put(buf, inner.x, y, g, pi.fg(glyph_role).bg(bg));
            pi.put(buf, inner.x + 2, y, line, pi.fg(text_role).bg(bg));
        }
        y += 1;
    }

    // Buttons on the bottom row, with the key that is not a button on the left.
    let by = inner.bottom() - 1;
    let spec_w: usize = buttons.iter().map(|b| b.key_label.chars().count() + 1 + b.label.chars().count()).sum::<usize>()
        + 3 * buttons.len().saturating_sub(1);
    let hint = "tab next field";
    if form.locked() {
        pi.put(buf, inner.x, by, "saving…", pi.fg(Role::Yellow).bg(bg));
    } else if !form.confirm_discard && inner.width as usize >= hint.len() + 2 + spec_w {
        pi.put(buf, inner.x, by, hint, pi.fg(Role::Overlay0).bg(bg));
    }
    let mut drawn = Drawn { buttons: Vec::new(), fields };

    if form.confirm_discard {
        // The question goes over the form, which is dimmed behind it.
        panel::dim(buf, rect);
        drawn.fields.clear();
        drawn.buttons = discard_question(p, buf, buttons);
    } else {
        drawn.buttons = panel::buttons(p, buf, inner, by, buttons);
    }
    drawn
}

fn discard_question(p: &Painter, buf: &mut Buffer, buttons: &[ButtonSpec]) -> Vec<crate::app::overlay::OverlayButton> {
    let message = "What you typed in this connection will be lost.";
    let width = 50.min(buf.area.width.saturating_sub(4)).max(24);
    let rows = panel::wrap(message, width.saturating_sub(4) as usize).len() as u16;
    let rect = panel::centred(buf.area, width, rows + 4);
    let inner = panel::panel(p, buf, rect, "Discard changes?", Role::Red, Role::Red);
    if inner.is_empty() {
        return Vec::new();
    }
    let text_area = Rect { height: inner.height.saturating_sub(2), ..inner };
    panel::paragraph(p, buf, text_area, message, Role::Text);
    panel::buttons(p, buf, inner, inner.bottom() - 1, buttons)
}

/// The glyph's and the text's roles for a status, `None` when there is none.
fn glyph_roles(status: &FormStatus) -> Option<(Role, Role)> {
    match status {
        FormStatus::Idle => None,
        FormStatus::Testing | FormStatus::Saving => Some((Role::Yellow, Role::Overlay0)),
        FormStatus::TestOk => Some((Role::Green, Role::Text)),
        FormStatus::TestFailed(_) | FormStatus::SaveFailed(_) => Some((Role::Red, Role::Red)),
    }
}

/// The status as wrapped lines (at most `STATUS_ROWS`, the last ending in `…`
/// if the message was longer).
fn status_lines(status: &FormStatus, width: usize) -> Vec<String> {
    let text = match status {
        FormStatus::Idle => return Vec::new(),
        FormStatus::Testing => "testing…".to_string(),
        FormStatus::Saving => "saving…".to_string(),
        FormStatus::TestOk => "Connection successful".to_string(),
        FormStatus::TestFailed(message) => message.clone(),
        FormStatus::SaveFailed(message) => format!("Not saved. {message}"),
    };
    let mut lines = panel::wrap(&text, width);
    if lines.len() > STATUS_ROWS {
        lines.truncate(STATUS_ROWS);
        if let Some(last) = lines.last_mut() {
            let keep = width.saturating_sub(1);
            *last = last.chars().take(keep).collect::<String>() + "…";
        }
    }
    lines
}

/// A text input: a filled box, the field's visible part, and the cursor as a
/// reversed cell when focused.
pub(super) fn draw_input(p: &Painter, buf: &mut Buffer, rect: Rect, field: &mut TextField, focused: bool, placeholder: Option<&str>) {
    let rect = rect.intersection(p.bounds);
    if rect.is_empty() {
        return;
    }
    let bg = p.theme.color(if focused { Role::Surface1 } else { Role::Surface0 });
    let base = Style::default().bg(bg);
    let pr = p.within(rect);
    pr.fill(buf, base);
    let window = field.window(rect.width);
    if window.text.is_empty() && !focused {
        if let Some(hint) = placeholder {
            pr.put(buf, rect.x, rect.y, hint, pr.fg(Role::Overlay0).bg(bg));
        }
    } else {
        buf.set_stringn(rect.x, rect.y, &window.text, rect.width as usize, pr.fg(Role::Text).bg(bg));
    }
    if focused {
        let x = rect.x + window.cursor_col.min(rect.width.saturating_sub(1));
        let cell = &mut buf[(x, rect.y)];
        cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
    }
}
