//! A single-line text field, used by every overlay that takes text (design
//! section 12, herdr survey section 6). Grapheme-aware cursor, word movement
//! and deletion, paste, horizontal scroll within a given width, and a masked
//! mode for secrets. Widths are display columns (`unicode-width`).
//!
//! Pure: it knows nothing of the terminal or of ratatui.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::keys::KeyCombo;

/// What a key did to the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// The field does not use this key; the caller may.
    Ignored,
    /// The cursor moved; the text is as it was.
    Moved,
    /// The text changed.
    Changed,
}

/// The part of the field that fits a width, and where the cursor is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// What to draw: the visible text, with a bullet per grapheme if masked.
    pub text: String,
    /// The cursor's column within the window (it may equal the window's text
    /// width, when the cursor is past the last character).
    pub cursor_col: u16,
}

#[derive(Clone, Default)]
pub struct TextField {
    text: String,
    /// Byte offset into `text`, always on a grapheme boundary.
    cursor: usize,
    /// Index of the first visible grapheme.
    scroll: usize,
    masked: bool,
    /// Input was cut to fit `MAX_BYTES` and nothing has been edited since.
    cut: bool,
}

// A masked field's text is a secret: it never reaches a log or a failed test.
impl fmt::Debug for TextField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("TextField");
        if self.masked {
            d.field("text", &format_args!("<masked, {} bytes>", self.text.len()));
        } else {
            d.field("text", &self.text);
        }
        d.field("cursor", &self.cursor).finish()
    }
}

impl TextField {
    /// The most a field holds, in bytes.
    pub const MAX_BYTES: usize = 4096;

    /// Whether input was cut to fit `MAX_BYTES` and nothing has been edited since.
    pub fn was_cut(&self) -> bool {
        self.cut
    }

