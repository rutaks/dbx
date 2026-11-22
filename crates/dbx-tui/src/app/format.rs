//! Reformat code (slice 14): a layout pass over the slice 10 tokenizer's tokens.
//!
//! The pass only decides what whitespace goes between tokens, and (when the
//! setting asks) the case of keywords: every token is copied from the source
//! text by its span, so strings, dollar-quoted bodies, `E''` strings, quoted
//! names and comments are never rebuilt. Before the result is returned it is
//! tokenized again and compared with the source's tokens; a result that
//! differs in anything but whitespace and keyword case is refused.
//!
//! Layout (see `docs/slices/14-reformat-code.md`):
//! - a clause (`select`, `from`, `where`, `group by`, `having`, `order by`,
//!   `limit`, `offset`, a join, `union`...) starts a line;
//! - the items of `select`, `set` and `values` go one per line, indented, and
//!   so do the columns of a `create table`;
//! - the `and`/`or` of `where`, `having` and `on` go one per line, indented;
//! - a subquery (a parenthesis that opens with `select` or `with`) and a CTE
//!   are indented one level, with their `)` on a line of its own;
//! - inside a call or any other parenthesis everything stays on its line;
//! - one blank line between statements.
//!
//! A comment stays on the line it was on, after the same token, or on a line
//! of its own, before the same token.

use sqlparser::dialect::Dialect as SqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

use super::highlight::{dialect_of, is_keyword, KEYWORD_FUNCTIONS};
use crate::core::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeywordCase {
    #[default]
    Leave,
    Upper,
    Lower,
}

/// Where the comma of a list that is broken over lines goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommaPosition {
    /// At the end of the line (the default).
    #[default]
    Trailing,
    /// At the start of the next line.
    Leading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub case: KeywordCase,
    /// Columns in one indent step.
    pub indent: usize,
    pub comma: CommaPosition,
}

impl Default for Options {
    fn default() -> Self {
        Options { case: KeywordCase::Leave, indent: 4, comma: CommaPosition::Trailing }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    /// The requested positions (character offsets into the source), where
    /// they are in `text`.
    pub carets: Vec<usize>,
    /// Tokens and comments the pass went through.
    pub tokens: usize,
}

// ---- lexing --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum K {
    /// An unquoted word: a keyword, a name, a function.
    Word,
    /// A quoted name.
    Ident,
    /// A string, number or placeholder.
    Lit,
    Comma,
    Semi,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Period,
    DColon,
    /// An operator that has a space either side.
    Op,
    /// Anything else: its spacing is kept as it was.
    Other,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: K,
    raw: String,
    /// Lower-cased, for words.
    lower: String,
    /// Whitespace or a comment's edge between the item before and this one.
    ws: bool,
    /// A line break (or a `--` comment, which ends in one) since the token
    /// before: where it matters between two string literals.
    nlt: bool,
    /// First thing on its line.
    bol: bool,
    /// A string literal that Postgres joins to a string literal before it
    /// when a line break lies between them.
    strlit: bool,
    s: usize,
    e: usize,
}

#[derive(Debug, Clone)]
struct Cm {
    text: String,
    line: bool,
    /// On a line of its own (a newline before it, or the first thing).
    own: bool,
    s: usize,
    e: usize,
}

#[derive(Debug, Clone)]
enum Item {
    Tok(Tok),
    Comment(Cm),
}

impl Item {
    fn span(&self) -> (usize, usize) {
        match self {
            Item::Tok(t) => (t.s, t.e),
            Item::Comment(c) => (c.s, c.e),
        }
    }
}

const SPACED_OPS: &[&str] = &[
    "=", "==", "<>", "!=", "<", ">", "<=", ">=", "<=>", "+", "-", "*", "/", "%", "||", "->", "->>", "#>", "#>>", "@>", "<@", "&&", "@@", "=>", ":=", "~", "~*",
    "!~", "!~*", "^", "&", "|", "<<", ">>",
];

