//! The editor's keys (slice 13): the actions bound in the editor's table, the
//! smart keys typed text goes through, copy and cut, and what run does when the
//! caret is in a subquery. The edits themselves are pure functions in
//! `edit_ops`; this is what needs the app: settings, the highlighter's tokens,
//! the statement range, the clipboard and notices.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::edit_ops::{self, Doc};
use super::keys::Action;
use super::overlay::{Candidate, Chooser, HelpOverlay, Overlay};
use super::settings::RunAtCaret;
use super::theme::Role;
use super::{App, Effects, Mode, PaneId};
use crate::core::{text_between, Cursor};

/// Whether an action changes the text (and so ends a live template, as an undo
/// does).
fn edits_text(action: Action) -> bool {
    !matches!(
        action,
        Action::SelectAll
            | Action::ExtendSelection
            | Action::ShrinkSelection
            | Action::Copy
            | Action::Paste
            | Action::Find
            | Action::FindNext
            | Action::FindPrev
            | Action::GotoLine
            | Action::WordLeft
            | Action::WordRight
            | Action::WordLeftSelect
            | Action::WordRightSelect
            | Action::LineStart
            | Action::LineEnd
            | Action::LineStartSelect
            | Action::LineEndSelect
            | Action::TextStart
            | Action::TextEnd
            | Action::TextStartSelect
            | Action::TextEndSelect
            | Action::MatchingBrace
    )
}

impl App {
    /// The statement range for the editing actions: asked of the Core's own
    /// statement finder, for the active connection's kind (Postgres when the
    /// tab has none yet: statements end at `;` in every dialect).
    pub(super) fn statement_range_at(&self, text: &str, at: Cursor) -> Option<(Cursor, Cursor)> {
        let kind = self.active_kind().unwrap_or_default();
        self.ui.syntax.statement_at(&kind, text, at)
    }

    /// Applies `f` to the document as one undo step.
    fn edit_doc(&mut self, fx: &mut Effects, f: impl FnOnce(&mut Doc) -> bool) {
        match self.ui.tab.editor.edit(f) {
            Ok(changed) => fx.repaint |= changed,
            Err(why) => self.notify(format!("Not done: {why}."), true, fx),
        }
    }

