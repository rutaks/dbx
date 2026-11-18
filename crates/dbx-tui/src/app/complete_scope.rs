//! What is in scope at the caret: the tables, aliases, subquery aliases and CTE
//! names the statement under the caret can refer to, and what columns each
//! one exposes.
//!
//! Two sources behind one `Scope`. The tokens of the statement (it works on
//! SQL that does not parse, which is what text is while it is typed) are read
//! into a tree of queries, each with its select blocks, CTEs and subqueries;
//! the caret's place in that tree gives the visible sources, innermost block
//! first. When dbx's analysis of the statement is available (it needs SQL that
//! parses) `Scope::apply_analysis` replaces the tables the tokens found with
//! the ones dbx found, block by block. CTEs and subquery aliases are never in
//! dbx's answer, so they always come from the tokens.
//!
//! Pure: tokens and lines in, a `Scope` out. Where the columns of a table come
//! from is the caller's business (`Source::columns` takes a lookup).

use super::complete_context::{lower, name_of, Tok, Token};
use super::highlight::is_keyword;
use crate::core::{Cursor, ForeignKey, References};

/// A table named in the statement, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableName {
    pub schema: Option<String>,
    pub name: String,
}

/// What a source gives columns of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    Table(TableName),
    /// A subquery in `from` (with its alias) or a CTE that is named in it.
    Derived(Derived),
}

/// One column of a derived table, or a `*` that stands for several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedColumn {
    Name(String),
    /// `*` or `q.*`: the columns of the sources inside.
    Star(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Derived {
    pub columns: Vec<DerivedColumn>,
    /// The sources of the query it is made from, for a `*`.
    pub inner: Vec<Source>,
    /// Every column it exposes is known by name: the query is one select whose
    /// items are all named (or are a `*` over sources that are known in turn).
    /// Where it is false, the columns listed are what the client could read
    /// and not everything there is.
    pub certain: bool,
}

/// Something a column can be taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// What a column is qualified with: the alias, else the table's own name,
    /// else the CTE's.
    pub label: String,
    pub aliased: bool,
    pub kind: SourceKind,
    /// Where it stands in the text.
    pub at: Cursor,
}

/// A column and its type, when known.
pub type NamedColumn = (String, Option<String>);

impl Source {
    /// Whether the client can say which columns this source has, given the
    /// columns of its tables: a table always can; a derived table only when
    /// every one of its columns is named.
    pub fn enumerable(&self) -> bool {
        match &self.kind {
            SourceKind::Table(_) => true,
            SourceKind::Derived(d) => d.certain,
        }
    }

    /// The columns this source exposes. `table` gives those of a table.
    pub fn columns(&self, table: &mut dyn FnMut(&TableName) -> Vec<NamedColumn>) -> Vec<NamedColumn> {
        match &self.kind {
            SourceKind::Table(t) => table(t),
            SourceKind::Derived(d) => d.columns(table),
        }
    }
}

impl Derived {
    pub fn columns(&self, table: &mut dyn FnMut(&TableName) -> Vec<NamedColumn>) -> Vec<NamedColumn> {
        let mut out = Vec::new();
        for c in &self.columns {
            match c {
                DerivedColumn::Name(n) => out.push((n.clone(), None)),
                DerivedColumn::Star(q) => {
                    for s in self.inner.iter().filter(|s| q.as_ref().is_none_or(|q| same_name(&s.label, q))) {
                        out.extend(s.columns(&mut *table));
                    }
                }
            }
        }
        out
    }
}

/// Names are compared without regard to case: Postgres folds what is not
/// quoted, and a quoted name that differs only in case is rare.
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.eq_ignore_ascii_case(b)
}

/// The sources visible in one select block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Level {
    pub sources: Vec<Source>,
    /// From the block's first token to its last.
    pub range: (Cursor, Cursor),
    /// The subqueries inside it, parentheses included.
    pub children: Vec<(Cursor, Cursor)>,
    /// A source of the block is not a table or a subquery the client can read:
    /// a function call, `values`, a parenthesised join, `lateral`.
    pub opaque: bool,
    /// The block joins with `using` or `natural`, which merge columns.
    pub merges: bool,
}

/// The clause the caret is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clause {
    /// Before any: the start of a statement.
    None,
    Select,
    /// `from`, `join`, `update`: a table is expected.
    From,
    Into,
    Where,
    On,
    GroupBy,
    OrderBy,
    Having,
    Set,
    Returning,
    Values,
    Other,
}

impl Clause {
    /// Whether a column can stand here.
    pub fn column_position(self) -> bool {
        matches!(self, Clause::Select | Clause::Where | Clause::On | Clause::GroupBy | Clause::OrderBy | Clause::Having | Clause::Set | Clause::Returning)
    }
}

/// One expression of the select list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    /// The expression as written, on one line.
    pub expr: String,
    pub alias: Option<String>,
    /// A bare column (`title`, `f.title`).
    pub plain: bool,
    /// The name the column has in the result, when the client can tell.
    pub name: Option<String>,
}

/// A `*` of a select list under the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Star {
    /// From the qualifier (`f.*`) or the `*` to the end of the `*`.
    pub start: Cursor,
    pub end: Cursor,
    pub qualifier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    /// The sources visible, the block the caret is in first and the blocks
    /// around it after (what a subquery may refer to). Empty before anything is
    /// named.
    pub levels: Vec<Level>,
    pub clause: Option<Clause>,
    /// What stands before the caret ends an expression, so an operator is what
    /// comes next, and not an operand.
    pub after_operand: bool,
    /// The word is the first thing after `on`.
    pub after_on: bool,
    /// The caret is in the column list of `insert into t (`.
    pub insert_into: Option<TableName>,
    /// The select list of the block the caret is in.
    pub select_items: Vec<SelectItem>,
    /// The `*` the caret is on or beside, in a select list.
    pub star: Option<Star>,
    /// The CTEs defined before the caret and visible to it.
    pub ctes: Vec<(String, Derived)>,
    /// Every alias, table and CTE name of the statement.
    pub names: Vec<String>,
    /// The first and last position of the statement's tokens.
    pub text: Option<(Cursor, Cursor)>,
    /// dbx's analysis supplied the tables.
    pub from_dbx: bool,
    /// The select list of the block the caret is in has a `*` or an item the
    /// client cannot name.
    pub select_uncertain: bool,
}

impl Scope {
    pub fn clause(&self) -> Clause {
        self.clause.unwrap_or(Clause::None)
    }

    /// Moves every position in the scope down by `by` rows: the scope was
    /// read from a slice of the buffer that starts `by` rows in.
    pub fn shift_rows(&mut self, by: usize) {
        if by == 0 {
            return;
        }
        let down = |c: &mut Cursor| c.row += by;
        fn source(s: &mut Source, by: usize) {
            s.at.row += by;
            if let SourceKind::Derived(d) = &mut s.kind {
                derived(d, by);
            }
        }
        fn derived(d: &mut Derived, by: usize) {
            d.inner.iter_mut().for_each(|s| source(s, by));
        }
        for level in &mut self.levels {
            level.sources.iter_mut().for_each(|s| source(s, by));
            down(&mut level.range.0);
            down(&mut level.range.1);
            for (a, b) in &mut level.children {
                down(a);
                down(b);
            }
        }
        if let Some(star) = &mut self.star {
            down(&mut star.start);
            down(&mut star.end);
        }
        if let Some((a, b)) = &mut self.text {
            down(a);
            down(b);
        }
        for (_, d) in &mut self.ctes {
            derived(d, by);
        }
    }

