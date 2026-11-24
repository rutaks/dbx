//! The bar on the editor's last row: find (type to search; `enter`, `f3` and
//! `cmd+g` go to the next match, `shift+f3` and `cmd+shift+g` to the previous,
//! `esc` closes) and go to line. While it is open it takes the keys that are
//! text; the editor's text is never touched by it.
//!
//! Search is plain text. It ignores case unless the query has a capital, as
//! IntelliJ's does. Matches are kept for the buffer's revision and the query,
//! and are what the editor highlights.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::{Action, KeyCombo};
use super::text_field::{Edit, TextField};
use super::{App, Effects, PaneId};
use crate::core::Cursor;

/// The most matches kept and highlighted.
pub const MATCH_LIMIT: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Find,
    Goto,
}

#[derive(Debug, Clone)]
pub struct Bar {
    pub kind: BarKind,
    pub field: TextField,
    /// Where the caret was when the bar opened: typing a query goes to the
    /// first match from here.
    pub origin: Cursor,
    /// Every match of the query, in order.
    pub matches: Vec<(Cursor, Cursor)>,
    /// The revision and query the matches were made for.
    stamp: Option<(u64, String)>,
    /// The match the editor's selection is on, as of the last step.
    pub current: Option<usize>,
}

impl Bar {
    fn new(kind: BarKind, text: &str, origin: Cursor) -> Bar {
        Bar { kind, field: TextField::with_text(text), origin, matches: Vec::new(), stamp: None, current: None }
    }

    /// The label drawn before the field.
    pub fn label(&self) -> &'static str {
        match self.kind {
            BarKind::Find => "find",
            BarKind::Goto => "line",
        }
    }
}

fn same(a: char, b: char, exact: bool) -> bool {
    a == b || (!exact && a.to_lowercase().eq(b.to_lowercase()))
}

/// Every place `query` occurs in `lines`, on one row each. Plain text; case is
/// ignored unless the query has a capital.
pub fn find_matches(lines: &[String], query: &str) -> Vec<(Cursor, Cursor)> {
    let q: Vec<char> = query.chars().collect();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    let exact = q.iter().any(|c| c.is_uppercase());
    for (row, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < q.len() {
            continue;
        }
        let mut col = 0;
        while col + q.len() <= chars.len() {
            if q.iter().zip(&chars[col..]).all(|(a, b)| same(*a, *b, exact)) {
                out.push((Cursor { row, col }, Cursor { row, col: col + q.len() }));
                if out.len() >= MATCH_LIMIT {
                    return out;
                }
                col += q.len();
            } else {
                col += 1;
            }
        }
    }
    out
}

fn key(c: Cursor) -> (usize, usize) {
    (c.row, c.col)
}

impl App {
    /// Whether the bar is open and the editor is where keys go.
    pub(super) fn bar_open(&self) -> bool {
        self.ui.tab.bar.is_some() && self.ui.focus == PaneId::Editor
    }

    /// Opens (or steps) the find bar, or opens go to line.
    pub(super) fn bar_action(&mut self, action: Action, fx: &mut Effects) {
        match action {
            Action::GotoLine => {
                let origin = self.ui.tab.editor.cursor();
                self.ui.tab.bar = Some(Bar::new(BarKind::Goto, "", origin));
                fx.repaint = true;
            }
            Action::Find => {
                if matches!(&self.ui.tab.bar, Some(b) if b.kind == BarKind::Find) {
                    return;
                }
                self.open_find(fx);
            }
            Action::FindNext | Action::FindPrev => {
                if !matches!(&self.ui.tab.bar, Some(b) if b.kind == BarKind::Find) {
                    self.open_find(fx);
                }
                self.step_match(action == Action::FindNext, fx);
            }
            _ => {}
        }
    }

    /// The find bar, with the selected text as the query when it is on one line,
    /// else the last one searched for.
    fn open_find(&mut self, fx: &mut Effects) {
        let doc = self.ui.tab.editor.doc();
        let selected = doc.sel().filter(|(a, b)| a.row == b.row).map(|(a, b)| doc.text_of(a, b));
        let origin = doc.sel().map_or(doc.caret, |(a, _)| a);
        let text = selected.unwrap_or_else(|| self.ui.tab.last_find.clone());
        let mut bar = Bar::new(BarKind::Find, &text, origin);
        self.refresh_matches(&mut bar);
        // The selection already on a match is the current one.
        bar.current = doc.sel().and_then(|s| bar.matches.iter().position(|m| *m == s));
        self.ui.tab.bar = Some(bar);
        fx.repaint = true;
    }

    /// Brings a bar's matches up to date with the buffer and its query.
    fn refresh_matches(&self, bar: &mut Bar) {
        if bar.kind != BarKind::Find {
            return;
        }
        let stamp = (self.ui.tab.editor.revision(), bar.field.text().to_string());
        if bar.stamp.as_ref() == Some(&stamp) {
            return;
        }
        bar.matches = find_matches(self.ui.tab.editor.lines(), &stamp.1);
        bar.stamp = Some(stamp);
        if bar.current.is_some_and(|i| i >= bar.matches.len()) {
            bar.current = None;
        }
    }

