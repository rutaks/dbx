//! The cell viewer: the whole value of one cell, wrapped by display width and
//! scrollable, with JSON pretty-printed for reading and the raw text one key
//! away. The state lives in `Overlay::CellViewer`; drawing is in `view`.
//!
//! The viewer holds a copy of the value taken when it opened, so a result that
//! is replaced behind it (a "load more" landing) cannot change what is shown,
//! and `row N` in its title stays true. The copy is one memcpy of the cell.
//!
//! **Lazy wrapping.** Only the lines on screen are ever wrapped: finding line
//! N walks forward from the nearest checkpoint (the byte offset of every
//! `STEP`th wrapped line, found on the way), so a 10 MB value draws as fast as
//! a short one and scrolling costs what it moves. Jumping to the end (`G`) is
//! the one thing that must wrap everything, once: wrapping by display width
//! cannot be done backwards.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::core::{Cell, CellKind, ResultSet};

/// A checkpoint every this many wrapped lines.
const STEP: usize = 256;
/// Pretty-printing re-reads the whole value, so it is skipped above this size
/// (the raw text is shown, and the viewer says why).
pub const PRETTY_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CellViewer {
    pub row: usize,
    pub col: usize,
    pub column: String,
    pub db_type: String,
    /// The exact stored value; `None` is NULL, or a cell the row lacks.
    pub value: Option<String>,
    /// The stored value's length in characters, counted once on opening.
    pub chars: usize,
    /// The row is shorter than the header: there is no cell, not even a NULL.
    pub missing: bool,
    pub kind: CellKind,
    /// JSON, pretty-printed (only for a JSON cell that parses and is small).
    pub pretty: Option<String>,
    /// Why a JSON cell is shown raw, when it is.
    pub note: Option<&'static str>,
    /// Showing the stored text rather than the pretty-printed form.
    pub raw: bool,
    /// The first wrapped line shown.
    pub scroll: usize,
    /// The text area as last drawn.
    pub width: usize,
    pub page: usize,
    wrap: Wrapper,
}

impl CellViewer {
    /// The viewer for the cell at (`row`, `col`) of `set`, or `None` where
    /// there is no such column. A cell the row does not have (a row shorter
    /// than the header) opens as `missing`, which is not NULL.
    pub fn open(set: &ResultSet, row: usize, col: usize) -> Option<Self> {
        let column = set.columns.get(col)?;
        let cell = set.rows.get(row)?.get(col);
        let missing = cell.is_none();
        let value = match cell {
            Some(Cell::Value(v)) => Some(v.clone()),
            _ => None,
        };
        let (mut pretty, mut note) = (None, None);
        if column.kind == CellKind::Json {
            if let Some(v) = &value {
                match pretty_json(v) {
                    Some(p) => pretty = Some(p),
                    None if v.len() > PRETTY_LIMIT => note = Some("too large to pretty-print: showing the stored text"),
                    None => note = Some("not valid JSON: showing the stored text"),
                }
            }
        }
        Some(CellViewer {
            row,
            col,
            column: column.name.clone(),
            db_type: column.db_type.clone(),
            chars: value.as_deref().map_or(0, |v| v.chars().count()),
            value,
            missing,
            kind: column.kind,
            pretty,
            note,
            raw: false,
            scroll: 0,
            width: 80,
            page: 10,
            wrap: Wrapper::default(),
        })
    }

    /// Whether there is a pretty form to switch to.
    pub fn can_toggle(&self) -> bool {
        self.pretty.is_some()
    }

    /// The text the viewer shows now (empty for NULL, which is drawn as such).
    pub fn text(&self) -> &str {
        match (&self.pretty, self.raw) {
            (Some(pretty), false) => pretty,
            _ => self.value.as_deref().unwrap_or(""),
        }
    }

    /// Tells the viewer how big its text area was drawn: the width wrapping
    /// is done at, and the rows a page scrolls by.
    pub fn set_area(&mut self, width: usize, page: usize) {
        self.width = width.max(1);
        self.page = page.max(1);
    }

    /// The stored value's length in bytes.
    pub fn bytes(&self) -> usize {
        self.value.as_deref().map_or(0, str::len)
    }

