//! The editor's actions as pure functions on a document: rows of characters, a
//! caret and an optional selection anchor in, the changed document out. The
//! `Editor` wrapper applies the result as one undo step; `App` supplies what
//! only it knows (the statement range, the highlighter's tokens, settings).
//!
//! Everything counts in characters, as `Cursor` does.

use crate::core::Cursor;

/// A document as the actions see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub rows: Vec<Vec<char>>,
    /// The other end of the selection; the caret is the end that moves.
    pub anchor: Option<Cursor>,
    pub caret: Cursor,
}

fn key(c: Cursor) -> (usize, usize) {
    (c.row, c.col)
}

fn at(row: usize, col: usize) -> Cursor {
    Cursor { row, col }
}

/// A character that is part of a word.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// What kind of run a character belongs to, for word-wise movement.
fn class(c: char) -> u8 {
    if is_word(c) {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

/// The spaces (or the tab) one level of indentation is.
pub fn indent_unit(hard_tabs: bool, width: usize) -> String {
    if hard_tabs {
        "\t".to_string()
    } else {
        " ".repeat(width.max(1))
    }
}

impl Doc {
    pub fn from_lines(lines: &[String], anchor: Option<Cursor>, caret: Cursor) -> Doc {
        let rows: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
        Doc { rows: if rows.is_empty() { vec![Vec::new()] } else { rows }, anchor, caret }
    }

    pub fn lines(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.iter().collect()).collect()
    }

    pub fn text(&self) -> String {
        self.lines().join("\n")
    }

    /// The selection, start before end, when it holds any text.
    pub fn sel(&self) -> Option<(Cursor, Cursor)> {
        let a = self.anchor?;
        if a == self.caret {
            return None;
        }
        Some(if key(a) < key(self.caret) { (a, self.caret) } else { (self.caret, a) })
    }

    fn last_row(&self) -> usize {
        self.rows.len() - 1
    }

    fn len(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, Vec::len)
    }

    fn end(&self) -> Cursor {
        at(self.last_row(), self.len(self.last_row()))
    }

    fn clamp(&self, c: Cursor) -> Cursor {
        let row = c.row.min(self.last_row());
        at(row, c.col.min(self.len(row)))
    }

    /// Moves the caret, extending the selection or ending it.
    pub fn move_caret(&mut self, to: Cursor, select: bool) {
        if select {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = self.clamp(to);
    }

    pub fn text_of(&self, a: Cursor, b: Cursor) -> String {
        let mut out = String::new();
        for row in a.row..=b.row {
            let from = if row == a.row { a.col } else { 0 };
            let to = if row == b.row { b.col } else { self.len(row) };
            out.extend(self.rows[row][from.min(self.len(row))..to.min(self.len(row)).max(from.min(self.len(row)))].iter());
            if row != b.row {
                out.push('\n');
            }
        }
        out
    }

    /// Removes `a..b`; the caret goes to `a` and the selection ends.
    pub fn delete(&mut self, a: Cursor, b: Cursor) {
        if key(a) >= key(b) {
            self.anchor = None;
            self.caret = a;
            return;
        }
        let tail: Vec<char> = self.rows[b.row][b.col.min(self.len(b.row))..].to_vec();
        let keep = a.col.min(self.len(a.row));
        self.rows[a.row].truncate(keep);
        self.rows[a.row].extend(tail);
        self.rows.drain(a.row + 1..=b.row);
        self.anchor = None;
        self.caret = a;
    }

    /// Inserts `text` at `pos` and returns where it ends. Moves nothing else.
    pub fn insert(&mut self, pos: Cursor, text: &str) -> Cursor {
        let pos = self.clamp(pos);
        let mut parts = text.split('\n');
        let first: Vec<char> = parts.next().unwrap_or("").chars().collect();
        let rest: Vec<Vec<char>> = parts.map(|p| p.chars().collect()).collect();
        let tail: Vec<char> = self.rows[pos.row].split_off(pos.col);
        if rest.is_empty() {
            let end = at(pos.row, pos.col + first.len());
            self.rows[pos.row].extend(first);
            self.rows[pos.row].extend(tail);
            return end;
        }
        self.rows[pos.row].extend(first);
        let n = rest.len();
        let last_len = rest[n - 1].len();
        for (i, mut r) in rest.into_iter().enumerate() {
            if i == n - 1 {
                r.extend(tail.iter());
            }
            self.rows.insert(pos.row + 1 + i, r);
        }
        at(pos.row + n, last_len)
    }

    /// Deletes the selection, if any.
    pub fn delete_selection(&mut self) -> bool {
        match self.sel() {
            Some((a, b)) => {
                self.delete(a, b);
                true
            }
            None => false,
        }
    }

    /// The rows an action on lines covers: those the selection touches (not the
    /// last when it ends in column 0), else the caret's.
    pub fn touched_rows(&self) -> (usize, usize) {
        match self.sel() {
            Some((a, b)) => (a.row, if b.col == 0 && b.row > a.row { b.row - 1 } else { b.row }),
            None => (self.caret.row, self.caret.row),
        }
    }

    fn first_non_blank(&self, row: usize) -> usize {
        self.rows[row].iter().position(|c| !c.is_whitespace()).unwrap_or(self.len(row))
    }

    fn leading(&self, row: usize, upto: usize) -> Vec<char> {
        self.rows[row].iter().take(upto).take_while(|c| c.is_whitespace()).copied().collect()
    }

    fn blank(&self, row: usize) -> bool {
        self.rows[row].iter().all(|c| c.is_whitespace())
    }

    /// The character at `c`, if there is one on its row.
    pub fn char_at(&self, c: Cursor) -> Option<char> {
        self.rows.get(c.row)?.get(c.col).copied()
    }

    pub fn char_before(&self, c: Cursor) -> Option<char> {
        self.rows.get(c.row)?.get(c.col.checked_sub(1)?).copied()
    }

    /// Applies `f` to the caret and the anchor.
    fn map_positions(&mut self, f: impl Fn(Cursor) -> Cursor) {
        self.caret = f(self.caret);
        self.anchor = self.anchor.map(&f);
    }
}

// ---- lines --------------------------------------------------------------------------

/// Duplicates the line, or the selected text (the copy follows it, selected).
pub fn duplicate(d: &mut Doc) -> bool {
    if let Some((a, b)) = d.sel() {
        let text = d.text_of(a, b);
        let end = d.insert(b, &text);
        d.anchor = Some(b);
        d.caret = end;
    } else {
        let row = d.caret.row;
        let line = d.rows[row].clone();
        d.rows.insert(row + 1, line);
        d.caret.row += 1;
    }
    true
}

/// Deletes the lines the caret or selection touches; the caret keeps its
/// column on the line that takes their place.
pub fn delete_line(d: &mut Doc) -> bool {
    let (r0, r1) = d.touched_rows();
    let col = d.caret.col;
    d.rows.drain(r0..=r1);
    if d.rows.is_empty() {
        d.rows.push(Vec::new());
    }
    let row = r0.min(d.last_row());
    d.anchor = None;
    d.caret = at(row, col.min(d.len(row)));
    true
}

/// Moves the touched lines one row up or down.
pub fn move_lines(d: &mut Doc, up: bool) -> bool {
    let (r0, r1) = d.touched_rows();
    if (up && r0 == 0) || (!up && r1 == d.last_row()) {
        return false;
    }
    if up {
        let above = d.rows.remove(r0 - 1);
        d.rows.insert(r1, above);
        d.map_positions(|c| at(c.row.saturating_sub(1), c.col));
    } else {
        let below = d.rows.remove(r1 + 1);
        d.rows.insert(r0, below);
        d.map_positions(|c| at(c.row + 1, c.col));
    }
    true
}

/// Where a statement is, as whole lines, and why it may not be moved as them.
fn statement_rows(d: &Doc, (from, to): (Cursor, Cursor)) -> Result<(usize, usize), &'static str> {
    let end_row = if to.col == 0 && to.row > from.row { to.row - 1 } else { to.row };
    let before_ok = d.rows[from.row].iter().take(from.col).all(|c| c.is_whitespace());
    let after_ok = d.rows[end_row].iter().skip(if end_row == to.row { to.col } else { 0 }).all(|c| c.is_whitespace() || *c == ';');
    if before_ok && after_ok {
        Ok((from.row, end_row))
    } else {
        Err("The statement shares its lines with another one: move it by lines (alt+shift+up or down) instead.")
    }
}

