//! The tab bar on the editor's top edge: each query tab's number, target and
//! run glyph, the active one in `accent` with a `×` that closes it, the
//! others in `overlay0`. When the tabs do not fit, the active one stays and
//! `…` stands for those left out. One row; nothing wraps.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{drawn_width, Painter};
use crate::app::layout::TabHit;
use crate::app::query;
use crate::app::theme::Role;
use crate::app::App;
use crate::core::display_safe;

/// The most cells a tab's target takes; a longer one ends in `…`.
const TARGET_MOST: usize = 24;
/// Between two tabs.
const GAP: u16 = 2;
/// The state glyph.
const GLYPH: &str = "●";
const CLOSE: &str = "×";
const HIDDEN: &str = "…";

/// One tab as it will be drawn.
struct Item {
    id: crate::core::TabId,
    /// `2 local pg / pagila`, already safe to draw and cut to size.
    label: String,
    glyph: Option<Role>,
    active: bool,
}

impl Item {
    fn width(&self) -> u16 {
        let mut w = drawn_width(&self.label);
        if self.glyph.is_some() {
            w += 2;
        }
        if self.active {
            w += 2;
        }
        w as u16
    }
}

/// `text` cut to `most` cells, ending in `…` when cut.
fn cut(text: &str, most: usize) -> String {
    if drawn_width(text) <= most {
        return text.to_string();
    }
    let mut out = String::new();
    for g in unicode_segmentation::UnicodeSegmentation::graphemes(text, true) {
        if drawn_width(&out) + drawn_width(g) + 1 > most {
            break;
        }
        out.push_str(g);
    }
    out.push('…');
    out
}

/// Which tabs fit in `room` cells around the active one (`at`): the widest
/// run of neighbours, with a `…` and a gap on each side that leaves some out.
fn window(widths: &[u16], at: usize, room: u16) -> (usize, usize) {
    let n = widths.len();
    let need = |lo: usize, hi: usize| -> u32 {
        let tabs: u32 = widths[lo..=hi].iter().map(|w| u32::from(*w)).sum::<u32>() + u32::from(GAP) * (hi - lo) as u32;
        let marks = u32::from(lo > 0) * (1 + u32::from(GAP)) + u32::from(hi + 1 < n) * (1 + u32::from(GAP));
        tabs + marks
    };
    let (mut lo, mut hi) = (at, at);
    loop {
        let mut grew = false;
        if hi + 1 < n && need(lo, hi + 1) <= u32::from(room) {
            hi += 1;
            grew = true;
        }
        if lo > 0 && need(lo - 1, hi) <= u32::from(room) {
            lo -= 1;
            grew = true;
        }
        if !grew {
            return (lo, hi);
        }
    }
}

/// Draws the bar along `row` and returns where each tab (and the active
/// tab's `×`) was drawn.
pub(super) fn draw(app: &App, p: &Painter, buf: &mut Buffer, row: Rect, editor_focused: bool) -> Vec<TabHit> {
    let p = p.within(row);
    if row.is_empty() {
        return Vec::new();
    }
    let active = app.ui.tab.id;
    let mut items: Vec<Item> = app
        .model
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let target = app.target_label_of(t.id).unwrap_or_else(|| "no target".to_string());
            let target = cut(&display_safe(&target), TARGET_MOST);
            Item { id: t.id, label: format!("{} {target}", i + 1), glyph: query::glyph(t), active: t.id == active }
        })
        .collect();
    let widths: Vec<u16> = items.iter().map(Item::width).collect();
    let at = items.iter().position(|i| i.active).unwrap_or(0);
    let (lo, hi) = window(&widths, at, row.width);
    if lo == hi {
        // Alone, and maybe wider than the row: the glyph, the `×` and the
        // `…` marks keep their room and the label gives way.
        let marks = u16::from(lo > 0) * (1 + GAP) + u16::from(hi + 1 < items.len()) * (1 + GAP);
        let item = &mut items[at];
        let extras = item.width() - drawn_width(&item.label) as u16;
        let label = row.width.saturating_sub(marks + extras).max(1) as usize;
        if drawn_width(&item.label) > label {
            item.label = cut(&item.label, label);
        }
    }
    let dim = p.fg(Role::Overlay0);
    let mut hits = Vec::new();
    let mut x = row.x;
    if lo > 0 {
        x = p.put(buf, x, row.y, HIDDEN, dim) + GAP;
    }
    for (i, item) in items[lo..=hi].iter().enumerate() {
        if i > 0 {
            x += GAP;
        }
        let from = x;
        // The active tab is accent, bold while the editor has focus: the
        // bar is the editor's title, and says which pane has the keys.
        let style = match (item.active, editor_focused) {
            (true, true) => p.bold(Role::Accent),
            (true, false) => p.fg(Role::Accent),
            (false, _) => dim,
        };
        x = p.put(buf, x, row.y, &item.label, style);
        if let Some(role) = item.glyph {
            x = p.put(buf, x + 1, row.y, GLYPH, p.fg(role));
        }
        let label = Rect::new(from, row.y, x.saturating_sub(from), 1).intersection(row);
        hits.push(TabHit { rect: label, tab: item.id, close: false });
        if item.active {
            let cross = x + 1;
            x = p.put(buf, cross, row.y, CLOSE, dim);
            if x > cross {
                hits.push(TabHit { rect: Rect::new(cross, row.y, 1, 1).intersection(row), tab: item.id, close: true });
            }
        }
    }
    if hi + 1 < items.len() {
        p.put(buf, x + GAP, row.y, HIDDEN, dim);
    }
    hits.retain(|h| !h.rect.is_empty());
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_keeps_the_active_tab_and_marks_what_it_leaves_out() {
        let widths = [10u16; 12];
        for at in [0, 5, 11] {
            let (lo, hi) = window(&widths, at, 40);
            assert!(lo <= at && at <= hi, "{at}: {lo}..{hi}");
            assert!(hi - lo < 11);
        }
        assert_eq!(window(&[10, 10], 0, 40), (0, 1), "both fit");
        assert_eq!(window(&[50], 0, 40), (0, 0), "the active alone, cut");
    }

    #[test]
    fn a_long_target_is_cut_with_dots() {
        assert_eq!(cut("abc", 5), "abc");
        assert_eq!(cut("abcdefgh", 5), "abcd…");
    }
}