    /// One of the editor's own actions, from its key or the prefix.
    pub(super) fn perform_edit(&mut self, action: Action, now: Instant, fx: &mut Effects) {
        let _ = now;
        if self.ui.focus != PaneId::Editor {
            return;
        }
        self.close_completion(fx);
        if edits_text(action) {
            self.leave_template(fx);
        }
        let (hard_tabs, width, smart_home) = (self.model.settings.hard_tabs, self.model.settings.tab_width, self.model.settings.smart_home);
        match action {
            Action::Duplicate => self.edit_doc(fx, edit_ops::duplicate),
            Action::DeleteLine => self.edit_doc(fx, edit_ops::delete_line),
            Action::MoveLineUp => self.edit_doc(fx, |d| edit_ops::move_lines(d, true)),
            Action::MoveLineDown => self.edit_doc(fx, |d| edit_ops::move_lines(d, false)),
            Action::MoveStatementUp => self.move_statement(true, fx),
            Action::MoveStatementDown => self.move_statement(false, fx),
            Action::LineComment => self.edit_doc(fx, edit_ops::toggle_line_comment),
            Action::BlockComment => self.edit_doc(fx, edit_ops::toggle_block_comment),
            Action::ExtendSelection => {
                let text = self.ui.tab.editor.text();
                let statement = self.statement_range_at(&text, self.ui.tab.editor.cursor());
                fx.repaint |= self.ui.tab.editor.extend_selection(statement);
            }
            Action::ShrinkSelection => fx.repaint |= self.ui.tab.editor.shrink_selection(),
            Action::SelectAll => self.edit_doc(fx, |d| {
                edit_ops::select_all(d);
                true
            }),
            Action::Undo => {
                self.leave_template(fx);
                fx.repaint |= self.ui.tab.editor.undo();
            }
            Action::Redo => {
                self.leave_template(fx);
                fx.repaint |= self.ui.tab.editor.redo();
            }
            Action::Copy => self.copy_selection(false, fx),
            Action::Cut => self.copy_selection(true, fx),
            Action::Paste => {
                // The terminal pastes by sending the text as a paste event; a key
                // that reaches us is one it did not turn into one.
                let text = "This terminal did not paste: use its own paste key (cmd+v in kitty, ctrl+shift+v in most others) and the text arrives as a paste.".to_string();
                self.notify(text, false, fx);
            }
            Action::Find | Action::FindNext | Action::FindPrev | Action::GotoLine => self.bar_action(action, fx),
            Action::NewLineAfter => self.edit_doc(fx, |d| edit_ops::new_line(d, true)),
            Action::NewLineBefore => self.edit_doc(fx, |d| edit_ops::new_line(d, false)),
            Action::JoinLines => self.edit_doc(fx, edit_ops::join_lines),
            Action::ToggleCase => self.edit_doc(fx, edit_ops::toggle_case),
            Action::Indent => {
                let in_literal = self.caret_in_literal_now();
                self.edit_doc(fx, |d| edit_ops::tab_past_closer(d, in_literal) || edit_ops::indent(d, hard_tabs, width));
            }
            Action::Unindent => self.edit_doc(fx, |d| edit_ops::unindent(d, width)),
            Action::WordLeft => self.edit_doc(fx, |d| edit_ops::word_move(d, false, false)),
            Action::WordRight => self.edit_doc(fx, |d| edit_ops::word_move(d, true, false)),
            Action::WordLeftSelect => self.edit_doc(fx, |d| edit_ops::word_move(d, false, true)),
            Action::WordRightSelect => self.edit_doc(fx, |d| edit_ops::word_move(d, true, true)),
            Action::DeleteWordBack => self.edit_doc(fx, |d| edit_ops::delete_word(d, false)),
            Action::DeleteWordForward => self.edit_doc(fx, |d| edit_ops::delete_word(d, true)),
            Action::LineStart => self.edit_doc(fx, |d| edit_ops::line_start(d, false, smart_home)),
            Action::LineStartSelect => self.edit_doc(fx, |d| edit_ops::line_start(d, true, smart_home)),
            Action::LineEnd => self.edit_doc(fx, |d| edit_ops::line_end(d, false)),
            Action::LineEndSelect => self.edit_doc(fx, |d| edit_ops::line_end(d, true)),
            Action::TextStart => self.edit_doc(fx, |d| edit_ops::text_edge(d, false, false)),
            Action::TextStartSelect => self.edit_doc(fx, |d| edit_ops::text_edge(d, false, true)),
            Action::TextEnd => self.edit_doc(fx, |d| edit_ops::text_edge(d, true, false)),
            Action::TextEndSelect => self.edit_doc(fx, |d| edit_ops::text_edge(d, true, true)),
            Action::MatchingBrace => self.matching_brace(fx),
            Action::Reformat => self.reformat(false, fx),
            Action::ReformatAll => self.reformat(true, fx),
            _ => {}
        }
    }

    fn move_statement(&mut self, up: bool, fx: &mut Effects) {
        let text = self.ui.tab.editor.text();
        let kind = self.active_kind().unwrap_or_default();
        let mut doc = self.ui.tab.editor.doc();
        let syntax = &self.ui.syntax;
        let moved = edit_ops::move_statement(&mut doc, up, &|at| syntax.statement_at(&kind, &text, at));
        match moved {
            Ok(true) => match self.ui.tab.editor.apply(&doc) {
                Ok(changed) => fx.repaint |= changed,
                Err(why) => self.notify(format!("Not done: {why}."), true, fx),
            },
            Ok(false) => {}
            Err(why) => self.notify(why.to_string(), true, fx),
        }
    }

