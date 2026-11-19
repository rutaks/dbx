//! The SQL editor: our wrapper around `ratatui-textarea`'s `TextArea`.
//!
//! `update` reaches the widget only through this type: the text, the cursor
//! and the selection in our own `Cursor`, keys, paste and setting the text.
//! The widget edits; `view` draws (slice 10: syntax colours, line numbers,
//! the statement about to run outlined), reading the lines, caret, selection
//! and scroll offset from here. The widget cannot style ranges, so its own
//! renderer is used only (into a scratch buffer, `layout`) to keep its view
//! following the caret.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use ratatui_textarea::{CursorMove, TextArea};

use super::edit_ops::{self, Doc, Mark};
use crate::core::Cursor;

/// The most a paste may hold, in bytes. The widget rebuilds its screen map on
/// every edit and walks each line on every frame, so a huge paste would make
/// every key slow; a 1 MB line stalled the client for over a minute.
pub const PASTE_LIMIT: usize = 256 * 1024;
/// The longest line a paste may hold, in bytes.
pub const LINE_LIMIT: usize = 16 * 1024;

/// What a mouse action can change: the caret, the selection, the scroll.
type MouseState = ((usize, usize), Option<((usize, usize), (usize, usize))>, (u16, u16));

/// The text and caret as they were before an edit: what undo goes back to.
#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    caret: Cursor,
}

/// The most edits undo goes back over.
const HISTORY: usize = 200;