/// Swaps the statement the caret is in with the one before or after it,
/// keeping what lies between them (blank lines). `range_at` places the
/// statement a position is in, in the document as it is.
pub fn move_statement(d: &mut Doc, up: bool, range_at: &dyn Fn(Cursor) -> Option<(Cursor, Cursor)>) -> Result<bool, &'static str> {
    let Some(here) = range_at(d.caret) else { return Ok(false) };
    let (c0, c1) = statement_rows(d, here)?;
    // The neighbour: asked for from the nearest line with text beyond this one.
    let neighbour = if up {
        let Some(row) = (0..c0).rev().find(|r| !d.blank(*r)) else { return Ok(false) };
        range_at(at(row, d.len(row)))
    } else {
        let Some(row) = (c1 + 1..d.rows.len()).find(|r| !d.blank(*r)) else { return Ok(false) };
        range_at(at(row, 0))
    };
    let Some(other) = neighbour else { return Ok(false) };
    let (o0, o1) = statement_rows(d, other)?;
    let ((a0, a1), (b0, b1)) = if up { ((o0, o1), (c0, c1)) } else { ((c0, c1), (o0, o1)) };
    if a1 >= b0 {
        return Ok(false);
    }
    let a: Vec<Vec<char>> = d.rows[a0..=a1].to_vec();
    let gap: Vec<Vec<char>> = d.rows[a1 + 1..b0].to_vec();
    let b: Vec<Vec<char>> = d.rows[b0..=b1].to_vec();
    let mut rows: Vec<Vec<char>> = d.rows[..a0].to_vec();
    rows.extend(b);
    rows.extend(gap);
    rows.extend(a);
    rows.extend(d.rows[b1 + 1..].iter().cloned());
    let (alen, glen, blen) = (a1 - a0 + 1, b0 - a1 - 1, b1 - b0 + 1);
    let map = move |c: Cursor| -> Option<Cursor> {
        if (a0..=a1).contains(&c.row) {
            Some(at(c.row - a0 + blen + glen + a0, c.col))
        } else if (b0..=b1).contains(&c.row) {
            Some(at(c.row - b0 + a0, c.col))
        } else if c.row > a1 && c.row < b0 {
            Some(at(c.row - (a1 + 1) + a0 + blen, c.col))
        } else {
            let _ = alen;
            None
        }
    };
    d.rows = rows;
    d.caret = map(d.caret).unwrap_or(d.caret);
    d.anchor = d.anchor.and_then(map);
    Ok(true)
}

