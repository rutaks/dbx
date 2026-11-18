//! What is being completed, from the tokens of the statement under the caret.
//!
//! It must work on SQL that does not parse (that is the state text is in while
//! it is typed), so it reads tokens, not a syntax tree: a small scanner that
//! knows words, quoted names, strings, comments and the punctuation that
//! matters (`.` `,` `;` brackets), and a few rules about the keyword before
//! the caret. The statement is the run of tokens between `;` on either side.
//! (dbx also splits a statement before a line that starts with a keyword like
//! `CREATE` after blank lines; that is the Core's business, and here a `;` is
//! the only boundary.)
//!
//! Pure: lines and a caret in, a `Context` out. What a qualifier *names* (a
//! schema, a table, an alias) is for the caller, who knows the schema: the
//! aliases the statement defines are in `refs`.

use crate::core::Cursor;

/// Which keywords the position allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keywords {
    /// Any of the curated list.
    Any,
    /// Only these, because the grammar allows nothing else here.
    Only(&'static [&'static str]),
    None,
}

/// A name before the dot, as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qualifier {
    /// `public.film.` is `["public", "film"]`.
    pub parts: Vec<String>,
    /// The dot is where a table is expected (after FROM, JOIN, ...): what is
    /// wanted after a schema is its tables.
    pub relation: bool,
}

/// What kinds of thing the position takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wants {
    pub keywords: Keywords,
    /// Tables and views.
    pub tables: bool,
    pub schemas: bool,
    /// After `name.`: what `name` names decides.
    pub qualified: Option<Qualifier>,
    /// Columns of the tables the statement names.
    pub columns: bool,
    pub functions: bool,
}

impl Wants {
    fn none() -> Self {
        Wants { keywords: Keywords::None, tables: false, schemas: false, qualified: None, columns: false, functions: false }
    }
}

/// A table named in the statement: `film f`, `public.film as f`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// The part of the word left of the caret, without an opening quote.
    pub word: String,
    /// Where the word starts (at its opening quote, for a quoted name).
    pub word_start: Cursor,
    /// Where the word ends on the right of the caret: what `tab` replaces
    /// (past the closing quote, for a quoted name).
    pub word_end: Cursor,
    /// The word began with `"`.
    pub quoted: bool,
    /// The quoted name has its closing quote (the caret is inside it).
    pub quote_closed: bool,
    /// The word stands right after a table's name, where an alias goes (or a
    /// keyword that follows the table): typing there does not open a popup of
    /// its own accord.
    pub alias_position: bool,
    pub wants: Wants,
    /// The tables the statement names, with their aliases.
    pub refs: Vec<TableRef>,
    /// The caret is inside a string or a comment, or on a number: nothing is
    /// offered.
    pub literal: bool,
}

impl Context {
    /// Whether anything at all can be offered here.
    pub fn offers(&self) -> bool {
        !self.literal
            && (self.wants.keywords != Keywords::None
                || self.wants.tables
                || self.wants.schemas
                || self.wants.qualified.is_some()
                || self.wants.columns
                || self.wants.functions)
    }
}