    /// The sources of the block the caret is in.
    pub fn sources(&self) -> &[Source] {
        self.levels.first().map_or(&[], |l| &l.sources)
    }

    /// The source a qualifier names: the innermost one first.
    pub fn find(&self, qualifier: &str) -> Option<&Source> {
        self.levels.iter().flat_map(|l| l.sources.iter()).find(|s| same_name(&s.label, qualifier))
    }

    /// Whether any source is in view.
    pub fn has_sources(&self) -> bool {
        self.levels.iter().any(|l| !l.sources.is_empty())
    }

    /// The text of the statement, from `lines`.
    pub fn statement_text(&self, lines: &[String]) -> Option<String> {
        let (from, to) = self.text?;
        Some(text_between_rows(lines, from, to))
    }

    /// Replaces the tables the tokens found with the ones dbx's analysis
    /// found, block by block. `origin` is where the analysed text starts in the
    /// buffer; `skip` the start of the word being typed, which is not a
    /// reference yet. A block dbx found no table in keeps what the tokens saw
    /// (dbx's analysis covers queries only: an `update` comes back empty).
    pub fn apply_analysis(&mut self, refs: &References, origin: Cursor, skip: Cursor) {
        let shift = |c: Cursor| Cursor { row: origin.row + c.row, col: if c.row == 0 { origin.col + c.col } else { c.col } };
        let inside = |c: Cursor, (from, to): (Cursor, Cursor)| (from.row, from.col) <= (c.row, c.col) && (c.row, c.col) <= (to.row, to.col);
        let mut used = false;
        for level in &mut self.levels {
            let found: Vec<Source> = refs
                .tables
                .iter()
                .filter(|t| {
                    let at = shift(t.span.start);
                    at != skip && inside(at, level.range) && !level.children.iter().any(|c| inside(at, *c))
                })
                .filter(|t| t.schema.is_some() || !level.sources.iter().any(|s| matches!(s.kind, SourceKind::Derived(_)) && same_name(&s.label, &t.name)))
                .map(|t| Source {
                    label: t.alias.clone().unwrap_or_else(|| t.name.clone()),
                    aliased: t.alias.is_some(),
                    kind: SourceKind::Table(TableName { schema: t.schema.clone(), name: t.name.clone() }),
                    at: shift(t.span.start),
                })
                .collect();
            if found.is_empty() {
                continue;
            }
            used = true;
            level.sources.retain(|s| !matches!(s.kind, SourceKind::Table(_)));
            level.sources.extend(found);
            level.sources.sort_by_key(|s| (s.at.row, s.at.col));
        }
        self.from_dbx = used;
    }

    /// Whether `keyword` can come next here: what a keyword that starts like
    /// the word is ranked by.
    pub fn keyword_is_valid(&self, keyword: &str) -> bool {
        const STARTERS: &[&str] = &[
            "select", "insert", "update", "delete", "with", "create", "alter", "drop", "truncate", "explain", "values", "table", "begin",
            "commit", "rollback", "grant", "revoke", "copy", "set", "show", "analyze", "vacuum", "call", "do",
        ];
        const OPERANDS: &[&str] = &["not", "null", "true", "false", "case", "exists", "interval", "array", "row", "cast", "current_date", "current_time", "current_timestamp", "current_user"];
        const OPERATORS: &[&str] = &["and", "or", "is", "in", "not", "like", "ilike", "between", "similar", "isnull", "notnull", "collate", "at", "over", "filter"];
        const ENDERS: &[&str] = &["group", "order", "having", "limit", "offset", "union", "intersect", "except", "window", "fetch", "for", "returning"];
        let k = keyword;
        let any = |l: &[&str]| l.contains(&k);
        match self.clause() {
            Clause::None => any(STARTERS),
            Clause::Select if self.after_operand => any(OPERATORS) || k == "as" || k == "from",
            Clause::Select => any(OPERANDS) || k == "distinct" || k == "all",
            Clause::Where | Clause::On | Clause::Having | Clause::Set if self.after_operand => any(OPERATORS) || any(ENDERS) || k == "where" && self.clause() == Clause::Set,
            Clause::GroupBy if self.after_operand => matches!(k, "having" | "order" | "limit" | "offset" | "union" | "intersect" | "except" | "window" | "fetch" | "for"),
            Clause::OrderBy if self.after_operand => matches!(k, "asc" | "desc" | "nulls" | "using" | "limit" | "offset" | "fetch" | "for" | "union" | "intersect" | "except"),
            Clause::Where | Clause::On | Clause::Having | Clause::Set | Clause::GroupBy | Clause::OrderBy | Clause::Returning => any(OPERANDS),
            _ => false,
        }
    }
}

pub(super) fn text_between_rows(lines: &[String], from: Cursor, to: Cursor) -> String {
    let mut out = String::new();
    for row in from.row..=to.row {
        let line: Vec<char> = lines.get(row).map(|l| l.chars().collect()).unwrap_or_default();
        let a = if row == from.row { from.col.min(line.len()) } else { 0 };
        let b = if row == to.row { to.col.min(line.len()) } else { line.len() };
        if row > from.row {
            out.push('\n');
        }
        out.extend(line[a..b.max(a)].iter());
    }
    out
}

// ---- reading the tokens -----------------------------------------------------

/// A select block: one `select ... from ...` (or `update`, `insert`...), which
/// a set operator ends.
struct Spec {
    lo: usize,
    hi: usize,
    sources: Vec<SrcDef>,
    subqueries: Vec<Sub>,
    items: Vec<Item>,
    /// Where a clause begins, outside any parenthesis.
    marks: Vec<(usize, Clause)>,
    /// `insert into <table> (`: the table, and where the `(` is.
    into: Option<(TableName, usize)>,
    /// A source is not a table or a subquery: a function, `values`, a
    /// parenthesised join, `lateral`.
    opaque: bool,
    /// `using (...)` or `natural` merges columns of the joined sources.
    merges: bool,
    /// The block has a select list (`values (1)` and `table t` do not).
    has_select: bool,
}

struct Sub {
    query: Query,
    /// Indices of the `(` and its `)` (or the end of the block).
    open: usize,
    close: usize,
    /// It cannot see the tables beside it: a derived table without `lateral`.
    isolated: bool,
}

enum SrcDef {
    Table { table: TableName, alias: Option<String>, at: usize },
    Derived { alias: Option<String>, columns: Vec<String>, sub: usize, at: usize },
}

struct Item {
    lo: usize,
    hi: usize,
    alias: Option<String>,
    /// The name the column has in the result.
    name: Option<String>,
    /// `*` or `q.*`, with the qualifier.
    star: Option<Option<String>>,
    plain: bool,
}

