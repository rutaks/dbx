//! Semantic colouring (slice 12): in a statement dbx's analysis parsed, a
//! name that resolves to a column of the loaded schema takes the column role,
//! and a name that certainly resolves to nothing takes the unresolved role.
//!
//! The rule is that the client claims only what it knows, and does nothing
//! otherwise:
//!
//! - A table is unresolved only when it is written with a schema whose whole
//!   list of relations is loaded (from the search, which has partitions, not
//!   from the sidebar's listing, which leaves them out) and it is not in it.
//!   An unqualified name is never unresolved: it may be in another schema of
//!   the search path, a catalogue view, a temporary table or a partition.
//! - A column is unresolved only when the statement has no source the client
//!   cannot list the columns of (a function, `values`, a parenthesised join,
//!   `lateral`, a subquery or CTE with an item it cannot name), the sources
//!   the name could come from have their columns loaded, and the name is not a
//!   system column, a whole-row reference, or something a select list's own
//!   names can make of it. A statement with a set operation is left alone.
//! - Nothing is unresolved when the statement did not parse, when what it
//!   needs is not loaded or failed to load, or for kinds other than Postgres.
//!
//! The marks are worked out when the text, an analysis, the loaded schema or
//! the target changes (`refresh_marks`, at the end of `update`), and the view
//! only reads them. dbx's analysis is asked about a statement only once the
//! text has been still for a moment (`PAUSE`, a deadline and not a tick), and
//! what is worked out for a statement is kept by its text and the versions of
//! what it read, so working it out again is a lookup. Completion reads the
//! same analyses (it never asks for one).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::complete::Need;
use super::complete_context::{scan, Tok, Token};
use super::complete_scope::{same_name, text_between_rows, Clause, NamedColumn, Parsed, Scope, Source, TableName};
use super::theme::Role;
use super::{App, Effects};
use crate::core::{ColumnReference, Cursor, DatabasePath, Dialect, References, SchemaPath, Span, TablePath};

/// The most text, statements and column references looked at.
const MAX_TEXT: usize = 64 * 1024;
const MAX_STATEMENTS: usize = 24;
const MAX_STATEMENT: usize = 16 * 1024;
const MAX_COLUMNS: usize = 400;
/// How long the text is still before the analysis of it is asked for.
pub const PAUSE: Duration = Duration::from_millis(200);
/// The most statements' results kept.
const MAX_CACHED: usize = 256;

/// Names that are a column of every table, or a value and not a column.
const NOT_COLUMNS: &[&str] = &[
    "ctid", "xmin", "xmax", "cmin", "cmax", "tableoid", "oid", "current_date", "current_time", "current_timestamp", "localtime", "localtimestamp",
    "current_user", "session_user", "user", "current_role", "current_catalog", "current_schema", "system_user",
];
/// Qualifiers that are not tables of the statement: the row a conflict or a
/// rule gives.
const SPECIAL_QUALIFIERS: &[&str] = &["excluded", "new", "old"];

/// A coloured stretch of a row, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    revision: u64,
    cache: u64,
    tree: u64,
    target: Option<DatabasePath>,
    dialect: Option<Dialect>,
}

/// What a statement's marks depend on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StatementKey {
    db: DatabasePath,
    text: String,
    cache: u64,
    tree: u64,
}

/// A mark relative to where its statement starts: (row, start, end, role),
/// the columns counted from the statement's own first column on its first row.
type Relative = (usize, usize, usize, Role);
/// A mark in the buffer: (row, start, end, role).
type Absolute = (usize, usize, usize, Role);
type Needs = Vec<(Need, String)>;

#[derive(Debug, Default)]
pub struct Marks {
    key: Option<Key>,
    rows: HashMap<usize, Vec<Mark>>,
    /// Row and first column of each unresolved name, in text order.
    unresolved: Vec<(usize, usize)>,
    computed: u64,
    marked: u64,
    /// The revision of the text last looked at, and when it changed.
    seen: Option<u64>,
    edited_at: Option<Instant>,
    /// When the analyses the marks want may be asked for.
    due: Option<Instant>,
    cache: HashMap<StatementKey, Vec<Relative>>,
}

