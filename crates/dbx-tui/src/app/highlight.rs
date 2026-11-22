//! SQL text in, styled spans out. A pure function of the lines, the kind of
//! database and its type names (`highlight`), plus `Highlighter`, which keeps
//! the last result: the whole buffer is tokenized once per text change and
//! never on a repaint or a caret move.

use std::collections::HashSet;

use sqlparser::dialect::{Dialect as SqlDialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer, Whitespace};

use super::theme::Role;
use crate::core::{Cursor, Dialect};

/// One coloured stretch of a line: characters `start..end` (not display
/// columns: the painter turns them into columns by the widths it draws).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub role: Role,
}

/// A bracket token: where it is, in lines of the whole buffer and characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    pub line: usize,
    pub col: usize,
    pub ch: char,
}

/// The spans of the lines `first_line..first_line + lines.len()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlighted {
    pub first_line: usize,
    pub lines: Vec<Vec<Span>>,
    /// Every bracket that is a token (not inside a string or a comment).
    pub brackets: Vec<Bracket>,
}

impl Highlighted {
    /// The spans of a line of the buffer, if the result covers it.
    pub fn line(&self, line: usize) -> Option<&[Span]> {
        self.lines.get(line.checked_sub(self.first_line)?).map(Vec::as_slice)
    }
}

/// The names of a database's types, lower-cased, as dbx lists them.
#[derive(Debug, Clone, Default)]
pub struct TypeNames {
    words: HashSet<String>,
    /// Which set this is: every `new` takes a fresh one, so two sets are
    /// compared by a number and not by their contents (0 is the empty set).
    generation: u64,
}

impl TypeNames {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        // `double precision` is typed as two words: each is a type word,
        // unless it is syntax (`with` in `timestamp with time zone`).
        let words = names
            .into_iter()
            .map(|n| n.to_lowercase())
            .filter(|n| !n.contains(['[', '(']))
            .flat_map(|n| n.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .filter(|w| !is_keyword(w));
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        TypeNames { words: words.collect(), generation: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) }
    }
}

/// Words that are syntax, not names. Not every word sqlparser knows: its list
/// holds hundreds that are also column and function names (`count`, `name`,
/// `date`, `year`), and colouring those as keywords would paint ordinary
/// identifiers. These are the core SQL words every dialect shares, plus the
/// common Postgres ones.
pub(crate) const KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "as", "asc", "begin", "between", "by", "call", "cascade", "case", "check",
    "collate", "column", "commit", "constraint", "copy", "create", "cross", "cube", "current", "database", "declare", "default",
    "delete", "desc", "distinct", "do", "drop", "else", "elsif", "end", "escape", "except", "execute", "exists", "explain", "false",
    "fetch", "filter", "first", "following", "for", "foreign", "from", "full", "function", "grant", "group", "grouping", "having",
    "if", "ilike", "in", "index", "inner", "insert", "intersect", "interval", "into", "is", "join", "key", "language", "last",
    "lateral", "left", "like", "limit", "locked", "materialized", "natural", "no", "not", "nowait", "null", "nulls", "of", "offset",
    "on", "only", "or", "order", "outer", "over", "owner", "partition", "preceding", "prepare", "primary", "procedure", "range",
    "recursive", "references", "rename", "replace", "restrict", "returning", "returns", "revoke", "right", "rollback", "rollup",
    "row", "rows", "savepoint", "schema", "select", "sequence", "set", "sets", "show", "similar", "skip", "some", "start", "table",
    "temp", "temporary", "then", "to", "transaction", "trigger", "true", "truncate", "unbounded", "union", "unique", "update",
    "use", "using", "vacuum", "values", "view", "when", "where", "while", "window", "with", "within",
];

/// Keywords that are calls when a parenthesis follows directly.
pub(crate) const KEYWORD_FUNCTIONS: &[&str] = &["left", "right", "replace", "if", "truncate", "rollup", "cube", "filter"];

pub(crate) fn is_keyword(lower: &str) -> bool {
    static SET: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    SET.get_or_init(|| KEYWORDS.iter().copied().collect()).contains(lower)
}

/// MySQL and the databases that speak its dialect: a double-quoted run is a
/// string there, not a name.
fn mysql_family(dialect: Option<Dialect>) -> bool {
    dialect == Some(Dialect::MySql)
}

