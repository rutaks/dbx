//! How a MySQL server may read a text (slice 23b): the confirmation gate's
//! model of the server's lexer, for each SQL mode and server version that
//! changes what is a comment, a quote or a statement. No dbx types: the gate in
//! `dbx.rs` hands what this reads to dbx's classifier and compares it with
//! dbx's own reading.
//!
//! What the server's lexer does (MySQL 8.4, `sql_lex.cc`):
//!
//! - `#` runs to the end of the line. So does `--` followed by ASCII
//!   whitespace or an ASCII control character (or the end of the text); `--`
//!   followed by anything else, a non-breaking space included, is two minus
//!   signs.
//! - `/* ... */` ends at the first `*/`: comments do not nest. `/*+ ... */` is
//!   an optimizer hint, a comment as far as statements go.
//! - `/*! ... */` is SQL. With a version, five digits (or six, when a sixth
//!   follows), `/*!80013 ... */` is SQL on a server of that version or later
//!   and a comment on an older one, in which one nested `/* */` is allowed.
//!   Fewer than five digits are no version: they are SQL. MariaDB also runs
//!   `/*M! ... */`; MySQL reads it as a comment.
//! - `'...'` is a string, and so is `"..."` unless the SQL mode has
//!   `ANSI_QUOTES`, which makes `"..."` a name. In a string a backslash escapes
//!   the next character unless the mode has `NO_BACKSLASH_ESCAPES`; a doubled
//!   quote is one quote. A name (backticks, or `"` under `ANSI_QUOTES`) has no
//!   escapes, only the doubled quote.
//!
//! Each `Mode` gives one `Reading` of the text: as written with the comments
//! gone (`plain`), the same with every quoted token rewritten so that any
//! lexer reads it alike (`neutral`), and what lies outside quotes and
//! comments (`skeleton`), by which two readings are compared.
//!
//! The text is read as UTF-8 (utf8mb4, dbx's `SET NAMES` at connect). Under a
//! client charset whose two-byte characters may end in `\` or `` ` `` it is
//! not the server's reading for text that is not ASCII; `dbx.rs`
//! (`mysql_unread`) does not lean on it there.

/// One way the server may read a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Mode {
    /// A backslash escapes in a string (no `NO_BACKSLASH_ESCAPES`).
    pub backslash_escapes: bool,
    /// `"` quotes a name (`ANSI_QUOTES`).
    pub ansi_quotes: bool,
    /// The server's version as `/*!80413 */` writes it: a comment with a
    /// version up to this is SQL, above it a comment.
    pub version: u32,
    /// The server is MariaDB, which runs `/*M! */` too.
    pub mariadb: bool,
}

#[cfg(test)]
impl Mode {
    /// The server's default mode, a server new enough for any version, MySQL.
    pub const DEFAULT: Mode = Mode { backslash_escapes: true, ansi_quotes: false, version: u32::MAX, mariadb: false };
}

/// More distinct versions than this in one text (a dump has a handful) and
/// only "every version comment is SQL" and "none is" are read, and the text is
/// taken as ambiguous.
const MAX_VERSIONS: usize = 6;

/// The modes worth reading `sql` in: both settings of each quote mode, and
/// for the version comments the text has, a server older than all of them
/// and one just new enough for each; MariaDB too when it has `/*M!`. The
/// first is `Mode::DEFAULT`'s quoting on a server new enough for all. The
/// flag says the versions were too many to read each (`MAX_VERSIONS`).
pub(super) fn modes(sql: &str) -> (Vec<Mode>, bool) {
    let c: Vec<char> = sql.chars().collect();
    let mut versions = std::collections::BTreeSet::new();
    let mut mariadb = false;
    for i in 0..c.len() {
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            if let Some(m) = marker(&c, i) {
                versions.extend(m.version);
                mariadb |= m.mariadb;
            }
        }
    }
    let capped = versions.len() > MAX_VERSIONS;
    let mut servers: Vec<u32> = if capped { vec![0] } else { std::iter::once(0).chain(versions).collect() };
    // Newest first, so the first mode is the one new enough for everything.
    servers.reverse();
    if servers[0] != u32::MAX {
        servers.insert(0, u32::MAX);
    }
    let kinds: &[bool] = if mariadb { &[false, true] } else { &[false] };
    let mut out = Vec::new();
    for (backslash_escapes, ansi_quotes) in [(true, false), (false, false), (true, true), (false, true)] {
        for &version in &servers {
            for &mariadb in kinds {
                out.push(Mode { backslash_escapes, ansi_quotes, version, mariadb });
            }
        }
    }
    (out, capped)
}