    /// Reformat code: the selection, else the statement under the caret, or
    /// with `whole` the whole editor. The result is one undo step; text that
    /// does not tokenize is left alone with a notice.
    fn reformat(&mut self, whole: bool, fx: &mut Effects) {
        let text = self.ui.tab.editor.text();
        let chars: Vec<char> = text.chars().collect();
        let doc = self.ui.tab.editor.doc();
        let mut starts = vec![0usize];
        starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
        let off = |c: Cursor| starts[c.row.min(starts.len() - 1)] + c.col;
        let range = if whole {
            Some((Cursor::default(), Cursor { row: starts.len() - 1, col: chars.len() - starts[starts.len() - 1] }))
        } else if let Some(sel) = doc.sel() {
            Some(sel)
        } else {
            self.statement_range_at(&text, doc.caret)
        };
        let Some((from, to)) = range else {
            return self.notify("Nothing to reformat: the caret is not in a statement.".to_string(), false, fx);
        };
        // Only the text between the first and the last character that is not
        // blank is replaced: the blanks around it stay as they are.
        let (mut a, mut b) = (off(from).min(chars.len()), off(to).min(chars.len()));
        while a < b && chars[a].is_whitespace() {
            a += 1;
        }
        while b > a && chars[b - 1].is_whitespace() {
            b -= 1;
        }
        if a == b {
            return self.notify("Nothing to reformat here.".to_string(), false, fx);
        }
        // Where the lines after the first sit: under the first, when it starts
        // its line, else at the line's own indentation.
        let row_start = chars[..a].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
        let before = &chars[row_start..a];
        let base = if before.iter().all(|c| c.is_whitespace()) { before.len() } else { chars[row_start..].iter().take_while(|c| **c == ' ' || **c == '\t').count() };

        let s = &self.model.settings;
        let opts = super::format::Options { case: s.format_keyword_case, indent: s.format_indent, comma: s.format_comma };
        let dialect = self.active_dialect();
        let region: String = chars[a..b].iter().collect();
        let (caret, anchor) = (off(doc.caret), doc.anchor.map(off));
        let inside = |p: usize| p.clamp(a, b) - a;
        let asked = [inside(caret), anchor.map_or(0, inside)];
        // A selection must start and end between tokens of the whole text: one
        // that begins inside a string, comment or `$$` body is not SQL.
        if !whole && doc.sel().is_some() {
            if let Err(why) = super::format::ends_between_tokens(&text, dialect, &[a, b]) {
                return self.notify(format!("Not reformatted: {why}."), true, fx);
            }
        }
        let done = match super::format::format_sql(&region, dialect, &opts, base, 0, &asked) {
            Ok(done) => done,
            Err(why) => return self.notify(format!("Not reformatted: {why}."), true, fx),
        };
        if done.text == region {
            return;
        }
        let new_len = done.text.chars().count();
        let place = |p: usize, mapped: usize| if p < a { p } else if p > b { p - (b - a) + new_len } else { a + mapped };
        let (new_caret, new_anchor) = (place(caret, done.carets[0]), anchor.map(|p| place(p, done.carets[1])));
        let mut new_text: String = chars[..a].iter().collect();
        new_text.push_str(&done.text);
        new_text.extend(&chars[b..]);

        let lines: Vec<String> = new_text.split('\n').map(str::to_string).collect();
        let at = |p: usize| {
            let (mut row, mut left) = (0, p);
            for l in &lines {
                let n = l.chars().count();
                if left <= n {
                    break;
                }
                left -= n + 1;
                row += 1;
            }
            Cursor { row: row.min(lines.len() - 1), col: left }
        };
        let doc = Doc::from_lines(&lines, new_anchor.map(at), at(new_caret));
        match self.ui.tab.editor.apply(&doc) {
            Ok(changed) => fx.repaint |= changed,
            Err(why) => self.notify(format!("Not done: {why}."), true, fx),
        }
    }

    fn matching_brace(&mut self, fx: &mut Effects) {
        let (kind, dialect) = (self.active_kind(), self.active_dialect());
        self.ui.tab.syntax.refresh(&mut self.ui.syntax, &self.ui.tab.editor, kind.as_ref(), dialect);
        let caret = self.ui.tab.editor.cursor();
        let Some([here, partner]) = super::highlight::matching_bracket(self.ui.tab.syntax.colours(), caret) else { return };
        // Beside the partner, on the outside of the pair.
        let after = (partner.0, partner.1) > (here.0, here.1);
        let to = Cursor { row: partner.0, col: if after { partner.1 + 1 } else { partner.1 } };
        self.ui.tab.editor.move_to(to);
        fx.repaint = true;
    }

    /// Copy puts the selection (or, with none, the caret's line with its line
    /// break) on the terminal's clipboard; cut also removes it. A text over the
    /// clipboard's limit is refused, and not cut.
    pub(super) fn copy_selection(&mut self, cut: bool, fx: &mut Effects) {
        let doc = self.ui.tab.editor.doc();
        let (text, what) = match doc.sel() {
            Some((a, b)) => (doc.text_of(a, b), "the selection"),
            None => (format!("{}\n", doc.lines()[doc.caret.row]), "the line"),
        };
        if !self.send_to_clipboard(text, what, fx) || !cut {
            return;
        }
        if doc.sel().is_some() {
            self.edit_doc(fx, |d| d.delete_selection());
        } else {
            self.edit_doc(fx, edit_ops::delete_line);
        }
    }