pub(super) fn dialect_of(dialect: Option<Dialect>) -> Box<dyn SqlDialect> {
    match dialect {
        Some(Dialect::PostgreSql) => Box::new(PostgreSqlDialect {}),
        Some(Dialect::Sqlite) => Box::new(SQLiteDialect {}),
        Some(Dialect::MySql) => Box::new(MySqlDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}

/// The role of token `i`, or `None` for what is drawn as plain text
/// (whitespace, operators).
fn role_of(tokens: &[TokenWithSpan], i: usize, types: &TypeNames) -> Option<Role> {
    Some(match &tokens[i].token {
        Token::Word(w) if w.quote_style.is_some() => Role::SynIdentifier,
        Token::Word(w) => {
            let lower = w.value.to_lowercase();
            let call = matches!(tokens.get(i + 1).map(|t| &t.token), Some(Token::LParen));
            let after_dot = i > 0 && matches!(tokens[i - 1].token, Token::Period);
            if after_dot {
                // `f.language`, `t.key`: a name, or a call (`s.count(1)`).
                if call { Role::SynFunction } else { Role::SynIdentifier }
            } else if is_keyword(&lower) && !(call && KEYWORD_FUNCTIONS.contains(&lower.as_str())) {
                Role::SynKeyword
            } else if types.words.contains(&lower) {
                Role::SynType
            } else if call {
                Role::SynFunction
            } else {
                Role::SynIdentifier
            }
        }
        Token::Number(..) | Token::HexStringLiteral(_) => Role::SynNumber,
        Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::TripleSingleQuotedString(_)
        | Token::TripleDoubleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::SingleQuotedByteStringLiteral(_)
        | Token::DoubleQuotedByteStringLiteral(_)
        | Token::TripleSingleQuotedByteStringLiteral(_)
        | Token::TripleDoubleQuotedByteStringLiteral(_)
        | Token::SingleQuotedRawStringLiteral(_)
        | Token::DoubleQuotedRawStringLiteral(_)
        | Token::TripleSingleQuotedRawStringLiteral(_)
        | Token::TripleDoubleQuotedRawStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::QuoteDelimitedStringLiteral(_)
        | Token::NationalQuoteDelimitedStringLiteral(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_) => Role::SynString,
        Token::Whitespace(Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_)) => Role::SynComment,
        Token::Comma | Token::SemiColon => Role::SynPunct,
        Token::Char(_) => Role::SynBad,
        _ => return None,
    })
}

/// Adds the characters from (`from_line`, `from_col`) to (`to_line`, `to_col`)
/// (0-based, the end exclusive) to the lines' spans, one span per line.
fn add(out: &mut [Vec<Span>], lens: &[usize], from: (usize, usize), to: (usize, usize), role: Role) {
    for line in from.0..=to.0.min(out.len().saturating_sub(1)) {
        let start = if line == from.0 { from.1 } else { 0 };
        let end = if line == to.0 { to.1 } else { lens[line] };
        if end > start {
            out[line].push(Span { start, end: end.min(lens[line]), role });
        }
    }
}

/// Colours `lines` (which start at line `first_line` of the buffer).
///
/// The tokenizer fails on an unterminated string, comment or quoted name, and
/// those are the normal state of text being typed. It keeps what it had read
/// before the failure, and the rest of the text is coloured as what opened:
/// a comment, a string, or (for a quoted name) a bad token.
pub fn highlight(lines: &[String], first_line: usize, dialect: Option<Dialect>, types: &TypeNames) -> Highlighted {
    let lens: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
    let mut out: Vec<Vec<Span>> = vec![Vec::new(); lines.len()];
    let mut brackets = Vec::new();
    let sql_dialect = dialect_of(dialect);
    // The tokenizer runs on the text from `base` (line, character) on; when it
    // refuses something that is not an unterminated run, that word is bad and
    // it starts again after it.
    let mut base = (0usize, 0usize);
    while base.0 < lines.len() {
        let mut text: String = lines[base.0].chars().skip(base.1).collect();
        for l in &lines[base.0 + 1..] {
            text.push('\n');
            text.push_str(l);
        }
        let mut tokens: Vec<TokenWithSpan> = Vec::new();
        let result = Tokenizer::new(sql_dialect.as_ref(), &text).tokenize_with_location_into_buf(&mut tokens);

        let pos = |l: sqlparser::tokenizer::Location| {
            let (line, col) = ((l.line as usize).saturating_sub(1), (l.column as usize).saturating_sub(1));
            (line + base.0, col + if line == 0 { base.1 } else { 0 })
        };
        for i in 0..tokens.len() {
            let (from, to) = (pos(tokens[i].span.start), pos(tokens[i].span.end));
            if let Some(role) = role_of(&tokens, i, types) {
                add(&mut out, &lens, from, to, role);
            }
            let ch = match tokens[i].token {
                Token::LParen => Some('('),
                Token::RParen => Some(')'),
                Token::LBracket => Some('['),
                Token::RBracket => Some(']'),
                Token::LBrace => Some('{'),
                Token::RBrace => Some('}'),
                _ => None,
            };
            if let Some(ch) = ch {
                brackets.push(Bracket { line: first_line + from.0, col: from.1, ch });
            }
        }

        let Err(error) = result else { break };
        // The token that failed starts where the last good one ended.
        let from = tokens.last().map_or(base, |t| pos(t.span.end));
        if from.0 >= lines.len() {
            break;
        }
        let rest: String = lines[from.0].chars().skip(from.1).collect();
        let last = (lines.len() - 1, lens[lines.len() - 1]);
        if rest.starts_with("/*") {
            add(&mut out, &lens, from, last, Role::SynComment);
        } else if rest.starts_with('"') && mysql_family(dialect) {
            add(&mut out, &lens, from, last, Role::SynString);
        } else if rest.starts_with('"') || rest.starts_with('`') {
            add(&mut out, &lens, from, last, Role::SynBad);
        } else if error.message.contains("Unterminated") {
            add(&mut out, &lens, from, last, Role::SynString);
        } else {
            // Something else the tokenizer refused: the rest of that word is
            // bad, and what follows is read afresh.
            let word = rest.chars().take_while(|c| !c.is_whitespace()).count().max(1);
            add(&mut out, &lens, from, (from.0, from.1 + word), Role::SynBad);
            base = (from.0, from.1 + word);
            continue;
        }
        break;
    }
    Highlighted { first_line, lines: out, brackets }
}

/// The bracket beside the caret (the one before it, else the one after it)
/// and its partner, as (line, character) pairs.
pub fn matching_bracket(h: &Highlighted, caret: Cursor) -> Option<[(usize, usize); 2]> {
    let find = |col: usize| h.brackets.binary_search_by_key(&(caret.row, col), |b| (b.line, b.col)).ok();
    let at = caret.col.checked_sub(1).and_then(find).or_else(|| find(caret.col))?;
    let here = h.brackets[at];
    let (open, close, forward) = match here.ch {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        _ => ('{', '}', false),
    };
    let mut depth = 0usize;
    let others: Box<dyn Iterator<Item = &Bracket>> = if forward { Box::new(h.brackets[at + 1..].iter()) } else { Box::new(h.brackets[..at].iter().rev()) };
    for b in others {
        let (same_way, opposite) = if forward { (open, close) } else { (close, open) };
        if b.ch == same_way {
            depth += 1;
        } else if b.ch == opposite {
            if depth == 0 {
                return Some([(here.line, here.col), (b.line, b.col)]);
            }
            depth -= 1;
        }
    }
    None
}

/// What the highlighter has done, for tests: work is counted, not timed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub runs: usize,
    pub lines_tokenized: usize,
}