/// A text as one `Mode` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reading {
    /// Comments gone (each one space, so what was either side is not joined),
    /// SQL in version comments kept, quotes as written but for a `"` name
    /// under `ANSI_QUOTES`, written in backticks.
    pub plain: String,
    /// `plain` with every string written `'...'` and every name `` `...` ``,
    /// their contents without quotes, backslashes, `#`, `;` or anything that
    /// could open a comment, and a `--` that is no comment written `- -`: a
    /// text every lexer, dbx's included, reads as this mode does.
    pub neutral: String,
    /// What lies outside quotes and comments, each quoted token one space,
    /// runs of whitespace one space.
    pub skeleton: String,
    /// A quote (a string or a name) opens and never closes: to the server, a
    /// syntax error where the quote opens.
    pub unterminated: bool,
    /// A character that is not ASCII outside every string literal: in a name,
    /// quoted or not, or in the SQL itself.
    pub bare_non_ascii: bool,
}

impl Reading {
    /// The statements: `neutral` split at its `;` (none is in a quote there).
    pub fn statements(&self) -> Vec<String> {
        self.neutral.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
    }
}

/// Runs of whitespace as one space, none at either end.
pub(super) fn collapsed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Default)]
struct Out {
    plain: String,
    neutral: String,
    skeleton: String,
    unterminated: bool,
    bare_non_ascii: bool,
}

impl Out {
    /// SQL, the same in every form.
    fn code(&mut self, s: &str) {
        self.plain.push_str(s);
        self.neutral.push_str(s);
        self.skeleton.push_str(s);
    }
    /// A comment, or the marks of a version comment.
    fn gap(&mut self) {
        self.code(" ");
    }
}

/// What opens at `c[i]` (a `/*`) when it is an executable comment: where its
/// SQL starts, its version and whether it is MariaDB's `/*M!`.
struct Marker {
    code_from: usize,
    version: Option<u32>,
    mariadb: bool,
}

fn marker(c: &[char], i: usize) -> Option<Marker> {
    let (at, mariadb) = match (c.get(i + 2), c.get(i + 3)) {
        (Some('!'), _) => (i + 3, false),
        (Some('M'), Some('!')) => (i + 4, true),
        _ => return None,
    };
    let digits = c[at..].iter().take_while(|d| d.is_ascii_digit()).count();
    if digits < 5 {
        return Some(Marker { code_from: at, version: None, mariadb });
    }
    let n = digits.min(6);
    let version = c[at..at + n].iter().collect::<String>().parse().ok();
    Some(Marker { code_from: at + n, version, mariadb })
}

/// Where a comment whose text starts at `c[i]` ends (after its `*/`, or the
/// end of the text), allowing `nest` levels of `/* */` inside it.
fn comment_end(c: &[char], mut i: usize, nest: u8) -> usize {
    while i < c.len() {
        if nest > 0 && c[i] == '/' && c.get(i + 1) == Some(&'*') {
            i = comment_end(c, i + 2, nest - 1);
            continue;
        }
        if c[i] == '*' && c.get(i + 1) == Some(&'/') {
            return i + 2;
        }
        i += 1;
    }
    c.len()
}

/// Where a line comment starting at `c[i]` ends: at the line break (which is
/// not part of it), or a NUL, or the end. (Ending it at a NUL can only show
/// more as SQL.)
fn line_end(c: &[char], mut i: usize) -> usize {
    while i < c.len() && c[i] != '\n' && c[i] != '\0' {
        i += 1;
    }
    i
}

/// A string's content as no lexer can misread: quotes, backslashes, `#`, `;`,
/// the characters of a comment's marks and control characters are spaces.
fn neutral_string(content: &str) -> String {
    content.chars().map(|ch| if matches!(ch, '\'' | '"' | '`' | '\\' | '#' | ';' | '-' | '/' | '*') || ch.is_control() { ' ' } else { ch }).collect()
}