/// `-- ` on each selected line (or the caret's), or off when they all have it.
pub fn toggle_line_comment(d: &mut Doc) -> bool {
    let (r0, r1) = d.touched_rows();
    let rows: Vec<usize> = (r0..=r1).filter(|r| !d.blank(*r)).collect();
    if rows.is_empty() {
        return false;
    }
    let starts = |d: &Doc, r: usize| {
        let i = d.first_non_blank(r);
        d.rows[r].get(i) == Some(&'-') && d.rows[r].get(i + 1) == Some(&'-')
    };
    let remove = rows.iter().all(|r| starts(d, *r));
    // Positions move with their row's edit: `(row, col, delta)`.
    let mut edits: Vec<(usize, usize, isize)> = Vec::new();
    for r in rows {
        let i = d.first_non_blank(r);
        if remove {
            let n = if d.rows[r].get(i + 2) == Some(&' ') { 3 } else { 2 };
            d.rows[r].drain(i..i + n);
            edits.push((r, i, -(n as isize)));
        } else {
            for (k, ch) in "-- ".chars().enumerate() {
                d.rows[r].insert(i + k, ch);
            }
            edits.push((r, i, 3));
        }
    }
    let shift = |c: Cursor| {
        let Some(&(_, i, delta)) = edits.iter().find(|(r, _, _)| *r == c.row) else { return c };
        if delta > 0 {
            if c.col > i {
                at(c.row, c.col + delta as usize)
            } else {
                c
            }
        } else {
            let n = (-delta) as usize;
            if c.col >= i + n {
                at(c.row, c.col - n)
            } else if c.col > i {
                at(c.row, i)
            } else {
                c
            }
        }
    };
    d.map_positions(shift);
    true
}