fn classify(token: &Token, raw: &str) -> K {
    match token {
        Token::Word(w) if w.quote_style.is_some() => K::Ident,
        Token::Word(_) => K::Word,
        Token::Number(..)
        | Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::EscapedStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::HexStringLiteral(_)
        | Token::Placeholder(_) => K::Lit,
        Token::Comma => K::Comma,
        Token::SemiColon => K::Semi,
        Token::LParen => K::LParen,
        Token::RParen => K::RParen,
        Token::LBracket => K::LBracket,
        Token::RBracket => K::RBracket,
        Token::Period => K::Period,
        Token::DoubleColon => K::DColon,
        _ if SPACED_OPS.contains(&raw) => K::Op,
        _ => K::Other,
    }
}

/// The tokens and comments of `chars`, with where each is, or why the text
/// does not tokenize.
fn lex(chars: &[char], dialect: &dyn SqlDialect) -> Result<Vec<Item>, String> {
    let text: String = chars.iter().collect();
    let tokens = Tokenizer::new(dialect, &text).tokenize_with_location().map_err(|e| format!("{} (line {}, column {})", e.message.to_lowercase(), e.location.line, e.location.column))?;
    let mut line_starts = vec![0usize];
    line_starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
    let at = |l: sqlparser::tokenizer::Location| -> usize {
        let line = (l.line as usize).saturating_sub(1);
        let col = (l.column as usize).saturating_sub(1);
        line_starts.get(line).map_or(chars.len(), |s| (s + col).min(chars.len()))
    };
    let mut items = Vec::new();
    let (mut ws, mut nl, mut nlt) = (false, true, false);
    for t in tokens {
        let (s, e) = (at(t.span.start), at(t.span.end));
        let e = e.max(s);
        match &t.token {
            Token::Whitespace(Whitespace::Newline) => {
                ws = true;
                nl = true;
                nlt = true;
            }
            Token::Whitespace(Whitespace::Space | Whitespace::Tab) => ws = true,
            Token::Whitespace(Whitespace::SingleLineComment { .. }) => {
                let raw: String = chars[s..e].iter().collect();
                let text = raw.trim_end().to_string();
                let e = s + text.chars().count();
                items.push(Item::Comment(Cm { text, line: true, own: nl, s, e }));
                ws = true;
                nl = true;
                nlt = true;
            }
            Token::Whitespace(Whitespace::MultiLineComment(_)) => {
                let text: String = chars[s..e].iter().collect();
                items.push(Item::Comment(Cm { text, line: false, own: nl, s, e }));
                ws = true;
                nl = false;
            }
            token => {
                let raw: String = chars[s..e].iter().collect();
                let kind = classify(token, &raw);
                let lower = if kind == K::Word { raw.to_lowercase() } else { String::new() };
                let strlit = matches!(token, Token::SingleQuotedString(_) | Token::EscapedStringLiteral(_) | Token::NationalStringLiteral(_) | Token::HexStringLiteral(_) | Token::UnicodeStringLiteral(_) | Token::SingleQuotedByteStringLiteral(_));
                items.push(Item::Tok(Tok { kind, raw, lower, ws, nlt, bol: nl, strlit, s, e }));
                ws = false;
                nl = false;
                nlt = false;
            }
        }
    }
    Ok(items)
}

/// What must be equal before and after: every token and comment, with
/// unquoted words lower-cased when the case may have changed.
///
/// Whitespace is not compared, except that a line break between two string
/// literals is (Postgres reads `'a'` newline `'b'` as `'ab'`, and `'a' 'b'` as
/// an error): it is a `"\n"` entry before the second.
fn stream(items: &[Item], fold: bool) -> Vec<String> {
    let mut out = Vec::with_capacity(items.len());
    let mut last_str = false;
    for i in items {
        match i {
            Item::Comment(c) => out.push(c.text.clone()),
            Item::Tok(t) => {
                if t.strlit && last_str && t.nlt {
                    out.push("\n".to_string());
                }
                last_str = t.strlit;
                out.push(if t.kind == K::Word && fold { t.lower.clone() } else { t.raw.clone() });
            }
        }
    }
    out
}