struct Cte {
    name: String,
    columns: Vec<String>,
    query: Query,
}

struct Query {
    /// The tokens inside, `hi` being the `)` or the end.
    lo: usize,
    hi: usize,
    ctes: Vec<Cte>,
    specs: Vec<Spec>,
}

struct Reader<'a> {
    t: &'a [&'a Token],
    lines: &'a [String],
    names: Vec<String>,
}

fn clause_of(word: &str) -> Option<Clause> {
    Some(match word {
        "select" => Clause::Select,
        "from" | "join" | "update" => Clause::From,
        "into" => Clause::Into,
        "where" => Clause::Where,
        "on" => Clause::On,
        "group" => Clause::GroupBy,
        "order" => Clause::OrderBy,
        "having" => Clause::Having,
        "set" => Clause::Set,
        "returning" => Clause::Returning,
        "values" => Clause::Values,
        "limit" | "offset" | "fetch" | "window" | "for" | "using" => Clause::Other,
        _ => return None,
    })
}

impl<'a> Reader<'a> {
    fn word(&self, i: usize, w: &str) -> bool {
        self.t.get(i).and_then(|t| lower(t)).is_some_and(|x| x == w)
    }

    fn is_star(&self, i: usize) -> bool {
        self.t.get(i).is_some_and(|t| {
            t.tok == Tok::Other && self.lines.get(t.start.row).and_then(|l| l.chars().nth(t.start.col)) == Some('*')
        })
    }