/// `/* */` around the selection (or the caret's line), or off when it is there.
pub fn toggle_block_comment(d: &mut Doc) -> bool {
    if let Some((a, b)) = d.sel() {
        let text = d.text_of(a, b);
        let chars: Vec<char> = text.chars().collect();
        // The selection is itself a comment.
        if chars.len() >= 4 && text.starts_with("/*") && text.ends_with("*/") {
            let mut inner: String = chars[2..chars.len() - 2].iter().collect();
            if inner.starts_with(' ') {
                inner.remove(0);
            }
            if inner.ends_with(' ') {
                inner.pop();
            }
            d.delete(a, b);
            let end = d.insert(a, &inner);
            d.anchor = Some(a);
            d.caret = end;
            return true;
        }
        // The markers stand just outside it.
        let before: String = d.rows[a.row][..a.col].iter().collect();
        let after: String = d.rows[b.row][b.col..].iter().collect();
        let (open, close) = (if before.ends_with("/* ") { 3 } else if before.ends_with("/*") { 2 } else { 0 }, if after.starts_with(" */") { 3 } else if after.starts_with("*/") { 2 } else { 0 });
        if open > 0 && close > 0 {
            // Closing first, so the opening's position stays valid.
            d.delete(b, at(b.row, b.col + close));
            d.delete(at(a.row, a.col - open), a);
            let (na, nb) = (at(a.row, a.col - open), if b.row == a.row { at(b.row, b.col - open) } else { b });
            d.anchor = Some(na);
            d.caret = nb;
            return true;
        }
        d.insert(b, " */");
        d.insert(a, "/* ");
        let nb = if b.row == a.row { at(b.row, b.col + 3) } else { b };
        d.anchor = Some(at(a.row, a.col + 3));
        d.caret = nb;
        return true;
    }
    let row = d.caret.row;
    let first = d.first_non_blank(row);
    let last = d.rows[row].iter().rposition(|c| !c.is_whitespace()).map(|i| i + 1).unwrap_or(first);
    if last <= first {
        return false;
    }
    let content: String = d.rows[row][first..last].iter().collect();
    let col = d.caret.col;
    if content.len() >= 4 && content.starts_with("/*") && content.ends_with("*/") {
        let close = if content.ends_with(" */") { 3 } else { 2 };
        let open = if content.starts_with("/* ") { 3 } else { 2 };
        d.rows[row].drain(last - close..last);
        d.rows[row].drain(first..first + open);
        d.caret.col = if col >= first + open { col - open } else if col > first { first } else { col };
    } else {
        for (k, ch) in " */".chars().enumerate() {
            d.rows[row].insert(last + k, ch);
        }
        for (k, ch) in "/* ".chars().enumerate() {
            d.rows[row].insert(first + k, ch);
        }
        if col > first {
            d.caret.col = col + 3;
        }
    }
    d.anchor = None;
    true
}

/// A line after the caret's, or before it, with its indentation.
pub fn new_line(d: &mut Doc, after: bool) -> bool {
    let row = d.caret.row;
    let indent = d.leading(row, usize::MAX);
    let at_row = if after { row + 1 } else { row };
    let n = indent.len();
    d.rows.insert(at_row, indent);
    d.anchor = None;
    d.caret = at(at_row, n);
    true
}

/// The caret's line and the next, or every selected line, as one.
pub fn join_lines(d: &mut Doc) -> bool {
    let (r0, r1) = match d.sel() {
        Some(_) => d.touched_rows(),
        None => (d.caret.row, d.caret.row + 1),
    };
    if r1 > d.last_row() || r0 >= r1 {
        return false;
    }
    let mut first_join = None;
    let mut line: Vec<char> = d.rows[r0].clone();
    for r in r0 + 1..=r1 {
        while line.last().is_some_and(|c| c.is_whitespace()) {
            line.pop();
        }
        let next: Vec<char> = d.rows[r].iter().skip_while(|c| c.is_whitespace()).copied().collect();
        first_join.get_or_insert(line.len());
        if !next.is_empty() {
            if !line.is_empty() {
                line.push(' ');
            }
            line.extend(next);
        }
    }
    d.rows.splice(r0..=r1, [line]);
    d.anchor = None;
    d.caret = at(r0, first_join.unwrap_or(0));
    true
}

// ---- selection ------------------------------------------------------------------------

/// The state of selection and caret, for walking the selection back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub anchor: Option<Cursor>,
    pub caret: Cursor,
}

pub fn mark(d: &Doc) -> Mark {
    Mark { anchor: d.sel().map(|_| d.anchor.unwrap_or(d.caret)), caret: d.caret }
}

pub fn restore(d: &mut Doc, m: Mark) {
    d.anchor = m.anchor;
    d.caret = d.clamp(m.caret);
}