    pub fn new() -> Self {
        Self::default()
    }
    pub fn masked() -> Self {
        Self { masked: true, ..Self::default() }
    }
    pub fn with_text(text: &str) -> Self {
        let mut f = Self::new();
        f.set_text(text);
        f
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn is_masked(&self) -> bool {
        self.masked
    }
    /// The cursor's position in graphemes from the start.
    pub fn cursor(&self) -> usize {
        self.text[..self.cursor].graphemes(true).count()
    }

    /// Replaces the text, with the cursor at its end.
    pub fn set_text(&mut self, text: &str) {
        let line = one_line(text);
        let keep = fit(&line, Self::MAX_BYTES);
        self.cut = keep.len() < line.len();
        self.text = keep.to_string();
        self.cursor = self.text.len();
        self.scroll = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        if self.text.len() + c.len_utf8() > Self::MAX_BYTES {
            self.cut = true;
            return;
        }
        {
            self.cut = false;
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
            // A combining mark joins the grapheme before it: the cursor stays
            // on a boundary, after the whole cluster.
            self.cursor = self.boundary_at_or_after(self.cursor);
        }
    }

    /// Pastes `s` as one line: a trailing line break is dropped, inner ones
    /// become one space, other control characters vanish.
    pub fn insert_str(&mut self, s: &str) {
        let line = one_line(s);
        let room = Self::MAX_BYTES.saturating_sub(self.text.len());
        let keep = fit(&line, room);
        self.cut = keep.len() < line.len();
        self.text.insert_str(self.cursor, keep);
        self.cursor = self.boundary_at_or_after(self.cursor + keep.len());
    }

    pub fn backspace(&mut self) -> bool {
        let start = self.prev_boundary(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.after_deletion();
        true
    }

    pub fn delete(&mut self) -> bool {
        let end = self.next_boundary(self.cursor);
        if end == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..end, "");
        self.after_deletion();
        true
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }
    pub fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }
    pub fn home(&mut self) {
        self.cursor = 0;
    }
    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start_before(self.cursor);
    }
    pub fn word_right(&mut self) {
        self.cursor = self.word_end_after(self.cursor);
    }

    pub fn delete_word_left(&mut self) -> bool {
        let start = self.word_start_before(self.cursor);
        self.cut(start, self.cursor)
    }
    pub fn delete_word_right(&mut self) -> bool {
        let end = self.word_end_after(self.cursor);
        self.cut(self.cursor, end)
    }
    pub fn kill_to_start(&mut self) -> bool {
        self.cut(0, self.cursor)
    }
    pub fn kill_to_end(&mut self) -> bool {
        self.cut(self.cursor, self.text.len())
    }

    /// Applies an editing key. Keys the field does not use (enter, tab, esc,
    /// the arrows up and down, anything with ctrl or alt that is not an
    /// editing binding) are `Ignored`, left to the form around it.
    pub fn handle_key(&mut self, key: &KeyEvent) -> Edit {
        let combo = KeyCombo::from_event(key);
        let (ctrl, alt) = (combo.mods.contains(KeyModifiers::CONTROL), combo.mods.contains(KeyModifiers::ALT));
        let plain = combo.mods.is_empty();
        let changed = |did: bool| if did { Edit::Changed } else { Edit::Ignored };
        match combo.code {
            KeyCode::Char(c) if plain => {
                self.insert_char(c);
                Edit::Changed
            }
            KeyCode::Left if plain => self.moved(Self::left),
            KeyCode::Right if plain => self.moved(Self::right),
            KeyCode::Left if ctrl || alt => self.moved(Self::word_left),
            KeyCode::Right if ctrl || alt => self.moved(Self::word_right),
            KeyCode::Home if plain => self.moved(Self::home),
            KeyCode::End if plain => self.moved(Self::end),
            KeyCode::Backspace if plain => changed(self.backspace()),
            KeyCode::Backspace if alt || ctrl => changed(self.delete_word_left()),
            KeyCode::Delete if plain => changed(self.delete()),
            KeyCode::Delete if alt || ctrl => changed(self.delete_word_right()),
            KeyCode::Char('a') if ctrl && !alt => self.moved(Self::home),
            KeyCode::Char('e') if ctrl && !alt => self.moved(Self::end),
            KeyCode::Char('w') if ctrl && !alt => changed(self.delete_word_left()),
            KeyCode::Char('u') if ctrl && !alt => changed(self.kill_to_start()),
            KeyCode::Char('k') if ctrl && !alt => changed(self.kill_to_end()),
            KeyCode::Char('b') if alt && !ctrl => self.moved(Self::word_left),
            KeyCode::Char('f') if alt && !ctrl => self.moved(Self::word_right),
            KeyCode::Char('d') if alt && !ctrl => changed(self.delete_word_right()),
            _ => Edit::Ignored,
        }
    }

    /// The part of the field that fits `width` columns, scrolled the least
    /// that keeps the cursor in view (with a cell for it past the last
    /// character). Remembers the scroll, so the window moves only when the
    /// cursor leaves it.
    pub fn window(&mut self, width: u16) -> Window {
        let width = width as usize;
        let cells = self.cells();
        let cursor = self.cursor();
        // prefix[i]: the columns before grapheme i, so any span is one subtraction.
        let mut prefix = Vec::with_capacity(cells.len() + 1);
        prefix.push(0usize);
        for c in &cells {
            prefix.push(prefix[prefix.len() - 1] + c.width);
        }
        let end = prefix[cells.len()];
        let before = |from: usize| prefix[cursor] - prefix[from];
        let mut scroll = self.scroll.min(cells.len());
        if cursor < scroll {
            scroll = cursor;
        }
        // The cursor, drawn on its own grapheme or on the cell past the end,
        // must fit: columns before it, plus its own cell.
        let own = cells.get(cursor).map_or(1, |c| c.width.max(1));
        while scroll < cursor && before(scroll) + own > width {
            scroll += 1;
        }
        // Do not leave empty room on the right while text is hidden on the left.
        let extra = usize::from(cursor == cells.len());
        while scroll > 0 && end - prefix[scroll - 1] + extra <= width {
            scroll -= 1;
        }

        let mut text = String::new();
        let mut used = 0;
        for cell in &cells[scroll..] {
            if used + cell.width > width {
                break;
            }
            used += cell.width;
            if self.masked {
                text.push('•');
            } else {
                text.push_str(cell.text);
            }
        }
        let cursor_col = before(scroll).min(width) as u16;
        self.scroll = scroll;
        Window { text, cursor_col }
    }

    /// A click at `column` of the window as last drawn: the cursor goes onto
    /// the grapheme there, or to the end when the click is past the text.
    pub fn click(&mut self, column: u16) {
        let cells = self.cells();
        let mut at = column as usize;
        let mut index = self.scroll.min(cells.len());
        while index < cells.len() {
            let w = cells[index].width;
            if at < w {
                break;
            }
            at -= w;
            index += 1;
        }
        self.cursor = cells.get(index).map_or(self.text.len(), |c| c.start);
    }

    // ---- internals -------------------------------------------------------------

    fn moved(&mut self, motion: fn(&mut Self)) -> Edit {
        motion(self);
        Edit::Moved
    }

    fn cut(&mut self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        self.text.replace_range(from..to, "");
        self.cursor = from;
        self.after_deletion();
        true
    }

    /// Neighbours can merge into one grapheme when what lay between them goes:
    /// put the cursor after the merged cluster, never inside it.
    fn after_deletion(&mut self) {
        self.cut = false;
        self.cursor = self.boundary_at_or_after(self.cursor);
    }

    fn cells(&self) -> Vec<Cell<'_>> {
        self.text
            .grapheme_indices(true)
            .map(|(start, text)| Cell {
                start,
                text,
                // A masked field draws one bullet per grapheme, whatever it is.
                width: if self.masked { 1 } else { UnicodeWidthStr::width(text) },
            })
            .collect()
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text[at..].graphemes(true).next().map_or(at, |g| at + g.len())
    }

    /// `at` itself if it is a grapheme boundary, else the next one.
    fn boundary_at_or_after(&self, at: usize) -> usize {
        self.text.grapheme_indices(true).map(|(i, _)| i).chain([self.text.len()]).find(|&i| i >= at).unwrap_or(at)
    }

    fn is_word(g: &str) -> bool {
        g.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
    }

    /// Back over any non-word graphemes, then over the word before them.
    fn word_start_before(&self, at: usize) -> usize {
        let before: Vec<(usize, &str)> = self.text[..at].grapheme_indices(true).collect();
        let mut i = before.len();
        while i > 0 && !Self::is_word(before[i - 1].1) {
            i -= 1;
        }
        while i > 0 && Self::is_word(before[i - 1].1) {
            i -= 1;
        }
        before.get(i).map_or(at, |(start, _)| *start)
    }

    /// Forward over any non-word graphemes, then over the word after them.
    fn word_end_after(&self, at: usize) -> usize {
        let mut end = at;
        let after: Vec<&str> = self.text[at..].graphemes(true).collect();
        let mut i = 0;
        while i < after.len() && !Self::is_word(after[i]) {
            end += after[i].len();
            i += 1;
        }
        while i < after.len() && Self::is_word(after[i]) {
            end += after[i].len();
            i += 1;
        }
        end
    }
}