    /// Wrapped lines `from..from + count` of what is shown. Only these lines
    /// are wrapped. A line carries what a plain drawing would lose: how it
    /// ended, and its trailing spaces and tabs.
    pub fn lines(&mut self, from: usize, count: usize) -> Vec<ViewLine> {
        let text = shown(&self.pretty, self.raw, &self.value);
        self.wrap.sync(self.width);
        self.wrap.extend(text, from);
        let Some(&checkpoint) = self.wrap.checkpoints.get(from / STEP) else { return Vec::new() };
        let width = self.wrap.width;
        let mut at = checkpoint;
        for _ in 0..from % STEP {
            if at >= text.len() {
                return Vec::new();
            }
            at = next_line(text, at, width).1;
        }
        let mut out = Vec::new();
        while out.len() < count && at < text.len() {
            let (end, next) = next_line(text, at, width);
            out.push(ViewLine::new(&text[at..end], &text[end..next], end == next && next < text.len()));
            at = next;
        }
        out
    }

    /// The number of wrapped lines, once the wrapping has reached the end.
    pub fn total(&self) -> Option<usize> {
        self.wrap.total
    }

    /// Scrolls by `delta` lines, never past the point where the last line is
    /// at the bottom (found by wrapping as far as the page below needs).
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.clamp();
    }

    fn clamp(&mut self) {
        let text = shown(&self.pretty, self.raw, &self.value);
        self.wrap.sync(self.width);
        self.wrap.extend(text, self.scroll.saturating_add(self.page));
        if let Some(total) = self.wrap.total {
            self.scroll = self.scroll.min(total.saturating_sub(self.page));
        }
    }

    /// Scrolls to the last page. Wraps the whole value once to find it.
    pub fn scroll_to_end(&mut self) {
        let text = shown(&self.pretty, self.raw, &self.value);
        self.wrap.sync(self.width);
        self.wrap.extend(text, usize::MAX / 2);
        self.scroll = self.wrap.total.unwrap_or(0).saturating_sub(self.page);
    }

    /// Pretty-printed or stored text (JSON cells only).
    pub fn toggle_raw(&mut self) {
        if self.can_toggle() {
            self.raw = !self.raw;
            self.scroll = 0;
            self.wrap = Wrapper::default();
        }
    }
}

/// One wrapped line, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLine {
    /// The text up to its trailing spaces and tabs, tabs as their glyph.
    pub body: String,
    /// Those trailing spaces and tabs, as visible glyphs. Only a line that
    /// ends where the text says so has any: a soft wrap is not an end.
    pub trailing: String,
    /// The glyph for how the line ended (`↵` LF, `←` CR, `←↵` CRLF), or empty
    /// at a soft wrap and at the end of the text.
    pub end: &'static str,
}

impl ViewLine {
    /// `content` is the line's text, `terminator` what follows it in the
    /// value, and `soft` is true when the line was cut by the width.
    fn new(content: &str, terminator: &str, soft: bool) -> Self {
        let end = match terminator {
            "\n" => "↵",
            "\r\n" => "←↵",
            "\r" => "←",
            _ => "",
        };
        let body_end = if soft { content.len() } else { content.trim_end_matches([' ', '\t']).len() };
        let glyph = |c: char| if c == ' ' { "·" } else { crate::app::view::TAB_MARK };
        ViewLine {
            body: content[..body_end].replace('\t', crate::app::view::TAB_MARK),
            trailing: content[body_end..].chars().map(glyph).collect(),
            end,
        }
    }
}

fn shown<'a>(pretty: &'a Option<String>, raw: bool, value: &'a Option<String>) -> &'a str {
    match (pretty, raw) {
        (Some(pretty), false) => pretty,
        _ => value.as_deref().unwrap_or(""),
    }
}

/// Checkpoints of the wrapped lines of one text at one width.
#[derive(Debug, Clone, Default)]
struct Wrapper {
    width: usize,
    /// Byte offsets of wrapped lines 0, STEP, 2 STEP, ...
    checkpoints: Vec<usize>,
    total: Option<usize>,
}

impl Wrapper {
    /// Forgets the checkpoints if the width changed.
    fn sync(&mut self, width: usize) {
        if self.width != width {
            *self = Wrapper { width, ..Wrapper::default() };
        }
    }

