//! The completion popup as drawn: over the editor below the caret's word, or
//! above it when there is no room below, clipped to the terminal. It is not an
//! overlay: nothing is dimmed and it takes no input of its own beyond the keys
//! the app reads for it.
//!
//! Each row is the name (the matched characters emphasised), and on the right
//! what the item is: its kind, and for a column or a function its type. The
//! selected row is drawn in the selection style.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{draw_scrollbar, editor::cell_of, width_of, Painter};
use crate::app::complete_popup::{Item, ItemKind};
use crate::app::layout::HitMap;
use crate::app::theme::Role;
use crate::app::App;
use crate::core::display_safe;

/// The least and most columns the popup takes.
const MIN_WIDTH: u16 = 28;
const MAX_WIDTH: u16 = 72;
/// The most a type is shown at.
const DETAIL_MAX: usize = 24;

/// What an item says on the right: its type, when it has one, then its kind.
fn right_text(item: &Item) -> (String, &'static str) {
    let detail = item.detail.as_deref().map(display_safe).map(|d| clip(&d, DETAIL_MAX)).unwrap_or_default();
    (detail, item.kind.label())
}

fn clip(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for g in s.graphemes(true) {
        if UnicodeWidthStr::width(out.as_str()) + UnicodeWidthStr::width(g) + 1 > max {
            break;
        }
        out.push_str(g);
    }
    out.push('…');
    out
}

pub(super) fn draw(app: &mut App, p: &Painter, buf: &mut Buffer, hits: &HitMap) {
    app.ui.hits.popup = Rect::default();
    if !app.completion_visible() {
        return;
    }
    let Some(anchor) = app.ui.complete.anchor() else { return };
    // The frame's tokens are the whole buffer's: a string or comment the
    // cheap read of the statement round the caret missed shows up here.
    if app.caret_in_literal() {
        app.ui.complete.reset();
        return;
    }
    // The cell the word starts in; the caret's, when that has scrolled away.
    let Some((word_x, row_y)) = cell_of(app, anchor).or_else(|| cell_of(app, app.ui.editor.cursor())) else { return };

    let screen = buf.area;
    let floor = if hits.mode_bar.is_empty() { screen.bottom() } else { hits.mode_bar.y };
    let below = floor.saturating_sub(row_y + 1) as usize;
    let above = row_y.saturating_sub(screen.top()) as usize;
    let wanted = app.ui.complete.popup.rows();
    let (go_below, room) = if below >= wanted || below >= above { (true, below) } else { (false, above) };
    if room == 0 {
        return;
    }
    app.ui.complete.popup.set_cap(room);
    // From here on the user sees this list and its highlighted item.
    app.ui.complete.popup.mark_seen();
    let rows = app.ui.complete.popup.rows() as u16;

    let popup = &app.ui.complete.popup;
    let name_w = popup.items().iter().map(|i| width_of(&i.name) as usize).max().unwrap_or(0).min(MAX_WIDTH as usize);
    let right_w = popup
        .items()
        .iter()
        .map(|i| {
            let (detail, kind) = right_text(i);
            UnicodeWidthStr::width(detail.as_str()) + if detail.is_empty() { 0 } else { 2 } + kind.len()
        })
        .max()
        .unwrap_or(0);
    let bar = u16::from(popup.len() > rows as usize);
    let content = (1 + name_w + 2 + right_w + 1) as u16 + bar;
    let width = content.clamp(MIN_WIDTH, MAX_WIDTH).min(screen.width);
    // The name starts where the word does, one cell in for the margin.
    let mut x = word_x.saturating_sub(1).max(screen.left());
    if x + width > screen.right() {
        x = screen.right().saturating_sub(width);
    }
    let y = if go_below { row_y + 1 } else { row_y.saturating_sub(rows) };
    let rect = Rect { x, y, width, height: rows }.intersection(screen);
    if rect.is_empty() {
        return;
    }
    app.ui.hits.popup = rect;

    let theme = p.theme;
    let pane = p.within(rect);
    pane.fill(buf, Style::default().bg(theme.color(Role::PopupBg)).fg(theme.color(Role::Text)));
    let top = popup.top();
    let selected = popup.selected();
    let text_right = rect.right().saturating_sub(1 + bar);
    for (i, item) in popup.window().iter().enumerate() {
        let row_y = rect.y + i as u16;
        if row_y >= rect.bottom() {
            break;
        }
        let is_selected = top + i == selected;
        let base = if is_selected {
            Style::default().bg(theme.color(Role::SelectionBg)).add_modifier(theme.modifier(Role::SelectionBg))
        } else {
            Style::default().bg(theme.color(Role::PopupBg))
        };
        if is_selected {
            pane.within(Rect { y: row_y, height: 1, ..rect }).fill(buf, base.fg(theme.color(Role::Text)));
        }
        // The right-hand column first, so the name gives way to it.
        let (detail, kind) = right_text(item);
        let right_w = UnicodeWidthStr::width(detail.as_str()) + if detail.is_empty() { 0 } else { 2 } + kind.len();
        let right_x = text_right.saturating_sub(right_w as u16);
        let mut at = right_x;
        if !detail.is_empty() {
            at = pane.put(buf, at, row_y, &detail, base.fg(theme.color(Role::Overlay1)));
            at += 2;
        }
        pane.put(buf, at, row_y, kind, base.fg(theme.color(Role::Overlay0)));

        // The name, a grapheme at a time, the matched characters bold.
        let name_limit = right_x.saturating_sub(1).max(rect.x + 1);
        let plain = base.fg(theme.color(if item.kind == ItemKind::Keyword { Role::SynKeyword } else { Role::Text }));
        let marked = base.fg(theme.color(Role::Accent)).add_modifier(Modifier::BOLD);
        let (mut cx, mut ch) = (rect.x + 1, 0usize);
        for g in item.name.graphemes(true) {
            let safe = display_safe(g);
            let w = UnicodeWidthStr::width(safe.as_str());
            let chars = g.chars().count();
            if w > 0 {
                if cx + w as u16 > name_limit {
                    pane.put(buf, cx, row_y, "…", plain);
                    break;
                }
                let hit = item.matched.iter().any(|m| (ch..ch + chars).contains(m));
                pane.glyph(buf, cx, row_y, &safe, w, if hit { marked } else { plain });
                cx += w as u16;
            }
            ch += chars;
        }
    }
    if bar == 1 {
        draw_scrollbar(&pane, buf, Rect { x: rect.right() - 1, y: rect.y, width: 1, height: rect.height }, popup.len(), top);
    }
}