struct Cell<'a> {
    start: usize,
    text: &'a str,
    width: usize,
}

/// The longest start of `s` that is at most `max` bytes and ends on a
/// grapheme boundary.
fn fit(s: &str, max: usize) -> &str {
    let mut end = 0;
    for g in s.graphemes(true) {
        if end + g.len() > max {
            break;
        }
        end += g.len();
    }
    &s[..end]
}

/// `s` as a single line: see `TextField::insert_str`.
fn one_line(s: &str) -> String {
    let s = s.trim_end_matches(['\r', '\n']);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn field(text: &str) -> TextField {
        TextField::with_text(text)
    }

    fn key(f: &mut TextField, s: &str) -> Edit {
        let combo = crate::app::keys::KeyCombo::parse(s).unwrap_or_else(|| panic!("bad key {s}"));
        f.handle_key(&KeyEvent::new(combo.code, combo.mods))
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_the_arrows_move_it() {
        let mut f = TextField::new();
        for c in "helo".chars() {
            f.insert_char(c);
        }
        f.left();
        f.insert_char('l');
        assert_eq!(f.text(), "hello");
        assert_eq!(f.cursor(), 4);
        f.right();
        f.right(); // already at the end: stays
        assert_eq!(f.cursor(), 5);
        f.home();
        assert_eq!(f.cursor(), 0);
        f.left(); // already at the start: stays
        f.end();
        assert_eq!(f.cursor(), 5);
    }

    #[test]
    fn the_cursor_moves_and_deletes_by_grapheme_not_by_byte_or_char() {
        // e + combining acute, x, a family emoji joined by ZWJ, and a flag.
        let mut f = field("e\u{301}x👨‍👩‍👧🇫🇷");
        assert_eq!(f.cursor(), 4, "four graphemes");
        f.left();
        f.left();
        assert_eq!(f.cursor(), 2, "two steps back from the end: before the family");
        f.right();
        assert_eq!(f.cursor(), 3, "one step over the whole family");
        assert!(f.backspace(), "removes the whole family, not one scalar of it");
        assert_eq!(f.text(), "e\u{301}x🇫🇷");
        assert_eq!(f.cursor(), 2);
        f.home();
        assert!(f.delete(), "removes e and its accent together");
        assert_eq!(f.text(), "x🇫🇷");
        f.end();
        assert!(f.backspace(), "removes both scalars of the flag");
        assert_eq!(f.text(), "x");
        assert!(!field("").backspace() && !field("").delete(), "nothing to delete");
    }

    #[test]
    fn word_movement_and_deletion_stop_at_word_boundaries() {
        let mut f = field("db.host_name  and-more");
        f.word_left();
        assert_eq!(f.cursor(), 18, "start of `more`");
        f.word_left();
        assert_eq!(f.cursor(), 14, "start of `and`");
        f.word_left();
        assert_eq!(f.cursor(), 3, "start of `host_name`, underscore belongs to the word");
        f.word_right();
        assert_eq!(f.cursor(), 12, "end of `host_name`");
        f.word_right();
        f.word_right();
        f.word_right();
        f.word_right();
        assert_eq!(f.cursor(), 22, "stops at the end");

        let mut f = field("one two  ");
        assert!(f.delete_word_left());
        assert_eq!(f.text(), "one ", "trailing spaces go with the word");
        assert!(f.delete_word_left());
        assert_eq!(f.text(), "");
        assert!(!f.delete_word_left());

        let mut f = field("one two three");
        f.home();
        assert!(f.delete_word_right());
        assert_eq!(f.text(), " two three", "the word, not the space after it");
        f.right();
        assert!(f.delete_word_right());
        assert_eq!(f.text(), "  three");
    }

    #[test]
    fn kill_to_start_and_to_end_cut_at_the_cursor() {
        let mut f = field("abcdef");
        f.left();
        f.left();
        assert!(f.kill_to_end());
        assert_eq!(f.text(), "abcd");
        assert!(!f.kill_to_end(), "nothing after the cursor");
        f.left();
        assert!(f.kill_to_start());
        assert_eq!(f.text(), "d");
        assert_eq!(f.cursor(), 0);
    }

    #[test]
    fn paste_goes_in_at_the_cursor_as_one_line() {
        let mut f = field("ac");
        f.left();
        f.insert_str("b");
        assert_eq!((f.text(), f.cursor()), ("abc", 2));

        let mut f = TextField::new();
        f.insert_str("db.example.com\r\n");
        assert_eq!(f.text(), "db.example.com", "a trailing newline is not part of a pasted value");
        let mut f = TextField::new();
        f.insert_str("one\ntwo\r\nthree\tfour\u{7}");
        assert_eq!(f.text(), "one two three four", "inner breaks become spaces, controls vanish");
    }

    #[test]
    fn keys_edit_as_the_mode_bar_promises_and_unbound_keys_are_left_to_the_caller() {
        let mut f = TextField::new();
        for k in ["h", "i", "shift+x"] {
            assert_eq!(key(&mut f, k), Edit::Changed);
        }
        assert_eq!(f.text(), "hiX");
        assert_eq!(key(&mut f, "left"), Edit::Moved);
        assert_eq!(key(&mut f, "backspace"), Edit::Changed);
        assert_eq!(f.text(), "hX");
        assert_eq!(key(&mut f, "home"), Edit::Moved);
        assert_eq!(key(&mut f, "delete"), Edit::Changed);
        assert_eq!(f.text(), "X");
        assert_eq!(key(&mut f, "backspace"), Edit::Ignored, "at the start there is nothing to delete");
        let mut f = field("one two");
        assert_eq!(key(&mut f, "ctrl+w"), Edit::Changed);
        assert_eq!(f.text(), "one ");
        assert_eq!(key(&mut f, "ctrl+a"), Edit::Moved);
        assert_eq!(f.cursor(), 0);
        assert_eq!(key(&mut f, "ctrl+e"), Edit::Moved);
        assert_eq!(key(&mut f, "ctrl+u"), Edit::Changed);
        assert!(f.is_empty());
        for k in ["enter", "tab", "esc", "up", "down", "f5", "ctrl+t", "ctrl+g", "alt+x"] {
            assert_eq!(key(&mut f, k), Edit::Ignored, "{k}");
        }
        assert!(f.is_empty());
        assert_eq!(f.handle_key(&KeyEvent::new(KeyCode::Char('é'), KeyModifiers::NONE)), Edit::Changed);
    }

    #[test]
    fn a_field_wider_than_its_window_scrolls_just_enough_to_keep_the_cursor_in_view() {
        let mut f = field("0123456789abcdefghij");
        let w = f.window(10);
        assert_eq!(w.text, "bcdefghij", "the cursor at the end leaves a cell for itself");
        assert_eq!(w.cursor_col, 9);
        // Off the end, the cell kept for the cursor is free, so the text moves into it.
        f.left();
        let w = f.window(10);
        assert_eq!((w.text.as_str(), w.cursor_col), ("abcdefghij", 9));
        // Moving left inside the window does not scroll it any further.
        f.left();
        f.left();
        assert_eq!(f.window(10).text, "abcdefghij");
        assert_eq!(f.window(10).cursor_col, 7);
        for _ in 0..7 {
            f.left();
        }
        let w = f.window(10);
        assert_eq!((w.text.as_str(), w.cursor_col), ("abcdefghij", 0));
        // Moving past its left edge scrolls by one.
        f.left();
        let w = f.window(10);
        assert_eq!((w.text.as_str(), w.cursor_col), ("9abcdefghi", 0));
        f.home();
        assert_eq!(f.window(10).text, "0123456789");
        assert_eq!(f.window(10).cursor_col, 0);
        // Text that fits is never scrolled, and shrinking the text scrolls back.
        let mut f = field("short");
        assert_eq!(f.window(10), Window { text: "short".into(), cursor_col: 5 });
        let mut f2 = field("0123456789abcdefghij");
        f2.window(10);
        f2.left();
        f2.kill_to_start();
        assert_eq!(f2.window(10).text, "j", "everything before the cursor was cut; the window starts over");
        f.home();
        assert_eq!(f.window(3).text, "sho");
    }

    #[test]
    fn wide_characters_take_two_columns_and_never_straddle_the_window_edge() {
        let mut f = field("日本語");
        let w = f.window(5);
        assert_eq!(w.text, "本語", "two wide graphemes and the cursor cell fit in five columns");
        assert_eq!(w.cursor_col, 4);
        f.home();
        let w = f.window(5);
        assert_eq!(w.text, "日本", "a third wide grapheme would need column six");
        assert_eq!(w.cursor_col, 0);
        let mut f = field("a😀b");
        f.home();
        assert_eq!(f.window(2).text, "a", "the emoji needs two columns of its own");
    }

    #[test]
    fn a_masked_field_shows_one_bullet_per_grapheme_and_keeps_the_text_out_of_debug() {
        let mut f = TextField::masked();
        f.insert_str("pw-Zq8!é👨‍👩‍👧");
        assert_eq!(f.text(), "pw-Zq8!é👨‍👩‍👧", "the value itself is intact");
        let w = f.window(30);
        assert_eq!(w.text, "•".repeat(9));
        assert_eq!(w.cursor_col, 9);
        assert!(!format!("{f:?}").contains("Zq8"), "{f:?}");
        assert!(f.is_masked());
        assert!(format!("{:?}", field("visible")).contains("visible"), "unmasked text is debuggable");
    }

    #[test]
    fn a_click_puts_the_cursor_on_the_grapheme_under_it_and_after_the_end_on_the_end() {
        let mut f = field("hello world");
        f.window(20);
        f.click(4);
        assert_eq!(f.cursor(), 4);
        f.click(0);
        assert_eq!(f.cursor(), 0);
        f.click(15);
        assert_eq!(f.cursor(), 11, "past the end means the end");
        let mut f = field("日本語");
        f.window(10);
        f.click(2);
        assert_eq!(f.cursor(), 1, "column 2 is the second grapheme");
        f.click(3);
        assert_eq!(f.cursor(), 1, "column 3 is still the second grapheme: it is two columns wide");
        // A click counts from the scrolled window's first column.
        let mut f = field("0123456789abcdefghij");
        f.window(10);
        f.click(0);
        assert_eq!(f.cursor(), 11, "the window starts at `b`");
    }

    #[test]
    fn after_a_deletion_the_cursor_is_on_a_grapheme_boundary_even_when_neighbours_merge() {
        // Hangul choseong + X + jungseong: deleting X merges the two into one syllable.
        let mut f = field("\u{1100}X\u{1161}");
        f.left();
        assert!(f.backspace());
        assert_eq!(f.text(), "\u{1100}\u{1161}");
        f.insert_char('Z');
        assert_eq!(f.text(), "\u{1100}\u{1161}Z", "the cursor was inside the merged cluster, so it moved after it");
        let mut f = field("\u{1100}X\u{1161}");
        f.left();
        f.left();
        assert!(f.delete());
        f.insert_char('Z');
        assert_eq!(f.text(), "\u{1100}\u{1161}Z", "the same after a forward delete");
    }

    #[test]
    fn a_click_skips_zero_width_graphemes_as_the_window_does() {
        let mut f = field("a\u{200b}bc");
        f.window(10);
        f.click(1);
        assert_eq!(f.cursor(), 2, "column 1 is `b`: the zero-width space takes no column");
        assert_eq!(f.window(10).text, "a\u{200b}bc");
    }

    #[test]
    fn a_field_holds_at_most_4_kib_and_says_when_input_was_cut() {
        let mut f = TextField::new();
        assert!(!f.was_cut());
        f.insert_str(&"é".repeat(5_000));
        assert!(f.text().len() <= TextField::MAX_BYTES, "{}", f.text().len());
        assert_eq!(f.text().len(), 4096, "as many whole characters as fit");
        assert!(f.was_cut());
        f.insert_char('x');
        assert_eq!(f.text().len(), 4096, "a full field takes no more");
        assert!(f.was_cut());
        assert!(f.backspace());
        assert!(!f.was_cut(), "an edit that makes room clears the notice");
        let mut g = TextField::new();
        g.insert_str("short");
        assert!(!g.was_cut());
        g.set_text(&"y".repeat(10_000));
        assert_eq!(g.text().len(), 4096);
        assert!(g.was_cut());
        // Never in the middle of a grapheme.
        let mut h = TextField::new();
        h.insert_str(&"a".repeat(4095));
        h.insert_str("👨\u{200d}👩");
        assert_eq!(h.text().len(), 4095);
    }

    #[test]
    fn the_window_of_a_full_field_is_computed_in_linear_time() {
        let mut f = TextField::new();
        f.insert_str(&"日".repeat(10_000));
        let started = std::time::Instant::now();
        for _ in 0..50 {
            f.window(60);
            f.left();
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(50), "{:?}", started.elapsed());
    }
}
