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
//! a short one and scrolling costs what it moves.
//!
//! **Jumping to the end (`G`)** cannot wrap everything: wrapping by display
//! width cannot be done backwards, and wrapping 10 MB took over a second. The
//! viewer anchors the tail instead: it wraps from a point about a page of
//! bytes before the end (the start of the line there when one is near, else a
//! grapheme boundary, so that a single huge line is cut where its wrapping
//! may differ from the full wrap's) and counts lines from there. The count of
//! lines before the anchor is not known, so the position reads `end`. Scrolling
//! up past the top of the tail wraps twice as much of the tail, and so on
//! until the anchor is the start of the text. `g` goes back to the top.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::core::{Cell, CellKind, ResultSet};

/// A checkpoint every this many wrapped lines.
const STEP: usize = 256;
/// A value this small is wrapped whole to find its end (a few milliseconds),
/// so it keeps its line numbers; only a larger one is anchored at its tail.
const EXACT_LIMIT: usize = 64 * 1024;
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
    /// How many graphemes wrapping has visited, in all: tests use it to show
    /// that a jump to the end wraps the tail and not the whole value.
    pub work: usize,
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
            work: 0,
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
    /// ended, and its trailing spaces and tabs. `from` counts from the anchor
    /// (the top, unless `G` anchored the tail).
    pub fn lines(&mut self, from: usize, count: usize) -> Vec<ViewLine> {
        self.sync_width();
        let text = shown(&self.pretty, self.raw, &self.value);
        self.wrap.extend(text, from, &mut self.work);
        let Some(&checkpoint) = self.wrap.checkpoints.get(from / STEP) else { return Vec::new() };
        let width = self.wrap.width;
        let mut at = checkpoint;
        for _ in 0..from % STEP {
            if at >= text.len() {
                return Vec::new();
            }
            at = next_line_counted(text, at, width, &mut self.work).1;
        }
        let mut out = Vec::new();
        while out.len() < count && at < text.len() {
            let (end, next) = next_line_counted(text, at, width, &mut self.work);
            out.push(ViewLine::new(&text[at..end], &text[end..next], end == next && next < text.len()));
            at = next;
        }
        out
    }

    /// The number of wrapped lines of the whole value, once the wrapping has
    /// reached the end from the top. `None` while the tail is anchored: the
    /// lines before the anchor were never counted.
    pub fn total(&self) -> Option<usize> {
        self.wrap.total.filter(|_| self.wrap.origin == 0)
    }

    /// Whether the tail is anchored (`G` jumped to the end of a long value).
    pub fn anchored(&self) -> bool {
        self.wrap.origin > 0
    }

    /// Where the `shown` lines drawn from `scroll` are in the value: their
    /// numbers, or `end` (and how far before it) while the tail is anchored
    /// and the line numbers are not known.
    pub fn position(&self, shown: usize) -> String {
        let first = self.scroll + 1;
        if self.anchored() {
            let below = self.wrap.total.map_or(0, |t| t.saturating_sub(self.scroll + shown));
            return if below == 0 { "end".to_string() } else { format!("end -{below}") };
        }
        match self.total() {
            Some(total) => format!("lines {first}-{} of {total}", self.scroll + shown),
            None => format!("lines {first}-{}", self.scroll + shown),
        }
    }

    /// Scrolls by `delta` lines, never past the point where the last line is
    /// at the bottom (found by wrapping as far as the page below needs).
    pub fn scroll_by(&mut self, delta: isize) {
        self.sync_width();
        // One step back doubles the wrapped tail, which may still be shorter
        // than the move: go on until the whole move fits or the origin is the
        // start of the value.
        while delta < 0 && self.wrap.origin > 0 && (self.scroll as isize) + delta < 0 {
            self.extend_back();
        }
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.clamp();
    }

    fn clamp(&mut self) {
        let text = shown(&self.pretty, self.raw, &self.value);
        self.wrap.extend(text, self.scroll.saturating_add(self.page), &mut self.work);
        if let Some(total) = self.wrap.total {
            self.scroll = self.scroll.min(total.saturating_sub(self.page));
        }
    }

    /// A width change forgets the wrapping; a tail anchored at the end is
    /// anchored again at the end (its old anchor means nothing at the new
    /// width), rather than wrapping the whole value to find a line number.
    fn sync_width(&mut self) {
        if self.wrap.width == self.width {
            return;
        }
        let was_tail = self.wrap.origin > 0;
        self.wrap = Wrapper { width: self.width, ..Wrapper::default() };
        if was_tail {
            self.scroll_to_end();
        }
    }

    /// Scrolls to the last page. Wraps the tail only: from about a page of
    /// bytes before the end, doubling that if it holds fewer lines than a
    /// page. Anchored there, the line numbers are unknown and the position
    /// reads `end` (see the module comment).
    pub fn scroll_to_end(&mut self) {
        self.sync_width();
        if self.wrap.total.is_none() {
            let text = shown(&self.pretty, self.raw, &self.value);
            let mut budget = if text.len() <= EXACT_LIMIT { usize::MAX } else { (self.page + 1).saturating_mul(self.width) };
            loop {
                let origin = tail_origin(text, budget);
                let mut wrap = Wrapper { width: self.width, origin, ..Wrapper::default() };
                wrap.extend(text, usize::MAX / 2, &mut self.work);
                let enough = wrap.total.unwrap_or(0) >= self.page;
                if enough || origin == 0 {
                    self.wrap = wrap;
                    break;
                }
                budget = budget.saturating_mul(2);
            }
        }
        self.scroll = self.wrap.total.unwrap_or(0).saturating_sub(self.page);
    }

    /// Back to the top: line 1, with the numbers known again.
    pub fn scroll_to_top(&mut self) {
        if self.wrap.origin > 0 {
            self.wrap = Wrapper { width: self.width, ..Wrapper::default() };
        }
        self.scroll = 0;
    }

    /// The tail's anchor moves back: twice as much of the tail is wrapped,
    /// and the first line shown stays the line that holds the same text.
    fn extend_back(&mut self) {
        let text = shown(&self.pretty, self.raw, &self.value);
        let from = line_start(&self.wrap, text, self.scroll, &mut self.work);
        let span = text.len() - self.wrap.origin;
        let origin = tail_origin(text, span.saturating_mul(2));
        let mut wrap = Wrapper { width: self.width, origin, ..Wrapper::default() };
        wrap.extend(text, usize::MAX / 2, &mut self.work);
        // The line of the new wrapping that holds byte `from`.
        let k = wrap.checkpoints.partition_point(|&c| c <= from).saturating_sub(1);
        let (mut at, mut line) = (wrap.checkpoints[k], k * STEP);
        loop {
            let next = next_line_counted(text, at, wrap.width, &mut self.work).1;
            if next > from || next >= text.len() {
                break;
            }
            at = next;
            line += 1;
        }
        self.wrap = wrap;
        self.scroll = line;
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

/// A way to move the viewer's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    Line(isize),
    /// This many pages (negative: up).
    Page(isize),
    Top,
    Bottom,
}