// ---- the scanner -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// An unquoted word, as typed.
    Word(String),
    /// `"name"`: the name without its quotes, and whether it was closed.
    Quoted(String, bool),
    Number,
    /// A string (`'..'`, `$$..$$`): whether it was closed.
    Str(bool),
    /// A comment: whether it was closed (a line comment never is: it runs to
    /// the end of the line, and a caret at that end is still in it).
    Comment(bool),
    Dot,
    Comma,
    Semi,
    LParen,
    RParen,
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: Cursor,
    /// Exclusive.
    end: Cursor,
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The tokens of the buffer. Never fails: what is unfinished (a string, a
/// comment, a quoted name) is a token that says so.
fn scan(lines: &[String]) -> Vec<Token> {
    let rows: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
    let mut out = Vec::new();
    let (mut row, mut col) = (0usize, 0usize);
    let at = |row, col| Cursor { row, col };
    while row < rows.len() {
        let line = &rows[row];
        if col >= line.len() {
            row += 1;
            col = 0;
            continue;
        }
        let c = line[col];
        let start = at(row, col);
        let next = line.get(col + 1).copied();
        if c.is_whitespace() {
            col += 1;
        } else if c == '-' && next == Some('-') {
            out.push(Token { tok: Tok::Comment(false), start, end: at(row, line.len()) });
            col = line.len();
        } else if c == '/' && next == Some('*') {
            // Up to `*/`, over lines.
            let (mut r, mut k, mut closed) = (row, col + 2, false);
            'block: while r < rows.len() {
                while k < rows[r].len() {
                    if rows[r][k] == '*' && rows[r].get(k + 1) == Some(&'/') {
                        k += 2;
                        closed = true;
                        break 'block;
                    }
                    k += 1;
                }
                if r + 1 == rows.len() {
                    break;
                }
                r += 1;
                k = 0;
            }
            out.push(Token { tok: Tok::Comment(closed), start, end: at(r, k) });
            (row, col) = (r, k);
        } else if c == '\'' {
            let (mut r, mut k, mut closed) = (row, col + 1, false);
            'string: while r < rows.len() {
                while k < rows[r].len() {
                    if rows[r][k] == '\'' {
                        if rows[r].get(k + 1) == Some(&'\'') {
                            k += 2;
                            continue;
                        }
                        k += 1;
                        closed = true;
                        break 'string;
                    }
                    k += 1;
                }
                if r + 1 == rows.len() {
                    break;
                }
                r += 1;
                k = 0;
            }
            out.push(Token { tok: Tok::Str(closed), start, end: at(r, k) });
            (row, col) = (r, k);
        } else if c == '$' && dollar_tag(line, col).is_some() {
            let tag: String = dollar_tag(line, col).unwrap_or_default();
            let (mut r, mut k, mut closed) = (row, col + tag.chars().count(), false);
            'dollar: while r < rows.len() {
                while k < rows[r].len() {
                    if dollar_tag(&rows[r], k).as_deref() == Some(tag.as_str()) {
                        k += tag.chars().count();
                        closed = true;
                        break 'dollar;
                    }
                    k += 1;
                }
                if r + 1 == rows.len() {
                    break;
                }
                r += 1;
                k = 0;
            }
            out.push(Token { tok: Tok::Str(closed), start, end: at(r, k) });
            (row, col) = (r, k);
        } else if c == '"' {
            // A quoted name stays on its line.
            let (mut k, mut closed) = (col + 1, false);
            let mut name = String::new();
            while k < line.len() {
                if line[k] == '"' {
                    if line.get(k + 1) == Some(&'"') {
                        name.push('"');
                        k += 2;
                        continue;
                    }
                    k += 1;
                    closed = true;
                    break;
                }
                name.push(line[k]);
                k += 1;
            }
            out.push(Token { tok: Tok::Quoted(name, closed), start, end: at(row, k) });
            col = k;
        } else if c.is_ascii_digit() {
            let mut k = col + 1;
            while k < line.len() && (is_word_char(line[k]) || (line[k] == '.' && line.get(k + 1).is_some_and(char::is_ascii_digit))) {
                k += 1;
            }
            out.push(Token { tok: Tok::Number, start, end: at(row, k) });
            col = k;
        } else if is_word_start(c) {
            let mut k = col + 1;
            while k < line.len() && is_word_char(line[k]) {
                k += 1;
            }
            let word: String = line[col..k].iter().collect();
            out.push(Token { tok: Tok::Word(word), start, end: at(row, k) });
            col = k;
        } else {
            let tok = match c {
                '.' => Tok::Dot,
                ',' => Tok::Comma,
                ';' => Tok::Semi,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                _ => Tok::Other,
            };
            out.push(Token { tok, start, end: at(row, col + 1) });
            col += 1;
        }
    }
    out
}

/// The `$tag$` that opens (or closes) a dollar-quoted string at `col`, with
/// its dollars: `$$` or `$body$`. Not `$1`.
fn dollar_tag(line: &[char], col: usize) -> Option<String> {
    let mut k = col + 1;
    while k < line.len() && (line[k].is_alphabetic() || line[k] == '_') {
        k += 1;
    }
    (line.get(k) == Some(&'$')).then(|| line[col..=k].iter().collect())
}

/// The text between a quoted name's opening quote and somewhere inside it, as
/// the name's own characters: a doubled quote is one.
fn unescape_quotes(raw: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        out.push(raw[i]);
        i += if raw[i] == '"' && raw.get(i + 1) == Some(&'"') { 2 } else { 1 };
    }
    out
}

fn before(a: Cursor, b: Cursor) -> bool {
    (a.row, a.col) < (b.row, b.col)
}

/// Whether `caret` is inside the token (not at its start; at its end only
/// when the token is not closed).
fn inside(t: &Token, caret: Cursor) -> bool {
    let closed = match t.tok {
        Tok::Str(c) | Tok::Comment(c) => c,
        _ => return false,
    };
    before(t.start, caret) && (before(caret, t.end) || (!closed && caret == t.end))
}