/// Whether two lexed texts have the same tokens and comments.
fn same_items(a: &[Item], b: &[Item], fold: bool) -> bool {
    stream(a, fold) == stream(b, fold)
}

/// Whether `before` and `after` (texts) have the same tokens, comments and
/// whitespace that carries meaning.
#[cfg(test)]
pub(super) fn same_tokens(before: &str, after: &str, dialect: Option<Dialect>, fold: bool) -> bool {
    let sql_dialect = dialect_of(dialect);
    let lexed = |t: &str| lex(&t.chars().collect::<Vec<_>>(), sql_dialect.as_ref());
    match (lexed(before), lexed(after)) {
        (Ok(a), Ok(b)) => same_items(&a, &b, fold),
        _ => false,
    }
}

// ---- the layout ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// The statement.
    Top,
    /// A parenthesis that holds a query.
    Sub,
    /// The column list of a `create table`.
    Cols,
    /// Any other parenthesis or bracket: everything stays on its line.
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    None,
    Select,
    From,
    Where,
    Having,
    Join,
    /// The `on` of a join.
    Cond,
    Values,
    Set,
    With,
    Other,
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    /// Where the frame's clauses start (a list's items, for `Cols`; the line's
    /// continuation, for `Plain`).
    base: usize,
    /// Where its closing parenthesis goes.
    close: usize,
    clause: Clause,
    first: Option<String>,
    tokens: usize,
    /// `select` was just written: its items break after `distinct`/`all`.
    hold: bool,
    case_depth: usize,
    between: bool,
    saw_table: bool,
    had_paren: bool,
}

impl Frame {
    fn new(kind: FrameKind, base: usize, close: usize) -> Frame {
        Frame { kind, base, close, clause: Clause::None, first: None, tokens: 0, hold: false, case_depth: 0, between: false, saw_table: false, had_paren: false }
    }
}

#[derive(Debug, Clone)]
struct Prev {
    kind: K,
    /// The word, lower-cased (`K::Word` only).
    word: String,
    keyword: bool,
    /// A prefix operator: no space follows.
    unary: bool,
    strlit: bool,
}

struct Held {
    parts: Vec<(usize, String)>,
    line: bool,
}

struct Fmt<'a> {
    opts: &'a Options,
    base: usize,
    out: String,
    n: usize,
    pending: Option<(usize, usize)>,
    line_indent: usize,
    held: Vec<Held>,
    frames: Vec<Frame>,
    prev: Option<Prev>,
    /// For each item: where it went in the output.
    placed: Vec<(usize, usize)>,
    /// A comment was written since the last `;`.
    comment_since_semi: bool,
    /// Keywords are re-cased only when reserved in MySQL (a table's name is
    /// case-sensitive there).
    mysql_names: bool,
}

/// MySQL's reserved words that the formatter lays out or re-cases. Any other
/// word may be the name of a table or a column (`language`, `status`), which is
/// case-sensitive on a case-sensitive file system, so it is never re-cased.
const MYSQL_RESERVED: &[&str] = &[
    "add", "all", "alter", "and", "as", "asc", "between", "by", "case", "check", "column", "constraint", "create", "cross", "database", "default",
    "delete", "desc", "describe", "distinct", "drop", "else", "exists", "explain", "false", "for", "foreign", "from", "group", "having", "if",
    "in", "index", "inner", "insert", "interval", "into", "is", "join", "key", "left", "like", "limit", "natural", "not", "null", "offset", "on",
    "or", "order", "outer", "primary", "procedure", "references", "replace", "right", "select", "set", "show", "table", "then", "to", "true",
    "union", "unique", "update", "use", "using", "values", "when", "where", "while", "with",
];

const JOIN_WORDS: &[&str] = &["left", "right", "full", "inner", "cross", "natural", "outer"];