    /// The index of the `)` that closes the `(` at `open`, or `hi`.
    fn close(&self, open: usize, hi: usize) -> usize {
        let mut depth = 0usize;
        for k in open..hi {
            match self.t[k].tok {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return k;
                    }
                }
                _ => {}
            }
        }
        hi
    }

    /// The text of tokens `lo..hi` on one line.
    fn text(&self, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        for k in lo..hi {
            let (a, b) = (self.t[k].start, self.t[k].end);
            let line: Vec<char> = self.lines.get(a.row).map(|l| l.chars().collect()).unwrap_or_default();
            let piece: String = if a.row == b.row { line[a.col.min(line.len())..b.col.min(line.len())].iter().collect() } else { String::new() };
            if k > lo && self.t[k - 1].end != a {
                out.push(' ');
            }
            out.push_str(&piece);
        }
        out
    }

    fn query(&mut self, lo: usize, hi: usize) -> Query {
        let mut i = lo;
        let mut ctes = Vec::new();
        if self.word(i, "with") {
            i += 1;
            if self.word(i, "recursive") {
                i += 1;
            }
            while i < hi {
                let Some(name) = name_of(self.t[i]).map(String::from) else { break };
                self.names.push(name.clone());
                i += 1;
                let mut columns = Vec::new();
                if i < hi && self.t[i].tok == Tok::LParen && !self.word(i + 1, "select") {
                    let close = self.close(i, hi);
                    columns = (i + 1..close).filter_map(|k| name_of(self.t[k]).map(String::from)).collect();
                    i = (close + 1).min(hi);
                }
                if self.word(i, "as") {
                    i += 1;
                }
                if self.word(i, "not") {
                    i += 1;
                }
                if self.word(i, "materialized") {
                    i += 1;
                }
                if i < hi && self.t[i].tok == Tok::LParen {
                    let close = self.close(i, hi);
                    let query = self.query(i + 1, close);
                    ctes.push(Cte { name, columns, query });
                    i = (close + 1).min(hi);
                } else {
                    break;
                }
                if i < hi && self.t[i].tok == Tok::Comma {
                    i += 1;
                } else {
                    break;
                }
            }
        }
        // The blocks, split at the set operators outside parentheses.
        let mut specs = Vec::new();
        let (mut start, mut depth) = (i, 0usize);
        let mut k = i;
        while k < hi {
            match self.t[k].tok {
                Tok::LParen => depth += 1,
                Tok::RParen => depth = depth.saturating_sub(1),
                _ => {}
            }
            if depth == 0 && ["union", "intersect", "except"].iter().any(|w| self.word(k, w)) {
                specs.push(self.spec(start, k));
                k += 1;
                if self.word(k, "all") || self.word(k, "distinct") {
                    k += 1;
                }
                start = k;
                continue;
            }
            k += 1;
        }
        specs.push(self.spec(start, hi.max(start)));
        Query { lo, hi, ctes, specs }
    }

    /// The `(` of a function call at `i` (`name(`, `schema.name(`, after
    /// `lateral`), if that is what stands there.
    fn call_at(&self, mut i: usize, hi: usize) -> Option<usize> {
        while i < hi && (self.word(i, "only") || self.word(i, "lateral")) {
            i += 1;
        }
        let start = i;
        while i < hi && name_of(self.t[i]).is_some() {
            i += 1;
            if i < hi && self.t[i].tok == Tok::Dot {
                i += 1;
            } else {
                break;
            }
        }
        (i > start && i < hi && self.t[i].tok == Tok::LParen && name_of(self.t[i - 1]).is_some()).then_some(i)
    }

    /// A table name at `i` (`schema.name`, after `only` and `lateral`), with an
    /// alias when `alias_ok`. Returns it with the index after it.
    fn table_at(&mut self, mut i: usize, hi: usize, alias_ok: bool) -> Option<(TableName, Option<String>, usize, usize)> {
        while i < hi && (self.word(i, "only") || self.word(i, "lateral")) {
            i += 1;
        }
        let at = i;
        let mut parts: Vec<String> = Vec::new();
        while i < hi {
            let Some(name) = name_of(self.t[i]) else { break };
            parts.push(name.to_string());
            i += 1;
            if i < hi && self.t[i].tok == Tok::Dot {
                i += 1;
                if !(i < hi && name_of(self.t[i]).is_some()) {
                    return None;
                }
            } else {
                break;
            }
        }
        let name = parts.pop()?;
        let schema = parts.pop();
        let mut alias = None;
        if self.word(i, "as") {
            alias = (i + 1 < hi).then(|| name_of(self.t[i + 1]).map(String::from)).flatten();
            i += if alias.is_some() { 2 } else { 1 };
        } else if alias_ok && i < hi {
            let ok = match &self.t[i].tok {
                Tok::Word(w) => !is_keyword(&w.to_lowercase()),
                Tok::Quoted(..) => true,
                _ => false,
            };
            if ok {
                alias = name_of(self.t[i]).map(String::from);
                i += 1;
            }
        }
        Some((TableName { schema, name }, alias, at, i))
    }

    fn spec(&mut self, lo: usize, hi: usize) -> Spec {
        let mut spec = Spec {
            lo,
            hi,
            sources: Vec::new(),
            subqueries: Vec::new(),
            items: Vec::new(),
            marks: Vec::new(),
            into: None,
            opaque: false,
            merges: false,
            has_select: false,
        };
        let (mut i, mut depth, mut clause, mut list) = (lo, 0usize, Clause::None, false);
        let mut select_from: Option<usize> = None;
        let mut select_to: Option<usize> = None;
        while i < hi {
            let tk: &'a Token = self.t[i];
            match &tk.tok {
                Tok::LParen => {
                    let close = self.close(i, hi);
                    let lateral = i > lo && self.word(i - 1, "lateral");
                    let prev_from = i > lo && (self.word(i - 1, "from") || self.word(i - 1, "join") || lateral);
                    let prev_comma = i > lo && self.t[i - 1].tok == Tok::Comma && list;
                    let table_pos = depth == 0 && clause == Clause::From && (prev_from || prev_comma);
                    if self.word(i + 1, "select") || self.word(i + 1, "with") {
                        let query = self.query(i + 1, close);
                        let sub = spec.subqueries.len();
                        spec.subqueries.push(Sub { query, open: i, close, isolated: table_pos && !lateral });
                        let at = i;
                        i = (close + 1).min(hi);
                        if table_pos {
                            let mut alias = None;
                            let mut j = i;
                            if self.word(j, "as") {
                                j += 1;
                            }
                            if j < hi {
                                let ok = match &self.t[j].tok {
                                    Tok::Word(w) => !is_keyword(&w.to_lowercase()),
                                    Tok::Quoted(..) => true,
                                    _ => false,
                                };
                                if ok {
                                    alias = name_of(self.t[j]).map(String::from);
                                    j += 1;
                                }
                            }
                            let mut columns = Vec::new();
                            if alias.is_some() && j < hi && self.t[j].tok == Tok::LParen {
                                let c = self.close(j, hi);
                                columns = (j + 1..c).filter_map(|k| name_of(self.t[k]).map(String::from)).collect();
                                j = (c + 1).min(hi);
                            }
                            if let Some(a) = &alias {
                                self.names.push(a.clone());
                            }
                            spec.sources.push(SrcDef::Derived { alias, columns, sub, at });
                            i = j;
                        }
                        continue;
                    }
                    // `from (values ...) v`, `from (a join b on ...)`: a source
                    // the client does not read.
                    if table_pos {
                        spec.opaque = true;
                    }
                    depth += 1;
                    i += 1;
                }
                Tok::RParen => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
                Tok::Comma if depth == 0 && list && matches!(clause, Clause::From | Clause::On) => {
                    clause = Clause::From;
                    spec.marks.push((i, Clause::From));
                    i += 1;
                    i = self.take_table(&mut spec, i, hi, true);
                }
                Tok::Word(w) if depth == 0 => {
                    let w = w.to_lowercase();
                    i += 1;
                    if w == "natural" || (w == "using" && self.t.get(i).is_some_and(|t| t.tok == Tok::LParen)) {
                        spec.merges = true;
                    }
                    if w == "lateral" {
                        spec.opaque = true;
                    }
                    let Some(next) = clause_of(&w) else { continue };
                    if w == "using" && self.t.get(i).is_some_and(|t| t.tok == Tok::LParen) {
                        continue;
                    }
                    if (w == "group" || w == "order") && !self.word(i, "by") {
                        continue;
                    }
                    if clause == Clause::Select && select_to.is_none() {
                        select_to = Some(i - 1);
                    }
                    if next == Clause::Select {
                        select_from = Some(i);
                    }
                    clause = next;
                    spec.marks.push((i - 1, next));
                    match w.as_str() {
                        "from" | "update" => {
                            list = true;
                            i = self.take_table(&mut spec, i, hi, true);
                        }
                        "join" => i = self.take_table(&mut spec, i, hi, true),
                        "into" => {
                            list = false;
                            if let Some((table, alias, at, next_i)) = self.table_at(i, hi, false) {
                                spec.into = Some((table.clone(), next_i));
                                self.names.push(table.name.clone());
                                spec.sources.push(SrcDef::Table { table, alias, at });
                                i = next_i;
                            }
                        }
                        "select" | "where" | "group" | "order" | "having" | "set" | "returning" | "values" | "limit" | "offset" | "fetch" | "window" | "for" => {
                            list = false;
                        }
                        _ => {}
                    }
                }
                _ => i += 1,
            }
        }
        if let Some(from) = select_from {
            let to = select_to.unwrap_or(hi).min(hi);
            spec.items = self.items(from, to);
            spec.has_select = true;
        }
        spec
    }

    fn take_table(&mut self, spec: &mut Spec, i: usize, hi: usize, alias_ok: bool) -> usize {
        // A name (with its schema) followed by `(` is a function call, which
        // gives columns the client cannot list.
        if let Some(open) = self.call_at(i, hi) {
            spec.opaque = true;
            return (self.close(open, hi) + 1).min(hi);
        }
        // A subquery here is the caller's: it does not start with a name.
        match self.table_at(i, hi, alias_ok) {
            Some((table, alias, at, next)) => {
                self.names.push(alias.clone().unwrap_or_else(|| table.name.clone()));
                self.names.push(table.name.clone());
                spec.sources.push(SrcDef::Table { table, alias, at });
                next
            }
            None => i,
        }
    }

    /// The expressions of a select list `lo..hi`, split at commas outside parentheses.
    fn items(&self, mut lo: usize, hi: usize) -> Vec<Item> {
        if self.word(lo, "distinct") || self.word(lo, "all") {
            lo += 1;
            if self.word(lo, "on") && self.t.get(lo + 1).is_some_and(|t| t.tok == Tok::LParen) {
                lo = (self.close(lo + 1, hi) + 1).min(hi);
            }
        }
        let mut out = Vec::new();
        let (mut start, mut depth) = (lo, 0usize);
        for k in lo..=hi {
            let end = k == hi;
            if !end {
                match self.t[k].tok {
                    Tok::LParen => depth += 1,
                    Tok::RParen => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            if end || (depth == 0 && self.t[k].tok == Tok::Comma) {
                if k > start {
                    out.push(self.item(start, k));
                }
                start = k + 1;
            }
        }
        out
    }

    fn item(&self, lo: usize, hi: usize) -> Item {
        // `*` and `q.*`.
        if self.is_star(hi - 1) && (hi - lo == 1 || (hi - lo == 3 && name_of(self.t[lo]).is_some() && self.t[lo + 1].tok == Tok::Dot)) {
            let qualifier = (hi - lo == 3).then(|| name_of(self.t[lo]).map(String::from)).flatten();
            return Item { lo, hi, alias: None, name: None, star: Some(qualifier), plain: false };
        }
        // An alias: `as name`, or a name after something that ends an expression.
        let mut expr_hi = hi;
        let mut alias = None;
        if hi - lo >= 3 && self.word(hi - 2, "as") && name_of(self.t[hi - 1]).is_some() {
            alias = name_of(self.t[hi - 1]).map(String::from);
            expr_hi = hi - 2;
        } else if hi - lo >= 2 {
            let last = self.t[hi - 1];
            let named = match &last.tok {
                Tok::Word(w) => !is_keyword(&w.to_lowercase()),
                Tok::Quoted(..) => true,
                _ => false,
            };
            let before = &self.t[hi - 2].tok;
            let ends = match before {
                Tok::Word(w) => !matches!(w.to_lowercase().as_str(), "and" | "or" | "not" | "is" | "in" | "like" | "between" | "case" | "when" | "then" | "else" | "distinct"),
                Tok::Quoted(..) | Tok::Number | Tok::Str(_) | Tok::RParen => true,
                _ => false,
            };
            if named && ends {
                alias = name_of(last).map(String::from);
                expr_hi = hi - 1;
            }
        }
        // A bare column: names joined by dots.
        let plain = expr_hi > lo
            && (lo..expr_hi).all(|k| if (k - lo).is_multiple_of(2) { name_of(self.t[k]).is_some() } else { self.t[k].tok == Tok::Dot })
            && (expr_hi - lo) % 2 == 1;
        let name = alias.clone().or_else(|| self.implicit_name(lo, expr_hi, plain));
        Item { lo, hi: expr_hi, alias, name, star: None, plain }
    }

    /// Whether token `i` is the single character `c`.
    fn is_char(&self, i: usize, c: char) -> bool {
        self.t.get(i).is_some_and(|t| t.tok == Tok::Other && self.lines.get(t.start.row).and_then(|l| l.chars().nth(t.start.col)) == Some(c))
    }

    /// The name Postgres gives a select item that has no alias, when the
    /// client can be sure of it: a column (`film_id`, `f.film_id`), a column
    /// cast to one type (`film_id::text`), and a call named for its function.
    /// Anything else is `?column?` or depends on the server's version.
    fn implicit_name(&self, lo: usize, hi: usize, plain: bool) -> Option<String> {
        if plain {
            // `true` is named `bool` and `null` has no name.
            if hi - lo == 1 && (self.word(lo, "true") || self.word(lo, "false") || self.word(lo, "null")) {
                return None;
            }
            return name_of(self.t[hi - 1]).map(String::from);
        }
        // `col::type` and `col::type(n)`.
        if let Some(c) = (lo + 1..hi).find(|k| self.is_char(*k, ':') && self.is_char(*k + 1, ':')) {
            let column_part = c > lo && (lo..c).all(|k| if (k - lo).is_multiple_of(2) { name_of(self.t[k]).is_some() } else { self.t[k].tok == Tok::Dot }) && (c - lo) % 2 == 1;
            let ty = c + 2;
            let ty_ok = ty < hi && matches!(self.t[ty].tok, Tok::Word(_)) && (ty + 1 == hi || (self.t[ty + 1].tok == Tok::LParen && self.close(ty + 1, hi) + 1 == hi));
            return (column_part && ty_ok).then(|| name_of(self.t[c - 1]).map(String::from)).flatten();
        }
        // A call is named for the function: `lower(x)`, `count(*) over (...)`.
        let Tok::Word(f) = &self.t[lo].tok else { return None };
        if hi - lo < 3 || self.t[lo + 1].tok != Tok::LParen {
            return None;
        }
        let f = f.to_lowercase();
        // Syntax that Postgres names differently (or differently by version).
        if matches!(f.as_str(), "cast" | "trim" | "extract" | "treat" | "case" | "array" | "row" | "exists" | "not") {
            return None;
        }
        let close = self.close(lo + 1, hi);
        let rest = close + 1;
        let tail_ok = rest == hi || self.word(rest, "over") || self.word(rest, "filter") || self.word(rest, "within");
        (close < hi && self.t[close].tok == Tok::RParen && tail_ok).then_some(f)
    }

    // ---- resolving --------------------------------------------------------------

    fn cursor(&self, i: usize, fallback: Cursor) -> Cursor {
        self.t.get(i).map_or(fallback, |t| t.start)
    }

    /// What a query exposes: its first block's select list.
    fn derive(&self, q: &Query, visible: &[(String, Derived)], explicit: &[String]) -> Derived {
        let mut vis = visible.to_vec();
        for c in &q.ctes {
            let d = self.derive(&c.query, &vis, &c.columns);
            vis.push((c.name.clone(), d));
        }
        let spec = &q.specs[0];
        let inner = self.sources(spec, &vis);
        let columns = if explicit.is_empty() {
            spec.items
                .iter()
                .filter_map(|it| match &it.star {
                    Some(q) => Some(DerivedColumn::Star(q.clone())),
                    None => it.name.clone().map(DerivedColumn::Name),
                })
                .collect()
        } else {
            explicit.iter().cloned().map(DerivedColumn::Name).collect()
        };
        let named = spec.items.iter().all(|it| it.star.is_some() || it.name.is_some());
        let star = spec.items.iter().any(|it| it.star.is_some());
        let shape = spec.has_select
            && !spec.opaque
            && q.specs.len() == 1
            && named
            && !(star && spec.merges)
            && (explicit.is_empty() || (!star && explicit.len() == spec.items.len()));
        let certain = shape && (!star || inner.iter().all(Source::enumerable));
        Derived { columns, inner, certain }
    }

    fn sources(&self, spec: &Spec, vis: &[(String, Derived)]) -> Vec<Source> {
        let here = Cursor::default();
        spec.sources
            .iter()
            .map(|def| match def {
                SrcDef::Table { table, alias, at } => {
                    let cte = table.schema.is_none().then(|| vis.iter().rev().find(|(n, _)| same_name(n, &table.name))).flatten();
                    let at = self.cursor(*at, here);
                    match cte {
                        Some((name, d)) => Source { label: alias.clone().unwrap_or_else(|| name.clone()), aliased: alias.is_some(), kind: SourceKind::Derived(d.clone()), at },
                        None => Source { label: alias.clone().unwrap_or_else(|| table.name.clone()), aliased: alias.is_some(), kind: SourceKind::Table(table.clone()), at },
                    }
                }
                SrcDef::Derived { alias, columns, sub, at } => {
                    let d = self.derive(&spec.subqueries[*sub].query, vis, columns);
                    Source { label: alias.clone().unwrap_or_default(), aliased: alias.is_some(), kind: SourceKind::Derived(d), at: self.cursor(*at, here) }
                }
            })
            .collect()
    }

    fn level(&self, spec: &Spec, vis: &[(String, Derived)], fallback: Cursor) -> Level {
        let start = if spec.lo < spec.hi { self.cursor(spec.lo, fallback) } else { fallback };
        let end = if spec.lo < spec.hi { self.t[spec.hi - 1].end } else { fallback };
        let children = spec.subqueries.iter().map(|s| (self.cursor(s.open, fallback), self.t.get(s.close).map_or(end, |t| t.end))).collect();
        let mut sources = self.sources(spec, vis);
        // A derived table with no alias cannot be named: it has no label.
        let all = sources.len();
        sources.retain(|s| !s.label.is_empty());
        let opaque = spec.opaque || sources.len() != all;
        Level { sources, range: (start, end), children, opaque, merges: spec.merges }
    }

    /// Walks down to the block the caret (token index `j`) is in. `levels` is
    /// built outermost first.
    fn locate(&self, q: &Query, mut vis: Vec<(String, Derived)>, j: usize, fallback: Cursor, levels: &mut Vec<Level>, found: &mut Found) {
        for c in &q.ctes {
            if c.query.lo <= j && j <= c.query.hi {
                // A CTE's body sees the CTEs before it and not the blocks around it.
                levels.clear();
                return self.locate(&c.query, vis, j, fallback, levels, found);
            }
            let d = self.derive(&c.query, &vis, &c.columns);
            vis.push((c.name.clone(), d));
        }
        let spec = q
            .specs
            .iter()
            .find(|s| s.lo <= j && j <= s.hi)
            .or_else(|| q.specs.iter().find(|s| s.lo >= j))
            .unwrap_or_else(|| q.specs.last().expect("a query has a block"));
        if let Some(sub) = spec.subqueries.iter().find(|s| s.open < j && j <= s.close) {
            if sub.isolated {
                levels.clear();
            } else {
                // Without the derived table the caret is in: its alias is no
                // source of its own body.
                let mut level = self.level(spec, &vis, fallback);
                let open_at = self.t[sub.open].start;
                level.sources.retain(|s| s.at != open_at);
                levels.push(level);
            }
            return self.locate(&sub.query, vis, j, fallback, levels, found);
        }
        levels.push(self.level(spec, &vis, fallback));
        found.ctes = vis;
        // The clause: the last one that began before the caret.
        found.clause = spec.marks.iter().rev().find(|(i, _)| *i < j).map(|(_, c)| *c);
        found.items = spec
            .items
            .iter()
            .filter(|it| it.star.is_none())
            .map(|it| SelectItem { expr: self.text(it.lo, it.hi), alias: it.alias.clone(), plain: it.plain, name: it.name.clone() })
            .collect();
        found.select_uncertain = spec.items.iter().any(|it| it.star.is_some() || it.name.is_none());
        let caret = (found.caret.row, found.caret.col);
        found.star = spec.items.iter().find_map(|it| {
            let qualifier = it.star.clone()?;
            let (start, end) = (self.t[it.lo].start, self.t[it.hi - 1].end);
            ((start.row, start.col) <= caret && caret <= (end.row, end.col)).then_some(Star { start, end, qualifier })
        });
        if let Some((table, open)) = &spec.into {
            if self.t.get(*open).is_some_and(|t| t.tok == Tok::LParen) && *open < j && self.close(*open, spec.hi) >= j {
                found.insert_into = Some(table.clone());
            }
        }
    }
}

#[derive(Default)]
struct Found {
    caret: Cursor,
    ctes: Vec<(String, Derived)>,
    clause: Option<Clause>,
    items: Vec<SelectItem>,
    star: Option<Star>,
    insert_into: Option<TableName>,
    select_uncertain: bool,
}

/// A statement's tokens read once into its tree of queries, for any number of
/// questions about where in it a position is (`at`): the semantic colours ask
/// once for every name in a statement, so the reading is not repeated.
pub(super) struct Parsed<'a> {
    reader: Reader<'a>,
    root: Option<Query>,
    names: Vec<String>,
}

