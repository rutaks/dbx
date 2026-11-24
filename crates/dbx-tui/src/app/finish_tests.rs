//! Layer 2: complete current statement.

use super::complete_scope_tests::put;
use super::complete_tests::*;
use super::finish::complete_statement;
use crate::core::Cursor;

fn rows(sql: &str) -> Vec<String> {
    sql.split('\n').map(String::from).collect()
}

fn finished(sql: &str) -> Option<String> {
    let lines = rows(sql);
    let end = Cursor { row: lines.len() - 1, col: lines.last().map_or(0, |l| l.chars().count()) };
    complete_statement(&lines, end, Some(crate::core::Dialect::PostgreSql)).ok().flatten().map(|e| {
        let mut out = lines.clone();
        let line: Vec<char> = out[e.at.row].chars().collect();
        out[e.at.row] = line[..e.at.col].iter().collect::<String>() + &e.text + &line[e.at.col..].iter().collect::<String>();
        out.join("\n")
    })
}

#[test]
fn it_closes_the_open_brackets_and_adds_the_semicolon() {
    assert_eq!(finished("select count(*").as_deref(), Some("select count(*);"));
    assert_eq!(finished("select f(g(1), (2").as_deref(), Some("select f(g(1), (2));"));
    assert_eq!(finished("select 1").as_deref(), Some("select 1;"));
    assert_eq!(finished("select (1;").as_deref(), Some("select (1);"));
}

#[test]
fn it_does_nothing_when_there_is_nothing_to_add() {
    assert_eq!(finished("select 1;"), None);
    assert_eq!(finished("select (1);"), None);
    assert_eq!(finished(""), None);
    assert_eq!(finished("   \n"), None);
    assert_eq!(finished("select 1) ;"), None, "an extra closing bracket is not ours to remove");
}

#[test]
fn brackets_in_strings_comments_quoted_names_and_dollar_bodies_do_not_count() {
    assert_eq!(finished("select '(' as p").as_deref(), Some("select '(' as p;"));
    assert_eq!(finished("select \"a(\" from t").as_deref(), Some("select \"a(\" from t;"));
    assert_eq!(finished("select 1 -- (\n").as_deref(), Some("select 1; -- (\n"), "the semicolon goes before the comment");
    assert_eq!(finished("select /* ( */ 1").as_deref(), Some("select /* ( */ 1;"));
    assert_eq!(finished("do $$ begin perform f( ; end $$").as_deref(), Some("do $$ begin perform f( ; end $$;"));
    assert_eq!(finished("select ')' , (1").as_deref(), Some("select ')' , (1);"));
    assert_eq!(finished("select 'it''s ('").as_deref(), Some("select 'it''s (';"));
}

#[test]
fn it_works_on_the_statement_under_the_caret() {
    let lines = rows("select (1;\nselect f(2\n;select 3");
    let edit = complete_statement(&lines, Cursor { row: 1, col: 3 }, None).unwrap().expect("the second statement");
    assert_eq!((edit.at, edit.text.as_str()), (Cursor { row: 1, col: 10 }, ")"), "its semicolon is on the next row: only the bracket is missing");
    let edit = complete_statement(&lines, Cursor { row: 0, col: 3 }, None).unwrap().expect("the first");
    assert_eq!((edit.at, edit.text.as_str()), (Cursor { row: 0, col: 9 }, ")"));
}

#[tokio::test]
async fn the_keys_complete_the_statement_in_one_undo_step() {
    for keys in ["cmd+shift+enter", "ctrl+shift+enter", "ctrl+g ;"] {
        let mut h = editing(pagila()).await;
        put(&mut h, "select count(*|");
        h.press(keys);
        assert_eq!(text(&h), "select count(*);", "{keys}");
        assert_eq!(h.app.ui.tab.editor.cursor(), Cursor { row: 0, col: 16 }, "{keys}");
        h.press("ctrl+z");
        assert_eq!(text(&h), "select count(*", "{keys}: one undo");
    }
}

#[tokio::test]
async fn with_nothing_to_add_the_key_changes_nothing_and_leaves_no_undo_step() {
    let mut h = editing(pagila()).await;
    put(&mut h, "select 1;|");
    let revision = h.app.ui.tab.editor.revision();
    h.press("cmd+shift+enter");
    assert_eq!(text(&h), "select 1;");
    assert_eq!(h.app.ui.tab.editor.revision(), revision);
}

#[tokio::test]
async fn the_prefix_hint_names_the_key_from_the_live_bindings() {
    let mut h = editing(pagila()).await;
    h.press("ctrl+g");
    let bar = super::run_tests::mode_bar(&mut h);
    assert!(bar.contains("; finish"), "{bar}");
    h.press("esc");
    h.app.ui.keys.rebind(super::keys::Action::FinishStatement, "s");
    h.press("ctrl+g");
    assert!(super::run_tests::mode_bar(&mut h).contains("s finish"));
}

#[tokio::test]
async fn escape_strings_and_nested_comments_are_read_as_the_highlighter_reads_them() {
    for (sql, want) in [
        ("select E'it\\'s (' as p|", "select E'it\\'s (' as p;"),
        ("select /* /* */ ( */ 1|", "select /* /* */ ( */ 1;"),
        ("select count(E'a\\'b'|", "select count(E'a\\'b');"),
    ] {
        let mut h = editing(pagila()).await;
        put(&mut h, sql);
        h.press("cmd+shift+enter");
        assert_eq!(text(&h), want, "{sql}");
    }
}

#[tokio::test]
async fn a_statement_with_an_unterminated_string_comment_or_dollar_body_is_left_alone_with_a_notice() {
    for (sql, what) in [("select 'abc|", "string"), ("select 1 /* later|", "comment"), ("do $$ begin perform 1|", "dollar"), ("select (1, 'x|", "string")] {
        let mut h = editing(pagila()).await;
        put(&mut h, sql);
        let revision = h.app.ui.tab.editor.revision();
        h.press("cmd+shift+enter");
        assert_eq!(text(&h), sql.replace('|', ""), "{sql}");
        assert_eq!(h.app.ui.tab.editor.revision(), revision, "{sql}");
        let said = h.app.ui.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
        assert!(said.contains("not finished") && said.contains(what), "{sql}: {said:?}");
    }
    // A statement before the broken one is still finished.
    let mut h = editing(pagila()).await;
    put(&mut h, "select 1|;\nselect 'abc");
    h.press("cmd+shift+enter");
    assert_eq!(text(&h), "select 1;\nselect 'abc", "already complete: nothing to add, and no notice for it");
}