// ---- the rules -------------------------------------------------------------

/// Keywords that start a clause that takes table names.
fn relation_keyword(w: &str) -> bool {
    matches!(w, "from" | "join" | "into" | "update" | "table")
}

/// Keywords after which a table-name list is over.
fn clause_keyword(w: &str) -> bool {
    matches!(
        w,
        "select" | "where" | "on" | "group" | "order" | "having" | "set" | "values" | "returning" | "limit" | "offset" | "union"
            | "intersect" | "except" | "using" | "window" | "when" | "then" | "else" | "case" | "and" | "or" | "fetch" | "for" | "with"
    )
}

/// Keywords that end a FROM list (a comma after them is not another table).
fn list_ends(w: &str) -> bool {
    matches!(
        w,
        "select" | "where" | "group" | "order" | "having" | "set" | "values" | "returning" | "limit" | "offset" | "union" | "intersect"
            | "except" | "window" | "fetch" | "for" | "with"
    )
}

/// What can follow a table name (and its alias) in a FROM list.
const AFTER_RELATION: &[&str] = &[
    "where", "join", "inner", "left", "right", "full", "cross", "natural", "on", "using", "group", "order", "having", "limit",
    "offset", "union", "intersect", "except", "window", "fetch", "for", "returning", "set",
];

/// The words that can follow `w` and nothing else, where the grammar leaves no
/// choice.
fn only_after(w: &str) -> Option<&'static [&'static str]> {
    Some(match w {
        "order" | "group" => &["by"],
        "insert" => &["into"],
        "delete" => &["from"],
        "left" | "right" | "full" => &["join", "outer"],
        "inner" | "cross" | "natural" => &["join"],
        "union" | "intersect" | "except" => &["all", "select"],
        "is" => &["not", "null", "true", "false"],
        _ => return None,
    })
}

fn lower(t: &Token) -> Option<String> {
    match &t.tok {
        Tok::Word(w) => Some(w.to_lowercase()),
        _ => None,
    }
}

/// A name token's text.
fn name_of(t: &Token) -> Option<&str> {
    match &t.tok {
        Tok::Word(w) => Some(w),
        Tok::Quoted(n, _) => Some(n),
        _ => None,
    }
}

/// Whether the last name of `before` (it ends with a name) is a table's own
/// name, not an alias: the name, with its schema before it, follows a keyword
/// that takes tables, or a comma in such a list.
fn names_a_table(before: &[&Token]) -> bool {
    let mut k = before.len();
    // The last name, and the `schema.` parts in front of it.
    if k == 0 || name_of(before[k - 1]).is_none() {
        return false;
    }
    k -= 1;
    while k >= 2 && before[k - 1].tok == Tok::Dot && name_of(before[k - 2]).is_some() {
        k -= 2;
    }
    match k.checked_sub(1).map(|i| before[i]) {
        Some(t) if t.tok == Tok::Comma => true,
        Some(t) => lower(t).is_some_and(|w| relation_keyword(&w) || w == "only" || w == "lateral"),
        None => false,
    }
}

