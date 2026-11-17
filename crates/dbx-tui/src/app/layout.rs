//! Where everything goes, and the hit map built from it.
//!
//! `view` recomputes this on every frame and stores it in `Ui`, so mouse
//! handling and tests always use the rectangles that were actually drawn.

use ratatui::layout::Rect;

use super::PaneId;

/// Every clickable or measurable rectangle of the last frame. A rectangle that
/// is not on screen (a hidden pane when another is zoomed) is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HitMap {
    pub area: Rect,
    /// The sidebar's filled column, without its separator.
    pub sidebar: Rect,
    /// The one-column `│` between the sidebar and the rest.
    pub sidebar_separator: Rect,
    pub editor: Rect,
    /// The results pane, starting at the divider row that carries its title.
    pub results: Rect,
    pub mode_bar: Rect,
}

const SIDEBAR_WIDTH: u16 = 26;
const WIDE: u16 = 60;

impl HitMap {
    pub fn compute(area: Rect, focus: PaneId, zoomed: bool) -> HitMap {
        let mut map = HitMap { area, ..HitMap::default() };
        if area.is_empty() {
            return map;
        }
        map.mode_bar = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let main = Rect::new(area.x, area.y, area.width, area.height - 1);
        if zoomed {
            match focus {
                PaneId::Sidebar => map.sidebar = main,
                PaneId::Editor => map.editor = main,
                PaneId::Results => map.results = main,
            }
            return map;
        }

        // Sidebar column plus its separator; none if there is no room for both.
        let total = if main.width >= WIDE { SIDEBAR_WIDTH } else { main.width / 3 };
        let total = if total >= 2 { total } else { 0 };
        let right_x = main.x + total;
        if total > 0 {
            map.sidebar = Rect::new(main.x, main.y, total - 1, main.height);
            map.sidebar_separator = Rect::new(main.x + total - 1, main.y, 1, main.height);
        }
        let (rw, rh) = (main.width - total, main.height);
        // The editor takes the top three fifths; results start at the divider.
        let editor_h = if rh >= 3 { (rh * 3 / 5).clamp(1, rh - 2) } else { rh };
        map.editor = Rect::new(right_x, main.y, rw, editor_h);
        map.results = Rect::new(right_x, main.y + editor_h, rw, rh - editor_h);
        map
    }

    /// The pane under a screen position.
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        let hit = |r: Rect| r.contains((x, y).into());
        [(self.sidebar, PaneId::Sidebar), (self.editor, PaneId::Editor), (self.results, PaneId::Results)]
            .into_iter()
            .find(|(r, _)| hit(*r))
            .map(|(_, p)| p)
    }
}