#[derive(Debug, Clone)]
pub struct Editor {
    area: TextArea<'static>,
    /// Counts changes to the text: what the highlighter and the statement
    /// finder key their caches by.
    revision: u64,
    /// Keys the widget has been given (a test watches that one the client
    /// kept for itself never reached it).
    keys_given: u64,
    /// Undo and redo. The widget's own history is off (it records an edit as
    /// several entries and cannot group them): every edit, however many
    /// changes it makes, is one snapshot here.
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The selection before each "extend selection", so "shrink" can walk back,
    /// and the state the last one left (a shrink after anything else is a no-op).
    extended: Vec<Mark>,
    extended_to: Option<Mark>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor { area: Self::widget(Vec::new()), revision: 0, keys_given: 0, undo: Vec::new(), redo: Vec::new(), extended: Vec::new(), extended_to: None }
    }

    /// A widget over `lines` with its history off.
    fn widget(lines: Vec<String>) -> TextArea<'static> {
        let mut area = if lines.is_empty() { TextArea::default() } else { TextArea::new(lines) };
        area.set_max_histories(0);
        area
    }

    /// The whole buffer, rows joined with `\n`.
    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    /// The buffer's rows.
    pub fn lines(&self) -> &[String] {
        self.area.lines()
    }

    /// Changes whenever the text does (typing, paste, undo, `set_text`).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replaces the buffer and puts the cursor at its end.
    pub fn set_text(&mut self, text: &str) {
        self.revision += 1;
        self.undo.clear();
        self.redo.clear();
        self.extended.clear();
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.area = Self::widget(lines);
        self.area.move_cursor(ratatui_textarea::CursorMove::Bottom);
        self.area.move_cursor(ratatui_textarea::CursorMove::End);
    }

    pub fn is_empty(&self) -> bool {
        self.area.is_empty()
    }

    /// Where the cursor is: row, and character within the row.
    pub fn cursor(&self) -> Cursor {
        let c = self.area.cursor();
        Cursor { row: c.0, col: c.1 }
    }

    /// The selection, start before end, if any text is selected.
    pub fn selection(&self) -> Option<(Cursor, Cursor)> {
        let ((r0, c0), (r1, c1)) = self.area.selection_range()?;
        let (a, b) = (Cursor { row: r0, col: c0 }, Cursor { row: r1, col: c1 });
        (a != b).then_some((a, b))
    }

    /// A key for the widget (typing, moving, selecting). The caller has
    /// already taken the keys that are the client's (the prefix, run, the
    /// editing actions): what is left is text and the plain movement keys, and
    /// anything with Ctrl, Alt or Cmd that is not bound to an action is dropped
    /// here, so the widget's Emacs keys (`ctrl+k`, `ctrl+u`...) and a Cmd key
    /// the editor does not use never type a character or edit. Returns whether
    /// anything visible changed: the text, the cursor or the selection.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.keys_given += 1;
        if !widget_takes(&key) {
            return false;
        }
        let before = (self.area.cursor(), self.area.selection_range());
        let edits = matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete | KeyCode::Tab);
        let snap = edits.then(|| self.snapshot());
        let edited = self.area.input(key);
        if let (true, Some(snap)) = (edited, snap) {
            self.record(snap);
        }
        edited || before != (self.area.cursor(), self.area.selection_range())
    }

    fn snapshot(&self) -> Snapshot {
        let c = self.area.cursor();
        Snapshot { lines: self.area.lines().to_vec(), caret: Cursor { row: c.0, col: c.1 } }
    }

    /// An edit was made after `before`: it is one step to undo, and what was
    /// undone can no longer be redone.
    fn record(&mut self, before: Snapshot) {
        self.revision += 1;
        self.redo.clear();
        if self.undo.len() == HISTORY {
            self.undo.remove(0);
        }
        self.undo.push(before);
    }

    /// Puts `lines` in the widget in place (its view keeps its scroll).
    fn load(&mut self, lines: &[String]) {
        self.area.cancel_selection();
        if !self.area.is_empty() {
            self.area.move_cursor(CursorMove::Jump(0, 0));
            let total = self.area.lines().iter().map(|l| l.chars().count()).sum::<usize>() + self.area.lines().len() - 1;
            self.area.delete_str(total);
        }
        self.area.insert_str(lines.join("\n"));
    }

    fn put_caret(&mut self, anchor: Option<Cursor>, caret: Cursor) {
        let jump = |c: Cursor| CursorMove::Jump(c.row.min(u16::MAX as usize) as u16, c.col.min(u16::MAX as usize) as u16);
        self.area.cancel_selection();
        if let Some(a) = anchor.filter(|a| *a != caret) {
            self.area.move_cursor(jump(a));
            self.area.start_selection();
        }
        self.area.move_cursor(jump(caret));
    }

    /// Undoes the last edit. Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else { return false };
        let now = self.snapshot();
        self.redo.push(now);
        self.restore(snap);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else { return false };
        let now = self.snapshot();
        self.undo.push(now);
        self.restore(snap);
        true
    }

    fn restore(&mut self, snap: Snapshot) {
        self.load(&snap.lines);
        self.put_caret(None, snap.caret);
        self.revision += 1;
        self.extended.clear();
    }

    /// The text, caret and selection as the actions see them.
    pub fn doc(&self) -> Doc {
        let c = self.area.cursor();
        let caret = Cursor { row: c.0, col: c.1 };
        let anchor = self.area.selection_range().and_then(|((r0, c0), (r1, c1))| {
            // The caret is one end of the selection; the anchor is the other.
            let (a, b) = (Cursor { row: r0, col: c0 }, Cursor { row: r1, col: c1 });
            Some(if a == caret { b } else { a }).filter(|x| *x != caret)
        });
        Doc::from_lines(self.area.lines(), anchor, caret)
    }

    /// Makes the editor what `doc` is, as one undo step when the text changed.
    /// Returns whether anything visible changed, or why `doc` cannot be held.
    pub fn apply(&mut self, doc: &Doc) -> Result<bool, &'static str> {
        let far = |c: Cursor| c.row > u16::MAX as usize || c.col > u16::MAX as usize;
        if doc.rows.len() > u16::MAX as usize || far(doc.caret) || doc.anchor.is_some_and(far) {
            return Err("that place is too far into the text to be edited here");
        }
        let lines = doc.lines();
        let before = (self.doc().caret, self.doc().anchor);
        let text_changed = lines != self.area.lines();
        if text_changed {
            let snap = self.snapshot();
            self.load(&lines);
            self.record(snap);
        }
        self.put_caret(doc.anchor, doc.caret);
        let after = (self.doc().caret, self.doc().anchor);
        Ok(text_changed || before != after)
    }

    /// Runs `f` on the document and applies the result. `f` says whether it did
    /// anything.
    pub fn edit(&mut self, f: impl FnOnce(&mut Doc) -> bool) -> Result<bool, &'static str> {
        let mut doc = self.doc();
        if !f(&mut doc) {
            return Ok(false);
        }
        self.apply(&doc)
    }

    /// Selects the next larger thing around the selection (see
    /// `edit_ops::extend_selection`).
    pub fn extend_selection(&mut self, statement: Option<(Cursor, Cursor)>) -> bool {
        let mut doc = self.doc();
        let was = edit_ops::mark(&doc);
        if self.extended_to != Some(was) {
            self.extended.clear();
        }
        if !edit_ops::extend_selection(&mut doc, statement) {
            return false;
        }
        self.extended.push(was);
        self.extended_to = Some(edit_ops::mark(&doc));
        self.apply(&doc).unwrap_or(false)
    }

    /// Goes back to the selection before the last "extend selection".
    pub fn shrink_selection(&mut self) -> bool {
        let mut doc = self.doc();
        if self.extended_to != Some(edit_ops::mark(&doc)) {
            self.extended.clear();
            return false;
        }
        let Some(back) = self.extended.pop() else { return false };
        edit_ops::restore(&mut doc, back);
        self.extended_to = Some(edit_ops::mark(&doc));
        self.apply(&doc).unwrap_or(false)
    }

    /// How many keys the widget has been given.
    pub fn keys_given(&self) -> u64 {
        self.keys_given
    }

    /// Replaces the text between two positions of one row with `text`, and
    /// puts the cursor after it. Any selection ends. Returns whether the
    /// buffer changed, or why nothing was done: the position is a row or
    /// column the widget cannot address (past `u16::MAX`), or `text` holds a
    /// control character (a line break would split it across rows). One undo
    /// step, whatever it changes.
    pub fn replace(&mut self, from: Cursor, to: Cursor, text: &str) -> Result<bool, &'static str> {
        debug_assert!(from.row == to.row && from.col <= to.col, "a replacement stays on one row");
        if from.row > u16::MAX as usize || from.col > u16::MAX as usize {
            return Err("that place is too far into the text to be edited here");
        }
        if text.chars().any(char::is_control) {
            return Err("it holds a control character");
        }
        let Some(line) = self.area.lines().get(from.row) else { return Err("that row is no longer there") };
        let line_len = line.chars().count();
        let (start, end) = (from.col.min(line_len), to.col.min(line_len).max(from.col.min(line_len)));
        let unchanged = line.chars().skip(start).take(end - start).eq(text.chars());
        self.area.cancel_selection();
        let jump = |area: &mut TextArea<'static>, col: usize| area.move_cursor(CursorMove::Jump(from.row as u16, col.min(u16::MAX as usize) as u16));
        if unchanged {
            jump(&mut self.area, start + text.chars().count());
            return Ok(false);
        }
        let snap = self.snapshot();
        jump(&mut self.area, start);
        let deleted = self.area.delete_str(end - start);
        let inserted = self.area.insert_str(text);
        let changed = deleted || inserted;
        if changed {
            self.record(snap);
        }
        Ok(changed)
    }

    /// Puts the caret at `at` (clamped to the text) and ends any selection.
    pub fn move_to(&mut self, at: Cursor) {
        self.area.cancel_selection();
        let row = at.row.min(self.area.lines().len().saturating_sub(1));
        let col = at.col.min(self.area.lines().get(row).map_or(0, |l| l.chars().count()));
        self.area.move_cursor(CursorMove::Jump(row.min(u16::MAX as usize) as u16, col.min(u16::MAX as usize) as u16));
    }

    /// Selects `from..to` of one row, with the caret at `to`: typing replaces it.
    pub fn select_range(&mut self, from: Cursor, to: Cursor) {
        self.move_to(from);
        if to != from {
            self.area.start_selection();
            self.move_to_keeping_selection(to);
        }
    }

    fn move_to_keeping_selection(&mut self, at: Cursor) {
        self.area.move_cursor(CursorMove::Jump(at.row.min(u16::MAX as usize) as u16, at.col.min(u16::MAX as usize) as u16));
    }

    /// Inserts pasted text at the cursor, replacing any selection. Returns
    /// whether the buffer changed, or, for a paste over `PASTE_LIMIT` or with
    /// a line over `LINE_LIMIT`, why none of it was pasted: SQL is never cut.
    pub fn paste(&mut self, text: &str) -> Result<bool, String> {
        let text = clean_paste(text);
        if text.len() > PASTE_LIMIT {
            return Err(format!("it is {} KiB, and a paste holds at most {} KiB", text.len().div_ceil(1024), PASTE_LIMIT / 1024));
        }
        if let Some(line) = text.split('\n').map(str::len).max().filter(|&l| l > LINE_LIMIT) {
            return Err(format!(
                "one of its lines is {} KiB, and a line may be at most {} KiB",
                line.div_ceil(1024),
                LINE_LIMIT / 1024
            ));
        }
        let snap = self.snapshot();
        let changed = self.area.insert_str(text);
        if changed {
            self.record(snap);
        }
        Ok(changed)
    }

    // ---- the mouse ---------------------------------------------------------
    //
    // Positions are in cells from the top left of the area the text was last
    // drawn in. The widget records how far it was scrolled when it draws (its
    // `scroll_offset`), and maps a display position back to the character
    // drawn there (`screen_to_data`: wide characters, tabs and soft wraps
    // included), so a click is mapped through the last frame's offset.

    /// The character position drawn at (`col`, `row`) of the area.
    fn at(&self, col: i32, row: i32) -> (u16, u16) {
        let (top, left) = self.area.scroll_offset();
        let row = (row.max(0) as usize).saturating_add(top as usize);
        let col = (col.max(0) as usize).saturating_add(left as usize);
        let at = self.area.screen_to_data(row, col);
        (at.0.min(u16::MAX as usize) as u16, at.1.min(u16::MAX as usize) as u16)
    }

    fn state(&self) -> MouseState {
        let c = self.area.cursor();
        ((c.0, c.1), self.area.selection_range(), self.area.scroll_offset())
    }

    /// The first display line and column drawn, as of the last frame.
    pub fn scroll_top(&self) -> (usize, usize) {
        let (row, col) = self.area.scroll_offset();
        (row as usize, col as usize)
    }

    /// The line of the text drawn at row `row` of the area.
    pub fn row_at(&self, col: i32, row: i32) -> usize {
        self.at(col, row).0 as usize
    }

    /// A press: the caret goes to the clicked position, and any selection ends.
    /// Returns whether anything visible changed.
    pub fn click(&mut self, col: i32, row: i32) -> bool {
        let before = self.state();
        self.area.cancel_selection();
        let (r, c) = self.at(col, row);
        self.area.move_cursor(CursorMove::Jump(r, c));
        before != self.state()
    }

    /// A drag with the button down: the selection runs from where the press
    /// put the caret to the pointer (clamped to the text; past the area's edge
    /// the view follows the caret).
    pub fn drag(&mut self, col: i32, row: i32) -> bool {
        let before = self.state();
        if !self.area.is_selecting() {
            self.area.start_selection();
        }
        let (r, c) = self.at(col, row);
        self.area.move_cursor(CursorMove::Jump(r, c));
        before != self.state()
    }

    /// The button is up: a selection that selects nothing is not one.
    pub fn end_drag(&mut self) -> bool {
        if self.area.is_selecting() && self.selection().is_none() {
            self.area.cancel_selection();
            return true;
        }
        false
    }

    /// A double-click: selects the word (or the run of spaces, or of symbols)
    /// at the position, with the caret at its end.
    pub fn select_word(&mut self, col: i32, row: i32) -> bool {
        let before = self.state();
        self.area.cancel_selection();
        let (r, c) = self.at(col, row);
        self.area.move_cursor(CursorMove::Jump(r, c));
        let chars: Vec<char> = self.area.lines().get(r as usize).map(|l| l.chars().collect()).unwrap_or_default();
        if !chars.is_empty() {
            let at = (c as usize).min(chars.len() - 1);
            let class = |ch: char| if ch.is_alphanumeric() || ch == '_' { 0 } else if ch.is_whitespace() { 1 } else { 2 };
            let k = class(chars[at]);
            let (mut from, mut to) = (at, at + 1);
            while from > 0 && class(chars[from - 1]) == k {
                from -= 1;
            }
            while to < chars.len() && class(chars[to]) == k {
                to += 1;
            }
            self.area.move_cursor(CursorMove::Jump(r, from as u16));
            self.area.start_selection();
            self.area.move_cursor(CursorMove::Jump(r, to as u16));
        }
        before != self.state()
    }

    /// The wheel: the view moves `rows` and `cols`; the caret moves only as far
    /// as it must to stay in it. Returns whether anything changed.
    pub fn scroll(&mut self, rows: i16, cols: i16) -> bool {
        let before = self.state();
        self.area.scroll((rows, cols));
        before != self.state()
    }

    /// Lets the widget work out where its view is for `area`: it follows the
    /// caret while it draws, and records the scroll offset the mouse mapping
    /// (`at`) and our own painter read. The widget draws into a scratch
    /// buffer that is thrown away: the screen is painted by `view` from
    /// `lines`, `cursor`, `selection` and `scroll_top`.
    pub fn layout(&mut self, area: Rect) {
        let mut scratch = Buffer::empty(area);
        Widget::render(&self.area, area, &mut scratch);
    }
}