/// The tables a statement names. `skip` is the token being typed.
fn table_refs(toks: &[&Token], skip: impl Fn(&Token) -> bool) -> Vec<TableRef> {
    let toks: Vec<&Token> = toks.iter().copied().filter(|t| !skip(t)).collect();
    let mut refs = Vec::new();
    // `list` is the bracket depth of the FROM list a comma continues.
    let (mut i, mut list, mut depth) = (0usize, None::<usize>, 0usize);
    while i < toks.len() {
        let word = lower(toks[i]);
        let take = match word.as_deref() {
            Some("from" | "update") => {
                list = Some(depth);
                Some(true)
            }
            Some("join") => Some(true),
            Some("into") => {
                list = None;
                Some(false)
            }
            Some(w) if list_ends(w) => {
                list = None;
                None
            }
            _ if toks[i].tok == Tok::Comma && list == Some(depth) => Some(true),
            _ => None,
        };
        match toks[i].tok {
            Tok::LParen => depth += 1,
            Tok::RParen => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
        let Some(alias_without_as) = take else { continue };
        // `only`, `lateral`: not part of the name.
        while toks.get(i).and_then(|t| lower(t)).is_some_and(|w| w == "only" || w == "lateral") {
            i += 1;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut complete = true;
        while let Some(name) = toks.get(i).and_then(|t| name_of(t)) {
            parts.push(name.to_string());
            i += 1;
            if toks.get(i).is_some_and(|t| t.tok == Tok::Dot) {
                i += 1;
                complete = toks.get(i).and_then(|t| name_of(t)).is_some();
            } else {
                complete = true;
                break;
            }
        }
        if parts.is_empty() || !complete {
            continue;
        }
        let name = parts.pop().unwrap_or_default();
        let schema = parts.pop();
        let mut alias = None;
        if toks.get(i).and_then(|t| lower(t)).as_deref() == Some("as") {
            alias = toks.get(i + 1).and_then(|t| name_of(t)).map(String::from);
            i += if alias.is_some() { 2 } else { 1 };
        } else if alias_without_as {
            if let Some(next) = toks.get(i) {
                let is_alias = match &next.tok {
                    Tok::Word(w) => !super::highlight::is_keyword(&w.to_lowercase()),
                    Tok::Quoted(..) => true,
                    _ => false,
                };
                if is_alias {
                    alias = name_of(next).map(String::from);
                    i += 1;
                }
            }
        }
        refs.push(TableRef { schema, name, alias });
    }
    refs
}

/// The most bytes of text a context reads, and the most rows either way from
/// the caret: a statement is not read past them. Typing in a huge buffer costs
/// what typing in a statement does.
const WINDOW_BYTES: usize = 16 * 1024;
const WINDOW_ROWS: usize = 200;

/// The rows `from..to` the statement around `row` is read from: from the
/// nearest row above that has a `;` in it, to the nearest below that has one,
/// within the budget. The caret's own row is always in.
pub fn window(lines: &[String], row: usize) -> (usize, usize) {
    let row = row.min(lines.len().saturating_sub(1));
    let mut spent = lines.get(row).map_or(0, String::len);
    let (mut from, mut to) = (row, row + 1);
    let mut r = row;
    while r > 0 && row - r < WINDOW_ROWS && !lines[r].contains(';') {
        let next = &lines[r - 1];
        if spent + next.len() > WINDOW_BYTES {
            break;
        }
        spent += next.len();
        r -= 1;
        from = r;
    }
    let mut r = row;
    while r + 1 < lines.len() && r - row < WINDOW_ROWS && !lines[r].contains(';') {
        let next = &lines[r + 1];
        if spent + next.len() > WINDOW_BYTES {
            break;
        }
        spent += next.len();
        r += 1;
        to = r + 1;
    }
    (from, to)
}

/// The context of the caret at `caret` in `lines`.
pub fn analyse(lines: &[String], caret: Cursor) -> Context {
    let (from, to) = window(lines, caret.row);
    let to = to.min(lines.len());
    let mut context = analyse_rows(&lines[from.min(to)..to], Cursor { row: caret.row.saturating_sub(from), col: caret.col });
    context.word_start.row += from;
    context.word_end.row += from;
    context
}

fn analyse_rows(lines: &[String], caret: Cursor) -> Context {
    let tokens = scan(lines);
    let nothing = |at: Cursor| Context {
        word: String::new(),
        word_start: at,
        word_end: at,
        quoted: false,
        quote_closed: false,
        alias_position: false,
        wants: Wants::none(),
        refs: Vec::new(),
        literal: false,
    };
    // Inside a string or a comment, or on a number: nothing.
    if tokens.iter().any(|t| inside(t, caret)) {
        return Context { literal: true, ..nothing(caret) };
    }
    if let Some(num) = tokens.iter().find(|t| t.tok == Tok::Number && before(t.start, caret) && !before(t.end, caret)) {
        return Context { literal: true, ..nothing(num.start) };
    }

    // The word: a name token the caret is in or at the end of (left of the
    // caret is the partial word; the rest of it is what `tab` replaces).
    let (mut word, mut word_start, mut word_end, mut quoted, mut quote_closed) = (String::new(), caret, caret, false, false);
    for t in &tokens {
        match &t.tok {
            Tok::Word(w) if !before(caret, t.start) && !before(t.end, caret) && (before(t.start, caret) || caret == t.start) => {
                word = w.chars().take(caret.col.saturating_sub(t.start.col)).collect();
                (word_start, word_end) = (t.start, t.end);
            }
            Tok::Quoted(name, closed) if before(t.start, caret) && (before(caret, t.end) && (!closed || caret.col < t.end.col) || (!closed && caret == t.end)) => {
                // What is left of the caret is measured in the text, where `""`
                // is two characters, and the name has it as one.
                let raw = lines.get(caret.row).map(|l| l.chars().skip(t.start.col + 1).take(caret.col.saturating_sub(t.start.col + 1)).collect::<Vec<char>>());
                word = unescape_quotes(&raw.unwrap_or_default());
                let _ = name;
                (word_start, word_end, quoted, quote_closed) = (t.start, t.end, true, *closed);
            }
            _ => {}
        }
    }
    // A caret at the start of a word that ends where another begins is not
    // in it: the first match wins, and tokens do not touch.

    // The statement: between the `;` before the word and the one after it.
    let sig: Vec<&Token> = tokens.iter().filter(|t| !matches!(t.tok, Tok::Comment(_))).collect();
    let from = sig.iter().rposition(|t| t.tok == Tok::Semi && !before(word_start, t.end)).map_or(0, |i| i + 1);
    let to = sig.iter().position(|t| t.tok == Tok::Semi && !before(t.start, word_end)).unwrap_or(sig.len());
    let stmt = &sig[from..to.max(from)];
    let being_typed = |t: &Token| word_start == t.start && (word_end == t.end || !word.is_empty() || quoted);
    let refs = table_refs(stmt, |t| being_typed(t) && name_of(t).is_some());

    // What stands before the word: its qualifier, then the token before that.
    let mut j = stmt.iter().position(|t| !before(t.start, word_start)).unwrap_or(stmt.len());
    let mut parts: Vec<String> = Vec::new();
    while j > 0 && stmt[j - 1].tok == Tok::Dot {
        match j.checked_sub(2).and_then(|k| name_of(stmt[k])) {
            Some(name) => {
                parts.insert(0, name.to_string());
                j -= 2;
            }
            // `(1).`, `*.`: a dot that follows nothing we can name.
            None => return Context { word, word_start, word_end, quoted, quote_closed, refs, ..nothing(caret) },
        }
    }
    let lead = j.checked_sub(1).map(|k| stmt[k]);

    // The clause the word is in: the last keyword before it that sets one.
    let mut in_relation = false;
    for t in &stmt[..j] {
        if let Some(w) = lower(t) {
            if relation_keyword(&w) {
                in_relation = true;
            } else if clause_keyword(&w) {
                in_relation = false;
            }
        }
    }
    let lead_word = lead.and_then(lower);
    let relation_position = in_relation
        && (lead_word.as_deref().is_some_and(|w| relation_keyword(w) || w == "only" || w == "lateral")
            || lead.is_some_and(|t| t.tok == Tok::Comma));

    let mut alias_position = false;
    let mut wants = Wants::none();
    if !parts.is_empty() {
        wants.qualified = Some(Qualifier { parts, relation: relation_position });
    } else if relation_position {
        wants.tables = true;
        wants.schemas = true;
        wants.keywords = match lead_word.as_deref() {
            Some("from") => Keywords::Only(&["lateral", "only"]),
            Some("update") => Keywords::Only(&["only"]),
            Some("join") => Keywords::Only(&["lateral"]),
            None => Keywords::Only(&["lateral"]),
            _ => Keywords::None,
        };
    } else if in_relation && lead.is_some_and(|t| name_of(t).is_some() || t.tok == Tok::RParen) && lead_word.as_deref().is_none_or(|w| !super::highlight::is_keyword(w)) {
        // After a table's name, or its alias: what can follow it. Right after
        // `as` a name is being made up.
        if lead_word.as_deref() != Some("as") {
            wants.keywords = Keywords::Only(AFTER_RELATION);
            alias_position = lead.is_some_and(|t| t.tok == Tok::RParen) || lead.is_some_and(|_| names_a_table(&stmt[..j]));
        }
    } else if lead_word.as_deref() == Some("as") && in_relation {
        // Nothing: an alias is being typed.
    } else if let Some(only) = lead_word.as_deref().and_then(only_after) {
        wants.keywords = Keywords::Only(only);
    } else {
        wants.keywords = Keywords::Any;
        wants.columns = true;
        wants.functions = true;
    }
    // Whatever is quoted is a name: no keyword or function is.
    if quoted {
        wants.keywords = Keywords::None;
        wants.functions = false;
    }
    Context { word, word_start, word_end, quoted, quote_closed, alias_position, wants, refs, literal: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` with `|` marking the caret.
    fn ctx(text: &str) -> Context {
        let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
        let mut caret = None;
        for (row, line) in lines.iter_mut().enumerate() {
            if let Some(i) = line.find('|') {
                caret = Some(Cursor { row, col: line[..i].chars().count() });
                line.replace_range(i..i + 1, "");
            }
        }
        analyse(&lines, caret.expect("a caret marked with |"))
    }

    fn at(row: usize, col: usize) -> Cursor {
        Cursor { row, col }
    }

    fn table_ref(schema: Option<&str>, name: &str, alias: Option<&str>) -> TableRef {
        TableRef { schema: schema.map(String::from), name: name.into(), alias: alias.map(String::from) }
    }

    #[test]
    fn after_from_the_word_wants_tables_and_schemas_and_the_keywords_valid_there() {
        let c = ctx("select * from fi|");
        assert_eq!((c.word.as_str(), c.word_start, c.word_end), ("fi", at(0, 14), at(0, 16)));
        assert!(c.wants.tables && c.wants.schemas);
        assert!(!c.wants.columns && !c.wants.functions && c.wants.qualified.is_none());
        assert_eq!(c.wants.keywords, Keywords::Only(&["lateral", "only"]));
        assert!(c.offers());
    }

    #[test]
    fn after_each_relation_keyword_tables_are_wanted() {
        for (sql, keywords) in [
            ("select * from |", Keywords::Only(&["lateral", "only"])),
            ("select * from film f join |", Keywords::Only(&["lateral"])),
            ("insert into |", Keywords::None),
            ("update |", Keywords::Only(&["only"])),
            ("truncate table |", Keywords::None),
            ("select * from film f, |", Keywords::Only(&["lateral"])),
        ] {
            let c = ctx(sql);
            assert!(c.wants.tables && c.wants.schemas, "{sql}");
            assert!(!c.wants.columns && !c.wants.functions, "{sql}");
            assert_eq!(c.wants.keywords, keywords, "{sql}");
        }
    }

    #[test]
    fn after_a_name_and_a_dot_the_qualifier_is_kept_and_the_word_is_what_follows_it() {
        let c = ctx("select * from public.fi|");
        assert_eq!(c.word, "fi");
        let q = c.wants.qualified.as_ref().expect("a qualifier");
        assert_eq!((q.parts.as_slice(), q.relation), (["public".to_string()].as_slice(), true));
        assert!(!c.wants.functions && !c.wants.columns && c.wants.keywords == Keywords::None);

        let c = ctx("select f.| from film f");
        assert_eq!(c.word, "");
        assert_eq!(c.wants.qualified, Some(Qualifier { parts: vec!["f".into()], relation: false }));

        let c = ctx("select public.film.ti| from film");
        assert_eq!(c.wants.qualified.unwrap().parts, ["public", "film"]);
        assert_eq!(c.word, "ti");
    }

    #[test]
    fn aliases_defined_in_the_statement_are_collected_with_and_without_as() {
        let c = ctx("select f.| from film f join film_actor as fa on fa.film_id = f.film_id, actor");
        assert_eq!(c.refs, [table_ref(None, "film", Some("f")), table_ref(None, "film_actor", Some("fa")), table_ref(None, "actor", None)]);
    }

    #[test]
    fn a_keyword_after_a_table_is_not_its_alias_and_schemas_and_quotes_are_read() {
        let c = ctx("select 1 from film where |");
        assert_eq!(c.refs, [table_ref(None, "film", None)]);
        let c = ctx("select 1 from public.film f, \"Order Items\" oi left join staff on true where |");
        assert_eq!(
            c.refs,
            [table_ref(Some("public"), "film", Some("f")), table_ref(None, "Order Items", Some("oi")), table_ref(None, "staff", None)]
        );
        let c = ctx("update film f set title = 'x' where f.|");
        assert_eq!(c.refs, [table_ref(None, "film", Some("f"))]);
    }

    #[test]
    fn only_the_statement_under_the_caret_counts() {
        let c = ctx("select * from other o;\nselect f.| from film f;\nselect * from third t");
        assert_eq!(c.refs, [table_ref(None, "film", Some("f"))]);
        let c = ctx("select 1; select fi|");
        assert!(c.refs.is_empty());
    }

    #[test]
    fn the_word_being_typed_is_not_a_table_of_the_statement() {
        assert!(ctx("select * from fi|").refs.is_empty());
        assert!(ctx("select * from public.fi|").refs.is_empty(), "nor the schema in front of it");
        assert_eq!(ctx("select * from film f, ac|").refs, [table_ref(None, "film", Some("f"))]);
    }

    #[test]
    fn elsewhere_keywords_columns_of_the_named_tables_and_functions_are_wanted() {
        let c = ctx("select fi| from film");
        assert_eq!(c.wants, Wants { keywords: Keywords::Any, tables: false, schemas: false, qualified: None, columns: true, functions: true });
        let c = ctx("select * from film where fi|");
        assert!(c.wants.columns && c.wants.functions && c.wants.keywords == Keywords::Any && !c.wants.tables);
        let c = ctx("sel|");
        assert!(c.wants.keywords == Keywords::Any && c.offers());
    }

    #[test]
    fn some_keywords_take_only_the_words_the_grammar_allows_next() {
        for (sql, only) in [
            ("select * from film order |", &["by"][..]),
            ("select * from film group |", &["by"]),
            ("insert |", &["into"]),
            ("delete |", &["from"]),
            ("select * from film f left |", &["join", "outer"]),
            ("select 1 union |", &["all", "select"]),
        ] {
            let c = ctx(sql);
            assert_eq!(c.wants.keywords, Keywords::Only(only), "{sql}");
            assert!(!c.wants.columns && !c.wants.functions, "{sql}");
        }
    }

    #[test]
    fn after_a_table_name_only_the_words_that_can_follow_it_are_wanted() {
        let c = ctx("select * from film f|");
        assert!(matches!(c.wants.keywords, Keywords::Only(w) if w.contains(&"where") && w.contains(&"join")));
        assert!(!c.wants.tables && !c.wants.columns && !c.wants.functions);
        let c = ctx("select * from film wh|");
        assert_eq!(c.word, "wh");
        assert!(matches!(c.wants.keywords, Keywords::Only(w) if w.contains(&"where")));
        // Right after `as` a new name is being made up.
        assert!(!ctx("select * from film as f|").offers());
    }

    #[test]
    fn strings_and_comments_offer_nothing_but_their_edges_are_outside() {
        for sql in [
            "select 'fi|",
            "select 'fi|' from t",
            "select 'it''s fi|",
            "select 1 -- fi|",
            "-- fi|\nselect 1",
            "/* fi| */ select 1",
            "select 1 /* fi|",
            "select $$ fi|$$",
        ] {
            let c = ctx(sql);
            assert!(c.literal && !c.offers(), "{sql}");
        }
        for sql in ["select 'a' fi|", "select 'a'| from t", "select /* c */ fi|", "-- c\nselect fi|", "select 'it''s' fi|"] {
            assert!(!ctx(sql).literal, "{sql}");
        }
        assert_eq!(ctx("select 'a' fi|").word, "fi");
    }

    #[test]
    fn a_number_offers_nothing() {
        let c = ctx("select 12|");
        assert!(c.literal && !c.offers());
        assert!(!ctx("select 1.5|").offers());
        assert!(ctx("select a1|").offers(), "a name with a digit in it is a name");
    }

    #[test]
    fn a_quoted_name_is_one_word_that_starts_at_its_quote() {
        let c = ctx("select * from \"Or|");
        assert!(c.quoted);
        assert_eq!((c.word.as_str(), c.word_start, c.word_end), ("Or", at(0, 14), at(0, 17)));
        let c = ctx("select * from \"Or|der Items\" x");
        assert_eq!(c.word, "Or");
        assert_eq!(c.word_end, at(0, 27), "past the closing quote");
        assert_eq!(ctx("select \"a b\"| from t").word, "", "after the closing quote nothing is being typed");
    }

    #[test]
    fn a_quoted_name_takes_names_only_and_says_whether_it_is_closed() {
        let c = ctx("select \"Or|\"");
        assert!(c.quoted && c.quote_closed);
        assert_eq!((c.wants.keywords, c.wants.functions), (Keywords::None, false), "no keyword or function is a quoted name");
        assert!(c.wants.columns && c.offers());
        let c = ctx("select \"Or|");
        assert!(c.quoted && !c.quote_closed);
        assert_eq!((c.wants.keywords, c.wants.functions), (Keywords::None, false));
        let c = ctx("select * from \"Or|\"");
        assert!(c.wants.tables && c.quote_closed);
        assert_eq!(c.wants.keywords, Keywords::None);
        let c = ctx("select Or|");
        assert!(!c.quoted && c.wants.functions && c.wants.keywords == Keywords::Any, "unquoted words are as before");
    }

    #[test]
    fn the_typed_part_of_a_quoted_name_is_measured_in_the_text_not_in_the_name() {
        // `"a""b"` is the name `a"b`: the caret after the `b` has typed `a"b`.
        let c = ctx("select \"a\"\"b|c\"");
        assert_eq!(c.word, "a\"b");
        assert_eq!(c.word_end, at(0, 14));
        assert_eq!(ctx("select \"a\"\"|b\"").word, "a\"");
        assert_eq!(ctx("select \"a\"\"\"\"b|\"").word, "a\"\"b");
        assert_eq!(ctx("select \"|a\"\"b\"").word, "");
    }

    #[test]
    fn right_after_a_tables_name_the_word_is_in_the_alias_position_and_elsewhere_it_is_not() {
        for sql in [
            "select * from film f|",
            "select * from film |",
            "select * from public.film f|",
            "select * from \"Order Items\" o|",
            "select * from film f, actor a|",
            "select * from film f join actor a|",
            "update film s|",
        ] {
            let c = ctx(sql);
            assert!(c.alias_position, "{sql}");
            assert_eq!(c.wants.keywords, Keywords::Only(AFTER_RELATION), "{sql}: the keyword key still lists what follows");
        }
        for sql in [
            "select * from film f wh|",
            "select * from film as f wh|",
            "select * from fi|",
            "select * from |",
            "select * from film f where x|",
            "select * from film f join |",
            "select f|",
            "select * from film f on f.a = b|",
        ] {
            assert!(!ctx(sql).alias_position, "{sql}");
        }
    }

    #[test]
    fn the_word_runs_on_to_the_right_of_the_caret_for_replacing() {
        let c = ctx("select * from fil|m_x");
        assert_eq!((c.word.as_str(), c.word_start, c.word_end), ("fil", at(0, 14), at(0, 20)));
        let c = ctx("select * from |film");
        assert_eq!((c.word.as_str(), c.word_start, c.word_end), ("", at(0, 14), at(0, 18)));
        let c = ctx("select * from |");
        assert_eq!((c.word_start, c.word_end), (at(0, 14), at(0, 14)));
    }

    #[test]
    fn the_caret_may_be_on_any_row_and_columns_count_characters() {
        let c = ctx("select 'é😀'\n  from film f\n where f.ti|");
        assert_eq!((c.word.as_str(), c.word_start), ("ti", at(2, 9)));
        assert_eq!(c.refs, [table_ref(None, "film", Some("f"))]);
        let c = ctx("select 'é😀', fi|");
        assert_eq!(c.word_start, at(0, 13));
    }

    #[test]
    fn only_the_rows_of_the_statement_around_the_caret_are_read() {
        let lines: Vec<String> = (0..10_000).map(|i| format!("select {i};")).collect();
        assert_eq!(window(&lines, 5_000), (5_000, 5_001), "a row with its own `;` is its own statement");
        let lines: Vec<String> = ["select 1;", "select f.", "  from film f", "  where true;", "select 3;"].map(String::from).to_vec();
        assert_eq!(window(&lines, 1), (0, 4), "from the row with the `;` before it to the row with the one after");
        // No `;` anywhere: a budget in rows, and in bytes.
        let lines: Vec<String> = (0..10_000).map(|_| "select 1".to_string()).collect();
        let (from, to) = window(&lines, 5_000);
        assert!(to - from <= 2 * WINDOW_ROWS + 1 && from < 5_000 && to > 5_000, "{from}..{to}");
        let wide: Vec<String> = (0..16).map(|_| "z".repeat(16 * 1024 - 1)).collect();
        let (from, to) = window(&wide, 8);
        assert!((to - from) * (16 * 1024) <= WINDOW_BYTES + 16 * 1024, "{from}..{to}");
        assert!(from <= 8 && 8 < to);
        // And the answer is the same as reading everything, shifted back.
        let mut lines: Vec<String> = (0..3_000).map(|i| format!("select {i};")).collect();
        lines.extend(["select f.".to_string(), "  from film f".to_string(), ";".to_string()]);
        let c = analyse(&lines, Cursor { row: 3_000, col: 9 });
        assert_eq!(c.refs, [TableRef { schema: None, name: "film".into(), alias: Some("f".into()) }]);
        assert_eq!(c.word_start, Cursor { row: 3_000, col: 9 });
    }

    #[test]
    fn an_empty_buffer_and_a_caret_past_its_end_are_read_without_trouble() {
        let c = analyse(&[], Cursor { row: 0, col: 0 });
        assert!(c.word.is_empty() && c.refs.is_empty());
        let lines = vec![String::new()];
        let c = analyse(&lines, Cursor { row: 5, col: 9 });
        assert!(c.word.is_empty());
        let c = ctx("|");
        assert_eq!((c.word.as_str(), c.word_start), ("", at(0, 0)));
        assert!(c.offers(), "an empty statement takes keywords");
    }

    #[test]
    fn a_dot_after_something_that_is_not_a_name_offers_nothing() {
        assert!(!ctx("select (1).|").offers());
        assert!(!ctx("select *.|").offers());
    }
}
