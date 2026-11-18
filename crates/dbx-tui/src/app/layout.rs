//! Where everything goes, and the hit map built from it.
//!
//! `view` recomputes this on every frame and stores it in `Ui`, so mouse
//! handling and tests always use the rectangles that were actually drawn.

use ratatui::layout::Rect;

use super::keys::Action;
use super::PaneId;

/// A hint in the mode bar as drawn: where it is, and what a click on it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintHit {
    pub rect: Rect,
    pub action: Action,
}

/// Every clickable or measurable rectangle of the last frame. A rectangle that
/// is not on screen (a hidden pane when another is zoomed) is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HitMap {
    pub area: Rect,
    /// The sidebar's filled column, without its separator.
    pub sidebar: Rect,
    /// The one-column `│` between the sidebar and the rest. Dragging it
    /// resizes the sidebar.
    pub sidebar_separator: Rect,
    /// The connection rows as drawn: the first is the list's `scroll`th entry.
    /// Empty when no list is shown. Filled while drawing the sidebar.
    pub sidebar_list: Rect,
    /// The sidebar's scrollbar column, when the list overflows. Filled while
    /// drawing the sidebar.
    pub sidebar_bar: Rect,
    pub editor: Rect,
    /// Where the editor's text is drawn (inside its title row and margin): a
    /// click there places the caret. Filled while drawing the editor.
    pub editor_text: Rect,
    /// The marker and line-number columns left of `editor_text`. Filled
    /// while drawing the editor.
    pub editor_gutter: Rect,
    /// The results pane, starting at the divider row that carries its title.
    pub results: Rect,
    /// The row between the editor and the results (the results' title row).
    /// Dragging it resizes the editor. Empty while a pane is zoomed.
    pub divider: Rect,
    pub mode_bar: Rect,
    /// The completion popup as drawn (empty when it is not). Filled while
    /// drawing; a press on it accepts the item under it.
    pub popup: Rect,
}

/// What the user set by dragging a divider or from the resize mode. `None`
/// keeps the automatic size. Stored as thousandths of the space the divider
/// moves in (the width, the height), so a resized terminal keeps the
/// proportions; `app` only holds the values, the shell keeps them in a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutPrefs {
    /// The sidebar with its separator, in thousandths of the width.
    pub sidebar: Option<u16>,
    /// The editor, in thousandths of the height above the mode bar.
    pub editor: Option<u16>,
}

/// The most a stored proportion may be.
pub const PERMILLE_MAX: u16 = 999;

/// The sidebar and its separator take about 30% of the width, between these
/// (wide enough for a column row a few levels down: `◆ film_id` and
/// `mpaa_rating ?`), so a narrow terminal keeps a usable editor.
const SIDEBAR_MIN: u16 = 26;
const SIDEBAR_MAX: u16 = 34;

/// The least the sidebar (with its separator) keeps when dragged, and the
/// least the editor and results keep beside it.
pub const SIDEBAR_LEAST: u16 = 14;
pub const RIGHT_LEAST: u16 = 24;
/// The least height of the editor (its title and two lines) and of the
/// results (the divider, the header and two rows).
pub const EDITOR_LEAST: u16 = 3;
pub const RESULTS_LEAST: u16 = 4;

/// `width * permille / 1000`, rounded.
fn of(total: u16, permille: u16) -> u16 {
    ((u32::from(total) * u32::from(permille) + 500) / 1000) as u16
}

/// The proportion that gives `size` of `total` (never 0 or 1000).
fn permille_of(size: u16, total: u16) -> u16 {
    if total == 0 {
        return 0;
    }
    ((u32::from(size) * 1000 + u32::from(total) / 2) / u32::from(total)).clamp(1, u32::from(PERMILLE_MAX)) as u16
}

impl LayoutPrefs {
    /// A value read from a file, or typed by hand: only what is a usable
    /// proportion is kept.
    pub fn sane(self) -> Self {
        let ok = |p: Option<u16>| p.filter(|p| (1..=PERMILLE_MAX).contains(p));
        LayoutPrefs { sidebar: ok(self.sidebar), editor: ok(self.editor) }
    }
}

impl HitMap {
    /// The sidebar with its separator: the automatic size, or the user's
    /// clamped so the sidebar and the rest both keep a usable width.
    pub fn sidebar_total(width: u16, prefs: &LayoutPrefs) -> u16 {
        let total = match prefs.sidebar {
            None => (width * 30 / 100).clamp(SIDEBAR_MIN, SIDEBAR_MAX).min(width / 2),
            Some(p) => {
                let hi = width.saturating_sub(RIGHT_LEAST).max(width / 2);
                of(width, p).clamp(SIDEBAR_LEAST.min(hi), hi)
            }
        };
        if total >= 2 {
            total
        } else {
            0
        }
    }

    /// The editor's height out of `height` rows: the top three fifths, or the
    /// user's, clamped so both panes keep their least size.
    pub fn editor_height(height: u16, prefs: &LayoutPrefs) -> u16 {
        if height < 3 {
            return height;
        }
        let want = match prefs.editor {
            None => height * 3 / 5,
            Some(p) => {
                let hi = height.saturating_sub(RESULTS_LEAST).max(height / 2);
                of(height, p).clamp(EDITOR_LEAST.min(hi), hi)
            }
        };
        want.clamp(1, height - 2)
    }

    pub fn compute(area: Rect, focus: PaneId, zoomed: bool, prefs: &LayoutPrefs) -> HitMap {
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
        let total = Self::sidebar_total(main.width, prefs);
        let right_x = main.x + total;
        if total > 0 {
            map.sidebar = Rect::new(main.x, main.y, total - 1, main.height);
            map.sidebar_separator = Rect::new(main.x + total - 1, main.y, 1, main.height);
        }
        let (rw, rh) = (main.width - total, main.height);
        // The editor takes the top three fifths; results start at the divider.
        let editor_h = Self::editor_height(rh, prefs);
        map.editor = Rect::new(right_x, main.y, rw, editor_h);
        map.results = Rect::new(right_x, main.y + editor_h, rw, rh - editor_h);
        if rh >= 3 {
            map.divider = Rect::new(right_x, main.y + editor_h, rw, 1);
        }
        map
    }

    /// The proportion that makes the sidebar (with its separator) `total`
    /// wide in a terminal `width` wide, clamped to what the layout allows.
    pub fn sidebar_permille(width: u16, total: u16) -> u16 {
        let hi = width.saturating_sub(RIGHT_LEAST).max(width / 2);
        permille_of(total.clamp(SIDEBAR_LEAST.min(hi), hi), width)
    }

    /// The proportion that makes the editor `rows` high out of `height`.
    pub fn editor_permille(height: u16, rows: u16) -> u16 {
        let hi = height.saturating_sub(RESULTS_LEAST).max(height / 2);
        permille_of(rows.clamp(EDITOR_LEAST.min(hi), hi), height)
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