    /// Finds checkpoints as far as line `upto` (or the end of the text).
    fn extend(&mut self, text: &str, upto: usize) {
        if self.checkpoints.is_empty() {
            self.checkpoints.push(0);
        }
        while self.total.is_none() && (self.checkpoints.len() - 1) * STEP <= upto {
            let index = self.checkpoints.len() - 1;
            let (mut at, mut walked) = (self.checkpoints[index], 0);
            while walked < STEP && at < text.len() {
                at = next_line(text, at, self.width).1;
                walked += 1;
            }
            if at >= text.len() {
                self.total = Some(index * STEP + walked);
            } else {
                self.checkpoints.push(at);
            }
        }
    }
}

/// `text` as JSON re-indented with two spaces, or `None` if it is not JSON or
/// too large. Only whitespace outside strings changes: every literal, number
/// and key keeps its exact text and order (parsing and printing it again would
/// reorder keys and round big numbers). Validated as it goes, by the JSON
/// grammar, with its own number check, so `1e400` (which Postgres's `jsonb`
/// holds and a float cannot) is still JSON.
pub fn pretty_json(text: &str) -> Option<String> {
    if text.len() > PRETTY_LIMIT {
        return None;
    }
    let mut p = Pretty { src: text, b: text.as_bytes(), at: 0, out: String::with_capacity(text.len() * 2) };
    p.value(0)?;
    p.space();
    (p.at == p.b.len()).then_some(p.out)
}

/// The deepest nesting pretty-printed (recursion must not outrun the stack).
const MAX_DEPTH: usize = 256;

struct Pretty<'a> {
    src: &'a str,
    b: &'a [u8],
    at: usize,
    out: String,
}