#[derive(Debug)]
struct Cached {
    revision: u64,
    dialect: Option<Dialect>,
    types: u64,
    result: Highlighted,
}

#[derive(Debug, Default)]
pub struct Highlighter {
    pub stats: Stats,
    cached: Option<Cached>,
}

impl Highlighter {
    /// The result of the last `colours`.
    pub fn current(&self) -> &Highlighted {
        static EMPTY: Highlighted = Highlighted { first_line: 0, lines: Vec::new(), brackets: Vec::new() };
        self.cached.as_ref().map_or(&EMPTY, |c| &c.result)
    }

    /// The colours of all of `lines`, recomputed only when the text
    /// (`revision`), the kind or the type names (by generation) changed:
    /// never because of a repaint, a scroll or a caret move. The whole buffer
    /// is tokenized, so a comment or string opened anywhere above the screen
    /// is seen.
    pub fn colours(&mut self, revision: u64, lines: &[String], dialect: Option<Dialect>, types: &TypeNames) -> &Highlighted {
        let fresh = self.cached.as_ref().is_some_and(|c| c.revision == revision && c.dialect == dialect && c.types == types.generation());
        if !fresh {
            self.stats.runs += 1;
            self.stats.lines_tokenized += lines.len();
            let result = highlight(lines, 0, dialect, types);
            self.cached = Some(Cached { revision, dialect, types: types.generation(), result });
        }
        &self.cached.as_ref().expect("set above").result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    fn pg_types() -> TypeNames {
        TypeNames::new(["varchar", "timestamptz", "integer", "text"].map(String::from))
    }

    /// The spans of line `row`, without punctuation, as (text, role).
    fn parts(text: &str, kind: Dialect, row: usize) -> Vec<(String, Role)> {
        let ls = lines(text);
        let h = highlight(&ls, 0, Some(kind), &pg_types());
        let chars: Vec<char> = ls[row].chars().collect();
        h.line(row)
            .expect("the line is covered")
            .iter()
            .filter(|s| s.role != Role::SynPunct)
            .map(|s| (chars[s.start..s.end].iter().collect(), s.role))
            .collect()
    }

    fn pg(text: &str) -> Vec<(String, Role)> {
        parts(text, Dialect::PostgreSql, 0)
    }

    fn p(text: &str, role: Role) -> (String, Role) {
        (text.to_string(), role)
    }

    #[test]
    fn a_select_gets_keyword_string_number_keyword_identifier_and_comment_spans_at_their_columns() {
        let ls = lines("select 'a', 12 from t -- note");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(
            h.line(0).unwrap(),
            &[
                Span { start: 0, end: 6, role: Role::SynKeyword },
                Span { start: 7, end: 10, role: Role::SynString },
                Span { start: 10, end: 11, role: Role::SynPunct },
                Span { start: 12, end: 14, role: Role::SynNumber },
                Span { start: 15, end: 19, role: Role::SynKeyword },
                Span { start: 20, end: 21, role: Role::SynIdentifier },
                Span { start: 22, end: 29, role: Role::SynComment },
            ]
        );
    }

    #[test]
    fn an_unterminated_string_runs_to_the_end_and_what_came_before_keeps_its_colour() {
        assert_eq!(
            pg("select 12, 'abc"),
            [p("select", Role::SynKeyword), p("12", Role::SynNumber), p("'abc", Role::SynString)]
        );
        // Across lines: the rest of the buffer is the string.
        let ls = lines("select 1;\nselect 'abc\nfrom t where");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(0).unwrap()[0], Span { start: 0, end: 6, role: Role::SynKeyword });
        assert_eq!(h.line(1).unwrap().last(), Some(&Span { start: 7, end: 11, role: Role::SynString }));
        assert_eq!(h.line(2).unwrap(), &[Span { start: 0, end: 12, role: Role::SynString }]);
    }