/// The word the caret is in or at the end of.
fn word_at(d: &Doc, c: Cursor) -> Option<(Cursor, Cursor)> {
    let row = &d.rows[c.row];
    let in_word = |i: usize| row.get(i).is_some_and(|ch| is_word(*ch));
    let probe = if in_word(c.col) { c.col } else if c.col > 0 && in_word(c.col - 1) { c.col - 1 } else { return None };
    let (mut a, mut b) = (probe, probe + 1);
    while a > 0 && in_word(a - 1) {
        a -= 1;
    }
    while in_word(b) {
        b += 1;
    }
    Some((at(c.row, a), at(c.row, b)))
}

/// Selects the next larger thing around the selection: the word, the
/// statement, everything. `statement` is the statement range the caret is in.
pub fn extend_selection(d: &mut Doc, statement: Option<(Cursor, Cursor)>) -> bool {
    let (s0, s1) = d.sel().unwrap_or((d.caret, d.caret));
    let all = (at(0, 0), d.end());
    let contains = |(a, b): (Cursor, Cursor)| key(a) <= key(s0) && key(s1) <= key(b) && (key(a), key(b)) != (key(s0), key(s1));
    let word = if d.sel().is_none() { word_at(d, d.caret) } else { None };
    let Some((a, b)) = [word, statement, Some(all)].into_iter().flatten().find(|c| contains(*c)) else { return false };
    d.anchor = Some(a);
    d.caret = b;
    true
}

pub fn select_all(d: &mut Doc) {
    d.anchor = Some(at(0, 0));
    d.caret = d.end();
}

// ---- case, indentation ---------------------------------------------------------------------

/// Upper case when any letter of the selection (or the word at the caret) is
/// lower, else lower case.
pub fn toggle_case(d: &mut Doc) -> bool {
    let (a, b) = match d.sel() {
        Some(r) => r,
        None => match word_at(d, d.caret) {
            Some(r) => r,
            None => return false,
        },
    };
    let text = d.text_of(a, b);
    let to = if text.chars().any(char::is_lowercase) { text.to_uppercase() } else { text.to_lowercase() };
    if to == text || to.chars().count() != text.chars().count() {
        return false;
    }
    let (anchor, caret) = (d.anchor, d.caret);
    d.delete(a, b);
    d.insert(a, &to);
    d.anchor = anchor;
    d.caret = caret;
    true
}

/// Tab: a selection over several lines indents them; otherwise indentation
/// goes in at the caret, to the next stop.
pub fn indent(d: &mut Doc, hard_tabs: bool, width: usize) -> bool {
    let unit = indent_unit(hard_tabs, width);
    let multi = d.sel().is_some_and(|(a, b)| a.row != b.row && !(b.col == 0 && b.row == a.row + 1));
    if multi {
        let (r0, r1) = d.touched_rows();
        for r in r0..=r1 {
            if !d.rows[r].is_empty() {
                for (k, ch) in unit.chars().enumerate() {
                    d.rows[r].insert(k, ch);
                }
            }
        }
        let n = unit.chars().count();
        let filled: Vec<bool> = d.rows.iter().map(|r| !r.is_empty()).collect();
        d.map_positions(|c| if (r0..=r1).contains(&c.row) && filled[c.row] { at(c.row, c.col + n) } else { c });
        return true;
    }
    d.delete_selection();
    let col = d.caret.col;
    let text = if hard_tabs { "\t".to_string() } else { " ".repeat(width.max(1) - col % width.max(1)) };
    let end = d.insert(d.caret, &text);
    d.caret = end;
    true
}

/// Shift+tab: takes one level of indentation off each line touched.
pub fn unindent(d: &mut Doc, width: usize) -> bool {
    let (r0, r1) = d.touched_rows();
    let mut removed = vec![0usize; r1 - r0 + 1];
    for r in r0..=r1 {
        let row = &mut d.rows[r];
        let n = if row.first() == Some(&'\t') { 1 } else { row.iter().take(width.max(1)).take_while(|c| **c == ' ').count() };
        row.drain(..n);
        removed[r - r0] = n;
    }
    if removed.iter().all(|n| *n == 0) {
        return false;
    }
    d.map_positions(|c| if (r0..=r1).contains(&c.row) { at(c.row, c.col.saturating_sub(removed[c.row - r0])) } else { c });
    true
}

// ---- movement ----------------------------------------------------------------------------------