fn query_has(q: &Query, f: &dyn Fn(&Spec) -> bool) -> bool {
    q.ctes.iter().any(|c| query_has(&c.query, f)) || q.specs.iter().any(|s| f(s) || s.subqueries.iter().any(|sub| query_has(&sub.query, f)))
}

impl<'a> Parsed<'a> {
    pub(super) fn new(stmt: &'a [&'a Token], lines: &'a [String]) -> Self {
        let mut reader = Reader { t: stmt, lines, names: Vec::new() };
        let root = (!stmt.is_empty()).then(|| reader.query(0, stmt.len()));
        let mut names = std::mem::take(&mut reader.names);
        names.sort();
        names.dedup();
        Parsed { reader, root, names }
    }

    /// A source somewhere in the statement is one the client cannot read the
    /// columns of: a function, `values`, a parenthesised join, `lateral`.
    pub(super) fn has_opaque_source(&self) -> bool {
        self.root.as_ref().is_some_and(|q| query_has(q, &|s| s.opaque))
    }

    /// The statement has a `union`, `intersect` or `except`, or a block that
    /// is not a select.
    pub(super) fn has_set_operation(&self) -> bool {
        self.root.as_ref().is_some_and(|q| query_has_query(q, &|q| q.specs.len() > 1))
    }

    /// Every name the statement defines or uses as a table: tables, aliases,
    /// CTEs.
    pub(super) fn names(&self) -> &[String] {
        &self.names
    }

    /// The scope at `caret`; `j` is the index of the first token after it.
    pub(super) fn at(&self, caret: Cursor, j: usize) -> Scope {
        let Some(root) = &self.root else { return Scope::default() };
        let reader = &self.reader;
        let stmt = reader.t;
        let mut levels = Vec::new();
        let mut found = Found { caret, ..Found::default() };
        reader.locate(root, Vec::new(), j, caret, &mut levels, &mut found);
        levels.reverse();
        let after_operand = j > 0
            && match &stmt[j - 1].tok {
                Tok::Word(w) => !is_keyword(&w.to_lowercase()) || matches!(w.to_lowercase().as_str(), "null" | "true" | "false" | "end"),
                Tok::Quoted(..) | Tok::Number | Tok::Str(_) | Tok::RParen => true,
                _ => false,
            };
        let after_on = j > 0 && reader.word(j - 1, "on") && found.clause == Some(Clause::On);
        Scope {
            levels,
            clause: found.clause,
            after_operand,
            after_on,
            insert_into: found.insert_into,
            select_items: found.items,
            star: found.star,
            ctes: found.ctes,
            names: self.names.clone(),
            text: Some((stmt[0].start, stmt[stmt.len() - 1].end)),
            from_dbx: false,
            select_uncertain: found.select_uncertain,
        }
    }
}