/// Whether the widget is given `key`: text and the plain movement and editing
/// keys. A key with Cmd, or with Ctrl or Alt alone, is the client's or nobody's
/// (the widget's Emacs bindings are not ours); Ctrl and Alt together is what
/// Windows terminals send for a character typed with AltGr.
fn widget_takes(key: &KeyEvent) -> bool {
    let m = key.modifiers;
    if m.contains(KeyModifiers::SUPER) || m.contains(KeyModifiers::HYPER) || m.contains(KeyModifiers::META) {
        return false;
    }
    let ctrl_alt = m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::ALT);
    if (m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT)) && !(ctrl_alt && matches!(key.code, KeyCode::Char(_))) {
        return false;
    }
    matches!(
        key.code,
        KeyCode::Char(_)
            | KeyCode::Enter
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Tab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
    )
}

/// Pasted text as the buffer may hold it: `\r\n` and lone `\r` become `\n`
/// (terminals send either), tabs stay, other control characters become `�`
/// and bidirectional overrides are dropped, as `display_safe` does for what
/// is drawn, because the widget draws the buffer as it is.
fn clean_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{fffd}' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn at(row: usize, col: usize) -> Cursor {
        Cursor { row, col }
    }

    #[test]
    fn a_replacement_is_one_undo_step_and_one_redo_step() {
        let mut e = Editor::new();
        e.set_text("select * from f");
        e.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(e.text(), "select * from fa");
        assert_eq!(e.replace(at(0, 14), at(0, 16), "film_actor"), Ok(true));
        assert_eq!(e.text(), "select * from film_actor");
        e.undo();
        assert_eq!(e.text(), "select * from fa", "one undo gives back the text before");
        e.redo();
        assert_eq!(e.text(), "select * from film_actor", "one redo gives the text after");
        e.undo();
        e.undo();
        assert_eq!(e.text(), "select * from f", "the next undo is the step before the replacement");
        e.redo();
        e.redo();
        assert_eq!(e.text(), "select * from film_actor");
    }

    #[test]
    fn a_replacement_that_changes_nothing_leaves_no_history() {
        let mut e = Editor::new();
        e.set_text("select");
        e.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(e.replace(at(0, 0), at(0, 6), "select"), Ok(false));
        e.undo();
        assert_eq!(e.text(), "select", "the one undo was the typed character");
    }

    #[test]
    fn the_widget_is_given_text_and_plain_keys_only() {
        let key = |code, mods| KeyEvent::new(code, mods);
        for taken in [
            key(KeyCode::Char('a'), KeyModifiers::NONE),
            key(KeyCode::Char('A'), KeyModifiers::SHIFT),
            key(KeyCode::Left, KeyModifiers::SHIFT),
            key(KeyCode::Enter, KeyModifiers::NONE),
            key(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT),
        ] {
            assert!(widget_takes(&taken), "{taken:?}");
        }
        for refused in [
            key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            key(KeyCode::Char('d'), KeyModifiers::SUPER),
            key(KeyCode::Char('f'), KeyModifiers::ALT),
            key(KeyCode::Left, KeyModifiers::CONTROL),
            key(KeyCode::F(5), KeyModifiers::NONE),
            key(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            assert!(!widget_takes(&refused), "{refused:?}");
        }
    }

    #[test]
    fn an_edit_that_changes_several_places_is_one_undo_step() {
        let mut e = Editor::new();
        e.set_text("a\nb\nc");
        let mut doc = e.doc();
        doc.rows.swap(0, 2);
        doc.rows[1].push('!');
        assert_eq!(e.apply(&doc), Ok(true));
        assert_eq!(e.text(), "c\nb!\na");
        assert!(e.undo());
        assert_eq!(e.text(), "a\nb\nc");
        assert!(!e.undo(), "nothing before it");
        assert!(e.redo());
        assert_eq!(e.text(), "c\nb!\na");
    }

    #[test]
    fn a_replacement_on_a_row_the_widget_cannot_address_is_refused_and_edits_nothing() {
        let mut e = Editor::new();
        e.set_text(&"\n".repeat(70_000));
        let before = e.text();
        let revision = e.revision();
        assert!(e.replace(at(65_536, 0), at(65_536, 0), "x").is_err());
        assert!(e.text() == before, "a row above u16::MAX was edited");
        assert_eq!(e.revision(), revision);
    }
}