impl CellViewer {
    /// Moves the window. Returns whether anything visible changed, so that a
    /// key that moves nothing (a scroll at the end) repaints nothing.
    pub fn scroll(&mut self, how: Scroll) -> bool {
        let before = (self.scroll, self.anchored());
        match how {
            Scroll::Line(n) => self.scroll_by(n),
            Scroll::Page(n) => self.scroll_by(n * self.page as isize),
            Scroll::Top => self.scroll_to_top(),
            Scroll::Bottom => self.scroll_to_end(),
        }
        before != (self.scroll, self.anchored())
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

/// Checkpoints of the wrapped lines of one text at one width, counted from
/// `origin` (0, or the anchor of a tail).
#[derive(Debug, Clone, Default)]
struct Wrapper {
    width: usize,
    /// The byte offset line 0 starts at.
    origin: usize,
    /// Byte offsets of wrapped lines 0, STEP, 2 STEP, ... counted from `origin`.
    checkpoints: Vec<usize>,
    total: Option<usize>,
}

impl Wrapper {
    /// Finds checkpoints as far as line `upto` (or the end of the text).
    fn extend(&mut self, text: &str, upto: usize, work: &mut usize) {
        if self.checkpoints.is_empty() {
            self.checkpoints.push(self.origin);
        }
        while self.total.is_none() && (self.checkpoints.len() - 1) * STEP <= upto {
            let index = self.checkpoints.len() - 1;
            let (mut at, mut walked) = (self.checkpoints[index], 0);
            while walked < STEP && at < text.len() {
                at = next_line_counted(text, at, self.width, work).1;
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

/// The byte offset where wrapped line `line` of `wrap` starts (its
/// checkpoints reach it).
fn line_start(wrap: &Wrapper, text: &str, line: usize, work: &mut usize) -> usize {
    let mut at = wrap.checkpoints.get(line / STEP).copied().unwrap_or(0);
    for _ in 0..line % STEP {
        at = next_line_counted(text, at, wrap.width, work).1;
    }
    at
}

/// Where wrapping starts to show the last page, given about `budget` bytes
/// before the end: the start of the line at that point when it begins within
/// another `budget` bytes (so the wrapping is the one a full wrap gives), else
/// a grapheme boundary (a single huge line: its tail wraps from here). 0 when
/// the whole text is within `budget`.
fn tail_origin(text: &str, budget: usize) -> usize {
    use unicode_segmentation::GraphemeCursor;
    if text.len() <= budget {
        return 0;
    }
    let mut c = text.len() - budget;
    while !text.is_char_boundary(c) {
        c += 1;
    }
    let mut cursor = GraphemeCursor::new(c, text.len(), true);
    while !cursor.is_boundary(text, 0).unwrap_or(true) {
        c += 1;
        while !text.is_char_boundary(c) {
            c += 1;
        }
        cursor.set_cursor(c);
    }
    let bytes = text.as_bytes();
    let lo = c.saturating_sub(budget);
    match bytes[lo..c].iter().rposition(|b| *b == b'\n' || *b == b'\r') {
        Some(at) => {
            let p = lo + at;
            if bytes[p] == b'\r' && bytes.get(p + 1) == Some(&b'\n') {
                p + 2
            } else {
                p + 1
            }
        }
        None => c,
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
    next_line_counted(text, start, width, &mut 0)
}

/// `next_line`, adding the graphemes it looked at to `work`.
fn next_line_counted(text: &str, start: usize, width: usize, work: &mut usize) -> (usize, usize) {
    let width = width.max(1);
    let (mut used, mut at) = (0usize, start);
    for g in text[start..].graphemes(true) {
        *work += 1;
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