impl<'a> Fmt<'a> {
    fn new(opts: &'a Options, base: usize, items: usize) -> Self {
        Fmt {
            opts,
            base,
            out: String::new(),
            n: 0,
            pending: None,
            line_indent: base,
            held: Vec::new(),
            frames: vec![Frame::new(FrameKind::Top, base, base)],
            prev: None,
            placed: vec![(0, 0); items],
            comment_since_semi: false,
            mysql_names: false,
        }
    }

    fn unit(&self) -> usize {
        self.opts.indent.max(1)
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the statement's frame")
    }

    fn push(&mut self, s: &str) {
        self.out.push_str(s);
        self.n += s.chars().count();
    }

    fn newline(&mut self, nl: usize, indent: usize) {
        if !self.out.is_empty() {
            for _ in 0..nl {
                self.push("\n");
            }
            self.push(&" ".repeat(indent));
        }
        self.line_indent = indent;
    }

    /// Where a line goes that continues the current expression.
    fn cont(&self) -> usize {
        let f = self.frames.last().expect("frame");
        match f.kind {
            FrameKind::Plain => f.base,
            _ if f.tokens == 0 => f.base,
            _ => f.base + self.unit(),
        }
    }

    fn flush_held(&mut self, first_nl: usize, indent: usize) {
        let held = std::mem::take(&mut self.held);
        for (k, h) in held.into_iter().enumerate() {
            self.newline(if k == 0 { first_nl } else { 1 }, indent);
            let mut first = true;
            for (idx, text) in h.parts {
                if !first {
                    self.push(" ");
                }
                first = false;
                let s = self.n;
                self.push(&text);
                self.placed[idx] = (s, self.n);
            }
        }
    }

    /// Writes `text` for item `idx`: on a new line when a break is pending,
    /// otherwise after a space when `gap`.
    fn put(&mut self, idx: usize, text: &str, gap: bool) {
        if !self.held.is_empty() {
            let natural = self.cont();
            let (nl, ind) = self.pending.take().unwrap_or((1, natural));
            self.flush_held(nl, ind);
            self.newline(1, ind);
        } else if let Some((nl, ind)) = self.pending.take() {
            self.newline(nl, ind);
        } else if gap && !self.out.is_empty() {
            self.push(" ");
        }
        let s = self.n;
        self.push(text);
        self.placed[idx] = (s, self.n);
    }

    fn comment(&mut self, idx: usize, c: &Cm) {
        self.comment_since_semi = true;
        if let Some(last) = self.held.last_mut() {
            if c.own || last.line {
                self.held.push(Held { parts: vec![(idx, c.text.clone())], line: c.line });
            } else {
                last.parts.push((idx, c.text.clone()));
                last.line = c.line;
            }
            return;
        }
        if c.own || self.out.is_empty() {
            self.held.push(Held { parts: vec![(idx, c.text.clone())], line: c.line });
            return;
        }
        // After a token on its line: stays there.
        self.push(" ");
        let s = self.n;
        self.push(&c.text);
        self.placed[idx] = (s, self.n);
        if c.line && self.pending.is_none() {
            self.pending = Some((1, self.cont()));
        }
    }

    fn finish(&mut self) {
        if !self.held.is_empty() {
            let (nl, ind) = self.pending.take().unwrap_or((1, self.base));
            self.flush_held(nl, ind);
        }
    }

    fn is_operand(prev: &Prev) -> bool {
        match prev.kind {
            K::Ident | K::Lit | K::RParen | K::RBracket => true,
            K::Word => !prev.keyword || matches!(prev.word.as_str(), "null" | "true" | "false" | "end"),
            _ => false,
        }
    }