impl Marks {
    pub fn unresolved_count(&self) -> usize {
        self.unresolved.len()
    }

    /// How many times the marks were computed (for tests).
    pub fn computed(&self) -> u64 {
        self.computed
    }

    /// How many statements were worked out in full, and not found in the
    /// cache (for tests).
    pub fn statements_marked(&self) -> u64 {
        self.marked
    }

    /// When the app should wake to ask for what the marks are waiting to ask.
    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    /// The marks of a row, by start.
    pub fn row(&self, row: usize) -> &[Mark] {
        self.rows.get(&row).map_or(&[], Vec::as_slice)
    }
}

struct Statement {
    from: Cursor,
    text: String,
    names_a_table: bool,
    tokens: Vec<Token>,
}

fn statements(lines: &[String]) -> Vec<Statement> {
    let tokens = scan(lines);
    let sig: Vec<&Token> = tokens.iter().filter(|t| !matches!(t.tok, Tok::Comment(_))).collect();
    let mut out = Vec::new();
    for group in sig.split(|t| t.tok == Tok::Semi) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let names_a_table = group
            .iter()
            .any(|t| matches!(&t.tok, Tok::Word(w) if ["from", "update", "into", "join", "table"].contains(&w.to_lowercase().as_str())));
        out.push(Statement {
            from: first.start,
            text: text_between_rows(lines, first.start, last.end),
            names_a_table,
            tokens: group.iter().map(|t| (*t).clone()).collect(),
        });
    }
    out
}

/// What working out the marks gives, and what is missing to give more.
#[derive(Default)]
struct Outcome {
    found: Vec<(usize, usize, usize, Role)>,
    needs: Vec<(Need, String)>,
    /// The statements (by index) whose analysis is wanted and not at hand.
    parse: Vec<(usize, String)>,
    /// Results to keep: a statement that was worked out in full.
    keep: Vec<(StatementKey, Vec<Relative>)>,
    /// How many statements were worked out in full.
    worked: u64,
}

impl App {
    /// Brings the marks up to date when something they depend on changed, and
    /// asks for the analyses they want once the text has been still.
    pub(super) fn refresh_marks(&mut self, fx: &mut Effects) {
        let Some(now) = self.ui.now else { return };
        let dialect = self.active_dialect();
        let target = self.tab().target.clone();
        let key = Key {
            revision: self.ui.tab.editor.revision(),
            cache: self.model.completions.version,
            tree: self.model.schema.version(),
            target: target.clone(),
            dialect,
        };
        if self.ui.tab.marks.seen != Some(key.revision) {
            self.ui.tab.marks.seen = Some(key.revision);
            self.ui.tab.marks.edited_at = Some(now);
        }
        let due = self.ui.tab.marks.due.is_some_and(|d| now >= d);
        if self.ui.tab.marks.key.as_ref() == Some(&key) && !due {
            return;
        }
        self.ui.tab.marks.key = Some(key);
        self.ui.tab.marks.computed += 1;
        let outcome = match target {
            Some(db) if self.core_names() => self.compute_marks(&db),
            _ => Outcome::default(),
        };
        let Outcome { found, mut needs, parse, keep, worked } = outcome;
        self.ui.tab.marks.marked += worked;
        for (key, relative) in keep {
            if self.ui.tab.marks.cache.len() >= MAX_CACHED {
                self.ui.tab.marks.cache.clear();
            }
            self.ui.tab.marks.cache.insert(key, relative);
        }
        // What the text says about itself is asked for once it has been still.
        let still_since = self.ui.tab.marks.edited_at.unwrap_or(now);
        if parse.is_empty() {
            self.ui.tab.marks.due = None;
        } else if now >= still_since + PAUSE {
            self.ui.tab.marks.due = None;
            if let Some(db) = self.tab().target.clone() {
                needs.extend(parse.into_iter().map(|(i, text)| (Need::Parse(db.clone(), i), text)));
            }
        } else {
            self.ui.tab.marks.due = Some(still_since + PAUSE);
        }
        let mut rows: HashMap<usize, Vec<Mark>> = HashMap::new();
        let mut unresolved = Vec::new();
        for (row, start, end, role) in found {
            rows.entry(row).or_default().push(Mark { start, end, role });
            if role == Role::SynUnresolved {
                unresolved.push((row, start));
            }
        }
        for marks in rows.values_mut() {
            marks.sort_by_key(|m| m.start);
        }
        unresolved.sort_unstable();
        unresolved.dedup();
        if rows != self.ui.tab.marks.rows {
            fx.repaint = true;
        }
        self.ui.tab.marks.rows = rows;
        self.ui.tab.marks.unresolved = unresolved;
        if !needs.is_empty() {
            self.request_completion(needs, fx);
        }
    }