    /// Whether the caret is in a string or comment, per the highlighter's
    /// tokens (the whole buffer is tokenized once per change: the frame's own
    /// pass is the same cache).
    pub(super) fn caret_in_literal_now(&mut self) -> bool {
        let (kind, dialect) = (self.active_kind(), self.active_dialect());
        self.ui.tab.syntax.refresh(&mut self.ui.syntax, &self.ui.tab.editor, kind.as_ref(), dialect);
        let caret = self.ui.tab.editor.cursor();
        let Some(before) = caret.col.checked_sub(1) else { return false };
        let Some(span) = self.ui.tab.syntax.colours().line(caret.row).and_then(|spans| spans.iter().find(|s| s.start <= before && before < s.end)).copied() else {
            return false;
        };
        if !matches!(span.role, Role::SynString | Role::SynComment) {
            return false;
        }
        if caret.col < span.end {
            return true;
        }
        // At the end of the token: inside only when it is not closed.
        let chars: Vec<char> = self.ui.tab.editor.lines()[caret.row].chars().skip(span.start).take(span.end - span.start).collect();
        match span.role {
            Role::SynString => !(chars.len() >= 2 && chars.first() == chars.last()),
            _ => {
                let line_comment = chars.starts_with(&['-', '-']) || chars.starts_with(&['#']);
                let closed = chars.len() >= 2 && chars[chars.len() - 2..] == ['*', '/'] && (chars.len() >= 4 || !chars.starts_with(&['/', '*']));
                line_comment || !closed
            }
        }
    }

    /// A key typed in the editor that the smart keys may take: a character
    /// (pairs, typing over a closer, surrounding a selection), `enter`
    /// (indentation) and `backspace` (an empty pair). Returns whether it did.
    pub(super) fn smart_key(&mut self, key: &KeyEvent, fx: &mut Effects) -> bool {
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        if !plain || self.ui.tab.template.active() {
            return false;
        }
        let s = &self.model.settings;
        let (auto_pair, smart_enter, hard_tabs, width) = (s.auto_pair, s.smart_enter, s.hard_tabs, s.tab_width);
        match key.code {
            KeyCode::Char(c) if auto_pair && matches!(c, '(' | '[' | ')' | ']' | '\'' | '"') => {
                let in_literal = matches!(c, '\'' | '"') && self.caret_in_literal_now();
                let mut handled = false;
                let before = self.ui.tab.editor.revision();
                let result = self.ui.tab.editor.edit(|d| match edit_ops::type_char(d, c, in_literal) {
                    Some(changed) => {
                        handled = true;
                        changed
                    }
                    None => false,
                });
                match result {
                    Ok(changed) => fx.repaint |= changed || self.ui.tab.editor.revision() != before,
                    Err(why) => self.notify(format!("Not done: {why}."), true, fx),
                }
                handled
            }
            KeyCode::Enter if smart_enter => {
                self.edit_doc(fx, |d| edit_ops::smart_enter(d, hard_tabs, width));
                true
            }
            KeyCode::Backspace if auto_pair => {
                let mut handled = false;
                self.edit_doc(fx, |d| {
                    handled = edit_ops::backspace_pair(d);
                    handled
                });
                handled
            }
            _ => false,
        }
    }

    // ---- what run does with the caret in a subquery ---------------------------------------

    /// Run (`cmd+enter`, `ctrl+enter`, `f5`, prefix `r`): with a selection, or
    /// where the caret is in no subquery, the Core's own pick; inside one, as
    /// the setting says: ask, or the smallest.
    pub(super) fn run_requested(&mut self, now: Instant, fx: &mut Effects) {
        if self.model.settings.run_at_caret == RunAtCaret::Statement || self.ui.tab.editor.selection().is_some() {
            return self.run_statement(now, fx);
        }
        let candidates = self.run_candidates();
        // Nothing to choose between unless the caret is in a subquery.
        if candidates.first().is_none_or(|c| c.label != "subquery") {
            return self.run_statement(now, fx);
        }
        if self.model.settings.run_at_caret == RunAtCaret::Smallest {
            return self.run_range(Some(candidates[0].range), now, fx);
        }
        // Nothing to ask about when the run cannot start.
        if !self.can_run(fx) {
            return;
        }
        self.ui.mode = Mode::Normal;
        self.ui.overlay = Some(Overlay::Chooser(Chooser { items: candidates, selected: 0 }));
        fx.repaint = true;
    }