    /// Whether a space goes between the previous token and `cur`.
    fn space(prev: &Prev, cur: &Tok) -> bool {
        use K::*;
        if matches!(cur.kind, Comma | Semi | RParen | RBracket | Period | DColon) {
            return false;
        }
        if matches!(prev.kind, LParen | LBracket | Period | DColon) {
            return false;
        }
        if prev.unary {
            // `- -1` is not `--1` (a comment): two operators keep the source's space.
            return cur.ws && matches!(cur.kind, Op | Other);
        }
        let wordish = |k: K| matches!(k, Word | Ident | Lit);
        if !cur.ws && wordish(prev.kind) && wordish(cur.kind) {
            return false;
        }
        if cur.kind == Op && cur.raw == "&" && prev.kind == Word && prev.word == "u" && !cur.ws {
            return false;
        }
        match cur.kind {
            LParen => match prev.kind {
                Word if prev.keyword => true,
                Word | Ident | RParen | RBracket | Lit => cur.ws,
                _ => true,
            },
            LBracket => match prev.kind {
                Word | Ident | RParen | RBracket => cur.ws,
                _ => true,
            },
            Other => cur.ws,
            _ if prev.kind == Other => cur.ws,
            _ => true,
        }
    }

    fn next_tok(items: &[Item], i: usize) -> Option<&Tok> {
        items[i + 1..].iter().find_map(|x| if let Item::Tok(t) = x { Some(t) } else { None })
    }

    fn is_join_start(&self, items: &[Item], i: usize, lw: &str) -> bool {
        if !(lw == "join" || JOIN_WORDS.contains(&lw)) || lw == "outer" {
            return false;
        }
        if self.prev.as_ref().is_some_and(|p| p.kind == K::Word && JOIN_WORDS.contains(&p.word.as_str())) {
            return false;
        }
        if lw == "join" {
            return true;
        }
        for x in &items[i + 1..] {
            match x {
                Item::Comment(_) => {}
                Item::Tok(t) if t.kind == K::Word && JOIN_WORDS.contains(&t.lower.as_str()) => {}
                Item::Tok(t) => return t.kind == K::Word && t.lower == "join",
            }
        }
        false
    }

    fn keyword_text(&self, items: &[Item], i: usize, t: &Tok, after_dot: bool) -> String {
        if t.kind != K::Word || after_dot || self.opts.case == KeywordCase::Leave || !is_keyword(&t.lower) {
            return t.raw.clone();
        }
        if self.mysql_names && (!MYSQL_RESERVED.contains(&t.lower.as_str()) || matches!(Self::next_tok(items, i), Some(n) if n.kind == K::Period)) {
            return t.raw.clone();
        }
        let call = matches!(Self::next_tok(items, i), Some(n) if n.kind == K::LParen);
        if call && KEYWORD_FUNCTIONS.contains(&t.lower.as_str()) {
            return t.raw.clone();
        }
        match self.opts.case {
            KeywordCase::Upper => t.raw.to_uppercase(),
            _ => t.raw.to_lowercase(),
        }
    }

    fn run(&mut self, items: &[Item]) {
        for (i, item) in items.iter().enumerate() {
            match item {
                Item::Comment(c) => self.comment(i, c),
                Item::Tok(t) => self.token(items, i, t),
            }
        }
        self.finish();
    }