fn query_has_query(q: &Query, f: &dyn Fn(&Query) -> bool) -> bool {
    f(q) || q.ctes.iter().any(|c| query_has_query(&c.query, f)) || q.specs.iter().any(|s| s.subqueries.iter().any(|sub| query_has_query(&sub.query, f)))
}

/// The scope at the caret, from the tokens `stmt` of one statement (without
/// the word being typed). `j` is the index of the first token after the caret.
pub(super) fn read(stmt: &[&Token], lines: &[String], caret: Cursor, j: usize) -> Scope {
    Parsed::new(stmt, lines).at(caret, j)
}

// ---- aliases ------------------------------------------------------------------

/// Words that cannot be an alias, or would be read as something else after a
/// table: Postgres' reserved words (the keyword table of its manual, with the
/// ones that may not be a table or column alias), sqlparser's reserved lists
/// for aliases and identifiers, and the highlighter's keywords.
pub fn is_reserved(word: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    const POSTGRES: &[&str] = &[
        "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric", "authorization", "binary", "both", "case", "cast", "check",
        "collate", "collation", "column", "concurrently", "constraint", "create", "cross", "current_catalog", "current_date", "current_role",
        "current_schema", "current_time", "current_timestamp", "current_user", "default", "deferrable", "desc", "distinct", "do", "else", "end",
        "except", "false", "fetch", "for", "foreign", "freeze", "from", "full", "grant", "group", "having", "ilike", "in", "initially", "inner",
        "intersect", "into", "is", "isnull", "join", "lateral", "leading", "left", "like", "limit", "localtime", "localtimestamp", "natural", "not",
        "notnull", "null", "offset", "on", "only", "or", "order", "outer", "overlaps", "placing", "primary", "references", "returning", "right",
        "select", "session_user", "similar", "some", "symmetric", "system_user", "table", "tablesample", "then", "to", "trailing", "true", "union",
        "unique", "user", "using", "variadic", "verbose", "when", "where", "window", "with",
    ];
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    let set = SET.get_or_init(|| {
        use sqlparser::keywords::{RESERVED_FOR_COLUMN_ALIAS, RESERVED_FOR_IDENTIFIER, RESERVED_FOR_TABLE_ALIAS};
        let mut set: HashSet<String> = POSTGRES.iter().map(|w| (*w).to_string()).collect();
        for k in RESERVED_FOR_TABLE_ALIAS.iter().chain(RESERVED_FOR_COLUMN_ALIAS).chain(RESERVED_FOR_IDENTIFIER) {
            set.insert(format!("{k:?}").to_lowercase());
        }
        set
    });
    let lower = word.to_lowercase();
    is_keyword(&lower) || set.contains(&lower)
}

