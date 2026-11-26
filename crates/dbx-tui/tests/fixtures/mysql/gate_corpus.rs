// The texts the MySQL confirmation gate is tested on (slice 23b): places where
// a reading of a statement can differ from the server's, by SQL mode, server
// version or lexer rule. Shared, through `include!`, by the unit tests
// (`src/core/dbx/mysql_gate_tests.rs`) and the live differential test
// (`tests/mysql.rs`).
//
// `{S}` is where a second statement goes (a `select 'MARK'`, a write to a
// scratch table, a delete in a production database). The flag says the second
// statement is no statement of its own however the server reads the text
// (every SQL mode, every version: it is in a comment or a quote, or there is
// no `;` before it): the server never runs it, and the gate may let it
// through. The live test checks the claim. `false` claims nothing.

pub const GATE_CORPUS: &[(&str, bool)] = &[
    // `--` and then whitespace that is not ASCII: two minus signs to MySQL.
    ("select 1 --\u{a0} from (select 2 as \u{a0}) x; {S}", false),
    ("select 1 --\u{85} from (select 2 as \u{85}) x; {S}", false),
    ("select 1 --\u{3000} from (select 2 as \u{3000}) x; {S}", false),
    ("select 1 --\u{a0}; {S}", false),
    ("select '\u{a0}--'; {S}", false),
    // A full-width semicolon is no semicolon.
    ("select 1\u{ff1b} {S}", true),
    // Backslashes and quotes, which the SQL mode reads differently.
    ("select '\\'' as \"\\\"; {S} #\"", false),
    ("select 'a\\'; {S}; -- '", false),
    ("select \"a\\\"; {S}; -- \"", false),
    ("select '\\''; {S}; -- '", false),
    ("select 'a\\\\'; {S}", false),
    ("select 'a\\\\\\'; {S}; -- '", false),
    ("select 'it\\'s'; {S}", false),
    ("select \"a\"\"b\"; {S}", false),
    ("select '''; {S}; -- '", true),
    ("select '\\0'; {S}", false),
    ("select \"\\'\"; {S}; -- \"", false),
    ("select '; {S}; -- '", true),
    ("select \"; {S}; -- \"", true),
    // Literals with a prefix.
    ("select _binary'a\\'; {S}; -- '", false),
    ("select X'4142'; {S}", false),
    ("select x'41\\'; {S}; -- '", false),
    ("select N'a\\'; {S}; -- '", false),
    // Backticks.
    ("select `a;b` from (select 1 as `a;b`) x; {S}", false),
    ("select ```x``; {S} #```", true),
    // `--` without a space, and comments that do not nest.
    ("select 5 --1; {S}", false),
    ("select 1 /* /* */; {S}; /* */", false),
    ("select 1 /* ; {S} */", true),
    ("select 1 /* \u{a0} */; {S}", false),
    // Comments that run to the end of the line.
    ("select 1 # x\n; {S}", false),
    ("select 1 # x; {S}", true),
    ("select 1 #\u{a0}; {S}", true),
    ("select 1 -- x; {S}", true),
    ("select 1 --\t; {S}", true),
    ("select 1 -- x\n; {S}", false),
    ("select 1 -- x\r; {S}", true),
    ("select 1 -- \\\n; {S}", false),
    ("select 1 --\u{0}; {S}", false),
    // Executable comments, by version (this server is 8.4.13) and MariaDB's.
    ("select 1 /*!50000 ; {S} */", false),
    ("select 1 /*!40101 , 2 */; {S}", false),
    ("select 1 /*!99999 -- */ --1; {S}", false),
    ("select 1 /*!99999 -- */; {S}", false),
    ("select 1 /*M!100000 -- */; {S}", false),
    ("select 1 /*!99999 ' */ ; {S}; select '*/'", false),
    ("select 1 /*!50000 /* */ ; {S} */", false),
    ("select 1 /*!123456 '*/; {S}; select '*/'", false),
    ("select 1 /*!1234 */; {S}", false),
    ("select 1 /*!*/; {S}", false),
    ("select 1 /*! /* */ */; {S}", false),
    ("select 1 /*+ BKA(t) */; {S}", false),
    // What other databases quote: nothing to MySQL.
    ("select 1 as $$; {S}; select 1 as $$", false),
    ("select q'[x]'; {S}", false),
    // A quote the SQL mode ends early with no `;` outside quotes in any
    // reading: one statement however the server reads it, a syntax error in
    // the reading whose quote never closes. (`false`: the gate asks about the
    // readings dbx cannot parse; the live test checks nothing runs past it.)
    ("select 'O\\'Brien' {S} '", false),
    ("select 'a\\' union {S} '", true),
    ("select \"a\\\"b\" {S} \"", false),
    ("select 'O\\'Brien', \"a\\\"b\" {S}", true),
    ("select 'C:\\temp\\file', 'it\\'s', 'a\\\\' as x {S}", false),
];

/// Texts that hide their second statement in a quote when the server reads
/// them as UTF-8 (`utf8mb4`), and not when the client charset is one whose
/// two-byte characters may end in a `\` or a `` ` `` (big5, cp932, gb18030,
/// gbk, sjis): the bytes of a UTF-8 character there join the next byte into
/// one character, which swallows the backslash or the backtick. U+4E01 is
/// E4 B8 81: gbk reads E4 B8, then 81 and the next byte (`81 5C`, `81 60`),
/// as two characters; U+4E21 (E4 B8 A1) does the same in big5 and gbk.
pub const CHARSET_CORPUS: &[&str] = &[
    "select 1 as `\u{4e01}`, '`; {S}; -- '",
    "select '\u{4e01}\\\\', '; {S}; -- '",
    "select 1 as `\u{4e21}`, '`; {S}; -- '",
    "select '\u{4e21}\\\\', '; {S}; -- '",
];

/// `text` with its `{S}` replaced by `statement`.
#[allow(dead_code)]
pub fn with_statement(text: &str, statement: &str) -> String {
    text.replace("{S}", statement)
}
