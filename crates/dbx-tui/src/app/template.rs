//! Live templates and postfix templates (slice 12).
//!
//! A live template is an abbreviation typed where a statement starts (`sel`,
//! `del`), expanded with `tab`. The expansion is one line with variables
//! (`$table$`); the editor is then in template mode: the caret is on the first
//! variable, typing replaces it, `tab` and `shift+tab` move between the
//! variables, and the last `tab` puts the caret at the template's end. A
//! variable that appears twice (`$alias$`) is one: typing in one place types in
//! the other. A postfix template (`film.from`) rewrites an expression and the
//! word after its dot.
//!
//! `expand` and `postfix_at` are pure; the rest is the `App` half: the
//! session that follows the text while the user types, and the names the
//! templates need from the Core (a table's columns and foreign keys), which are
//! asked for and the expansion made when they have come.

use super::complete::Need;
use super::complete_context::{analyse, scan, Tok};
use super::complete_insert::quote_identifier;
use super::complete_popup::ItemKind;
use super::complete_scope::generate_alias;
use super::keys::{Action, KeyCombo};
use super::{App, Effects, PaneId};
use crate::core::{Cursor, SchemaPath, TablePath};

/// One live template.
pub struct Template {
    pub abbr: &'static str,
    /// `$name$` is a variable (one per name, wherever it occurs), `$END$` the
    /// place the caret goes after the last variable.
    pub pattern: &'static str,
    /// The variables that name a table, which open the table completion.
    pub tables: &'static [&'static str],
}

pub const TEMPLATES: [Template; 9] = [
    Template { abbr: "sel", pattern: "select * from $table$;", tables: &["table"] },
    Template { abbr: "selw", pattern: "select * from $table$ $alias$ where $alias$.$END$;", tables: &["table"] },
    Template { abbr: "selc", pattern: "select count(*) from $table$ $alias$ where $alias$.$END$;", tables: &["table"] },
    Template { abbr: "lim", pattern: "select * from $table$ limit $size$;", tables: &["table"] },
    Template { abbr: "ins", pattern: "insert into $table$ ($columns$) values ($END$);", tables: &["table"] },
    Template { abbr: "upd", pattern: "update $table_name$ set $col$ = $value$ where $END$;", tables: &["table_name"] },
    Template { abbr: "del", pattern: "delete from $table$ where $END$;", tables: &["table"] },
    Template { abbr: "tab", pattern: "create table $table$ ( $col$ $type$ $null$ );", tables: &[] },
    Template { abbr: "view", pattern: "create view $view$ as select * from $table$ ;", tables: &["table"] },
];

impl Template {
    pub fn named(abbr: &str) -> Option<&'static Template> {
        TEMPLATES.iter().find(|t| t.abbr.eq_ignore_ascii_case(abbr))
    }

    /// What the popup shows beside the abbreviation.
    pub fn preview(&self) -> String {
        self.pattern.replace("$END$", "")
    }
}

/// A variable of an expansion: where it stands, in characters from the start
/// of the text (one place for each time the pattern names it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: String,
    pub table: bool,
    pub spans: Vec<(usize, usize)>,
}

/// A template's text with its variables empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    /// In order of first appearance.
    pub vars: Vec<Var>,
    pub end: usize,
}

pub fn expand(t: &Template) -> Expansion {
    let chars: Vec<char> = t.pattern.chars().collect();
    let (mut text, mut len, mut end) = (String::new(), 0usize, None);
    let mut vars: Vec<Var> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            if let Some(j) = chars[i + 1..].iter().position(|c| *c == '$') {
                let name: String = chars[i + 1..i + 1 + j].iter().collect();
                i += j + 2;
                if name == "END" {
                    end = Some(len);
                } else {
                    match vars.iter_mut().find(|v| v.name == name) {
                        Some(v) => v.spans.push((len, len)),
                        None => vars.push(Var { table: t.tables.contains(&name.as_str()), name, spans: vec![(len, len)] }),
                    }
                }
                continue;
            }
        }
        text.push(chars[i]);
        len += 1;
        i += 1;
    }
    Expansion { text, vars, end: end.unwrap_or(len) }
}