    /// Called by the view: matches follow edits made while the bar is open.
    pub(super) fn refresh_bar(&mut self) {
        if let Some(mut bar) = self.ui.tab.bar.take() {
            self.refresh_matches(&mut bar);
            self.ui.tab.bar = Some(bar);
        }
    }

    /// Selects match `i`, the caret at its end.
    fn select_match(&mut self, i: usize, fx: &mut Effects) {
        let Some(bar) = &mut self.ui.tab.bar else { return };
        let Some(&(a, b)) = bar.matches.get(i) else { return };
        bar.current = Some(i);
        self.ui.tab.editor.select_range(a, b);
        fx.repaint = true;
    }

    /// The next (or previous) match after the one selected, wrapping round.
    fn step_match(&mut self, forward: bool, fx: &mut Effects) {
        let Some(mut bar) = self.ui.tab.bar.take() else { return };
        self.refresh_matches(&mut bar);
        let n = bar.matches.len();
        let doc = self.ui.tab.editor.doc();
        let on = doc.sel().and_then(|s| bar.matches.iter().position(|m| *m == s));
        let (from, to) = doc.sel().unwrap_or((doc.caret, doc.caret));
        let next = if n == 0 {
            None
        } else if let Some(i) = on {
            Some(if forward { (i + 1) % n } else { (i + n - 1) % n })
        } else if forward {
            Some(bar.matches.iter().position(|(a, _)| key(*a) >= key(to)).unwrap_or(0))
        } else {
            Some(bar.matches.iter().rposition(|(_, b)| key(*b) <= key(from)).unwrap_or(n - 1))
        };
        self.ui.tab.bar = Some(bar);
        match next {
            Some(i) => self.select_match(i, fx),
            None => fx.repaint = true,
        }
        if let Some(b) = &self.ui.tab.bar {
            self.ui.tab.last_find = b.field.text().to_string();
        }
    }

    /// The query changed: the first match from where the bar opened is selected.
    fn query_changed(&mut self, fx: &mut Effects) {
        let Some(mut bar) = self.ui.tab.bar.take() else { return };
        self.refresh_matches(&mut bar);
        let first = bar.matches.iter().position(|(a, _)| key(*a) >= key(bar.origin)).or(if bar.matches.is_empty() { None } else { Some(0) });
        bar.current = None;
        self.ui.tab.last_find = bar.field.text().to_string();
        self.ui.tab.bar = Some(bar);
        if let Some(i) = first {
            self.select_match(i, fx);
        }
        fx.repaint = true;
    }

    /// A key while the bar is open: everything but the prefix and the direct
    /// keys (which were taken before) is the bar's.
    pub(super) fn bar_key(&mut self, key: &KeyEvent, combo: &KeyCombo, fx: &mut Effects) {
        let Some(kind) = self.ui.tab.bar.as_ref().map(|b| b.kind) else { return };
        if combo.code == KeyCode::Esc && combo.mods.is_empty() {
            self.ui.tab.bar = None;
            fx.repaint = true;
            return;
        }
        if kind == BarKind::Find {
            let step = match self.ui.keys.lookup_editor(combo) {
                Some(Action::FindNext) => Some(true),
                Some(Action::FindPrev) => Some(false),
                _ if *combo == KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE) => Some(true),
                _ if *combo == KeyCombo::new(KeyCode::Enter, KeyModifiers::SHIFT) => Some(false),
                _ => None,
            };
            if let Some(forward) = step {
                return self.step_match(forward, fx);
            }
        } else if combo.code == KeyCode::Enter {
            return self.go_to_line(fx);
        }
        // Go to line takes digits and nothing else.
        if kind == BarKind::Goto && matches!(combo.code, KeyCode::Char(c) if !c.is_ascii_digit()) {
            return;
        }
        let Some(bar) = &mut self.ui.tab.bar else { return };
        match bar.field.handle_key(key) {
            Edit::Changed if kind == BarKind::Find => self.query_changed(fx),
            Edit::Ignored => {}
            _ => fx.repaint = true,
        }
    }

    /// A paste while the bar is open goes into its field.
    pub(super) fn bar_paste(&mut self, text: &str, fx: &mut Effects) {
        let Some(bar) = &mut self.ui.tab.bar else { return };
        let kind = bar.kind;
        if kind == BarKind::Goto {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            bar.field.insert_str(&digits);
            fx.repaint = true;
        } else {
            bar.field.insert_str(text);
            self.query_changed(fx);
        }
    }

    fn go_to_line(&mut self, fx: &mut Effects) {
        let Some(bar) = &self.ui.tab.bar else { return };
        let Ok(n) = bar.field.text().parse::<usize>() else { return };
        let row = n.saturating_sub(1).min(self.ui.tab.editor.lines().len().saturating_sub(1));
        self.ui.tab.bar = None;
        self.ui.tab.editor.move_to(Cursor { row, col: 0 });
        fx.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn matches_are_plain_text_and_case_follows_the_query() {
        let l = lines("Select a, SELECT b\nselect (c)");
        assert_eq!(find_matches(&l, "select").len(), 3);
        assert_eq!(find_matches(&l, "SELECT").len(), 1);
        assert_eq!(find_matches(&l, "(c)").len(), 1, "no regular expressions");
        assert!(find_matches(&l, "").is_empty());
        assert_eq!(find_matches(&lines("aaaa"), "aa").len(), 2, "matches do not overlap");
    }
}
