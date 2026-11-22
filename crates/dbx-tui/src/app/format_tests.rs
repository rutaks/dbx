//! Layer 2: the acceptance tests of slice 14 (reformat code). The formatter is
//! a pure function (`format::format_sql`); the key and the undo go through `App`.

use sqlparser::dialect::{Dialect as SqlDialect, GenericDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

use super::format::{format_sql, same_tokens, CommaPosition, KeywordCase, Options};
use super::run_tests::on_pagila;
use super::settings::Settings;
use crate::core::{Cursor, Dialect};
use crate::harness::Harness;

const AWKWARD: &str = include_str!("format_corpus.sql");

fn pagila_schema() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pagila/pagila-schema.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Pagila's schema is a fixture ({}): {e}", path.display()))
}

fn fmt(src: &str) -> String {
    fmt_with(src, &Options::default())
}

fn fmt_with(src: &str, opts: &Options) -> String {
    match format_sql(src, Some(Dialect::PostgreSql), opts, 0, 0, &[]) {
        Ok(f) => f.text,
        Err(why) => panic!("refused {src:?}: {why}"),
    }
}

fn dialect_for(kind: Option<Dialect>) -> Box<dyn SqlDialect> {
    match kind {
        Some(Dialect::MySql) => Box::new(MySqlDialect {}),
        Some(Dialect::PostgreSql) => Box::new(PostgreSqlDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}

/// The comparable form of a token stream, independent of the formatter: every
/// token but whitespace, comments trimmed, (when the case is not left) unquoted
/// words lower-cased, and a `\n` where a newline separated two string literals
/// (in Postgres that newline is what joins them).
fn stream_in(text: &str, fold_case: bool, kind: Option<Dialect>) -> Vec<String> {
    let dialect = dialect_for(kind);
    let tokens = Tokenizer::new(dialect.as_ref(), text).tokenize().unwrap_or_else(|e| panic!("does not tokenize: {e}"));
    let is_str = |t: &Token| matches!(t, Token::SingleQuotedString(_) | Token::EscapedStringLiteral(_) | Token::NationalStringLiteral(_) | Token::HexStringLiteral(_) | Token::UnicodeStringLiteral(_) | Token::SingleQuotedByteStringLiteral(_));
    let (mut out, mut last_str, mut nl) = (Vec::new(), false, false);
    for t in tokens {
        match t {
            Token::Whitespace(Whitespace::Newline) => nl = true,
            Token::Whitespace(Whitespace::SingleLineComment { comment, prefix }) => {
                nl = true;
                out.push(format!("{prefix}{}", comment.trim_end()));
            }
            Token::Whitespace(Whitespace::MultiLineComment(c)) => out.push(format!("/*{c}*/")),
            Token::Whitespace(_) => {}
            t => {
                if is_str(&t) && last_str && nl {
                    out.push("\n".to_string());
                }
                last_str = is_str(&t);
                nl = false;
                match t {
                    Token::Word(w) if w.quote_style.is_none() && fold_case => out.push(w.value.to_lowercase()),
                    t => out.push(t.to_string()),
                }
            }
        }
    }
    out
}

fn stream(text: &str, fold_case: bool) -> Vec<String> {
    stream_in(text, fold_case, Some(Dialect::PostgreSql))
}

fn comments(text: &str) -> Vec<String> {
    stream(text, false).into_iter().filter(|t| t.starts_with("--") || t.starts_with("/*")).collect()
}

fn opts(case: KeywordCase) -> Options {
    Options { case, ..Options::default() }
}

// ---- layout ------------------------------------------------------------------------

#[test]
fn a_one_line_select_becomes_the_documented_layout() {
    assert_eq!(
        fmt("select a,b from t where x=1 and y=2 order by a"),
        "select\n    a,\n    b\nfrom t\nwhere x = 1\n    and y = 2\norder by a"
    );
}

#[test]
fn a_subquery_and_a_cte_each_indent_one_level() {
    assert_eq!(
        fmt("select a from t where b in (select c from u where d=1)"),
        "select\n    a\nfrom t\nwhere b in (\n    select\n        c\n    from u\n    where d = 1\n)"
    );
    assert_eq!(
        fmt("with x as (select 1 as n), y as (select n from x) select * from y"),
        "with x as (\n    select\n        1 as n\n),\ny as (\n    select\n        n\n    from x\n)\nselect\n    *\nfrom y"
    );
}

#[test]
fn joins_get_their_own_lines_and_on_conditions_indent() {
    assert_eq!(
        fmt("select a from t left outer join u on t.id=u.id and u.k=1 join v using (id) where a>1"),
        "select\n    a\nfrom t\nleft outer join u on t.id = u.id\n    and u.k = 1\njoin v using (id)\nwhere a > 1"
    );
}

#[test]
fn statements_are_one_blank_line_apart_and_calls_casts_and_stars_keep_their_shape() {
    assert_eq!(
        fmt("select count(*),t.*,a::int,-1,x[1] from t;select 2;;"),
        "select\n    count(*),\n    t.*,\n    a::int,\n    -1,\n    x[1]\nfrom t;\n\nselect\n    2;;"
    );
}

#[test]
fn between_does_not_break_its_and_and_case_keeps_its_ands() {
    assert_eq!(
        fmt("select a from t where b between 1 and 2 and case when c and d then 1 end=1"),
        "select\n    a\nfrom t\nwhere b between 1 and 2\n    and case when c and d then 1 end = 1"
    );
}

#[test]
fn create_table_puts_each_column_on_its_own_line() {
    assert_eq!(
        fmt("create table x (id int primary key, name text default 'a,b', unique (name))"),
        "create table x (\n    id int primary key,\n    name text default 'a,b',\n    unique (name)\n)"
    );
}

#[test]
fn the_settings_change_the_indent_and_the_comma() {
    let o = Options { indent: 2, comma: CommaPosition::Leading, ..Options::default() };
    assert_eq!(fmt_with("select a,b from t where x=1 and y=2", &o), "select\n  a\n  , b\nfrom t\nwhere x = 1\n  and y = 2");
}

#[test]
fn comments_stay_where_they_were_relative_to_their_tokens() {
    assert_eq!(
        fmt("select a, -- first\n b /* mid */, c\n-- own line\nfrom t"),
        "select\n    a, -- first\n    b /* mid */,\n    c\n-- own line\nfrom t"
    );
    assert_eq!(fmt("-- head\nselect 1; -- tail\n/* next */ select 2"), "-- head\nselect\n    1; -- tail\n\n/* next */\nselect\n    2");
}

// ---- keyword case --------------------------------------------------------------------

#[test]
fn upper_case_changes_keywords_and_not_quoted_names_strings_or_comments() {
    let out = fmt_with("select \"select\", 'from', a.from -- where\nfrom t where left(a,1)=1", &opts(KeywordCase::Upper));
    assert_eq!(out, "SELECT\n    \"select\",\n    'from',\n    a.from -- where\nFROM t\nWHERE left(a, 1) = 1");
    let lower = fmt_with("SELECT A FROM T WHERE X=1", &opts(KeywordCase::Lower));
    assert_eq!(lower, "select\n    A\nfrom T\nwhere X = 1");
    assert_eq!(fmt_with("Select a From t", &opts(KeywordCase::Leave)), "Select\n    a\nFrom t");
}

// ---- safety -------------------------------------------------------------------------

const KINDS: [Option<Dialect>; 3] = [Some(Dialect::PostgreSql), Some(Dialect::MySql), None];

fn assert_safe(name: &str, src: &str) {
    assert_safe_in(name, src, &Some(Dialect::PostgreSql));
}

/// Formats `src` for `kind` under every case setting. The result must keep the
/// stream; a refusal is safe, and returned as `false`.
fn assert_safe_in(name: &str, src: &str, kind: &Option<Dialect>) -> bool {
    let kind = *kind;
    let mut all = true;
    for case in [KeywordCase::Leave, KeywordCase::Upper, KeywordCase::Lower] {
        let out = match format_sql(src, kind, &opts(case), 0, 0, &[]) {
            Ok(f) => f.text,
            Err(why) if kind == Some(Dialect::PostgreSql) => panic!("{name} {case:?}: refused: {why}"),
            Err(_) => {
                all = false;
                continue;
            }
        };
        let fold = case != KeywordCase::Leave;
        let (before, after) = (stream_in(src, fold, kind), stream_in(&out, fold, kind));
        if before != after {
            let at = before.iter().zip(&after).position(|(a, b)| a != b).unwrap_or(before.len().min(after.len()));
            panic!("{name} {kind:?} {case:?}: token streams differ at {at}: {:?} vs {:?}", before.get(at), after.get(at));
        }
        assert_eq!(comments(src), comments(&out), "{name} {case:?}: every comment kept, in order");
    }
    all
}

#[test]
fn the_token_stream_and_every_comment_survive_over_the_awkward_corpus() {
    assert_safe("awkward", AWKWARD);
    assert!(comments(AWKWARD).len() >= 12, "the corpus has many comments");
}

#[test]
fn the_token_stream_and_every_comment_survive_over_pagilas_schema() {
    let schema = pagila_schema();
    assert!(schema.lines().count() > 500, "the real schema");
    assert_safe("pagila", &schema);
}

#[test]
fn each_statement_of_the_corpus_alone_is_safe_too() {
    for (i, part) in AWKWARD.split(";\n").enumerate() {
        if part.trim().is_empty() || Tokenizer::new(&PostgreSqlDialect {}, part).tokenize().is_err() {
            continue;
        }
        assert_safe(&format!("part {i}"), part);
    }
}

#[test]
fn formatting_the_output_again_changes_nothing() {
    for (name, src) in [("awkward", AWKWARD.to_string()), ("pagila", pagila_schema())] {
        for case in [KeywordCase::Leave, KeywordCase::Upper] {
            let once = fmt_with(&src, &opts(case));
            let twice = fmt_with(&once, &opts(case));
            if once != twice {
                let at = once.lines().zip(twice.lines()).position(|(a, b)| a != b).unwrap_or(0);
                panic!("{name} {case:?}: not idempotent at line {at}:\n{:?}\n{:?}", once.lines().nth(at), twice.lines().nth(at));
            }
        }
    }
}

#[test]
fn text_that_does_not_tokenize_is_refused_with_the_reason() {
    for src in ["select 'abc", "select /* never closed", "select \"abc", "select $$ open"] {
        let err = format_sql(src, Some(Dialect::PostgreSql), &Options::default(), 0, 0, &[]).unwrap_err();
        assert!(!err.is_empty(), "{src}");
    }
}

fn refused_in(src: &str, kind: Option<Dialect>) -> String {
    match format_sql(src, kind, &Options::default(), 0, 0, &[]) {
        Ok(f) => panic!("{src:?} should be refused, got {:?}", f.text),
        Err(why) => why,
    }
}

#[test]
fn the_corpus_and_pagilas_schema_are_safe_under_every_dialect_too() {
    let schema = pagila_schema();
    for kind in &KINDS {
        let mut formatted = 0;
        for (name, src) in [("awkward", AWKWARD), ("pagila", schema.as_str())] {
            if assert_safe_in(name, src, kind) {
                formatted += 1;
            }
        }
        // Refusing is safe; refusing everything would be no test of the layout.
        assert!(formatted >= 1, "{kind:?}: something was formatted and checked");
    }
}

#[test]
fn a_newline_between_two_string_literals_is_kept_and_the_check_notices_one_dropped() {
    for src in [
        "select 'foo'\n'bar' as a",
        "select E'foo'\n'bar' as a",
        "select N'foo'\n'bar' as a",
        "select U&'foo'\n'bar' as a",
        "select 'foo' /* c */\n'bar' as a",
        "select 'foo' -- c\n'bar' as a",
        "select a,\n'foo'\n\n  'bar', 'x'\n'y' from t",
    ] {
        assert_safe(src, src);
        let out = fmt(src);
        assert!(stream(&out, false).contains(&"\n".to_string()), "{src:?} -> {out:?}: the newline stays");
        assert_eq!(fmt(&out), out, "{src:?}: stable");
    }
    // The check itself: a result that drops the newline is not the same tokens.
    let pg = Some(Dialect::PostgreSql);
    assert!(!same_tokens("select 'foo'\n'bar'", "select 'foo' 'bar'", pg, false));
    assert!(!same_tokens("select E'foo'\n'bar'", "select E'foo' 'bar'", pg, false));
    assert!(same_tokens("select 'foo'\n'bar'", "select   'foo'\n\n   'bar'", pg, false));
    assert!(same_tokens("select 'foo' , 'bar'", "select 'foo', 'bar'", pg, false));
}

#[test]
fn lines_that_are_client_commands_refuse_the_region_with_what_to_do() {
    let mysql = Some(Dialect::MySql);
    let cases: [(&str, Option<Dialect>); 7] = [
        ("DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END//\nDELIMITER ;", mysql),
        ("delimiter //\nselect 1//", None),
        ("select 1 from t\nGO\nselect 2 from u\nGO", None),
        ("select 1\n  go\n", Some(Dialect::PostgreSql)),
        ("\\x\n\\d t", Some(Dialect::PostgreSql)),
        ("select 1;\n\\d t", Some(Dialect::PostgreSql)),
        ("  \\timing on\nselect 1", Some(Dialect::PostgreSql)),
    ];
    for (src, kind) in cases {
        let why = refused_in(src, kind);
        assert!(why.contains("client command") && why.contains("select"), "{src:?}: {why}");
    }
    // A backslash, `go` or `delimiter` that is not a line of its own is SQL.
    assert!(format_sql("select 'a\n\\d t\ngo\ndelimiter' from t", Some(Dialect::PostgreSql), &Options::default(), 0, 0, &[]).is_ok());
    assert!(format_sql("select 1 -- a\n-- \\d t\n/* x\ngo\n*/ from t", Some(Dialect::PostgreSql), &Options::default(), 0, 0, &[]).is_ok());
    assert!(format_sql("select a as go from t", None, &Options::default(), 0, 0, &[]).is_ok());
}

#[test]
fn mysql_family_names_are_never_re_cased_only_reserved_words() {
    let upper = opts(KeywordCase::Upper);
    let run = |src: &str, kind: Option<Dialect>, o: &Options| format_sql(src, kind, o, 0, 0, &[]).unwrap().text;
    let my = Some(Dialect::MySql);
    assert_eq!(run("select * from language", my, &upper), "SELECT\n    *\nFROM language");
    assert_eq!(
        run("select a from film join language l on l.id=film.lid", my, &upper),
        "SELECT\n    a\nFROM film\nJOIN language l ON l.id = film.lid"
    );
    assert_eq!(run("insert into language values (1)", my, &upper), "INSERT INTO language\nVALUES\n    (1)");
    assert_eq!(run("update language set a=1", my, &upper), "UPDATE language\nSET\n    a = 1");
    assert_eq!(run("select language.id, db.language from language", my, &upper), "SELECT\n    language.id,\n    db.language\nFROM language");
    assert_eq!(run("SELECT * FROM Language", my, &opts(KeywordCase::Lower)), "select\n    *\nfrom Language");
    let maria = Some(Dialect::MySql);
    assert_eq!(run("select * from language", maria, &upper), "SELECT\n    *\nFROM language");
    assert_eq!(run("select * from language", None, &upper), "SELECT\n    *\nFROM language");
    // Postgres and SQLite stay as they were.
    for kind in [Some(Dialect::PostgreSql), Some(Dialect::Sqlite)] {
        assert_eq!(run("select * from language", kind, &upper), "SELECT\n    *\nFROM LANGUAGE");
    }
}

#[test]
fn a_comment_after_a_semicolon_before_the_next_semicolon_is_not_a_refusal() {
    let out = fmt("select 1; -- c\n;");
    assert_eq!(stream(&out, false), stream("select 1; -- c\n;", false));
    assert_safe("semi", "select 1; -- c\n;");
    assert_safe("semi", "select 1; /* c */ ;\n-- d\n; select 2");
    assert_eq!(fmt("select 1;;"), "select\n    1;;");
}

#[test]
fn two_operators_in_a_row_keep_the_space_the_source_had() {
    for src in ["select - -1", "select 1 - -1", "select a - - b", "select 1 + +1", "select 1 - +1", "select a * - b", "select - 1, -2, 1 - -x"] {
        assert_safe(src, src);
    }
    assert_eq!(fmt("select 1 - -1"), "select\n    1 - -1");
    assert_eq!(fmt("select - -1"), "select\n    - -1");
    assert_eq!(fmt("select a - - b"), "select\n    a - -b");
}

// ---- the caret --------------------------------------------------------------------------

#[test]
fn the_caret_stays_on_the_same_token_and_offset_inside_it() {
    let src = "select a,b from tab where x=1";
    let at = src.find("tab").unwrap() + 2;
    let f = format_sql(src, Some(Dialect::PostgreSql), &Options::default(), 0, 0, &[at, 0, src.len()]).unwrap();
    let chars: Vec<char> = f.text.chars().collect();
    assert_eq!(chars[f.carets[0] - 2..f.carets[0] + 1].iter().collect::<String>(), "tab");
    assert_eq!(f.carets[1], 0);
    assert_eq!(f.carets[2], f.text.chars().count());
}

// ---- performance --------------------------------------------------------------------------

#[test]
fn a_2000_line_script_formats_in_one_pass_over_its_tokens() {
    let line = "select a,b from t where x=1 and y=2 order by a;";
    let src = vec![line; 2000].join("\n");
    let start = std::time::Instant::now();
    let f = format_sql(&src, Some(Dialect::PostgreSql), &Options::default(), 0, 0, &[]).unwrap();
    let took = start.elapsed();
    // Work done: every one of the script's tokens went through, once.
    assert_eq!(f.tokens, 2000 * stream(line, false).len());
    assert_eq!(f.text.matches("order by a;").count(), 2000);
    // The slice asks for 200 ms; a debug build is several times slower than
    // the release one, so the budget here is ten times that.
    assert!(took < std::time::Duration::from_millis(2000), "took {took:?}");
}

// ---- in the editor ---------------------------------------------------------------------------

async fn editing() -> Harness {
    on_pagila().await
}

fn set(h: &mut Harness, text: &str, anchor: Option<Cursor>, caret: Cursor) {
    h.app.ui.editor.set_text(text);
    match anchor {
        Some(a) => h.app.ui.editor.select_range(a, caret),
        None => h.app.ui.editor.move_to(caret),
    }
}

fn c(row: usize, col: usize) -> Cursor {
    Cursor { row, col }
}

fn notices(h: &Harness) -> Option<String> {
    h.app.ui.notice.as_ref().map(|n| n.text.clone())
}

#[tokio::test]
async fn every_reformat_key_formats_the_statement_under_the_caret_and_one_undo_restores_the_text() {
    for keys in ["alt+cmd+l", "ctrl+alt+l", "ctrl+g ="] {
        let mut h = editing().await;
        let before = "select 1;\nselect a,b from t where x=1\n  and y=2;\nselect 3";
        set(&mut h, before, None, c(1, 8));
        h.press(keys);
        assert_eq!(
            h.app.ui.editor.text(),
            "select 1;\nselect\n    a,\n    b\nfrom t\nwhere x = 1\n    and y = 2;\nselect 3",
            "{keys}"
        );
        assert_eq!(h.app.ui.editor.cursor(), c(2, 5), "{keys}: the caret stays right after the `a` it was after");
        h.press("ctrl+z");
        assert_eq!(h.app.ui.editor.text(), before, "{keys}: one undo restores the text exactly");
        h.press("ctrl+shift+z");
        assert!(h.app.ui.editor.text().contains("from t\nwhere"), "{keys}: and redo formats again");
    }
}

#[tokio::test]
async fn the_second_action_formats_the_whole_editor() {
    let mut h = editing().await;
    let before = "select a from t;select b from u\n";
    set(&mut h, before, None, c(0, 3));
    h.press("ctrl+g +");
    assert_eq!(h.app.ui.editor.text(), "select\n    a\nfrom t;\n\nselect\n    b\nfrom u\n");
    h.press("ctrl+z");
    assert_eq!(h.app.ui.editor.text(), before);
}

#[tokio::test]
async fn with_a_selection_only_the_selection_changes_and_stays_selected() {
    let mut h = editing().await;
    let before = "select 1 from t;\n  select a,b from u where x=1;\nselect 3 from v";
    set(&mut h, before, Some(c(1, 2)), c(1, 30));
    h.press("cmd+alt+l");
    assert_eq!(
        h.app.ui.editor.text(),
        "select 1 from t;\n  select\n      a,\n      b\n  from u\n  where x = 1;\nselect 3 from v"
    );
    let (a, b) = h.app.ui.editor.selection().expect("still selected");
    assert_eq!((a, b), (c(1, 2), c(5, 14)));
    h.press("ctrl+z");
    assert_eq!(h.app.ui.editor.text(), before);
}

#[tokio::test]
async fn keyword_case_is_a_setting_and_a_quoted_column_named_select_does_not_change() {
    let mut h = editing().await;
    h.app.model.settings = Settings { format_keyword_case: KeywordCase::Upper, ..Settings::default() };
    set(&mut h, "select \"select\" from t", None, c(0, 0));
    h.press("ctrl+g =");
    assert_eq!(h.app.ui.editor.text(), "SELECT\n    \"select\"\nFROM t");
}

#[tokio::test]
async fn an_unterminated_string_leaves_the_buffer_untouched_and_raises_one_notice() {
    let mut h = editing().await;
    let before = "select a,b from t where x='oops";
    set(&mut h, before, None, c(0, 4));
    let rev = h.app.ui.editor.revision();
    h.press("ctrl+alt+l");
    assert_eq!(h.app.ui.editor.text(), before);
    assert_eq!(h.app.ui.editor.revision(), rev, "not even a no-op edit");
    let notice = notices(&h).expect("a notice");
    assert!(notice.contains("Not reformatted") && notice.to_lowercase().contains("unterminated"), "{notice}");
    assert!(!h.app.ui.editor.undo(), "and no undo step was made");
}

#[tokio::test]
async fn text_already_formatted_is_left_alone_with_no_undo_step() {
    let mut h = editing().await;
    let before = "select\n    a\nfrom t";
    set(&mut h, before, None, c(1, 5));
    h.press("ctrl+g =");
    assert_eq!(h.app.ui.editor.text(), before);
    assert_eq!(h.app.ui.editor.cursor(), c(1, 5));
    assert!(!h.app.ui.editor.undo());
}

#[tokio::test]
async fn the_prefix_keys_are_in_the_mode_bar_and_the_help() {
    let mut h = editing().await;
    h.press("ctrl+g");
    let bar = super::testing::frame_rows(&mut h.app, 400, 30).pop().unwrap();
    assert!(bar.contains("= reformat") && bar.contains("+ reformat all"), "{bar}");
    let keys = h.app.ui.keys.all_keys(super::keys::Action::Reformat);
    assert_eq!(keys, ["alt+cmd+l", "ctrl+alt+l", "ctrl+g ="]);
}

#[tokio::test]
async fn a_selection_inside_a_string_comment_quoted_name_or_dollar_body_is_refused_with_a_notice() {
    let cases = [
        ("select 'select a,b from t where x=1' from u", "select a,b from t where x=1"),
        ("select 1 -- select a,b from t where x=1\nfrom u", "select a,b from t where x=1"),
        ("select \"select a,b from t where x=1\" from u", "select a,b from t where x=1"),
        ("select $$select a,b from t where x=1$$ from u", "select a,b from t where x=1"),
        ("select /* select a,b from t where x=1 */ 1", "select a,b from t where x=1"),
    ];
    for (text, picked) in cases {
        let mut h = editing().await;
        set(&mut h, text, None, c(0, 0));
        let flat = text.replace('\n', " ");
        let start = flat.find(picked).unwrap();
        let rev = h.app.ui.editor.revision();
        set(&mut h, text, Some(c(0, start)), c(0, start + picked.chars().count()));
        h.press("ctrl+alt+l");
        assert_eq!(h.app.ui.editor.text(), text, "{text}");
        assert_eq!(h.app.ui.editor.revision(), rev + 1, "{text}: only the set_text above");
        assert!(!h.app.ui.editor.undo(), "{text}: no undo step");
        let notice = notices(&h).unwrap_or_else(|| panic!("{text}: a notice"));
        assert!(notice.contains("Not reformatted") && notice.contains("select"), "{text}: {notice}");
    }
    // Both ends outside every string and comment: fine, even around one.
    let mut h = editing().await;
    set(&mut h, "select 'a,b' as s,c from t", Some(c(0, 0)), c(0, 26));
    h.press("ctrl+alt+l");
    assert_eq!(h.app.ui.editor.text(), "select\n    'a,b' as s,\n    c\nfrom t");
    // A selection ending in the middle of a word is not on a token boundary.
    let mut h = editing().await;
    set(&mut h, "select abc from t", Some(c(0, 0)), c(0, 9));
    h.press("ctrl+alt+l");
    assert_eq!(h.app.ui.editor.text(), "select abc from t");
    assert!(notices(&h).is_some());
}
