//! Drawing the dump overlay, the dump's progress and the quit question
//! (slice 20). Names and paths go through the painter, which replaces control
//! characters.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::form::draw_input;
use super::{message_box, panel, Drawn, Painter};
use crate::app::dump::{known_losses_note, DumpField, DumpOverlay, DumpStage};
use crate::app::overlay::{ButtonSpec, OverlayAction, OverlayButton};
use crate::app::theme::Role;
use crate::core::DumpContent;

/// Columns for a row's label, including the gap after it.
const LABEL_W: u16 = 8;
/// The most rows a failure message takes.
const STATUS_ROWS: usize = 6;

/// One choice drawn as `● label` or `○ label`, clickable.
#[allow(clippy::too_many_arguments)]
fn radio(p: &Painter, buf: &mut Buffer, x: u16, y: u16, on: bool, focused: bool, label: &str, action: OverlayAction, inner: Rect, drawn: &mut Drawn) -> u16 {
    let bg = p.theme.color(Role::PanelBg);
    let style = if on && focused { p.bold(Role::Accent) } else if on { p.fg(Role::Text) } else { p.fg(Role::Overlay0) }.bg(bg);
    let mut end = p.put(buf, x, y, if on { "●" } else { "○" }, if on { p.fg(Role::Accent).bg(bg) } else { style });
    end = p.put(buf, end + 1, y, label, style);
    drawn.buttons.push(OverlayButton { rect: Rect::new(x, y, end - x, 1).intersection(inner), action });
    end + 2
}

pub(super) fn draw(o: &mut DumpOverlay, buttons: &'static [ButtonSpec], p: &Painter, buf: &mut Buffer) -> Drawn {
    let width = 78.min(buf.area.width.saturating_sub(4)).max(24);
    let inner_w = width.saturating_sub(4) as usize;
    let note = panel::wrap(
        "dbx writes CREATE statements for tables, views, functions and sequences, then the rows as INSERT statements. Nothing in the database is changed.",
        inner_w,
    );
    let losses = if o.loses { panel::wrap(&known_losses_note(), inner_w) } else { Vec::new() };
    let status: Vec<String> = match &o.stage {
        DumpStage::Sent(_) => vec!["checking the path…".to_string()],
        DumpStage::Failed(message) => {
            let mut lines = panel::wrap(message, inner_w.saturating_sub(2));
            lines.truncate(STATUS_ROWS);
            lines
        }
        _ => Vec::new(),
    };
    // Border, the rows, a blank, the note, a blank and the status when there
    // is one, a blank, the buttons, border.
    let height = 2 + DumpField::ALL.len() + 1 + note.len() + if losses.is_empty() { 0 } else { losses.len() + 1 } + if status.is_empty() { 0 } else { status.len() + 1 } + 1 + 1;
    let rect = panel::centred(buf.area, width, height as u16);
    let title = o.title();
    let inner = panel::panel(p, buf, rect, &title, Role::Accent, Role::Accent);
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
    for field in DumpField::ALL {
        let focused = o.focus == field && !locked;
        let label = if focused { pi.bold(Role::Accent) } else { pi.fg(Role::Subtext0) }.bg(bg);
        pi.put(buf, inner.x, y, field.label(), label);
        match field {
            DumpField::Content => {
                let mut x = input_x;
                for (i, content) in DumpContent::ALL.iter().enumerate() {
                    x = radio(&pi, buf, x, y, *content == o.content, focused, content.label(), OverlayAction::DumpContent(i), inner, &mut drawn);
                }
            }
            DumpField::Scope => {
                let x = radio(&pi, buf, input_x, y, o.whole, focused, "whole database", OverlayAction::DumpScope(true), inner, &mut drawn);
                if let Some(schema) = o.schema.clone() {
                    radio(&pi, buf, x, y, !o.whole, focused, &format!("schema {schema}"), OverlayAction::DumpScope(false), inner, &mut drawn);
                }
            }
            DumpField::Drop => {
                let on = o.drop_tables;
                radio(&pi, buf, input_x, y, on, focused, "drop tables (DROP TABLE IF EXISTS before each)", OverlayAction::DumpDrop(!on), inner, &mut drawn);
            }
            DumpField::Path => {
                let rect = Rect::new(input_x, y, input_w, 1);
                draw_input(&pi, buf, rect, &mut o.path, focused, None);
                drawn.buttons.push(OverlayButton { rect: rect.intersection(inner), action: OverlayAction::DumpFocus(DumpField::Path) });
            }
        }
        y += 1;
    }
    y += 1;
    for line in &note {
        pi.put(buf, inner.x, y, line, pi.fg(Role::Subtext0).bg(bg));
        y += 1;
    }
    if !losses.is_empty() {
        y += 1;
        for line in &losses {
            pi.put(buf, inner.x, y, line, pi.fg(Role::Yellow).bg(bg));
            y += 1;
        }
    }
    if !status.is_empty() {
        y += 1;
        let (glyph, text) = if matches!(o.stage, DumpStage::Failed(_)) { (Role::Red, Role::Red) } else { (Role::Yellow, Role::Overlay0) };
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
    if let DumpStage::Replace(path) = &o.stage {
        // The question goes over the overlay, dimmed behind it, and takes the clicks.
        panel::dim(buf, rect);
        let message = format!("{path} is there already. Replacing it puts the dump in its place, and cannot be undone.");
        drawn.buttons = message_box(p, buf, "Replace the file?", &message, buttons, Role::Red, Role::Red);
    } else {
        drawn.buttons.extend(panel::buttons(p, buf, inner, by, buttons));
    }
    drawn
}

/// The dump's progress (or how it ended), and the quit question: a box of
/// lines with its buttons. `question` makes it red.
pub(super) fn draw_progress(title: &str, lines: &[String], failure: bool, question: bool, buttons: &'static [ButtonSpec], p: &Painter, buf: &mut Buffer) -> Drawn {
    if question {
        let text = lines.join(" ");
        return Drawn { buttons: message_box(p, buf, title, &text, buttons, Role::Red, Role::Red), fields: Vec::new() };
    }
    let width = 72.min(buf.area.width.saturating_sub(4)).max(24);
    let inner_w = width.saturating_sub(4) as usize;
    let wrapped: Vec<String> = lines.iter().flat_map(|l| panel::wrap(l, inner_w)).collect();
    let rect = panel::centred(buf.area, width, wrapped.len() as u16 + 4);
    let inner = panel::panel(p, buf, rect, title, Role::Accent, if failure { Role::Red } else { Role::Accent });
    if inner.is_empty() {
        return Drawn::default();
    }
    let pi = p.within(inner);
    let bg = p.theme.color(Role::PanelBg);
    for (i, line) in wrapped.iter().enumerate() {
        pi.put(buf, inner.x, inner.y + i as u16, line, pi.fg(if failure { Role::Red } else { Role::Text }).bg(bg));
    }
    Drawn { buttons: panel::buttons(p, buf, inner, inner.bottom() - 1, buttons), fields: Vec::new() }
}