    fn compute_marks(&self, db: &DatabasePath) -> Outcome {
        let mut out = Outcome::default();
        let lines = self.ui.tab.editor.lines();
        if lines.iter().map(String::len).sum::<usize>() > MAX_TEXT {
            return out;
        }
        let all = statements(lines);
        let wanted: Vec<(usize, &Statement)> =
            all.iter().enumerate().take(MAX_STATEMENTS).filter(|(_, st)| st.names_a_table && st.text.len() <= MAX_STATEMENT).collect();
        // Nothing to resolve, nothing to ask.
        if wanted.is_empty() {
            return out;
        }
        let Some(current) = self.current_schema_of(db) else {
            out.needs.push((Need::CurrentSchema(db.clone()), String::new()));
            return out;
        };
        for (i, st) in wanted {
            let key = StatementKey { db: db.clone(), text: st.text.clone(), cache: self.model.completions.schema_version, tree: self.model.schema.version() };
            if let Some(relative) = self.ui.tab.marks.cache.get(&key) {
                out.found.extend(relative.iter().map(|&r| absolute(st.from, r)));
                continue;
            }
            let known = self.model.completions.parsed.get(&(db.clone(), st.text.clone()));
            let tab = self.ui.tab.id;
            let asked = self.model.completions.inflight.iter().any(|((t, n), (_, m))| *t == tab && matches!(n, Need::Parse(..)) && *m == st.text);
            let (marks, needs) = match known {
                None if asked => continue,
                None => {
                    out.parse.push((i, st.text.clone()));
                    continue;
                }
                // Does not parse: nothing is known about it.
                Some(None) => (Vec::new(), Vec::new()),
                Some(Some(refs)) => {
                    out.worked += 1;
                    self.mark_statement(db, &current, st, refs)
                }
            };
            let relative: Vec<Relative> = marks.iter().map(|&m| relative(st.from, m)).collect();
            out.found.extend(marks);
            if needs.is_empty() {
                out.keep.push((key, relative));
            }
            out.needs.extend(needs);
        }
        out
    }

    /// The tables of a schema, when the whole listing is loaded: the search's,
    /// which has the partitions the sidebar's listing leaves out. An empty
    /// list is not knowledge.
    fn known_relations(&self, db: &DatabasePath, schema: &str) -> Option<Vec<String>> {
        let path = SchemaPath { database: db.clone(), schema: schema.to_string() };
        let held = self.model.completions.relations.get(&path)?;
        (held.prefix.is_empty() && !held.incomplete && !held.found.is_empty()).then(|| held.found.iter().map(|c| c.name.clone()).collect())
    }

