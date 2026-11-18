//! What the mouse is in the middle of: a drag that began on a divider, a
//! scrollbar or the editor's text, and the last click, for double-clicks.
//!
//! No timers: a double-click is two presses of the same thing within
//! `DOUBLE_CLICK` of each other, judged by the `now` each `update` is given.

use std::time::{Duration, Instant};

/// Two presses on the same thing this close together are a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(350);

/// A drag in progress, from the press that began it to the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    /// The sidebar's separator: the sidebar follows the pointer's column.
    Sidebar,
    /// The editor/results divider: the editor follows the pointer's row.
    Split,
    /// A scrollbar's thumb, grabbed `grab` rows below its top.
    SidebarBar { grab: u16 },
    GridBar { grab: u16 },
    /// A selection in the editor, begun by the press.
    EditorSelect,
}

/// What a press landed on, as far as double-clicks care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKey {
    SidebarRow(usize),
    Cell(usize, usize),
    /// A divider: the sidebar's (`true`) or the editor's.
    Divider(bool),
    /// A place in the editor text (row, character).
    Text(usize, usize),
}

/// The last press, to tell a second one from a first.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clicks {
    last: Option<(Instant, ClickKey)>,
}

impl Clicks {
    /// Records a press and says whether it is the second of a double-click.
    /// After a double-click the next press starts over.
    pub fn press(&mut self, now: Instant, key: ClickKey) -> bool {
        let double = matches!(self.last, Some((at, k)) if k == key && now.saturating_duration_since(at) <= DOUBLE_CLICK);
        self.last = if double { None } else { Some((now, key)) };
        double
    }

    /// Forgets the last press (it did something that should not pair).
    pub fn forget(&mut self) {
        self.last = None;
    }
}

/// The scroll position a scrollbar click or drag asks for: the thumb's top
/// goes to `top` (a row of the track), for a list of `total` rows `room` high.
pub fn scroll_for_thumb(total: usize, room: usize, thumb_len: usize, top: usize) -> usize {
    let max_scroll = total.saturating_sub(room);
    let max_top = room.saturating_sub(thumb_len);
    if max_top == 0 {
        return 0;
    }
    ((top.min(max_top) * max_scroll + max_top / 2) / max_top).min(max_scroll)
}

/// Where a press at track row `row` (0 is the top) grabs the thumb: the same
/// grab as a press on the thumb itself, or its middle when the press missed it.
/// Returns the grab and whether the press was off the thumb (so the list
/// jumps).
pub fn grab(thumb: (usize, usize), row: usize) -> (usize, bool) {
    let (top, len) = thumb;
    if (top..top + len).contains(&row) {
        (row - top, false)
    } else {
        (len / 2, true)
    }
}