pub fn word_left(d: &Doc, from: Cursor) -> Cursor {
    let mut c = from;
    loop {
        if c.col == 0 {
            if c.row == 0 {
                return c;
            }
            c = at(c.row - 1, d.len(c.row - 1));
        } else if d.rows[c.row][c.col - 1].is_whitespace() {
            c.col -= 1;
        } else {
            break;
        }
    }
    let k = class(d.rows[c.row][c.col - 1]);
    while c.col > 0 && class(d.rows[c.row][c.col - 1]) == k {
        c.col -= 1;
    }
    c
}

pub fn word_right(d: &Doc, from: Cursor) -> Cursor {
    let mut c = from;
    loop {
        if c.col >= d.len(c.row) {
            if c.row == d.last_row() {
                return c;
            }
            c = at(c.row + 1, 0);
        } else if d.rows[c.row][c.col].is_whitespace() {
            c.col += 1;
        } else {
            break;
        }
    }
    let k = class(d.rows[c.row][c.col]);
    while c.col < d.len(c.row) && class(d.rows[c.row][c.col]) == k {
        c.col += 1;
    }
    c
}

pub fn word_move(d: &mut Doc, right: bool, select: bool) -> bool {
    let to = if right { word_right(d, d.caret) } else { word_left(d, d.caret) };
    let before = (d.caret, d.anchor);
    d.move_caret(to, select);
    before != (d.caret, d.anchor)
}

/// Backspace and delete by word. At the edge of a line they join it, as the
/// plain keys do.
pub fn delete_word(d: &mut Doc, forward: bool) -> bool {
    if d.delete_selection() {
        return true;
    }
    let c = d.caret;
    let (a, b) = if forward {
        if c.col >= d.len(c.row) {
            if c.row == d.last_row() {
                return false;
            }
            (c, at(c.row + 1, 0))
        } else {
            (c, word_right(d, c))
        }
    } else if c.col == 0 {
        if c.row == 0 {
            return false;
        }
        (at(c.row - 1, d.len(c.row - 1)), c)
    } else {
        (word_left(d, c), c)
    };
    d.delete(a, b);
    true
}

/// `home`: the first non-blank character, then column 0 (or always column 0).
pub fn line_start(d: &mut Doc, select: bool, smart: bool) -> bool {
    let first = d.first_non_blank(d.caret.row);
    let col = if smart && d.caret.col != first { first } else { 0 };
    let before = (d.caret, d.anchor);
    d.move_caret(at(d.caret.row, col), select);
    before != (d.caret, d.anchor)
}

pub fn line_end(d: &mut Doc, select: bool) -> bool {
    let before = (d.caret, d.anchor);
    d.move_caret(at(d.caret.row, d.len(d.caret.row)), select);
    before != (d.caret, d.anchor)
}

pub fn text_edge(d: &mut Doc, end: bool, select: bool) -> bool {
    let before = (d.caret, d.anchor);
    d.move_caret(if end { d.end() } else { at(0, 0) }, select);
    before != (d.caret, d.anchor)
}

// ---- smart keys -----------------------------------------------------------------------------------

fn closer_of(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '\'' => Some('\''),
        '"' => Some('"'),
        _ => None,
    }
}

/// Whether the row, left of the caret, has an opener the closer `c` would
/// close (quotes: an odd number of them, so the caret is inside a string).
fn closes_something(d: &Doc, c: char) -> bool {
    let row = &d.rows[d.caret.row][..d.caret.col.min(d.len(d.caret.row))];
    match c {
        ')' | ']' => {
            let open = if c == ')' { '(' } else { '[' };
            row.iter().filter(|x| **x == open).count() > row.iter().filter(|x| **x == c).count()
        }
        _ => row.iter().filter(|x| **x == c).count() % 2 == 1,
    }
}