    /// The columns of a table the loaded schema has, or `None` when that is
    /// not (yet) known: its columns are not loaded, failed to load, or there
    /// are none (a table always has one).
    fn marked_columns(&self, db: &DatabasePath, current: &str, t: &TableName, needs: &mut Vec<(Need, String)>) -> Option<Vec<NamedColumn>> {
        let schema = t.schema.clone().unwrap_or_else(|| current.to_string());
        let path = TablePath { schema: SchemaPath { database: db.clone(), schema }, table: t.name.clone() };
        let cols = self.columns_of(&path, needs);
        (!cols.is_empty()).then(|| cols.into_iter().map(|c| (c.name, Some(c.db_type))).collect())
    }

    fn mark_statement(&self, db: &DatabasePath, current: &str, st: &Statement, refs: &References) -> (Vec<Absolute>, Needs) {
        let (mut out, mut needs) = (Vec::new(), Vec::new());
        let lines = self.ui.tab.editor.lines();
        let toks: Vec<&Token> = st.tokens.iter().collect();
        let parsed = Parsed::new(&toks, lines);
        // Whatever a set operation touches is left alone.
        if parsed.has_set_operation() {
            return (out, needs);
        }
        let origin = st.from;
        let shift = |c: Cursor| Cursor { row: origin.row + c.row, col: if c.row == 0 { origin.col + c.col } else { c.col } };
        // The part of a span that is the name: after the last dot.
        let range = |span: Span| -> Option<(usize, usize, usize)> {
            let (a, b) = (shift(span.start), shift(span.end));
            let line: Vec<char> = lines.get(a.row)?.chars().collect();
            if a.row != b.row || a.col >= b.col || b.col > line.len() {
                return None;
            }
            let from = line[a.col..b.col].iter().rposition(|c| *c == '.').map_or(a.col, |i| a.col + i + 1);
            (from < b.col).then_some((a.row, from, b.col))
        };
        // Only a table written with its schema, and found in the whole list of it.
        self.mark_tables(db, refs, &range, &mut out, &mut needs);
        // What the client can be sure of about the statement as a whole: it
        // reads every source of it, and dbx and the tokens see the same tables.
        let names = parsed.names();
        let same_tables = refs.tables.iter().all(|t| names.iter().any(|n| same_name(n, &t.name)) && t.alias.as_ref().is_none_or(|a| names.iter().any(|n| same_name(n, a))));
        let reliable = same_tables && !parsed.has_opaque_source();
        let j_of = |at: Cursor| toks.iter().position(|t| (t.start.row, t.start.col) >= (at.row, at.col)).unwrap_or(toks.len());
        for (qualifier, span) in &refs.unknown_qualifiers {
            if !reliable || SPECIAL_QUALIFIERS.iter().any(|q| same_name(q, qualifier)) || names.iter().any(|n| same_name(n, qualifier)) {
                continue;
            }
            let (a, b) = (shift(span.start), shift(span.end));
            if a.row == b.row && a.col < b.col {
                out.push((a.row, a.col, b.col, Role::SynUnresolved));
            }
        }
        for c in refs.columns.iter().take(MAX_COLUMNS) {
            if NOT_COLUMNS.iter().any(|n| same_name(n, &c.name)) {
                continue;
            }
            let at = shift(c.span.start);
            let scope = parsed.at(at, j_of(at));
            let Some(role) = self.column_role(db, current, &scope, c, reliable, &mut needs) else { continue };
            if let Some((r, a, b)) = range(c.span) {
                out.push((r, a, b, role));
            }
        }
        (out, needs)
    }