    #[test]
    fn an_unterminated_block_comment_colours_the_rest_as_comment() {
        assert_eq!(pg("select 1 /* later"), [p("select", Role::SynKeyword), p("1", Role::SynNumber), p("/* later", Role::SynComment)]);
        let ls = lines("select 1 /* a\nb\nc");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(0).unwrap().last(), Some(&Span { start: 9, end: 13, role: Role::SynComment }));
        assert_eq!(h.line(1).unwrap(), &[Span { start: 0, end: 1, role: Role::SynComment }]);
        assert_eq!(h.line(2).unwrap(), &[Span { start: 0, end: 1, role: Role::SynComment }]);
    }

    #[test]
    fn a_terminated_block_comment_ends_where_it_ends_and_covers_each_of_its_lines() {
        let ls = lines("/* a\nb */ select 1");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(0).unwrap(), &[Span { start: 0, end: 4, role: Role::SynComment }]);
        assert_eq!(h.line(1).unwrap()[0], Span { start: 0, end: 4, role: Role::SynComment });
        assert_eq!(h.line(1).unwrap()[1], Span { start: 5, end: 11, role: Role::SynKeyword });
    }

    #[test]
    fn a_word_directly_followed_by_a_parenthesis_is_a_function_and_alone_an_identifier() {
        assert_eq!(pg("select count(*) from t")[..2], [p("select", Role::SynKeyword), p("count", Role::SynFunction)]);
        assert_eq!(
            pg("select count from t"),
            [p("select", Role::SynKeyword), p("count", Role::SynIdentifier), p("from", Role::SynKeyword), p("t", Role::SynIdentifier)]
        );
        // `in (` and `values(` are syntax, not calls; `count (` with a space is not directly followed.
        assert_eq!(pg("a in (1)")[1], p("in", Role::SynKeyword));
        assert_eq!(pg("insert into t values(1)")[3], p("values", Role::SynKeyword));
        assert_eq!(pg("select count (*)")[1], p("count", Role::SynIdentifier));
        // A function is a function whatever its case.
        assert_eq!(pg("SELECT Lower(x)")[1], p("Lower", Role::SynFunction));
    }

    #[test]
    fn type_names_take_the_type_role_and_only_where_they_are_not_quoted() {
        assert_eq!(
            pg("create table t (a varchar(20), b timestamptz)"),
            [
                p("create", Role::SynKeyword),
                p("table", Role::SynKeyword),
                p("t", Role::SynIdentifier),
                p("a", Role::SynIdentifier),
                p("varchar", Role::SynType),
                p("20", Role::SynNumber),
                p("b", Role::SynIdentifier),
                p("timestamptz", Role::SynType),
            ]
        );
        assert_eq!(pg("select \"text\", x::TEXT")[1], p("\"text\"", Role::SynIdentifier));
        assert_eq!(pg("select \"text\", x::TEXT")[3], p("TEXT", Role::SynType));
    }

    #[test]
    fn the_words_of_a_multi_word_type_are_all_types() {
        let types = TypeNames::new(["double precision", "character varying", "integer[]", "with time zone"].map(String::from));
        let ls = lines("select a::double precision, b::character varying, with");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &types);
        let chars: Vec<char> = ls[0].chars().collect();
        let got: Vec<(String, Role)> = h.line(0).unwrap().iter().filter(|s| s.role == Role::SynType || chars[s.start..s.end].iter().collect::<String>() == "with").map(|s| (chars[s.start..s.end].iter().collect(), s.role)).collect();
        assert_eq!(
            got,
            [p("double", Role::SynType), p("precision", Role::SynType), p("character", Role::SynType), p("varying", Role::SynType), p("with", Role::SynKeyword)]
        );
    }

    #[test]
    fn columns_are_characters_so_wide_text_before_a_keyword_does_not_shift_it() {
        // 日本語 is three characters (six display columns): the keyword is at
        // character 13 either way, and the painter maps characters to columns.
        assert_eq!(
            pg("select '日本語' from t"),
            [p("select", Role::SynKeyword), p("'日本語'", Role::SynString), p("from", Role::SynKeyword), p("t", Role::SynIdentifier)]
        );
        let ls = lines("select '日本語' from t");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert!(h.line(0).unwrap().contains(&Span { start: 13, end: 17, role: Role::SynKeyword }));
    }

    #[test]
    fn dollar_quoted_and_escape_strings_colour_as_strings_on_postgres() {
        assert_eq!(
            pg("select $$a; 'b$$, $t$ x $t$, E'it\\'s' from t"),
            [
                p("select", Role::SynKeyword),
                p("$$a; 'b$$", Role::SynString),
                p("$t$ x $t$", Role::SynString),
                p("E'it\\'s'", Role::SynString),
                p("from", Role::SynKeyword),
                p("t", Role::SynIdentifier),
            ]
        );
        // The semicolon inside the dollar quotes is not punctuation.
        let ls = lines("select $$a;b$$");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert!(h.line(0).unwrap().iter().all(|s| s.role != Role::SynPunct));
        // Unterminated: the rest of the buffer is the string.
        let ls = lines("select 1, $$abc\ndef; select 2");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(1).unwrap(), &[Span { start: 0, end: 13, role: Role::SynString }]);
    }

    #[test]
    fn an_unterminated_quoted_identifier_is_a_bad_token_and_what_came_before_keeps_its_colour() {
        assert_eq!(pg("select \"abc"), [p("select", Role::SynKeyword), p("\"abc", Role::SynBad)]);
    }

    #[test]
    fn commas_and_semicolons_are_punctuation_and_operators_are_plain() {
        let ls = lines("select 1, 2;");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        let roles: Vec<(usize, Role)> = h.line(0).unwrap().iter().map(|s| (s.start, s.role)).collect();
        assert_eq!(roles, [(0, Role::SynKeyword), (7, Role::SynNumber), (8, Role::SynPunct), (10, Role::SynNumber), (11, Role::SynPunct)]);
        let ls = lines("select a+1");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert!(!h.line(0).unwrap().iter().any(|s| s.start == 8), "the + has no span: {:?}", h.line(0));
    }

    #[test]
    fn mysql_strings_may_be_double_quoted_and_postgres_double_quotes_are_names() {
        assert_eq!(parts("select \"a\"", Dialect::MySql, 0)[1], p("\"a\"", Role::SynString));
        assert_eq!(parts("select \"a\"", Dialect::PostgreSql, 0)[1], p("\"a\"", Role::SynIdentifier));
        assert_eq!(parts("select `a`", Dialect::MySql, 0)[1], p("`a`", Role::SynIdentifier));
    }

    #[test]
    fn the_bracket_beside_the_caret_is_matched_with_its_partner() {
        let ls = lines("select (a + (b)) from t");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        let at = |col| Cursor { row: 0, col };
        assert_eq!(matching_bracket(&h, at(8)), Some([(0, 7), (0, 15)]), "after the first (");
        assert_eq!(matching_bracket(&h, at(7)), Some([(0, 7), (0, 15)]), "before the first (");
        assert_eq!(matching_bracket(&h, at(16)), Some([(0, 15), (0, 7)]), "after the last )");
        assert_eq!(matching_bracket(&h, at(13)), Some([(0, 12), (0, 14)]), "the inner pair");
        assert_eq!(matching_bracket(&h, at(1)), None, "beside no bracket");
    }

    #[test]
    fn brackets_inside_strings_and_comments_are_not_brackets() {
        let ls = lines("select ')', (1) -- (\n, '(')");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        // After the real `(` (character 12): the partner is the real `)` at 14.
        assert_eq!(matching_bracket(&h, Cursor { row: 0, col: 13 }), Some([(0, 12), (0, 14)]));
        // Beside the `)` inside the string: nothing.
        assert_eq!(matching_bracket(&h, Cursor { row: 0, col: 9 }), None);
        // Beside the `(` inside the comment: nothing.
        assert_eq!(matching_bracket(&h, Cursor { row: 0, col: 20 }), None);
        // The real `)` on line 1 has no partner.
        assert_eq!(matching_bracket(&h, Cursor { row: 1, col: 7 }), None);
    }

    #[test]
    fn a_bracket_pairs_across_lines_and_square_and_curly_ones_pair_with_their_own_kind() {
        let ls = lines("select (a,\n [b]) ");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(matching_bracket(&h, Cursor { row: 0, col: 8 }), Some([(0, 7), (1, 4)]));
        assert_eq!(matching_bracket(&h, Cursor { row: 1, col: 2 }), Some([(1, 1), (1, 3)]));
    }

    #[test]
    fn a_word_directly_after_a_dot_is_an_identifier_even_when_it_is_a_keyword() {
        assert_eq!(
            pg("select f.language, t.key, x.from from f"),
            [
                p("select", Role::SynKeyword),
                p("f", Role::SynIdentifier),
                p("language", Role::SynIdentifier),
                p("t", Role::SynIdentifier),
                p("key", Role::SynIdentifier),
                p("x", Role::SynIdentifier),
                p("from", Role::SynIdentifier),
                p("from", Role::SynKeyword),
                p("f", Role::SynIdentifier),
            ]
        );
        // A type name or a call after a dot is still an identifier or a call.
        assert_eq!(pg("select s.text, s.count(1)")[2], p("text", Role::SynIdentifier));
        assert_eq!(pg("select s.count(1)")[2], p("count", Role::SynFunction));
    }

    #[test]
    fn an_unterminated_double_quote_is_a_string_on_mysql_and_its_family_and_a_bad_name_elsewhere() {
        assert_eq!(parts("select \"abc", Dialect::MySql, 0)[1], p("\"abc", Role::SynString));
        assert_eq!(parts("select \"abc", Dialect::MySql, 0)[1], p("\"abc", Role::SynString));
        assert_eq!(parts("select \"abc", Dialect::PostgreSql, 0)[1], p("\"abc", Role::SynBad));
    }

    #[test]
    fn one_stray_character_the_tokenizer_refuses_does_not_turn_the_rest_of_the_buffer_plain() {
        // A bad escape in a unicode string is an error that is not "unterminated".
        let ls = lines("select U&'x\\zz' from t;\nselect 'x' from t;\nselect 2");
        let h = highlight(&ls, 0, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(0).unwrap()[1], Span { start: 7, end: 15, role: Role::SynBad });
        assert!(h.line(0).unwrap().contains(&Span { start: 16, end: 20, role: Role::SynKeyword }), "{:?}", h.line(0));
        assert!(h.line(1).unwrap().contains(&Span { start: 7, end: 10, role: Role::SynString }), "{:?}", h.line(1));
        assert!(h.line(2).unwrap().contains(&Span { start: 7, end: 8, role: Role::SynNumber }), "{:?}", h.line(2));
    }

    // ---- the work done ----------------------------------------------------------

    fn big() -> Vec<String> {
        (0..5000).map(|i| format!("select {i}, 'x' from t where a = {i}; -- note")).collect()
    }

    #[test]
    fn text_in_a_comment_or_string_opened_far_above_the_view_has_the_comment_or_string_role() {
        let mut ls: Vec<String> = (0..1000).map(|i| format!("select {i};")).collect();
        ls[100] = "/* it's".to_string();
        ls[700] = "*/".to_string();
        let mut hl = Highlighter::default();
        let h = hl.colours(1, &ls, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(610).unwrap(), &[Span { start: 0, end: ls[610].chars().count(), role: Role::SynComment }]);
        // After the comment ends the code is code again.
        assert_eq!(h.line(701).unwrap()[0], Span { start: 0, end: 6, role: Role::SynKeyword });

        // A string opened above the view: the lines in it are the string.
        ls[100] = "select 'open".to_string();
        ls[700] = "close' from t".to_string();
        let h = hl.colours(2, &ls, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(610).unwrap(), &[Span { start: 0, end: ls[610].chars().count(), role: Role::SynString }]);
    }

    #[test]
    fn a_big_buffer_is_tokenized_once_per_edit_and_never_per_redraw_or_scroll() {
        let ls = big();
        let mut hl = Highlighter::default();
        let types = pg_types();
        hl.colours(1, &ls, Some(Dialect::PostgreSql), &types);
        assert_eq!(hl.stats, Stats { runs: 1, lines_tokenized: 5000 });
        let after_first = hl.stats;
        // The same text, drawn again, and scrolled far away.
        hl.colours(1, &ls, Some(Dialect::PostgreSql), &types);
        hl.colours(1, &ls, Some(Dialect::PostgreSql), &types);
        hl.colours(1, &ls, Some(Dialect::PostgreSql), &types);
        assert_eq!(hl.stats, after_first, "nothing changed, nothing was tokenized");
        assert!(hl.current().line(4010).is_some() && hl.current().line(10).is_some());
        // A keystroke: a new revision.
        hl.colours(2, &ls, Some(Dialect::PostgreSql), &types);
        assert_eq!(hl.stats.runs, 2);
    }

    #[test]
    fn a_small_buffer_is_tokenized_whole_so_a_comment_above_the_screen_still_counts() {
        let mut ls: Vec<String> = (0..100).map(|i| format!("select {i};")).collect();
        ls[0] = "/* open".to_string();
        let mut hl = Highlighter::default();
        let h = hl.colours(1, &ls, Some(Dialect::PostgreSql), &pg_types());
        assert_eq!(h.line(70).unwrap(), &[Span { start: 0, end: 10, role: Role::SynComment }]);
    }

    #[test]
    fn a_new_kind_or_new_type_names_recompute_the_same_text() {
        let ls = lines("select cast(x as varchar)");
        let mut hl = Highlighter::default();
        let none = TypeNames::default();
        let h = hl.colours(1, &ls, Some(Dialect::PostgreSql), &none);
        assert!(!h.line(0).unwrap().iter().any(|s| s.role == Role::SynType));
        let h = hl.colours(1, &ls, Some(Dialect::PostgreSql), &pg_types());
        assert!(h.line(0).unwrap().iter().any(|s| s.role == Role::SynType), "the types arrived");
        let runs = hl.stats.lines_tokenized;
        hl.colours(1, &ls, Some(Dialect::MySql), &pg_types());
        assert!(hl.stats.lines_tokenized > runs, "another kind, another dialect");
    }
}