/// What typing `c` does when the smart keys have a say. `None`: it is typed as
/// usual. `in_literal` says whether the caret is in a string or a comment.
pub fn type_char(d: &mut Doc, c: char, in_literal: bool) -> Option<bool> {
    if let (Some((a, b)), Some(close)) = (d.sel(), closer_of(c)) {
        d.insert(b, &close.to_string());
        d.insert(a, &c.to_string());
        let nb = if b.row == a.row { at(b.row, b.col + 1) } else { b };
        d.anchor = Some(at(a.row, a.col + 1));
        d.caret = nb;
        return Some(true);
    }
    if d.sel().is_some() {
        return None;
    }
    let (next, prev) = (d.char_at(d.caret), d.char_before(d.caret));
    let quote = c == '\'' || c == '"';
    // Typing over a closer.
    if matches!(c, ')' | ']') || quote {
        if next == Some(c) && (if quote { in_literal || closes_something(d, c) } else { closes_something(d, c) }) {
            d.caret.col += 1;
            d.anchor = None;
            return Some(true);
        }
        if !quote {
            return None;
        }
    }
    let close = closer_of(c)?;
    let word_next = next.is_some_and(is_word);
    if word_next || (quote && (prev.is_some_and(is_word) || in_literal)) {
        return None;
    }
    let pos = d.caret;
    d.insert(pos, &format!("{c}{close}"));
    d.anchor = None;
    d.caret = at(pos.row, pos.col + 1);
    Some(true)
}

/// Tab past a closing bracket or quote that stands right after the caret.
pub fn tab_past_closer(d: &mut Doc, in_literal: bool) -> bool {
    if d.sel().is_some() {
        return false;
    }
    let Some(c) = d.char_at(d.caret).filter(|c| matches!(c, ')' | ']' | '\'' | '"')) else { return false };
    let quote = c == '\'' || c == '"';
    if closes_something(d, c) || (quote && in_literal) {
        d.caret.col += 1;
        return true;
    }
    false
}

/// Backspace between an empty pair takes both.
pub fn backspace_pair(d: &mut Doc) -> bool {
    if d.sel().is_some() {
        return false;
    }
    let (Some(p), Some(n)) = (d.char_before(d.caret), d.char_at(d.caret)) else { return false };
    if closer_of(p) != Some(n) {
        return false;
    }
    d.delete(at(d.caret.row, d.caret.col - 1), at(d.caret.row, d.caret.col + 1));
    true
}

/// Enter: the line break keeps the indentation of the line up to the caret, and
/// between `(` and `)` (or `[` `]`) opens a block with the closer on its own line.
pub fn smart_enter(d: &mut Doc, hard_tabs: bool, width: usize) -> bool {
    d.delete_selection();
    let c = d.caret;
    let indent: String = d.leading(c.row, c.col).into_iter().collect();
    let between = matches!((d.char_before(c), d.char_at(c)), (Some('('), Some(')')) | (Some('['), Some(']')));
    let before: String = d.rows[c.row].iter().take(c.col).collect();
    let rest_blank = d.rows[c.row].iter().skip(c.col).all(|ch| ch.is_whitespace());
    let opens_comment = rest_blank && before.trim_end().ends_with("/*") && !before.contains("*/");
    let text = if opens_comment {
        format!("\n{indent} * \n{indent} */")
    } else if between {
        format!("\n{indent}{}\n{indent}", indent_unit(hard_tabs, width))
    } else {
        format!("\n{indent}")
    };
    let end = d.insert(c, &text);
    d.anchor = None;
    d.caret = if opens_comment { at(c.row + 1, indent.chars().count() + 3) } else if between { at(c.row + 1, indent.chars().count() + indent_unit(hard_tabs, width).chars().count()) } else { end };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str, row: usize, col: usize) -> Doc {
        Doc::from_lines(&text.split('\n').map(str::to_string).collect::<Vec<_>>(), None, at(row, col))
    }

    #[test]
    fn insert_and_delete_across_rows_are_inverses() {
        let mut d = doc("ab\ncd", 0, 1);
        let end = d.insert(at(0, 1), "x\ny\nz");
        assert_eq!(d.text(), "ax\ny\nzb\ncd");
        assert_eq!(end, at(2, 1));
        d.delete(at(0, 1), end);
        assert_eq!(d.text(), "ab\ncd");
        assert_eq!(d.caret, at(0, 1));
    }

    #[test]
    fn word_movement_crosses_line_ends_and_treats_punctuation_as_its_own_run() {
        let d = doc("a.b  c\n  d", 1, 0);
        assert_eq!(word_left(&d, at(1, 0)), at(0, 5));
        assert_eq!(word_left(&d, at(0, 3)), at(0, 2));
        assert_eq!(word_left(&d, at(0, 2)), at(0, 1));
        assert_eq!(word_right(&d, at(0, 0)), at(0, 1));
        assert_eq!(word_right(&d, at(0, 6)), at(1, 3));
    }
}