    /// What can run here, smallest first: the subquery (or CTE) the caret is in,
    /// the statement, and the whole buffer. The whole buffer comes last.
    fn run_candidates(&mut self) -> Vec<Candidate> {
        let text = self.ui.tab.editor.text();
        let caret = self.ui.tab.editor.cursor();
        let Some(statement) = self.statement_range_at(&text, caret) else { return Vec::new() };
        let scope = super::complete_context::analyse(self.ui.tab.editor.lines(), caret).scope;
        let mut out = Vec::new();
        // The subquery: the innermost block when a `(` stands right before it,
        // up to the `)` that closes it (the block's own range stops short of its
        // last token, so the text is scanned for the closing parenthesis).
        let around = scope.levels.first().and_then(|l| parenthesised(self.ui.tab.editor.lines(), l.range.0));
        if let Some(inner) = around {
            let strictly_inside = (inner.0.row, inner.0.col) >= (statement.0.row, statement.0.col)
                && (inner.1.row, inner.1.col) <= (statement.1.row, statement.1.col)
                && inner != statement
                && (inner.0.row, inner.0.col) < (inner.1.row, inner.1.col);
            if strictly_inside {
                out.push(Candidate::new("subquery", &text, inner));
            }
        }
        out.push(Candidate::new("statement", &text, statement));
        let rows = self.ui.tab.editor.lines();
        let last = rows.len().saturating_sub(1);
        let end = Cursor { row: last, col: rows.get(last).map_or(0, |l| l.chars().count()) };
        out.push(Candidate::new("all", &text, (Cursor::default(), end)));
        out
    }

    /// Runs the chooser's entry `index`.
    pub(super) fn chooser_run(&mut self, index: Option<usize>, now: Instant, fx: &mut Effects) {
        let Some(Overlay::Chooser(c)) = &self.ui.overlay else { return };
        let Some(item) = c.items.get(index.unwrap_or(c.selected)) else { return };
        let range = item.range;
        self.ui.overlay = None;
        fx.repaint = true;
        self.run_range(Some(range), now, fx);
    }

    pub(super) fn open_help(&mut self, fx: &mut Effects) {
        self.ui.overlay = Some(Overlay::Help(HelpOverlay::default()));
        self.ui.mode = Mode::Normal;
        fx.repaint = true;
    }
}

impl Candidate {
    /// An entry that runs `range` of `text`, with a one-line preview of it.
    pub fn new(label: &'static str, text: &str, range: (Cursor, Cursor)) -> Candidate {
        let shown = text_between(text, range.0, range.1);
        let first = shown.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        let more = shown.lines().filter(|l| !l.trim().is_empty()).count() > 1;
        Candidate { label, preview: if more { format!("{first} …") } else { first }, range }
    }
}


/// The text from `start` to the parenthesis that closes the one standing right
/// before it, when there is one (quotes and `--` comments on a row are skipped).
fn parenthesised(lines: &[String], start: Cursor) -> Option<(Cursor, Cursor)> {
    // The nearest non-blank character before `start` must be `(`.
    let (mut row, mut col) = (start.row, start.col);
    loop {
        if col == 0 {
            row = row.checked_sub(1)?;
            col = lines[row].chars().count();
            continue;
        }
        let c = lines[row].chars().nth(col - 1)?;
        if c.is_whitespace() {
            col -= 1;
        } else if c == '(' {
            break;
        } else {
            return None;
        }
    }
    let mut depth = 0usize;
    for (r, line) in lines.iter().enumerate().skip(start.row) {
        let chars: Vec<char> = line.chars().collect();
        let mut i = if r == start.row { start.col } else { 0 };
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '-') if chars.get(i + 1) == Some(&'-') => break,
                (None, '(') => depth += 1,
                (None, ')') if depth == 0 => {
                    let mut end = Cursor { row: r, col: i };
                    // Not the trailing blanks.
                    while end.col > 0 && r == end.row && chars[end.col - 1].is_whitespace() {
                        end.col -= 1;
                    }
                    return Some((start, end));
                }
                (None, ')') => depth -= 1,
                _ => {}
            }
            i += 1;
        }
    }
    None
}