impl Pretty<'_> {
    fn space(&mut self) {
        while matches!(self.b.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn newline(&mut self, depth: usize) {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("  ");
        }
    }

    fn value(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.space();
        match *self.b.get(self.at)? {
            b'{' => self.container(depth, b'{', b'}'),
            b'[' => self.container(depth, b'[', b']'),
            b'"' => self.string(),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn literal(&mut self, word: &str) -> Option<()> {
        if !self.src[self.at..].starts_with(word) {
            return None;
        }
        self.out.push_str(word);
        self.at += word.len();
        Some(())
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, copied as written.
    fn number(&mut self) -> Option<()> {
        let start = self.at;
        let digits = |p: &mut Self| {
            let from = p.at;
            while p.b.get(p.at).is_some_and(u8::is_ascii_digit) {
                p.at += 1;
            }
            p.at > from
        };
        if self.b.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.b.get(self.at)? {
            b'0' => self.at += 1,
            b'1'..=b'9' => {
                digits(self);
            }
            _ => return None,
        }
        if self.b.get(self.at) == Some(&b'.') {
            self.at += 1;
            if !digits(self) {
                return None;
            }
        }
        if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.b.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !digits(self) {
                return None;
            }
        }
        self.out.push_str(&self.src[start..self.at]);
        Some(())
    }

    fn string(&mut self) -> Option<()> {
        let start = self.at;
        self.at += 1;
        loop {
            match *self.b.get(self.at)? {
                b'"' => break,
                b'\\' => {
                    self.at += 1;
                    match *self.b.get(self.at)? {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            for k in 1..=4 {
                                if !self.b.get(self.at + k)?.is_ascii_hexdigit() {
                                    return None;
                                }
                            }
                            self.at += 4;
                        }
                        _ => return None,
                    }
                }
                0..=0x1f => return None,
                _ => {}
            }
            self.at += 1;
        }
        self.at += 1;
        self.out.push_str(&self.src[start..self.at]);
        Some(())
    }

    fn container(&mut self, depth: usize, open: u8, close: u8) -> Option<()> {
        self.at += 1;
        self.space();
        if self.b.get(self.at) == Some(&close) {
            self.at += 1;
            self.out.push(open as char);
            self.out.push(close as char);
            return Some(());
        }
        self.out.push(open as char);
        loop {
            self.newline(depth + 1);
            if open == b'{' {
                self.space();
                if self.b.get(self.at) != Some(&b'"') {
                    return None;
                }
                self.string()?;
                self.space();
                if self.b.get(self.at) != Some(&b':') {
                    return None;
                }
                self.at += 1;
                self.out.push_str(": ");
            }
            self.value(depth + 1)?;
            self.space();
            match self.b.get(self.at)? {
                b',' => {
                    self.at += 1;
                    self.out.push(',');
                }
                c if *c == close => {
                    self.at += 1;
                    self.newline(depth);
                    self.out.push(close as char);
                    return Some(());
                }
                _ => return None,
            }
        }
    }
}

/// One wrapped line of `text` from byte `start` at `width` cells: the end of
/// its content and where the next line starts. A line ends at a line break,
/// or before the grapheme that would pass `width` (a wide character that does
/// not fit starts the next line, and one wider than the width stands alone).
pub fn next_line(text: &str, start: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let (mut used, mut at) = (0usize, start);
    for g in text[start..].graphemes(true) {
        if matches!(g, "\n" | "\r\n" | "\r") {
            return (at, at + g.len());
        }
        // A tab is drawn as a one-cell glyph and a control character as `�`.
        let w = if g == "\t" || g.chars().next().is_some_and(char::is_control) { 1 } else { UnicodeWidthStr::width(g) };
        if used + w > width && used > 0 {
            return (at, at);
        }
        used += w;
        at += g.len();
    }
    (at, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_json_reindents_and_changes_nothing_else() {
        assert_eq!(pretty_json(r#"{"a":1,"b":[1,2],"c":{"d":null},"e":[],"f":{}}"#).unwrap(), "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ],\n  \"c\": {\n    \"d\": null\n  },\n  \"e\": [],\n  \"f\": {}\n}");
        // Key order, big and odd numbers, escapes and braces inside strings survive as written.
        let src = r#"{"z":12345678901234567890.123456789,"a":1e400,"s":"x\"}{,:\\","t":-0.0}"#;
        let out = pretty_json(src).unwrap();
        assert!(out.find("\"z\"").unwrap() < out.find("\"a\"").unwrap(), "{out}");
        for lit in ["12345678901234567890.123456789", "1e400", r#""x\"}{,:\\""#, "-0.0"] {
            assert!(out.contains(lit), "{lit} in {out}");
        }
        let squeezed: String = out.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(squeezed, src.chars().filter(|c| !c.is_whitespace()).collect::<String>(), "only whitespace changed");
        assert_eq!(pretty_json("[1, 2]").unwrap(), "[\n  1,\n  2\n]");
        assert_eq!(pretty_json("  7 ").unwrap(), "7");
    }

    #[test]
    fn pretty_json_refuses_what_is_not_json_or_is_too_large() {
        assert_eq!(pretty_json("{\"a\": }"), None);
        assert_eq!(pretty_json("plain text"), None);
        assert_eq!(pretty_json(""), None);
        let big = format!("[{}0]", "0,".repeat(PRETTY_LIMIT));
        assert_eq!(pretty_json(&big), None, "over the limit");
    }

    fn all(text: &str, width: usize) -> Vec<String> {
        let (mut out, mut at) = (Vec::new(), 0);
        while at < text.len() {
            let (end, next) = next_line(text, at, width);
            out.push(text[at..end].to_string());
            at = next;
        }
        out
    }

    #[test]
    fn lines_break_at_line_breaks_and_at_the_width_by_display_cells() {
        assert_eq!(all("ab\ncd\r\nef", 10), vec!["ab", "cd", "ef"]);
        assert_eq!(all("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(all("a\n\nb", 10), vec!["a", "", "b"], "a blank line is a line");
        // `日` is two cells: three fit in 7, and the fourth starts the next line.
        assert_eq!(all("日日日日", 7), vec!["日日日", "日"]);
        assert_eq!(all("日日日日", 1), vec!["日", "日", "日", "日"], "wider than the width: alone, never looping");
        // A combining mark stays with its letter.
        assert_eq!(all("e\u{301}e\u{301}e\u{301}", 2), vec!["e\u{301}e\u{301}", "e\u{301}"]);
    }

    #[test]
    fn wrapping_loses_no_text() {
        for text in ["", "one line", "a\nb\n\nc\n", "日本語 😀 e\u{301} tab\there", &"xyz ".repeat(300)] {
            for width in [1, 2, 3, 7, 40] {
                let (mut rebuilt, mut at) = (String::new(), 0);
                while at < text.len() {
                    let (end, next) = next_line(text, at, width);
                    assert!(next > at, "progress at {at} in {text:?}");
                    rebuilt.push_str(&text[at..end]);
                    at = next;
                }
                let kept: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                assert_eq!(rebuilt, kept, "width {width}");
            }
        }
    }
}