/// The alias a table gets after `from`: the initials of its words (split at
/// `_`, `-` and spaces), lower case; made unique among `taken` (the tables,
/// CTEs and aliases the statement has) and never a reserved word, with a
/// number from 2.
pub fn generate_alias(table: &str, taken: &[String]) -> String {
    let mut base: String = table
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter_map(|w| w.chars().find(|c| c.is_alphabetic()))
        .flat_map(char::to_lowercase)
        .collect();
    if base.is_empty() {
        base.push('t');
    }
    let free = |a: &str| !is_reserved(a) && !taken.iter().any(|t| same_name(t, a));
    if free(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}{n}")).find(|a| free(a)).unwrap_or(base)
}

// ---- join conditions -----------------------------------------------------------

/// One side of a table in a condition.
pub struct Side<'a> {
    pub label: &'a str,
    pub table: &'a str,
    pub schema: &'a str,
    pub keys: &'a [ForeignKey],
}

/// The conditions the foreign keys between `new` (the table being joined) and
/// each of the `earlier` tables give, in both directions, the earlier table on
/// the left: `f.film_id = fa.film_id`. A key over several columns is one
/// condition with the columns joined by `and`. `quote` writes a name.
pub fn join_conditions(new: &Side, earlier: &[Side], quote: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut out = Vec::new();
    let refers = |k: &ForeignKey, to: &Side| k.ref_table == to.table && k.ref_schema.as_deref().is_none_or(|s| s == to.schema);
    for e in earlier {
        // Keys of the new table that refer to the earlier one, then the other way.
        for (own, other, from_new) in [(new, e, true), (e, new, false)] {
            let mut names: Vec<&str> = own.keys.iter().filter(|k| refers(k, other)).map(|k| k.name.as_str()).collect();
            names.dedup();
            for name in names {
                let parts: Vec<String> = own
                    .keys
                    .iter()
                    .filter(|k| k.name == name && refers(k, other))
                    .map(|k| {
                        let (mine, theirs) = (format!("{}.{}", quote(own.label), quote(&k.column)), format!("{}.{}", quote(other.label), quote(&k.ref_column)));
                        // The earlier table on the left.
                        if from_new { format!("{theirs} = {mine}") } else { format!("{mine} = {theirs}") }
                    })
                    .collect();
                let cond = parts.join(" and ");
                if !out.contains(&cond) {
                    out.push(cond);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::complete_context::analyse;

    /// The scope at the `|` of `text`.
    fn scope(text: &str) -> Scope {
        let rows: Vec<String> = text.replace('|', "").split('\n').map(String::from).collect();
        let (row, col) = text.split('\n').enumerate().find_map(|(r, l)| l.find('|').map(|i| (r, l[..i].chars().count()))).expect("a caret");
        analyse(&rows, Cursor { row, col }).scope
    }

    fn labels(s: &Scope) -> Vec<Vec<String>> {
        s.levels.iter().map(|l| l.sources.iter().map(|x| x.label.clone()).collect()).collect()
    }

    fn no_tables(_: &TableName) -> Vec<NamedColumn> {
        vec![("a".into(), None), ("b".into(), None)]
    }

    #[test]
    fn a_flat_statement_has_one_level_with_its_tables_and_aliases() {
        let s = scope("select | from film f join film_actor as fa on true, actor");
        assert_eq!(labels(&s), [vec!["f", "fa", "actor"]]);
        assert_eq!(s.clause(), Clause::Select);
    }

    #[test]
    fn a_subquery_sees_its_own_tables_then_the_outer_ones_and_a_derived_table_only_its_own() {
        let s = scope("select * from film f where x in (select | from film_actor fa)");
        assert_eq!(labels(&s), [vec!["fa"], vec!["f"]]);
        let s = scope("select * from film f join (select | from film_actor fa) s on true");
        assert_eq!(labels(&s), [vec!["fa"]]);
        let s = scope("select * from film f join lateral (select | from film_actor fa) s on true");
        assert_eq!(labels(&s), [vec!["fa"], vec!["f"]], "lateral sees what is beside it");
        let s = scope("select * from film f where (select count(*) from (select | from actor a) q) > 1");
        assert_eq!(labels(&s), [vec!["a"]]);
    }

    #[test]
    fn set_operators_split_blocks_and_each_sees_its_own_tables() {
        let s = scope("select a from t1 union all select | from t2");
        assert_eq!(labels(&s), [vec!["t2"]]);
        let s = scope("select | from t1 union select a from t2");
        assert_eq!(labels(&s), [vec!["t1"]]);
    }

    #[test]
    fn a_derived_table_is_a_source_with_its_alias_and_select_list_names() {
        let s = scope("select | from (select film_id as id, title, count(*) n, lower(x), a + b from film) s");
        assert_eq!(labels(&s), [vec!["s"]]);
        let cols: Vec<String> = s.sources()[0].columns(&mut no_tables).into_iter().map(|c| c.0).collect();
        assert_eq!(cols, ["id", "title", "n", "lower"], "an expression with no name has none");
    }

    #[test]
    fn a_star_in_a_derived_table_stands_for_the_columns_of_what_it_selects_from() {
        let s = scope("select | from (select * from film) s");
        assert_eq!(s.sources()[0].columns(&mut no_tables).len(), 2);
        let s = scope("select | from (select f.* from film f, actor a) s");
        assert_eq!(s.sources()[0].columns(&mut no_tables).len(), 2, "only f's");
    }

    #[test]
    fn a_cte_is_visible_after_its_definition_and_not_inside_it() {
        let s = scope("with c as (select film_id, title from film) select | from c");
        assert_eq!(labels(&s), [vec!["c"]]);
        let cols: Vec<String> = s.sources()[0].columns(&mut no_tables).into_iter().map(|c| c.0).collect();
        assert_eq!(cols, ["film_id", "title"]);
        assert_eq!(s.ctes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["c"]);
        let s = scope("with c as (select | from film) select * from c");
        assert!(s.ctes.is_empty());
        let s = scope("with a as (select 1 x), b as (select | from a) select * from b");
        assert_eq!(s.ctes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["a"], "an earlier CTE is visible to a later one");
        assert_eq!(labels(&s), [vec!["a"]]);
    }

    #[test]
    fn a_cte_with_a_column_list_names_its_columns_by_it() {
        let s = scope("with c(x, y) as (select film_id, title from film) select | from c");
        let cols: Vec<String> = s.sources()[0].columns(&mut no_tables).into_iter().map(|c| c.0).collect();
        assert_eq!(cols, ["x", "y"]);
    }

    #[test]
    fn the_clause_and_whether_an_operand_just_ended_are_known() {
        for (sql, clause, operand) in [
            ("select | from t", Clause::Select, false),
            ("select a, | from t", Clause::Select, false),
            ("select a | from t", Clause::Select, true),
            ("select * from t where |", Clause::Where, false),
            ("select * from t where a = 1 |", Clause::Where, true),
            ("select * from t join u on |", Clause::On, false),
            ("select * from t group by |", Clause::GroupBy, false),
            ("select * from t order by a |", Clause::OrderBy, true),
            ("select * from t where a in (1, |", Clause::Where, false),
            ("|", Clause::None, false),
        ] {
            let s = scope(sql);
            assert_eq!((s.clause(), s.after_operand), (clause, operand), "{sql}");
        }
        assert!(scope("select * from t join u on |").after_on);
        assert!(!scope("select * from t join u on a = |").after_on);
    }

    #[test]
    fn the_column_list_of_an_insert_is_found_until_it_closes() {
        assert_eq!(scope("insert into film (|").insert_into, Some(TableName { schema: None, name: "film".into() }));
        assert_eq!(scope("insert into public.film (a, |").insert_into.map(|t| t.schema), Some(Some("public".into())));
        assert_eq!(scope("insert into film (a) values (|").insert_into, None);
        assert_eq!(scope("insert into film |").insert_into, None);
    }

    #[test]
    fn the_select_list_is_kept_for_group_by_and_a_star_is_found_under_the_caret() {
        let s = scope("select title as t, lower(title), f.rating, count(*) n from film f group by |");
        let items: Vec<(&str, Option<&str>, bool)> = s.select_items.iter().map(|i| (i.expr.as_str(), i.alias.as_deref(), i.plain)).collect();
        assert_eq!(items, [("title", Some("t"), true), ("lower(title)", None, false), ("f.rating", None, true), ("count(*)", Some("n"), false)]);
        assert!(scope("select *| from t").star.is_some());
        assert!(scope("select |* from t").star.is_some());
        assert!(scope("select * |from t").star.is_none());
        assert!(scope("select count(*|) from t").star.is_none(), "a star inside a call is not the list's");
        assert_eq!(scope("select f.*| from film f").star.and_then(|s| s.qualifier), Some("f".into()));
    }

    #[test]
    fn dbxs_tables_replace_the_tokens_in_the_block_they_are_in_and_not_in_a_subquery() {
        use crate::core::{Span, TableReference};
        let sql = "select a from film f where x in (select 1 from actor a)";
        let mut s = scope("select a| from film f where x in (select 1 from actor a)");
        let table = |name: &str, alias: &str, col: usize| TableReference {
            name: name.into(),
            schema: None,
            alias: Some(alias.into()),
            span: Span { start: Cursor { row: 0, col }, end: Cursor { row: 0, col: col + name.len() } },
            scope: 0,
        };
        let at = |t: &str| sql.find(t).unwrap();
        let refs = References { tables: vec![table("film", "g", at("film")), table("actor", "b", at("actor"))], ..References::default() };
        s.apply_analysis(&refs, Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 7 });
        assert!(s.from_dbx);
        assert_eq!(labels(&s), [vec!["g"]], "the subquery's table is not this block's");
    }

    #[test]
    fn a_block_dbx_found_nothing_in_keeps_the_tokens() {
        let mut s = scope("update film f set title = |");
        s.apply_analysis(&References::default(), Cursor::default(), Cursor { row: 9, col: 9 });
        assert!(!s.from_dbx);
        assert_eq!(labels(&s), [vec!["f"]]);
    }

    #[test]
    fn keywords_are_valid_where_the_grammar_takes_them() {
        let valid = |sql: &str, k: &str| scope(sql).keyword_is_valid(k);
        assert!(valid("|", "select") && !valid("|", "where"));
        assert!(valid("select a |", "from") && !valid("select |", "from"));
        assert!(valid("select * from t where a = 1 |", "and") && !valid("select * from t where |", "and"));
        assert!(valid("select * from t where |", "not") && valid("select * from t order by a |", "desc"));
        assert!(!valid("select * from t order by |", "asc"));
    }

    #[test]
    fn aliases_are_the_initials_made_unique() {
        let none: Vec<String> = Vec::new();
        assert_eq!(generate_alias("film", &none), "f");
        assert_eq!(generate_alias("film_actor", &none), "fa");
        assert_eq!(generate_alias("Order Items", &none), "oi");
        assert_eq!(generate_alias("film", &["f".to_string()]), "f2");
        assert_eq!(generate_alias("film", &["f".to_string(), "F2".to_string()]), "f3");
        assert_eq!(generate_alias("a_s", &none), "as2", "`as` is a keyword");
        assert_eq!(generate_alias("_1", &none), "t");
        // Reserved words of the dialect, whatever the highlighter calls a keyword.
        assert_eq!(generate_alias("u_s_e_r", &none), "user2");
        assert_eq!(generate_alias("l_e_a_d_i_n_g", &none), "leading2");
        assert_eq!(generate_alias("o_u_t_e_r", &none), "outer2");
        // A name the statement already has, as a table, a CTE or an alias.
        assert_eq!(generate_alias("film", &["F".to_string()]), "f2");
    }

    fn key(name: &str, column: &str, table: &str, ref_column: &str) -> ForeignKey {
        ForeignKey { name: name.into(), column: column.into(), ref_schema: Some("public".into()), ref_table: table.into(), ref_column: ref_column.into() }
    }

    #[test]
    fn conditions_come_from_keys_in_both_directions_with_the_earlier_table_on_the_left() {
        let q = |n: &str| n.to_string();
        let fa_keys = [key("k1", "film_id", "film", "film_id"), key("k2", "actor_id", "actor", "actor_id")];
        let film = Side { label: "f", table: "film", schema: "public", keys: &[] };
        let fa = Side { label: "fa", table: "film_actor", schema: "public", keys: &fa_keys };
        assert_eq!(join_conditions(&fa, &[film], &q), ["f.film_id = fa.film_id"]);
        let film = Side { label: "f", table: "film", schema: "public", keys: &[] };
        assert_eq!(join_conditions(&film, &[Side { label: "fa", ..fa }], &q), ["fa.film_id = f.film_id"]);
    }

    #[test]
    fn a_key_over_several_columns_is_one_condition() {
        let q = |n: &str| n.to_string();
        let keys = [key("k", "a", "t", "x"), key("k", "b", "t", "y")];
        let child = Side { label: "c", table: "child", schema: "public", keys: &keys };
        let parent = Side { label: "p", table: "t", schema: "public", keys: &[] };
        assert_eq!(join_conditions(&child, &[parent], &q), ["p.x = c.a and p.y = c.b"]);
    }
}