    fn mark_tables(
        &self,
        db: &DatabasePath,
        refs: &References,
        range: &dyn Fn(Span) -> Option<(usize, usize, usize)>,
        out: &mut Vec<(usize, usize, usize, Role)>,
        needs: &mut Vec<(Need, String)>,
    ) {
        for t in &refs.tables {
            // `pg_temp.t` is every session's own schema: no list is its list.
            let Some(schema) = t.schema.as_ref().filter(|s| !s.to_lowercase().starts_with("pg_temp") && !s.to_lowercase().starts_with("pg_toast_temp")) else {
                continue;
            };
            match self.known_relations(db, schema) {
                None => needs.push((Need::Relations(SchemaPath { database: db.clone(), schema: schema.clone() }), String::new())),
                Some(names) if !names.iter().any(|n| same_name(n, &t.name)) => {
                    if let Some((r, a, b)) = range(t.span) {
                        out.push((r, a, b, Role::SynUnresolved));
                    }
                }
                Some(_) => {}
            }
        }
    }

    fn column_role(&self, db: &DatabasePath, current: &str, scope: &Scope, c: &ColumnReference, reliable: bool, needs: &mut Vec<(Need, String)>) -> Option<Role> {
        let mut missing = false;
        let mut unlistable = false;
        let mut has = |source: &Source, needs: &mut Vec<(Need, String)>| -> bool {
            if !source.enumerable() {
                unlistable = true;
            }
            let cols = source.columns(&mut |t| {
                self.marked_columns(db, current, t, needs).unwrap_or_else(|| {
                    missing = true;
                    Vec::new()
                })
            });
            cols.iter().any(|(n, _)| same_name(n, &c.name))
        };
        match &c.qualifier {
            Some(q) => {
                let source = scope.find(q)?;
                if has(source, needs) {
                    Some(Role::SynColumn)
                } else if reliable && !missing && !unlistable {
                    Some(Role::SynUnresolved)
                } else {
                    None
                }
            }
            None => {
                // A whole-row reference (`row_to_json(f)`) names a source.
                if scope.levels.iter().flat_map(|l| l.sources.iter()).any(|s| same_name(&s.label, &c.name)) {
                    return None;
                }
                // The select list's own names are usable (in `order by` and
                // `group by`, and at the client's leniency elsewhere); one
                // the client cannot name makes the rest unknowable there.
                if scope.select_items.iter().any(|i| i.alias.as_deref().into_iter().chain(i.name.as_deref().filter(|_| !i.plain)).any(|a| same_name(a, &c.name))) {
                    return None;
                }
                let in_list_clause = matches!(scope.clause(), Clause::GroupBy | Clause::OrderBy);
                let sources: Vec<&Source> = scope.levels.iter().flat_map(|l| l.sources.iter()).collect();
                let mut found = false;
                for s in &sources {
                    found |= has(s, needs);
                }
                if found {
                    Some(Role::SynColumn)
                } else if reliable && !missing && !unlistable && !sources.is_empty() && !(in_list_clause && scope.select_uncertain) {
                    Some(Role::SynUnresolved)
                } else {
                    None
                }
            }
        }
    }

    /// `F2` and `shift+F2`: the caret goes to the next or previous unresolved
    /// name, wrapping round the buffer.
    pub(super) fn goto_unresolved(&mut self, forward: bool, fx: &mut Effects) {
        let caret = self.ui.tab.editor.cursor();
        let at = (caret.row, caret.col);
        let list = &self.ui.tab.marks.unresolved;
        let pick = if forward {
            list.iter().find(|p| **p > at).or(list.first())
        } else {
            list.iter().rev().find(|p| **p < at).or(list.last())
        };
        if let Some(&(row, col)) = pick {
            self.ui.tab.editor.move_to(Cursor { row, col });
            fx.repaint = true;
        }
    }
}

/// A mark relative to the start of its statement.
fn relative(origin: Cursor, (row, start, end, role): (usize, usize, usize, Role)) -> Relative {
    let col = |r: usize, c: usize| if r == origin.row { c - origin.col } else { c };
    (row - origin.row, col(row, start), col(row, end), role)
}

fn absolute(origin: Cursor, (row, start, end, role): Relative) -> (usize, usize, usize, Role) {
    let col = |c: usize| if row == 0 { origin.col + c } else { c };
    (origin.row + row, col(start), col(end), role)
}