    fn token(&mut self, items: &[Item], i: usize, t: &Tok) {
        let unit = self.unit();
        let after_dot = self.prev.as_ref().is_some_and(|p| p.kind == K::Period);
        let prev_word = self.prev.as_ref().filter(|p| p.kind == K::Word).map(|p| p.word.clone());
        let clause_level = matches!(self.frames.last().map(|f| f.kind), Some(FrameKind::Top | FrameKind::Sub));
        let mut unary = false;
        let mut after: Option<Clause> = None;

        // 1. Words that structure a clause frame.
        let mut starts: Option<Clause> = None;
        let mut no_break = false;
        if t.kind == K::Word && !after_dot && clause_level {
            let lw = t.lower.as_str();
            let join = self.is_join_start(items, i, lw);
            let f = self.frame();
            if f.first.is_none() {
                f.first = Some(lw.to_string());
                if lw == "with" {
                    f.clause = Clause::With;
                }
            }
            starts = if join {
                Some(Clause::Join)
            } else {
                match lw {
                    "select" => Some(Clause::Select),
                    "from" => match prev_word.as_deref() {
                        Some("distinct") => None,
                        Some("delete") => {
                            no_break = true;
                            Some(Clause::From)
                        }
                        _ => Some(Clause::From),
                    },
                    "where" => Some(Clause::Where),
                    "having" => Some(Clause::Having),
                    "group" | "order" | "limit" | "offset" | "fetch" | "window" | "returning" | "union" | "intersect" | "except" => Some(Clause::Other),
                    "values" => Some(Clause::Values),
                    "set" if f.first.as_deref() == Some("update") => Some(Clause::Set),
                    "insert" | "update" | "delete" if f.clause == Clause::With && f.tokens > 0 => {
                        f.first = Some(lw.to_string());
                        Some(Clause::Other)
                    }
                    _ => None,
                }
            };
            // `union all` / `union distinct`: the second word stays on the line.
            if starts.is_none() {
                let f = self.frame();
                match lw {
                    "on" if f.clause == Clause::Join => f.clause = Clause::Cond,
                    "and" | "or" if matches!(f.clause, Clause::Where | Clause::Having | Clause::Cond) && f.case_depth == 0 => {
                        if lw == "and" && f.between {
                            f.between = false;
                        } else {
                            let at = f.base + unit;
                            self.pending = Some((1, at));
                        }
                    }
                    "between" => f.between = true,
                    "case" => f.case_depth += 1,
                    "end" => f.case_depth = f.case_depth.saturating_sub(1),
                    "table" if f.first.as_deref() == Some("create") => f.saw_table = true,
                    _ => {}
                }
            }
        } else if t.kind == K::Word && !after_dot {
            let f = self.frame();
            match t.lower.as_str() {
                "between" => f.between = true,
                "and" if f.between => f.between = false,
                "case" => f.case_depth += 1,
                "end" => f.case_depth = f.case_depth.saturating_sub(1),
                _ => {}
            }
        }

        if let Some(clause) = starts {
            let f = self.frames.last_mut().expect("frame");
            f.hold = false;
            f.case_depth = 0;
            f.between = false;
            f.clause = clause;
            let (base, started) = (f.base, f.tokens > 0);
            if started && !no_break {
                self.pending = Some((1, base));
            }
            after = Some(clause);
        } else if clause_level && !matches!(t.kind, K::Semi | K::RParen) {
            // The first thing after `select` that is not `distinct`/`all`.
            let hold_ok = t.kind == K::Word && (matches!(t.lower.as_str(), "distinct" | "all") || (t.lower == "on" && prev_word.as_deref() == Some("distinct")))
                || (t.kind == K::LParen && prev_word.as_deref() == Some("on"));
            let f = self.frames.last_mut().expect("frame");
            if f.hold && !hold_ok {
                f.hold = false;
                let at = f.base + unit;
                self.pending = Some((1, at));
            }
        }

        // 2. The token.
        match t.kind {
            K::Semi => {
                self.frames.truncate(1);
                let base = self.base;
                let prev_semi = self.prev.as_ref().is_some_and(|p| p.kind == K::Semi);
                if prev_semi && !self.comment_since_semi {
                    self.pending = None;
                }
                self.comment_since_semi = false;
                self.put(i, &t.raw, false);
                self.frames[0] = Frame::new(FrameKind::Top, base, base);
                self.pending = Some((2, base));
                self.prev = Some(Prev { kind: K::Semi, word: String::new(), keyword: false, unary: false, strlit: false });
                return;
            }
            K::Comma => {
                let f = self.frames.last().expect("frame");
                let at = match (f.kind, f.clause) {
                    (FrameKind::Top | FrameKind::Sub, Clause::Select | Clause::Values | Clause::Set) => Some(f.base + unit),
                    (FrameKind::Top | FrameKind::Sub, Clause::With) | (FrameKind::Cols, _) => Some(f.base),
                    _ => None,
                };
                match at {
                    Some(at) if self.opts.comma == CommaPosition::Leading => {
                        self.pending = Some((1, at));
                        self.put(i, &t.raw, false);
                    }
                    Some(at) => {
                        self.put(i, &t.raw, false);
                        self.pending = Some((1, at));
                    }
                    None => self.put(i, &t.raw, false),
                }
                self.after_token(t, false, String::new());
                return;
            }
            K::LParen | K::LBracket => {
                let gap = Self::gap(self.prev.as_ref(), t);
                self.put(i, &t.raw, gap);
                let line = self.line_indent;
                let query = t.kind == K::LParen && matches!(Self::next_tok(items, i), Some(n) if n.kind == K::Word && matches!(n.lower.as_str(), "select" | "with"));
                let f = self.frames.last().expect("frame");
                let cols = t.kind == K::LParen
                    && f.kind == FrameKind::Top
                    && f.first.as_deref() == Some("create")
                    && f.saw_table
                    && !f.had_paren;
                if t.kind == K::LParen {
                    self.frame().had_paren = true;
                }
                self.frame().tokens += 1;
                if query {
                    self.frames.push(Frame::new(FrameKind::Sub, line + unit, line));
                    self.pending = Some((1, line + unit));
                } else if cols {
                    self.frames.push(Frame::new(FrameKind::Cols, line + unit, line));
                    self.pending = Some((1, line + unit));
                } else {
                    self.frames.push(Frame::new(FrameKind::Plain, line + unit, line));
                }
                self.prev = Some(Prev { kind: t.kind, word: String::new(), keyword: false, unary: false, strlit: false });
                return;
            }
            K::RParen | K::RBracket => {
                if self.frames.len() > 1 {
                    let f = self.frames.pop().expect("frame");
                    if matches!(f.kind, FrameKind::Sub | FrameKind::Cols) {
                        self.pending = Some((1, f.close));
                    }
                }
                self.put(i, &t.raw, false);
                self.after_token(t, false, String::new());
                return;
            }
            K::Op => {
                let operand = self.prev.as_ref().is_some_and(Self::is_operand);
                unary = matches!(t.raw.as_str(), "-" | "+" | "~") && !operand;
            }
            _ => {}
        }
        if t.strlit && t.nlt && self.prev.as_ref().is_some_and(|p| p.strlit) && self.pending.is_none() {
            // The line break is what joins two string literals.
            self.pending = Some((1, self.cont()));
        }
        let text = self.keyword_text(items, i, t, after_dot);
        let gap = Self::gap(self.prev.as_ref(), t);
        self.put(i, &text, gap);
        self.after_token(t, unary, t.lower.clone());

        // 3. What a clause keyword leaves behind.
        if let Some(clause) = after {
            match clause {
                Clause::Select => self.frame().hold = true,
                Clause::Values | Clause::Set => {
                    let at = self.frame().base + unit;
                    self.pending = Some((1, at));
                }
                _ => {}
            }
        }
    }