/// A name's content likewise, keeping single `-`, `/` and `*` (a database
/// may be called `my-db`): only the second character of `--`, `/*` and `*/`
/// is a space.
fn neutral_name(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut previous = ' ';
    for ch in content.chars() {
        let pair = matches!((previous, ch), ('-', '-') | ('/', '*') | ('*', '/'));
        let ch = if pair || matches!(ch, '\'' | '"' | '`' | '\\' | '#' | ';') || ch.is_control() { ' ' } else { ch };
        out.push(ch);
        previous = ch;
    }
    out
}

/// The quoted token that opens at `c[at]` with the quote `q`: where it ends
/// (after its closing quote, or the end of the text), and its forms.
fn quoted(c: &[char], at: usize, q: char, escapes: bool, name: bool, out: &mut Out) -> usize {
    let mut i = at + 1;
    let mut content = String::new();
    let mut closed = false;
    while i < c.len() {
        let d = c[i];
        i += 1;
        if escapes && d == '\\' {
            if let Some(&e) = c.get(i) {
                content.push(e);
                i += 1;
            }
        } else if d == q {
            if c.get(i) == Some(&q) {
                content.push(q);
                i += 1;
            } else {
                closed = true;
                break;
            }
        } else {
            content.push(d);
        }
    }
    out.unterminated |= !closed;
    out.bare_non_ascii |= name && !content.is_ascii();
    if name && q == '"' {
        out.plain.push('`');
        out.plain.push_str(&content.replace('`', "``"));
        out.plain.push('`');
    } else {
        out.plain.extend(&c[at..i]);
    }
    if name {
        out.neutral.push('`');
        out.neutral.push_str(&neutral_name(&content));
        out.neutral.push('`');
    } else {
        out.neutral.push('\'');
        out.neutral.push_str(&neutral_string(&content));
        out.neutral.push('\'');
    }
    out.skeleton.push(' ');
    i
}

/// `sql` as the server reads it in `mode`.
pub(super) fn read(sql: &str, mode: Mode) -> Reading {
    let c: Vec<char> = sql.chars().collect();
    let mut out = Out::default();
    let (mut i, mut open) = (0, 0_usize);
    while i < c.len() {
        let next = c.get(i + 1).copied();
        match c[i] {
            '\'' => i = quoted(&c, i, '\'', mode.backslash_escapes, false, &mut out),
            '"' => i = quoted(&c, i, '"', mode.backslash_escapes && !mode.ansi_quotes, mode.ansi_quotes, &mut out),
            '`' => i = quoted(&c, i, '`', false, true, &mut out),
            '#' => {
                i = line_end(&c, i);
                out.gap();
            }
            '-' if next == Some('-') && c.get(i + 2).is_none_or(|x| x.is_ascii_whitespace() || x.is_ascii_control()) => {
                i = line_end(&c, i);
                out.gap();
            }
            // Two minus signs, which a lexer that takes `--` for a comment must
            // not be given as one: a space between them says so.
            '-' if next == Some('-') => {
                out.code("- ");
                i += 1;
            }
            '/' if next == Some('*') => {
                match marker(&c, i) {
                    Some(m) if (!m.mariadb || mode.mariadb) && m.version.is_none_or(|v| v <= mode.version) => {
                        i = m.code_from;
                        open += 1;
                    }
                    // A version this server is too old for (MySQL's or
                    // MariaDB's): a comment, with one nested comment allowed.
                    Some(m) if m.version.is_some() && (!m.mariadb || mode.mariadb) => i = comment_end(&c, i + 2, 1),
                    _ => i = comment_end(&c, i + 2, 0),
                }
                out.gap();
            }
            '*' if open > 0 && next == Some('/') => {
                open -= 1;
                i += 2;
                out.gap();
            }
            ch => {
                let mut b = [0; 4];
                out.code(ch.encode_utf8(&mut b));
                out.bare_non_ascii |= !ch.is_ascii();
                i += 1;
            }
        }
    }
    Reading { plain: out.plain, neutral: out.neutral, skeleton: collapsed(&out.skeleton), unterminated: out.unterminated, bare_non_ascii: out.bare_non_ascii }
}
