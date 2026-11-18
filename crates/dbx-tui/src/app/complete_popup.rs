//! The completion popup's own state: the items, which one is selected and
//! which are in the window. Pure: no drawing, no keys; the app moves it and
//! the view draws it.

/// What an item is, for the right-hand column and the order of equals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Keyword,
    Schema,
    Table,
    View,
    MatView,
    Foreign,
    Column,
    Function,
    Procedure,
}

impl ItemKind {
    /// What the popup says on the right.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Keyword => "keyword",
            ItemKind::Schema => "schema",
            ItemKind::Table => "table",
            ItemKind::View => "view",
            ItemKind::MatView => "matview",
            ItemKind::Foreign => "foreign",
            ItemKind::Column => "column",
            ItemKind::Function => "function",
            ItemKind::Procedure => "procedure",
        }
    }
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The name as stored, which is what is listed.
    pub name: String,
    /// What goes into the text: the name, quoted when the dialect needs it,
    /// and a keyword in the case the user is typing in.
    pub insert: String,
    pub kind: ItemKind,
    /// The type of a column or the result of a function.
    pub detail: Option<String>,
    /// The characters of `name` the typed text matched.
    pub matched: Vec<usize>,
}

/// Rows the window shows at most.
pub const MAX_ROWS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Popup {
    items: Vec<Item>,
    selected: usize,
    top: usize,
    /// The most rows the screen has room for (0: `MAX_ROWS`).
    cap: usize,
    /// The user moved the selection since this list began.
    touched: bool,
    /// A frame has drawn this list, so the user may be about to act on what
    /// it showed.
    seen: bool,
}

impl Popup {
    /// Replaces the items. With `keep`, a selection the user can have acted
    /// on (they moved it, or a frame drew the list with it) stays on the same
    /// suggestion (by name and kind) when it is still there, wherever it now
    /// sorts: what `enter` is about to insert never changes under it.
    /// Otherwise, or when it is not there, it is the first: the best match,
    /// whichever it now is, and the list is new to the user.
    pub fn set_items(&mut self, items: Vec<Item>, keep: bool) {
        let was = if keep && (self.touched || self.seen) { self.selected_item().map(|i| (i.name.clone(), i.kind)) } else { None };
        self.items = items;
        match was.and_then(|(name, kind)| self.items.iter().position(|i| i.name == name && i.kind == kind)) {
            Some(at) => self.selected = at,
            None => {
                self.selected = 0;
                self.touched = false;
                self.seen = false;
            }
        }
        self.top = 0;
        self.follow();
    }

    /// Whether the user has moved the selection since the list was made.
    pub fn touched(&self) -> bool {
        self.touched
    }