// ---- postfix templates ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Postfix {
    From,
    CFrom,
    Join,
    Cast,
}

impl Postfix {
    fn parse(word: &str) -> Option<Postfix> {
        match word {
            "from" => Some(Postfix::From),
            "cfrom" => Some(Postfix::CFrom),
            "join" => Some(Postfix::Join),
            "cast" => Some(Postfix::Cast),
            _ => None,
        }
    }

    fn names_a_table(self) -> bool {
        self != Postfix::Cast
    }
}

/// An expression with a postfix word after its dot, the caret at the word's end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostfixSite {
    pub kind: Postfix,
    /// The expression's first character, and the end of the word: what is replaced.
    pub start: usize,
    pub end: usize,
    pub expr: String,
}

/// The postfix template, if any, the caret at `caret` of `line` is at the end of.
pub fn postfix_at(line: &str, caret: usize) -> Option<PostfixSite> {
    let chars: Vec<char> = line.chars().collect();
    if caret > chars.len() || chars.get(caret).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
        return None;
    }
    let mut w = caret;
    while w > 0 && chars[w - 1].is_ascii_alphabetic() {
        w -= 1;
    }
    let kind = Postfix::parse(&chars[w..caret].iter().collect::<String>())?;
    let dot = w.checked_sub(1).filter(|d| chars[*d] == '.')?;
    let prefix: String = chars[..dot].iter().collect();
    let tokens = scan(std::slice::from_ref(&prefix));
    let last = tokens.last().filter(|t| t.end.col == dot)?;
    let adjacent = |a: usize, b: usize| tokens[a].end == tokens[b].start;
    let n = tokens.len();
    let start = match &last.tok {
        Tok::Str(true) | Tok::Number if !kind.names_a_table() => last.start.col,
        Tok::Word(_) | Tok::Quoted(_, true) => {
            let mut i = n - 1;
            while i >= 2
                && tokens[i - 1].tok == Tok::Dot
                && matches!(tokens[i - 2].tok, Tok::Word(_) | Tok::Quoted(_, true))
                && adjacent(i - 2, i - 1)
                && adjacent(i - 1, i)
            {
                i -= 2;
            }
            tokens[i].start.col
        }
        Tok::RParen if !kind.names_a_table() => {
            let mut depth = 0usize;
            let mut open = None;
            for i in (0..n).rev() {
                match tokens[i].tok {
                    Tok::RParen => depth += 1,
                    Tok::LParen => {
                        depth -= 1;
                        if depth == 0 {
                            open = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let open = open?;
            // A call: its name is part of the expression.
            if open > 0 && matches!(tokens[open - 1].tok, Tok::Word(_)) && adjacent(open - 1, open) {
                tokens[open - 1].start.col
            } else {
                tokens[open].start.col
            }
        }
        _ => return None,
    };
    Some(PostfixSite { kind, start, end: caret, expr: chars[start..dot].iter().collect() })
}

/// `public.film`, `"My Table"`: the schema, if one is named, and the table.
pub fn table_of_expr(expr: &str) -> Option<(Option<String>, String)> {
    let mut parts: Vec<String> = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    match parts.as_slice() {
        [t] if !t.is_empty() => Some((None, t.clone())),
        [s, t] if !s.is_empty() && !t.is_empty() => Some((Some(s.clone()), t.clone())),
        _ => None,
    }
}

// ---- the session -----------------------------------------------------------------------

#[derive(Debug)]
struct Session {
    row: usize,
    /// Where the expansion starts, and how long it is, in characters.
    start: usize,
    len: usize,
    vars: Vec<Var>,
    /// Absolute columns.
    end: usize,
    current: usize,
    /// The text revision the session has accounted for: another one means the
    /// text changed in a way it did not follow.
    revision: u64,
}

/// A postfix expansion waiting for names the Core has not given yet.
#[derive(Debug)]
struct Waiting {
    revision: u64,
    caret: Cursor,
}

#[derive(Debug, Default)]
pub struct TemplateUi {
    session: Option<Session>,
    waiting: Option<Waiting>,
    /// The table (schema, name) whose columns are to go into the `columns`
    /// variable when they have come, and the text the table variable had when
    /// they were asked for: they are written only if it is still that.
    columns_for: Option<(Option<String>, String, String)>,
    /// Template expansions done (for tests).
    pub expanded: u64,
}

impl TemplateUi {
    /// Whether template mode is on.
    pub fn active(&self) -> bool {
        self.session.is_some()
    }

    fn in_table_variable(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.vars.get(s.current).is_some_and(|v| v.table))
    }
}

/// What the row was before a key, for `track_template`.
pub(super) type Before = (usize, String, usize, u64);

/// Where an edit of the row falls: from `p` to `old_end` (old columns) became
/// `new_len` characters.
fn region(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    (prefix, a.len() - suffix, b.len() - suffix - prefix)
}

impl App {
    fn row_chars(&self, row: usize) -> Vec<char> {
        self.ui.tab.editor.lines().get(row).map(|l| l.chars().collect()).unwrap_or_default()
    }

    // ---- tab: where a template may expand --------------------------------------------

    /// `tab` where a postfix template or an abbreviation is. Returns whether
    /// it expanded (or is waiting to).
    pub(super) fn tab_expands(&mut self, fx: &mut Effects) -> bool {
        if self.ui.tab.template.active() || self.ui.tab.editor.selection().is_some() {
            return false;
        }
        let caret = self.ui.tab.editor.cursor();
        let moved = self.completion_visible() && self.ui.tab.complete.popup.touched();
        if !moved {
            if let Some(line) = self.ui.tab.editor.lines().get(caret.row) {
                if let Some(site) = postfix_at(line, caret.col) {
                    self.expand_postfix(site, caret.row, fx);
                    return true;
                }
            }
        }
        self.expand_live(moved, fx)
    }

    fn expand_live(&mut self, moved: bool, fx: &mut Effects) -> bool {
        let caret = self.ui.tab.editor.cursor();
        let ctx = analyse(self.ui.tab.editor.lines(), caret);
        if ctx.literal || ctx.quoted || ctx.word.is_empty() || ctx.wants.qualified.is_some() || !ctx.statement_start {
            return false;
        }
        // A template the user has chosen in the list, or else the one the
        // word is exactly (even with another suggestion selected, unless the
        // user moved the selection to it).
        let chosen = self
            .completion_visible()
            .then(|| self.ui.tab.complete.popup.selected_item().filter(|i| i.kind == ItemKind::Template).and_then(|i| Template::named(&i.name)))
            .flatten();
        let exact = (!moved && ctx.word_end == caret).then(|| Template::named(&ctx.word)).flatten();
        let Some(template) = chosen.or(exact) else { return false };
        self.expand_template(template, ctx.word_start, ctx.word_end, fx);
        true
    }

    pub(super) fn expand_template(&mut self, t: &'static Template, from: Cursor, to: Cursor, fx: &mut Effects) {
        let exp = expand(t);
        if let Err(why) = self.ui.tab.editor.replace(from, to, &exp.text) {
            return self.notify(format!("Could not expand {}: {why}.", t.abbr), true, fx);
        }
        self.close_completion(fx);
        let at = |c: usize| from.col + c;
        let vars: Vec<Var> = exp
            .vars
            .into_iter()
            .map(|v| Var { spans: v.spans.into_iter().map(|(a, b)| (at(a), at(b))).collect(), ..v })
            .collect();
        self.ui.tab.template.expanded += 1;
        self.ui.tab.template.session = Some(Session {
            row: from.row,
            start: from.col,
            len: exp.text.chars().count(),
            vars,
            end: at(exp.end),
            current: 0,
            revision: self.ui.tab.editor.revision(),
        });
        self.move_to_var(0, fx);
        fx.repaint = true;
    }

    // ---- moving between the variables --------------------------------------------------

    /// The key of template mode, when it is the template's.
    pub(super) fn template_key(&mut self, combo: &KeyCombo, fx: &mut Effects) -> bool {
        let Some(action) = self.ui.keys.lookup_template(combo) else { return false };
        match action {
            // With the popup open these are its own: `tab` takes the table, `esc` closes it.
            Action::TemplateNext | Action::TemplateDone if self.completion_visible() => false,
            _ => {
                self.perform_template(action, fx);
                true
            }
        }
    }

    pub(super) fn perform_template(&mut self, action: Action, fx: &mut Effects) {
        if !self.ui.tab.template.active() {
            return;
        }
        match action {
            Action::TemplateNext => self.template_next(fx),
            Action::TemplatePrev => self.template_prev(fx),
            Action::TemplateDone => {
                self.close_completion(fx);
                self.leave_template(fx);
            }
            _ => {}
        }
    }

    fn template_next(&mut self, fx: &mut Effects) {
        let Some(current) = self.ui.tab.template.session.as_ref().map(|s| s.current) else { return };
        self.fill_columns_on_leaving(current, fx);
        let Some(s) = self.ui.tab.template.session.as_ref() else { return };
        if current + 1 < s.vars.len() {
            self.move_to_var(current + 1, fx);
        } else {
            let end = Cursor { row: s.row, col: s.end };
            self.close_completion(fx);
            self.leave_template(fx);
            self.ui.tab.editor.move_to(end);
            fx.repaint = true;
        }
    }

    fn template_prev(&mut self, fx: &mut Effects) {
        let Some(current) = self.ui.tab.template.session.as_ref().map(|s| s.current) else { return };
        self.move_to_var(current.saturating_sub(1), fx);
    }

    /// The caret on variable `index`: its text selected when it has any, so
    /// that typing replaces it.
    fn move_to_var(&mut self, index: usize, fx: &mut Effects) {
        let Some(s) = self.ui.tab.template.session.as_mut() else { return };
        s.current = index;
        let row = s.row;
        let Some(&(a, b)) = s.vars.get(index).and_then(|v| v.spans.iter().min_by_key(|(a, _)| *a)) else { return };
        self.close_completion(fx);
        if b > a {
            self.ui.tab.editor.select_range(Cursor { row, col: a }, Cursor { row, col: b });
        } else {
            self.ui.tab.editor.move_to(Cursor { row, col: a });
        }
        fx.repaint = true;
    }

    pub(super) fn leave_template(&mut self, fx: &mut Effects) {
        if self.ui.tab.template.session.take().is_some() {
            fx.repaint = true;
        }
        self.ui.tab.template.columns_for = None;
    }

    // ---- following the text --------------------------------------------------------------

    /// The row as it is before a key reaches the editor, while a template is on.
    pub(super) fn template_before(&self) -> Option<Before> {
        let s = self.ui.tab.template.session.as_ref()?;
        Some((s.row, self.ui.tab.editor.lines().get(s.row).cloned().unwrap_or_default(), self.ui.tab.editor.lines().len(), self.ui.tab.editor.revision()))
    }

    /// Accounts for what a key did to the row: the places of the variables
    /// move with the text, and typing in one place of a variable is typed in
    /// the others. A change the session cannot place, or a caret outside the
    /// template, ends template mode (the text stays).
    pub(super) fn track_template(&mut self, before: Option<Before>, fx: &mut Effects) {
        let Some((row, old, lines, revision)) = before else { return };
        if self.ui.tab.template.session.is_none() {
            return;
        }
        if self.ui.tab.editor.lines().len() != lines {
            return self.leave_template(fx);
        }
        if self.ui.tab.editor.revision() != revision {
            let new = self.ui.tab.editor.lines().get(row).cloned().unwrap_or_default();
            let (p, old_end, new_len) = region(&old, &new);
            if !self.apply_edit(p, old_end, new_len, fx) {
                return self.leave_template(fx);
            }
        }
        self.settle_session(fx);
    }

    /// A replacement made by the client in the row (a table accepted).
    fn template_replaced(&mut self, from: usize, to: usize, new_len: usize, fx: &mut Effects) {
        if self.ui.tab.template.session.is_some() && !self.apply_edit(from, to, new_len, fx) {
            self.leave_template(fx);
        }
        self.settle_session(fx);
    }

    fn settle_session(&mut self, fx: &mut Effects) {
        let caret = self.ui.tab.editor.cursor();
        let revision = self.ui.tab.editor.revision();
        let Some(s) = self.ui.tab.template.session.as_mut() else { return };
        s.revision = revision;
        if caret.row != s.row || caret.col < s.start || caret.col > s.start + s.len {
            self.leave_template(fx);
        }
    }

    /// The text of columns `from..to` of the session's row moved to `new_len`
    /// characters. Returns false when the edit is not inside one place of a
    /// variable.
    fn apply_edit(&mut self, p: usize, old_end: usize, new_len: usize, fx: &mut Effects) -> bool {
        let Some(s) = self.ui.tab.template.session.as_mut() else { return false };
        let inside = |(a, b): (usize, usize)| a <= p && old_end <= b;
        let mut found: Option<(usize, usize)> = None;
        let order = std::iter::once(s.current).chain((0..s.vars.len()).filter(|i| *i != s.current));
        for vi in order {
            if let Some(oi) = s.vars[vi].spans.iter().position(|sp| inside(*sp)) {
                found = Some((vi, oi));
                break;
            }
        }
        let Some((vi, oi)) = found else { return false };
        let delta = new_len as isize - (old_end - p) as isize;
        let shift = |c: usize| (c as isize + delta) as usize;
        for (i, v) in s.vars.iter_mut().enumerate() {
            for (j, sp) in v.spans.iter_mut().enumerate() {
                if (i, j) == (vi, oi) {
                    sp.1 = shift(sp.1);
                } else if sp.0 >= old_end {
                    *sp = (shift(sp.0), shift(sp.1));
                }
            }
        }
        if s.end >= old_end {
            s.end = shift(s.end);
        }
        s.len = shift(s.len);
        s.current = vi;
        if s.vars[vi].spans.len() > 1 {
            let (a, b) = s.vars[vi].spans[oi];
            let text: String = self.row_chars(self.ui.tab.template.session.as_ref().map_or(0, |s| s.row))[a..b].iter().collect();
            self.write_var(vi, &text, Some(oi), fx);
        }
        true
    }

    /// Puts `text` into every place of variable `vi` but `skip`, and the
    /// caret back where it was (moved by what was changed before it).
    fn write_var(&mut self, vi: usize, text: &str, skip: Option<usize>, fx: &mut Effects) {
        let Some(s) = self.ui.tab.template.session.as_ref() else { return };
        let row = s.row;
        let mut places: Vec<usize> = (0..s.vars[vi].spans.len()).filter(|j| Some(*j) != skip).collect();
        places.sort_by_key(|j| std::cmp::Reverse(s.vars[vi].spans[*j].0));
        let mut caret = self.ui.tab.editor.cursor();
        let new_len = text.chars().count();
        for j in places {
            let Some(s) = self.ui.tab.template.session.as_mut() else { return };
            let (a, b) = s.vars[vi].spans[j];
            let now: String = self.row_chars(row)[a..b].iter().collect();
            if now == text {
                continue;
            }
            if self.ui.tab.editor.replace(Cursor { row, col: a }, Cursor { row, col: b }, text).is_err() {
                return self.leave_template(fx);
            }
            let delta = new_len as isize - (b - a) as isize;
            let shift = |c: usize| (c as isize + delta) as usize;
            if caret.row == row && caret.col >= b {
                caret.col = shift(caret.col);
            }
            let Some(s) = self.ui.tab.template.session.as_mut() else { return };
            for (i, v) in s.vars.iter_mut().enumerate() {
                for (k, sp) in v.spans.iter_mut().enumerate() {
                    if (i, k) == (vi, j) {
                        sp.1 = a + new_len;
                    } else if sp.0 >= b {
                        *sp = (shift(sp.0), shift(sp.1));
                    }
                }
            }
            if s.end >= b {
                s.end = shift(s.end);
            }
            s.len = shift(s.len);
        }
        self.ui.tab.editor.move_to(caret);
        let revision = self.ui.tab.editor.revision();
        if let Some(s) = self.ui.tab.template.session.as_mut() {
            s.revision = revision;
        }
        fx.repaint = true;
    }

    /// Ends template mode when something other than the template's own
    /// keystrokes moved the text or the caret (a click, an undo, a paste).
    pub(super) fn template_housekeeping(&mut self, fx: &mut Effects) {
        let Some(s) = self.ui.tab.template.session.as_ref() else { return };
        let caret = self.ui.tab.editor.cursor();
        if s.revision != self.ui.tab.editor.revision()
            || self.ui.focus != PaneId::Editor
            || self.ui.overlay.is_some()
            || caret.row != s.row
            || caret.col < s.start
            || caret.col > s.start + s.len
        {
            self.leave_template(fx);
        }
    }

    // ---- a table accepted in a variable -----------------------------------------------------

    /// Whether the variable the caret is in names a table.
    pub(super) fn in_table_variable(&self) -> bool {
        self.ui.tab.template.in_table_variable()
    }

    /// The popup replaced `from..to` of the row with `new_len` characters in a
    /// variable. `table` is what was accepted, if it was a table.
    pub(super) fn template_accepted(&mut self, from: usize, to: usize, new_len: usize, table: Option<(String, Option<String>)>, fx: &mut Effects) {
        let table_var = self.in_table_variable();
        self.template_replaced(from, to, new_len, fx);
        let (Some((name, schema)), true) = (table, table_var) else { return };
        if let Some(vi) = self.var_index("alias") {
            let dialect = self.active_dialect();
            self.write_var(vi, &quote_identifier(&generate_alias(&name, std::slice::from_ref(&name)), dialect), None, fx);
        }
        if let Some(vi) = self.var_index("columns") {
            self.fill_columns(vi, schema.as_deref(), &name, fx);
        }
    }

    fn var_index(&self, name: &str) -> Option<usize> {
        self.ui.tab.template.session.as_ref()?.vars.iter().position(|v| v.name == name)
    }

    /// What the variable that names a table says now.
    fn table_variable_text(&self) -> String {
        let Some(vi) = self.ui.tab.template.session.as_ref().and_then(|s| s.vars.iter().position(|v| v.table)) else { return String::new() };
        self.var_text(vi).trim().to_string()
    }

    fn var_text(&self, vi: usize) -> String {
        let Some(s) = self.ui.tab.template.session.as_ref() else { return String::new() };
        let Some(&(a, b)) = s.vars.get(vi).and_then(|v| v.spans.first()) else { return String::new() };
        self.row_chars(s.row).get(a..b).map(|c| c.iter().collect()).unwrap_or_default()
    }

    /// The columns of a table go into the `columns` variable, now or when
    /// they have come.
    fn fill_columns(&mut self, vi: usize, schema: Option<&str>, table: &str, fx: &mut Effects) {
        if !self.core_names() {
            return;
        }
        let Some(db) = self.tab().target.clone() else { return };
        let waiting_for = |app: &mut App, needs: Vec<(Need, String)>, fx: &mut Effects| {
            if app.request_completion(needs, fx) {
                let asked_at = app.table_variable_text();
                app.ui.tab.template.columns_for = Some((schema.map(str::to_string), table.to_string(), asked_at));
            }
        };
        let Some(current) = self.model.completions.current_schema.get(&db).cloned() else {
            return waiting_for(self, vec![(Need::CurrentSchema(db), String::new())], fx);
        };
        let path = TablePath { schema: SchemaPath { database: db, schema: schema.map_or(current, str::to_string) }, table: table.to_string() };
        let mut needs = Vec::new();
        let cols = self.columns_of(&path, &mut needs);
        if cols.is_empty() {
            return waiting_for(self, needs, fx);
        }
        self.ui.tab.template.columns_for = None;
        let dialect = self.active_dialect();
        let list = cols.iter().map(|c| quote_identifier(&c.name, dialect)).collect::<Vec<_>>().join(", ");
        self.write_var(vi, &list, None, fx);
    }

    /// The user moves on from a table typed by hand: its columns go in too.
    fn fill_columns_on_leaving(&mut self, vi: usize, fx: &mut Effects) {
        let table_var = self.ui.tab.template.session.as_ref().is_some_and(|s| s.vars.get(vi).is_some_and(|v| v.table));
        let Some(cols) = self.var_index("columns").filter(|c| table_var && self.var_text(*c).is_empty()) else { return };
        if let Some((schema, table)) = table_of_expr(self.var_text(vi).trim()) {
            self.fill_columns(cols, schema.as_deref(), &table, fx);
        }
    }

    /// A reply came: a column list the template was waiting for may be made
    /// now, and so may a postfix expansion.
    pub(super) fn continue_template(&mut self, fx: &mut Effects) {
        if self.ui.tab.template.session.is_some() {
            if let (Some((schema, table, asked_at)), Some(vi)) = (self.ui.tab.template.columns_for.take(), self.var_index("columns")) {
                if self.var_text(vi).is_empty() {
                    // The reply is for the table that was in the variable when
                    // it was asked for. Another one is there now: it is the one
                    // whose columns are wanted, and are asked for.
                    let now = self.table_variable_text();
                    if now == asked_at {
                        self.fill_columns(vi, schema.as_deref(), &table, fx);
                    } else if let Some((schema, table)) = table_of_expr(&now) {
                        self.fill_columns(vi, schema.as_deref(), &table, fx);
                    }
                }
            }
        }
        self.continue_postfix(fx);
    }

    // ---- postfix expansion ------------------------------------------------------------------

    fn continue_postfix(&mut self, fx: &mut Effects) {
        let Some(w) = self.ui.tab.template.waiting.take() else { return };
        let caret = self.ui.tab.editor.cursor();
        if self.ui.tab.editor.revision() != w.revision || caret != w.caret || self.ui.focus != PaneId::Editor {
            return;
        }
        if let Some(site) = self.ui.tab.editor.lines().get(caret.row).and_then(|l| postfix_at(l, caret.col)) {
            self.expand_postfix(site, caret.row, fx);
        }
    }

    fn expand_postfix(&mut self, site: PostfixSite, row: usize, fx: &mut Effects) {
        self.ui.tab.template.waiting = None;
        let name = match site.kind {
            Postfix::From => ".from",
            Postfix::CFrom => ".cfrom",
            Postfix::Join => ".join",
            Postfix::Cast => ".cast",
        };
        let dialect = self.active_dialect();
        let expr = site.expr.clone();
        let (text, caret) = match site.kind {
            Postfix::From => (format!("select  from {expr}"), 7),
            Postfix::Cast => (format!("cast({expr} as )"), "cast(".len() + expr.chars().count() + " as ".len()),
            Postfix::CFrom | Postfix::Join => {
                if !self.core_names() {
                    return self.notify(format!("{name} needs a PostgreSQL connection for now."), true, fx);
                }
                let Some((schema, table)) = table_of_expr(&expr) else {
                    return self.notify(format!("{name} needs a table name before the dot."), true, fx);
                };
                let Some(db) = self.tab().target.clone() else {
                    return self.notify(format!("{name} needs a database: press u on one in the sidebar."), true, fx);
                };
                let Some(current) = self.model.completions.current_schema.get(&db).cloned() else {
                    let waiting = self.request_completion(vec![(Need::CurrentSchema(db), String::new())], fx);
                    return self.wait_for_names(waiting, name, fx);
                };
                let path = TablePath { schema: SchemaPath { database: db.clone(), schema: schema.clone().unwrap_or_else(|| current.clone()) }, table: table.clone() };
                if site.kind == Postfix::CFrom {
                    let mut needs = Vec::new();
                    let cols = self.columns_of(&path, &mut needs);
                    if cols.is_empty() {
                        let waiting = !needs.is_empty() && self.request_completion(needs, fx);
                        return self.wait_for_names(waiting, name, fx);
                    }
                    let list = cols.iter().map(|c| quote_identifier(&c.name, dialect)).collect::<Vec<_>>().join(", ");
                    let text = format!("select {list} from {expr}");
                    let caret = text.chars().count();
                    (text, caret)
                } else {
                    let cache = &self.model.completions;
                    let Some(keys) = cache.foreign_keys.get(&path).cloned().or_else(|| cache.failed.contains(&Need::ForeignKeys(path.clone())).then(Vec::new)) else {
                        let waiting = self.request_completion(vec![(Need::ForeignKeys(path), String::new())], fx);
                        return self.wait_for_names(waiting, name, fx);
                    };
                    match keys.first() {
                        None => {
                            let text = format!("from {expr} join  on ");
                            (text, "from ".len() + expr.chars().count() + " join ".len())
                        }
                        Some(first) => {
                            let group: Vec<_> = keys.iter().filter(|k| k.name == first.name).collect();
                            let q = |n: &str| quote_identifier(n, dialect);
                            let referenced = match &first.ref_schema {
                                Some(s) if *s != path.schema.schema => format!("{}.{}", q(s), q(&first.ref_table)),
                                _ => q(&first.ref_table),
                            };
                            let alias = q(&generate_alias(&first.ref_table, std::slice::from_ref(&table)));
                            let on = group.iter().map(|k| format!("{expr}.{} = {alias}.{}", q(&k.column), q(&k.ref_column))).collect::<Vec<_>>().join(" and ");
                            let text = format!("from {expr} join {referenced} {alias} on {on}");
                            let caret = text.chars().count();
                            (text, caret)
                        }
                    }
                }
            }
        };
        let (from, to) = (Cursor { row, col: site.start }, Cursor { row, col: site.end });
        match self.ui.tab.editor.replace(from, to, &text) {
            Ok(_) => {
                self.ui.tab.editor.move_to(Cursor { row, col: site.start + caret });
                self.ui.tab.template.expanded += 1;
            }
            Err(why) => self.notify(format!("Could not expand {name}: {why}."), true, fx),
        }
        self.close_completion(fx);
        fx.repaint = true;
    }

    /// Names are on their way: the expansion is made when they have come, if
    /// the text has not moved on. Otherwise they are not coming.
    fn wait_for_names(&mut self, waiting: bool, name: &str, fx: &mut Effects) {
        if waiting {
            self.ui.tab.template.waiting = Some(Waiting { revision: self.ui.tab.editor.revision(), caret: self.ui.tab.editor.cursor() });
        } else {
            self.notify(format!("Could not expand {name}: the names it needs are not known."), true, fx);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expansion_names_each_variable_once_with_all_its_places() {
        let e = expand(Template::named("selw").unwrap());
        assert_eq!(e.text, "select * from   where .;");
        let names: Vec<_> = e.vars.iter().map(|v| (v.name.as_str(), v.spans.clone())).collect();
        assert_eq!(names, [("table", vec![(14, 14)]), ("alias", vec![(15, 15), (22, 22)])]);
        assert_eq!(e.end, 23);
    }

    #[test]
    fn a_postfix_word_is_found_after_a_name_a_string_or_a_call() {
        let site = |l: &str| postfix_at(l, l.chars().count()).map(|s| (s.kind, s.start, s.expr));
        assert_eq!(site("x film.from"), Some((Postfix::From, 2, "film".into())));
        assert_eq!(site("public.film.cfrom"), Some((Postfix::CFrom, 0, "public.film".into())));
        assert_eq!(site("select lower(a).cast"), Some((Postfix::Cast, 7, "lower(a)".into())));
        assert_eq!(site("'x'.cast"), Some((Postfix::Cast, 0, "'x'".into())));
        assert_eq!(site("'x'.from"), None, "a string is not a table");
        assert_eq!(site("film.title"), None);
    }
}