    fn gap(prev: Option<&Prev>, cur: &Tok) -> bool {
        match prev {
            None => false,
            Some(p) => Self::space(p, cur),
        }
    }

    fn after_token(&mut self, t: &Tok, unary: bool, word: String) {
        self.frame().tokens += 1;
        let keyword = t.kind == K::Word && is_keyword(&t.lower) && !KEYWORD_FUNCTIONS.contains(&t.lower.as_str());
        self.prev = Some(Prev { kind: t.kind, word, keyword, unary, strlit: t.strlit });
    }
}

/// The line (counting from 1) of the first line that is a client command: a
/// first word `delimiter`, a lone `go`, or a backslash (psql). Only a token
/// that starts its line counts, so a string or comment is never one.
fn client_command_line(chars: &[char], items: &[Item]) -> Option<usize> {
    items.iter().find_map(|item| {
        let Item::Tok(t) = item else { return None };
        if !t.bol {
            return None;
        }
        let rest_blank = || chars[t.e..].iter().take_while(|c| **c != '\n').all(|c| c.is_whitespace());
        let command = t.raw.starts_with('\\') || (t.kind == K::Word && (t.lower == "delimiter" || (t.lower == "go" && rest_blank())));
        command.then(|| chars[..t.s].iter().filter(|c| **c == '\n').count() + 1)
    })
}