    /// A frame drew the list: from now on its highlighted item is what the
    /// user sees.
    pub fn mark_seen(&mut self) {
        self.seen |= !self.items.is_empty();
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many rows the window shows.
    pub fn rows(&self) -> usize {
        self.items.len().min(if self.cap == 0 { MAX_ROWS } else { self.cap.min(MAX_ROWS) })
    }

    /// Tells the popup how many rows the screen has room for, when that is
    /// fewer than `MAX_ROWS`; the window follows the selection in what is left.
    pub fn set_cap(&mut self, rows: usize) {
        self.cap = rows;
        self.follow();
    }

    /// The index of the window's first item.
    pub fn top(&self) -> usize {
        self.top
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    /// The items in the window.
    pub fn window(&self) -> &[Item] {
        &self.items[self.top..(self.top + self.rows()).min(self.items.len())]
    }

    /// One up. At the first item it stays. Returns whether anything moved.
    pub fn up(&mut self) -> bool {
        self.move_to(self.selected.saturating_sub(1))
    }

    /// One down. At the last item it stays.
    pub fn down(&mut self) -> bool {
        self.move_to(self.selected + 1)
    }

    pub fn page_up(&mut self) -> bool {
        self.move_to(self.selected.saturating_sub(self.rows().max(1)))
    }

    pub fn page_down(&mut self) -> bool {
        self.move_to(self.selected + self.rows().max(1))
    }

    /// Selects item `index` (clamped), as a click does.
    pub fn select(&mut self, index: usize) -> bool {
        self.move_to(index)
    }

    /// Moves the window by `rows` (the wheel), clamped, leaving the selection
    /// where it is. Returns whether the window moved.
    pub fn scroll(&mut self, rows: isize) -> bool {
        let max = self.items.len().saturating_sub(self.rows());
        let top = self.top.saturating_add_signed(rows).min(max);
        let moved = top != self.top;
        self.top = top;
        moved
    }

    /// Selects `index` (clamped to the list) and scrolls it into the window.
    fn move_to(&mut self, index: usize) -> bool {
        let index = index.min(self.items.len().saturating_sub(1));
        let before = (self.selected, self.top);
        self.touched |= index != self.selected;
        self.selected = index;
        self.follow();
        before != (self.selected, self.top)
    }

    /// Scrolls the window the least that shows the selection, and keeps it
    /// full: the window never runs past the end of the list.
    fn follow(&mut self) {
        let rows = self.rows();
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
        self.top = self.top.min(self.items.len().saturating_sub(rows));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: ItemKind) -> Item {
        Item { name: name.into(), insert: name.into(), kind, detail: None, matched: Vec::new() }
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n).map(|i| item(&format!("t{i:02}"), ItemKind::Table)).collect()
    }

    fn popup(n: usize) -> Popup {
        let mut p = Popup::default();
        p.set_items(items(n), false);
        p
    }

    #[test]
    fn up_at_the_first_item_and_down_at_the_last_stay_put() {
        let mut p = popup(3);
        assert!(!p.up());
        assert_eq!(p.selected(), 0);
        assert!(p.down() && p.down());
        assert_eq!(p.selected(), 2);
        assert!(!p.down(), "nothing moved");
        assert_eq!(p.selected(), 2);
        assert!(p.up());
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn a_list_longer_than_the_window_scrolls_to_keep_the_selection_in_it() {
        let mut p = popup(25);
        assert_eq!((p.rows(), p.top(), p.window().len()), (MAX_ROWS, 0, MAX_ROWS));
        for _ in 0..MAX_ROWS - 1 {
            p.down();
        }
        assert_eq!((p.selected(), p.top()), (9, 0), "the last row of the window");
        p.down();
        assert_eq!((p.selected(), p.top()), (10, 1), "one more scrolls by one");
        assert_eq!(p.window().first().unwrap().name, "t01");
        assert_eq!(p.window().last().unwrap().name, "t10");
        for _ in 0..30 {
            p.down();
        }
        assert_eq!((p.selected(), p.top()), (24, 15), "the window ends with the list");
        for _ in 0..30 {
            p.up();
        }
        assert_eq!((p.selected(), p.top()), (0, 0));
    }

    #[test]
    fn a_short_list_shows_all_of_its_items_and_no_more() {
        let p = popup(3);
        assert_eq!((p.rows(), p.window().len()), (3, 3));
        assert_eq!(popup(0).rows(), 0);
        assert!(popup(0).selected_item().is_none());
    }

    #[test]
    fn page_keys_move_a_window_at_a_time_and_stop_at_the_ends() {
        let mut p = popup(25);
        assert!(p.page_down());
        assert_eq!(p.selected(), 10);
        assert!(p.selected() >= p.top() && p.selected() < p.top() + p.rows());
        assert!(p.page_down());
        assert_eq!(p.selected(), 20);
        assert!(p.page_down());
        assert_eq!(p.selected(), 24);
        assert!(!p.page_down());
        assert_eq!(p.top() + p.rows(), 25);
        assert!(p.page_up());
        assert_eq!(p.selected(), 14);
        assert!(p.page_up() && p.page_up());
        assert_eq!(p.selected(), 0);
        assert!(!p.page_up());
    }

    #[test]
    fn new_items_select_the_first_unless_the_selection_is_kept_and_still_there() {
        let mut p = popup(5);
        p.down();
        p.down();
        assert_eq!(p.selected_item().unwrap().name, "t02");
        p.set_items(items(5), false);
        assert_eq!(p.selected(), 0);
        p.down();
        p.down();
        // The same suggestion at another place in a longer list.
        let mut more = items(5);
        more.insert(0, item("a", ItemKind::Table));
        p.set_items(more, true);
        assert_eq!(p.selected_item().unwrap().name, "t02", "kept by name");
        assert_eq!(p.selected(), 3);
        // Gone: the first.
        p.set_items(items(1), true);
        assert_eq!(p.selected(), 0);
        // Not moved from the first: the first is the best match, whatever it is now.
        let mut r = popup(3);
        let mut other = items(3);
        other.insert(0, item("better", ItemKind::Table));
        r.set_items(other, true);
        assert_eq!(r.selected_item().unwrap().name, "better");
        // The same name of another kind is another suggestion.
        let mut q = Popup::default();
        q.set_items(vec![item("film", ItemKind::Table), item("film", ItemKind::Column)], false);
        q.down();
        q.set_items(vec![item("film", ItemKind::Column), item("film", ItemKind::Table)], true);
        assert_eq!((q.selected(), q.selected_item().unwrap().kind), (0, ItemKind::Column));
    }

    #[test]
    fn a_list_the_user_has_seen_keeps_its_highlighted_item_when_it_is_re_ranked_and_one_not_seen_does_not() {
        let mut p = popup(3);
        p.mark_seen();
        let mut better = items(3);
        better.insert(0, item("better", ItemKind::Table));
        p.set_items(better.clone(), true);
        assert_eq!((p.selected(), p.selected_item().unwrap().name.as_str()), (1, "t00"), "held by identity");
        assert!(!p.touched(), "the user moved nothing");
        // Gone: the new list was never seen, so its best match is the top.
        p.set_items(items(1)[..0].to_vec(), true);
        p.set_items(better, true);
        assert_eq!(p.selected(), 0);
        // Seen once, and replaced by a list that is not a refresh of it.
        let mut q = popup(3);
        q.mark_seen();
        q.set_items(items(3), false);
        let mut other = items(3);
        other.insert(0, item("better", ItemKind::Table));
        q.set_items(other, true);
        assert_eq!(q.selected_item().unwrap().name, "better", "a new list is not seen until it is drawn");
    }

    #[test]
    fn moving_the_selection_marks_the_list_touched_until_it_is_replaced() {
        let mut p = popup(3);
        assert!(!p.touched());
        p.down();
        assert!(p.touched());
        p.up();
        assert!(p.touched(), "back on the first item, but moved");
        p.set_items(items(3), true);
        assert!(p.touched(), "kept");
        p.set_items(items(3), false);
        assert!(!p.touched());
    }

    #[test]
    fn a_kept_selection_far_down_is_scrolled_into_the_window() {
        let mut p = popup(25);
        p.select(22);
        assert!(p.selected() >= p.top() && p.selected() < p.top() + p.rows());
        p.set_items(items(25), true);
        assert_eq!(p.selected(), 22);
        assert!(p.selected() >= p.top() && p.selected() < p.top() + p.rows(), "{} in {}..", p.selected(), p.top());
    }

    #[test]
    fn a_screen_with_little_room_shrinks_the_window_and_the_selection_stays_in_it() {
        let mut p = popup(25);
        p.select(12);
        p.set_cap(3);
        assert_eq!(p.rows(), 3);
        assert!(p.selected() >= p.top() && p.selected() < p.top() + 3, "{} in {}..", p.selected(), p.top());
        p.set_cap(0);
        assert_eq!(p.rows(), MAX_ROWS);
    }

    #[test]
    fn selecting_by_index_clamps_and_the_wheel_moves_only_the_window() {
        let mut p = popup(25);
        assert!(p.select(7));
        assert_eq!(p.selected(), 7);
        assert!(!p.select(7));
        p.select(99);
        assert_eq!(p.selected(), 24);
        let mut p = popup(25);
        assert!(p.scroll(3));
        assert_eq!((p.top(), p.selected()), (3, 0), "the selection did not move");
        assert!(p.scroll(100));
        assert_eq!(p.top(), 15, "clamped so the window stays full");
        assert!(!p.scroll(1));
        assert!(p.scroll(-100));
        assert_eq!(p.top(), 0);
        assert!(!p.scroll(-1));
        // Up from a selection the wheel scrolled out of view brings it back.
        let mut p = popup(25);
        p.scroll(15);
        p.down();
        assert!(p.selected() >= p.top() && p.selected() < p.top() + p.rows());
    }
}
