//! The SQL editor: our wrapper around `ratatui-textarea`'s `TextArea`.
//!
//! `update` reaches the widget only through this type: the text, the cursor
//! and the selection in our own `Cursor`, keys, paste and setting the text.
//! Drawing is the widget's own renderer for now, styled from palette roles
//! (`style`); slice 10 replaces it with ours (syntax colouring, the statement
//! about to run outlined) without touching anything that calls this.

use crossterm::event::KeyEvent;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use ratatui_textarea::TextArea;

use super::theme::{Palette, Role};
use crate::core::Cursor;

/// The most a paste may hold, in bytes. The widget rebuilds its screen map on
/// every edit and walks each line on every frame, so a huge paste would make
/// every key slow; a 1 MB line stalled the client for over a minute.
pub const PASTE_LIMIT: usize = 256 * 1024;
/// The longest line a paste may hold, in bytes.
pub const LINE_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct Editor {
    area: TextArea<'static>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor { area: TextArea::default() }
    }

    /// The whole buffer, rows joined with `\n`.
    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }

    /// Replaces the buffer and puts the cursor at its end.
    pub fn set_text(&mut self, text: &str) {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.area = TextArea::new(lines);
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

    /// A key for the widget (typing, moving, selecting, undo). The caller has
    /// already taken the keys that are the client's (the prefix, run, ctrl+c).
    /// Returns whether anything visible may have changed.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.area.input(key);
        true
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
        Ok(self.area.insert_str(text))
    }

    /// Styles the widget from palette roles: the text, the cursor (reversed),
    /// the selection on `selection_bg`, the placeholder in `overlay0`.
    pub fn style(&mut self, theme: &Palette, focused: bool) {
        let text = Style::default().fg(theme.color(Role::Text));
        self.area.set_style(text);
        self.area.set_cursor_line_style(Style::default());
        self.area.set_selection_style(Style::default().bg(theme.color(Role::SelectionBg)));
        // An unfocused editor shows no cursor.
        self.area.set_cursor_style(if focused { text.add_modifier(Modifier::REVERSED) } else { text });
        self.area.set_placeholder_style(Style::default().fg(theme.color(Role::Overlay0)));
    }

    /// What the editor shows while it is empty.
    pub fn set_placeholder(&mut self, text: &str) {
        if self.area.placeholder_text() != text {
            self.area.set_placeholder_text(text);
        }
    }

    pub fn widget(&self) -> impl Widget + '_ {
        &self.area
    }
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