/// Whether each of `ends` (character offsets into `text`) is outside every
/// string, comment, quoted name and word of the whole `text`.
pub fn ends_between_tokens(text: &str, dialect: Option<Dialect>, ends: &[usize]) -> Result<(), String> {
    let chars: Vec<char> = text.chars().collect();
    let sql_dialect = dialect_of(dialect);
    let items = lex(&chars, sql_dialect.as_ref()).map_err(|why| format!("the editor's text does not tokenize ({why}), so a selection cannot be checked; fix that, or put the caret in the statement instead"))?;
    for &pos in ends {
        let k = items.partition_point(|i| i.span().0 < pos);
        // The last item that starts before the position.
        if let Some(prev) = k.checked_sub(1) {
            if items[prev].span().1 > pos {
                return Err("the selection starts or ends inside a string, comment, quoted name or word, so it is not SQL by itself; select whole words, or put the caret in the statement and reformat that".to_string());
            }
        }
    }
    Ok(())
}

/// Lays out `src`.
///
/// `base` is the indentation of the lines after the first (the column the
/// text sits at in the editor); `lead` the spaces the first line gets (the text
/// began at the start of its line); `carets` are character offsets into `src`
/// that come back as where they are in the result.
pub fn format_sql(src: &str, dialect: Option<Dialect>, opts: &Options, base: usize, lead: usize, carets: &[usize]) -> Result<Formatted, String> {
    let chars: Vec<char> = src.chars().collect();
    let sql_dialect = dialect_of(dialect);
    let items = lex(&chars, sql_dialect.as_ref())?;
    let Some(last) = items.last() else {
        return Ok(Formatted { text: src.to_string(), carets: carets.to_vec(), tokens: 0 });
    };
    let (first_s, last_e) = (items[0].span().0, last.span().1);

    if let Some(line) = client_command_line(&chars, &items) {
        return Err(format!(
            "line {line} is a client command (DELIMITER, GO or a backslash command), not SQL; select the SQL above or below it and reformat that"
        ));
    }

    let mut fmt = Fmt::new(opts, base, items.len());
    fmt.mysql_names = !matches!(dialect, Some(Dialect::PostgreSql | Dialect::Sqlite));
    fmt.run(&items);
    let suffix: String = chars[last_e..].iter().collect();
    let keep = if suffix.contains('\n') { suffix } else { String::new() };
    let mut text = " ".repeat(lead);
    text.push_str(&fmt.out);
    text.push_str(&keep);

    // Nothing but whitespace and keyword case may differ.
    let again = lex(&text.chars().collect::<Vec<_>>(), sql_dialect.as_ref()).map_err(|why| format!("the result would not tokenize ({why}), so the text is unchanged; select a smaller part and try again"))?;
    let fold = opts.case != KeywordCase::Leave;
    if !same_items(&items, &again, fold) {
        return Err("the result would change more than whitespace and keyword case, so the text is unchanged; select a smaller part and try again".to_string());
    }

    let total = text.chars().count();
    let out_end = lead + fmt.n;
    let mapped = carets
        .iter()
        .map(|&pos| {
            if pos <= first_s {
                return if pos == 0 { 0 } else { lead };
            }
            if pos >= last_e {
                return if pos >= chars.len() { total } else { (out_end + pos - last_e).min(total).min(if keep.is_empty() { out_end } else { total }) };
            }
            // The last item that starts at or before the position.
            let k = items.partition_point(|i| i.span().0 <= pos).saturating_sub(1);
            let (s, e) = items[k].span();
            let (os, oe) = fmt.placed[k];
            if pos < e {
                lead + (os + (pos - s)).min(oe)
            } else if let Some(next) = items.get(k + 1) {
                let same_line = !chars[e..next.span().0.min(chars.len())].contains(&'\n') || !chars[e..pos].contains(&'\n');
                if same_line { lead + oe } else { lead + fmt.placed[k + 1].0 }
            } else {
                lead + oe
            }
        })
        .collect();
    Ok(Formatted { text, carets: mapped, tokens: items.len() })
}
