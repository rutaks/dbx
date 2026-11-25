//! Building blocks for overlays (herdr's `panel`, `button`; survey section 6):
//! dim what is behind, draw a square-cornered box on `panel_bg`, wrap text
//! inside it, and lay out buttons that carry their key (`↵ retry`).
//! Look and feel: design section 12, "Overlays".

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::Painter;
use crate::core::display_safe;
use crate::app::overlay::{ButtonSpec, OverlayButton};
use crate::app::theme::Role;

/// Dims every cell of `area`, so the box drawn next stands out.
pub(super) fn dim(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

/// A box centred in `area`, as large as asked but never larger than `area`
/// less a one-cell margin.
pub(super) fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(area.width.min(4));
    let h = height.min(area.height.saturating_sub(2)).max(area.height.min(3));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Draws the box: `panel_bg` fill, a square border in `border`, and the title
/// on the top edge in bold `title_role`. Returns the area inside the border,
/// with one column of padding each side.
pub(super) fn panel(p: &Painter, buf: &mut Buffer, rect: Rect, title: &str, border: Role, title_role: Role) -> Rect {
    let p = p.within(rect);
    // Reset first: filling only patches a style, and the box must not keep
    // the dimming of what was behind it.
    for y in p.bounds.top()..p.bounds.bottom() {
        for x in p.bounds.left()..p.bounds.right() {
            buf[(x, y)].reset();
        }
    }
    let bg = p.theme.color(Role::PanelBg);
    p.fill(buf, Style::default().bg(bg));
    if rect.width < 2 || rect.height < 2 {
        return Rect::default();
    }
    let line = p.fg(border).bg(bg);
    let (l, r, t, b) = (rect.left(), rect.right() - 1, rect.top(), rect.bottom() - 1);
    for x in l + 1..r {
        p.put(buf, x, t, "─", line);
        p.put(buf, x, b, "─", line);
    }
    for y in t + 1..b {
        p.put(buf, l, y, "│", line);
        p.put(buf, r, y, "│", line);
    }
    for (x, y, g) in [(l, t, "┌"), (r, t, "┐"), (l, b, "└"), (r, b, "┘")] {
        p.put(buf, x, y, g, line);
    }
    if rect.width > 4 {
        let room = Rect::new(l + 1, t, rect.width - 3, 1);
        p.within(room).put(buf, l + 2, t, &format!(" {title} "), p.bold(title_role).bg(bg));
    }
    Rect::new(l + 2, t + 1, rect.width.saturating_sub(4), rect.height.saturating_sub(2))
}

/// Text in `role`, wrapped at word boundaries to `inner`'s width, from its top.
/// Returns the number of rows written.
pub(super) fn paragraph(p: &Painter, buf: &mut Buffer, inner: Rect, text: &str, role: Role) -> u16 {
    let p = p.within(inner);
    let style = p.fg(role).bg(p.theme.color(Role::PanelBg));
    let mut y = inner.y;
    for line in wrap(text, inner.width as usize) {
        if y >= inner.bottom() {
            break;
        }
        p.put(buf, inner.x, y, &line, style);
        y += 1;
    }
    y - inner.y
}

/// Buttons on one row, right-aligned in `inner`: the key in bold `accent`, the
/// label in `overlay0`, as the mode bar shows hints. Returns where each landed.
pub(super) fn buttons(p: &Painter, buf: &mut Buffer, inner: Rect, y: u16, specs: &[ButtonSpec]) -> Vec<OverlayButton> {
    let p = p.within(inner);
    let bg = p.theme.color(Role::PanelBg);
    let texts: Vec<(usize, &ButtonSpec)> =
        specs.iter().map(|s| (s.key_label.chars().count() + 1 + s.label.chars().count(), s)).collect();
    let total: usize = texts.iter().map(|(w, _)| w).sum::<usize>() + 3 * texts.len().saturating_sub(1);
    let mut x = inner.right().saturating_sub(total as u16).max(inner.x);
    let mut out = Vec::new();
    for (w, spec) in texts {
        let start = x;
        x = p.put(buf, x, y, spec.key_label, p.bold(Role::Accent).bg(bg));
        x = p.put(buf, x + 1, y, spec.label, p.fg(Role::Overlay0).bg(bg));
        out.push(OverlayButton { rect: Rect::new(start, y, w as u16, 1).intersection(inner), action: spec.action });
        x += 3;
    }
    out
}

/// Greedy word wrap by display width (wide characters take two columns). A
/// word longer than the width is cut; text with no spaces, as Japanese is
/// written, is cut at the width. Control characters are replaced.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if width == 0 {
        return lines;
    }
    for para in text.split('\n') {
        let para = display_safe(para);
        let (mut line, mut line_w) = (String::new(), 0usize);
        for word in para.split_whitespace() {
            let mut word = word.to_string();
            loop {
                let w = UnicodeWidthStr::width(word.as_str());
                if line_w == 0 && w <= width {
                    line = word;
                    line_w = w;
                    break;
                }
                if line_w > 0 && line_w + 1 + w <= width {
                    line.push(' ');
                    line.push_str(&word);
                    line_w += 1 + w;
                    break;
                }
                if line_w > 0 {
                    lines.push(std::mem::take(&mut line));
                    line_w = 0;
                    continue;
                }
                // A single word wider than a line: cut it at the width.
                let (head, rest) = split_at_width(&word, width);
                lines.push(head);
                word = rest;
                if word.is_empty() {
                    break;
                }
            }
        }
        lines.push(line);
    }
    lines
}

/// The first part of `word` that fits `width` columns (at least one grapheme),
/// and the rest.
pub(super) fn split_at_width(word: &str, width: usize) -> (String, String) {
    let mut used = 0;
    let mut end = 0;
    for g in word.graphemes(true) {
        let w = UnicodeWidthStr::width(g);
        if used + w > width && end > 0 {
            break;
        }
        used += w;
        end += g.len();
    }
    (word[..end].to_string(), word[end..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_only_words_that_cannot_fit() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        assert!(wrap("anything", 0).is_empty());
    }

    #[test]
    fn centred_boxes_fit_inside_any_area() {
        for w in 0..30u16 {
            for h in 0..12u16 {
                let area = Rect::new(1, 1, w, h);
                let r = centred(area, 60, 9);
                assert!(area.union(r) == area || area.is_empty(), "{r:?} outside {area:?}");
            }
        }
    }
}
