//! Layer 2: semantic colouring and `F2`, with dbx's analysis scripted on the
//! fake.

use super::complete_tests::*;
use super::run_tests::mode_bar;
use super::testing::{find_text, frame_rows, role_at};
use super::theme::Role;
use crate::core::{Call, ColumnReference, Cursor, References, Span, TableReference};
use crate::harness::Harness;

fn span(row: usize, from: usize, to: usize) -> Span {
    Span { start: Cursor { row, col: from }, end: Cursor { row, col: to } }
}

fn table(name: &str, alias: Option<&str>, at: Span) -> TableReference {
    TableReference { name: name.into(), schema: None, alias: alias.map(String::from), span: at, scope: 0 }
}

fn column(name: &str, qualifier: Option<&str>, at: Span) -> ColumnReference {
    ColumnReference { name: name.into(), qualifier: qualifier.map(String::from), span: at, scope: 0 }
}

/// `select title, nope from film`, parsed.
fn title_and_nope() -> (&'static str, References) {
    (
        "select title, nope from film",
        References {
            tables: vec![table("film", None, span(0, 24, 28))],
            columns: vec![column("title", None, span(0, 7, 12)), column("nope", None, span(0, 14, 18))],
            scopes: vec![(0, None)],
            ..References::default()
        },
    )
}

/// The text has been still for longer than the pause the marks wait.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

async fn shown(h: &mut Harness, sql: &str) {
    h.app.ui.editor.set_text(sql);
    // A key, so that the app looks at the text; the pause that lets it ask
    // about it; then the replies and what they make it ask for.
    h.press("end");
    for _ in 0..6 {
        h.advance(PAUSE);
        h.settle().await;
    }
}

fn role_of(h: &mut Harness, word: &str) -> Option<Role> {
    let (x, y) = find_text(&mut h.app, 100, 30, word).unwrap_or_else(|| panic!("{word} on screen"));
    role_at(&mut h.app, x, y).fg
}

async fn with_analysis(sql: &str, references: References) -> Harness {
    with_analysis_on(pagila(), sql, references).await
}

async fn with_analysis_on(catalogue: crate::core::Catalogue, sql: &str, references: References) -> Harness {
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(catalogue);
    fake.script_analysis(sql, references);
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    shown(&mut h, sql).await;
    h
}

#[tokio::test]
async fn a_column_takes_the_column_role_and_a_name_that_resolves_to_nothing_the_unresolved_role() {
    let (sql, refs) = title_and_nope();
    let mut h = with_analysis(sql, refs).await;
    assert_eq!(role_of(&mut h, "title"), Some(Role::SynColumn));
    assert_eq!(role_of(&mut h, "nope"), Some(Role::SynUnresolved));
    assert_eq!(role_of(&mut h, "film"), Some(Role::SynIdentifier), "a table that exists keeps the identifier colour");
    assert_eq!(h.app.ui.marks.unresolved_count(), 1);
}

#[tokio::test]
async fn a_table_qualified_with_a_schema_whose_relations_are_loaded_and_not_in_it_is_unresolved() {
    let sql = "select 1 from public.filmz";
    let mut t = table("filmz", None, span(0, 21, 26));
    t.schema = Some("public".into());
    let refs = References { tables: vec![t], scopes: vec![(0, None)], ..References::default() };
    let mut h = with_analysis(sql, refs).await;
    assert_eq!(role_of(&mut h, "filmz"), Some(Role::SynUnresolved));
    assert_eq!(h.app.ui.marks.unresolved_count(), 1);
}

#[tokio::test]
async fn an_unqualified_table_is_never_unresolved_whatever_the_current_schema_lacks() {
    // `pg_stat_activity` is in pg_catalog, `payment_p2022_01` is a partition
    // the adapter's table listing leaves out, `tmp` may be a temporary table.
    for name in ["pg_stat_activity", "payment_p2022_01", "tmp", "filmz"] {
        let sql = format!("select * from {name}");
        let refs = References { tables: vec![table(name, None, span(0, 14, 14 + name.len()))], scopes: vec![(0, None)], ..References::default() };
        let mut h = with_analysis(&sql, refs).await;
        assert_eq!(h.app.ui.marks.unresolved_count(), 0, "{name}");
        assert_ne!(role_of(&mut h, name), Some(Role::SynUnresolved), "{name}");
    }
}

#[tokio::test]
async fn a_partition_the_sidebars_listing_leaves_out_is_not_unresolved_when_qualified() {
    let mut cat = pagila();
    cat.relations.push(relation("public", "payment_p2022_01", crate::core::CandidateKind::Table));
    let sql = "select * from public.payment_p2022_01";
    let mut t = table("payment_p2022_01", None, span(0, 21, 37));
    t.schema = Some("public".into());
    let refs = References { tables: vec![t], scopes: vec![(0, None)], ..References::default() };
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(cat);
    fake.script_analysis(sql, refs);
    // The tree has loaded `public`: without the partition.
    fake.script_schemas(crate::core::Scripted::ok(vec!["public".into()]));
    fake.script_tables(crate::core::Scripted::ok(vec![crate::core::Table { name: "film".into(), kind: crate::core::TableKind::Table }]));
    fake.script_connections(crate::core::Scripted::ok(vec![super::connections_tests::conn("local pg", crate::core::DbKind::Postgres)]));
    fake.script_databases(crate::core::Scripted::ok(vec!["pagila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    for _ in 0..3 {
        h.press("j enter");
        h.settle().await;
    }
    h.press("enter");
    h.settle().await;
    h.press("u ctrl+g l");
    frame_rows(&mut h.app, 100, 30);
    shown(&mut h, sql).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0);
    assert_ne!(role_of(&mut h, "payment_p2022_01"), Some(Role::SynUnresolved));
}

/// The span of the `nth` (from 0) occurrence of `needle` in a one-line `sql`.
fn at(sql: &str, needle: &str, nth: usize) -> Span {
    let from = sql.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle} #{nth} in {sql}")).0;
    let col = sql[..from].chars().count();
    span(0, col, col + needle.chars().count())
}

/// The span of `len` characters at `offset` into the first occurrence of `within`.
fn at_in(sql: &str, within: &str, offset: usize, len: usize) -> Span {
    let s = at(sql, within, 0);
    span(0, s.start.col + offset, s.start.col + offset + len)
}

async fn nothing_unresolved(sql: &str, refs: References) {
    let h = with_analysis(sql, refs).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0, "{sql}");
}

#[tokio::test]
async fn a_function_in_from_is_a_source_the_client_cannot_list_so_its_columns_are_not_unresolved() {
    for (sql, name) in [
        ("select g from film f, generate_series(1,3) g", "g"),
        ("select u from film f, unnest(array[1,2]) u", "u"),
        ("select e from film f, jsonb_array_elements('[]') e", "e"),
    ] {
        let refs = References {
            tables: vec![table("film", Some("f"), at(sql, "film", 0))],
            columns: vec![column(name, None, at(sql, name, 0))],
            scopes: vec![(0, None)],
            ..References::default()
        };
        nothing_unresolved(sql, refs).await;
    }
}

#[tokio::test]
async fn a_whole_row_reference_a_system_column_and_a_value_function_are_not_unresolved() {
    let sql = "select row_to_json(f) from film f";
    let refs = References {
        tables: vec![table("film", Some("f"), at(sql, "film", 0))],
        columns: vec![column("f", None, {
            let s = at(sql, "(f)", 0);
            span(0, s.start.col + 1, s.start.col + 2)
        })],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
    for name in ["ctid", "xmin", "xmax", "cmin", "cmax", "tableoid"] {
        let sql = format!("select {name}, title from film");
        let refs = References {
            tables: vec![table("film", None, at(&sql, "film", 0))],
            columns: vec![column(name, None, at(&sql, name, 0)), column("title", None, at(&sql, "title", 0))],
            scopes: vec![(0, None)],
            ..References::default()
        };
        nothing_unresolved(&sql, refs).await;
    }
    let sql = "select f.ctid from film f";
    let refs = References {
        tables: vec![table("film", Some("f"), at(sql, "film", 0))],
        columns: vec![column("ctid", Some("f"), at(sql, "f.ctid", 0))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
}

#[tokio::test]
async fn a_cte_or_subquery_with_an_item_the_client_cannot_name_leaves_its_columns_unmarked() {
    // A cast keeps the column's name.
    let sql = "with c as (select film_id::text from film) select c.film_id from c";
    let refs = References {
        tables: vec![table("film", None, at_in(sql, "from film", 5, 4)), table("c", None, at_in(sql, "from c", 5, 1))],
        columns: vec![column("film_id", None, at_in(sql, "select film_id", 7, 7)), column("film_id", Some("c"), at(sql, "c.film_id", 0))],
        scopes: vec![(0, None), (1, Some(0))],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
    // `values` has columns named column1, column2 ... that the client does not list.
    let sql = "with v as (values (1)) select column1 from v";
    let refs = References {
        tables: vec![table("v", None, at_in(sql, "from v", 5, 1))],
        columns: vec![column("column1", None, at(sql, "column1", 0))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
    // An expression has no name to count on (it is `?column?`, or depends on the server).
    let sql = "select s.total from (select film_id + 1 from film) s";
    let refs = References {
        tables: vec![table("film", None, at(sql, "film", 0))],
        columns: vec![column("total", Some("s"), at(sql, "s.total", 0)), column("film_id", None, at_in(sql, "film_id + 1", 0, 7))],
        scopes: vec![(0, None), (1, Some(0))],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
}

#[tokio::test]
async fn nothing_in_a_statement_with_a_set_operation_is_marked() {
    let sql = "select title as x from film union select rating from film order by x";
    let refs = References {
        tables: vec![table("film", None, at(sql, "film", 0)), table("film", None, at(sql, "film", 1))],
        columns: vec![column("title", None, at(sql, "title", 0)), column("rating", None, at(sql, "rating", 0)), column("x", None, at(sql, "x", 1))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
    let sql = "select title from film union select nope from film";
    let refs = References {
        tables: vec![table("film", None, at(sql, "film", 0)), table("film", None, at(sql, "film", 1))],
        columns: vec![column("title", None, at(sql, "title", 0)), column("nope", None, at(sql, "nope", 0))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
}

#[tokio::test]
async fn a_select_list_alias_is_usable_in_order_by_and_a_name_the_list_cannot_say_is_left_alone() {
    let sql = "select title as t from film order by t";
    let refs = References {
        tables: vec![table("film", None, at(sql, "film", 0))],
        columns: vec![column("title", None, at(sql, "title", 0)), column("t", None, at(sql, "t", 4))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
    // `count(*)` is named count, which `order by` may use.
    let sql = "select count(*) from film order by count";
    let refs = References {
        tables: vec![table("film", None, at(sql, "film", 0))],
        columns: vec![column("count", None, at(sql, "count", 1))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    nothing_unresolved(sql, refs).await;
}

#[tokio::test]
async fn a_column_of_a_plain_table_whose_columns_are_loaded_is_still_unresolved_beside_a_known_subquery() {
    let sql = "select nope from film f, (select film_id from film_actor) s";
    let refs = References {
        tables: vec![table("film", Some("f"), at(sql, "film", 0)), table("film_actor", None, at(sql, "film_actor", 0))],
        columns: vec![column("nope", None, at(sql, "nope", 0)), column("film_id", None, at(sql, "film_id", 0))],
        scopes: vec![(0, None), (1, Some(0))],
        ..References::default()
    };
    let mut h = with_analysis(sql, refs).await;
    assert_eq!(role_of(&mut h, "nope"), Some(Role::SynUnresolved));
    assert_eq!(h.app.ui.marks.unresolved_count(), 1, "only nope");
}

#[tokio::test]
async fn an_alias_that_names_nothing_is_unresolved() {
    let sql = "select z.title from film f";
    let refs = References {
        tables: vec![table("film", Some("f"), span(0, 21, 25))],
        columns: vec![column("title", Some("z"), span(0, 9, 14))],
        scopes: vec![(0, None)],
        unknown_qualifiers: vec![("z".into(), span(0, 7, 8))],
    };
    let mut h = with_analysis(sql, refs).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 1);
    let (x, y) = find_text(&mut h.app, 100, 30, "z.title").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::SynUnresolved));
}

#[tokio::test]
async fn a_statement_that_does_not_parse_shows_no_unresolved_marks_at_all() {
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(pagila());
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    shown(&mut h, "select title, nope from film where").await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0);
    assert_eq!(role_of(&mut h, "nope"), Some(Role::SynIdentifier));
    assert_eq!(role_of(&mut h, "title"), Some(Role::SynIdentifier));
}

#[tokio::test]
async fn nothing_is_unresolved_while_the_schema_for_it_is_not_loaded_or_failed_to_load() {
    let (sql, refs) = title_and_nope();
    // The columns of `film` fail to load.
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(pagila());
    fake.script_analysis(sql, refs.clone());
    fake.script_columns(crate::core::Scripted::err(crate::core::CoreErrorKind::Other, "boom"));
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    shown(&mut h, sql).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0, "a failed load is not knowledge");
    assert_eq!(role_of(&mut h, "nope"), Some(Role::SynIdentifier));
    // And before the reply has come, nothing is claimed either.
    let mut cat = pagila();
    cat.delay = std::time::Duration::from_secs(60);
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(cat);
    fake.script_analysis(sql, refs);
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    shown(&mut h, sql).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0);
}

#[tokio::test]
async fn ctes_subquery_aliases_and_select_list_aliases_are_not_reported() {
    let sql = "with c as (select film_id from film) select film_id from c order by film_id";
    let refs = References {
        tables: vec![table("film", None, span(0, 32, 36))],
        columns: vec![
            column("film_id", None, span(0, 19, 26)),
            column("film_id", None, span(0, 45, 52)),
            column("film_id", None, span(0, 68, 75)),
        ],
        scopes: vec![(0, None), (1, Some(0))],
        ..References::default()
    };
    let h = with_analysis(sql, refs).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0);
    let sql = "select title as t from film order by t";
    let refs = References {
        tables: vec![table("film", None, span(0, 22, 26))],
        columns: vec![column("title", None, span(0, 7, 12)), column("t", None, span(0, 36, 37))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    let h = with_analysis(sql, refs).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 0, "an alias of the select list");
}

#[tokio::test]
async fn f2_and_shift_f2_move_between_unresolved_names_and_wrap() {
    let sql = "select nope, title, zip from film";
    let refs = References {
        tables: vec![table("film", None, span(0, 29, 33))],
        columns: vec![column("nope", None, span(0, 7, 11)), column("title", None, span(0, 13, 18)), column("zip", None, span(0, 20, 23))],
        scopes: vec![(0, None)],
        ..References::default()
    };
    let mut h = with_analysis(sql, refs).await;
    assert_eq!(h.app.ui.marks.unresolved_count(), 2);
    h.app.ui.editor.move_to(Cursor { row: 0, col: 0 });
    h.press("f2");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 7 });
    h.press("f2");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 20 });
    h.press("f2");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 7 }, "wraps");
    h.press("shift+f2");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 20 }, "backwards, wrapping");
    h.press("shift+f2");
    assert_eq!(h.app.ui.editor.cursor(), Cursor { row: 0, col: 7 });
}

#[tokio::test]
async fn f2_is_in_the_mode_bar_only_when_there_is_something_to_go_to_and_from_the_live_keys() {
    let mut h = editing(pagila()).await;
    h.app.ui.editor.set_text("select 1");
    assert!(!mode_bar(&mut h).contains("next error"));
    let (sql, refs) = title_and_nope();
    let mut h = with_analysis(sql, refs).await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("f2 next error") && bar.contains("shift+f2 prev error"), "{bar}");
    h.app.ui.keys.rebind(super::keys::Action::NextUnresolved, "f3");
    assert!(mode_bar(&mut h).contains("f3 next error"));
}

#[tokio::test]
async fn the_marks_are_computed_when_something_changes_and_never_per_frame() {
    let (sql, refs) = title_and_nope();
    let mut h = with_analysis(sql, refs).await;
    let computed = h.app.ui.marks.computed();
    let asked = h.fake.calls().len();
    for _ in 0..20 {
        frame_rows(&mut h.app, 100, 30);
    }
    h.press("left right");
    assert_eq!(h.app.ui.marks.computed(), computed, "frames and caret moves recompute nothing");
    assert_eq!(h.fake.calls().len(), asked, "and ask nothing");
    h.type_text(" ");
    assert!(h.app.ui.marks.computed() > computed, "a text change does");
    assert!(h.fake.calls().iter().any(|c| matches!(c, Call::Analyze(_))));
}

fn analyses(h: &Harness) -> Vec<String> {
    h.fake.calls().into_iter().filter_map(|c| if let Call::Analyze(sql) = c { Some(sql) } else { None }).collect()
}

#[tokio::test]
async fn a_burst_of_keystrokes_asks_nothing_until_the_text_has_been_still_and_then_asks_once() {
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(pagila());
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    h.type_text("select * from film");
    h.settle().await;
    assert!(analyses(&h).is_empty(), "not while the user is typing: {:?}", analyses(&h));
    assert!(h.app.next_deadline().is_some(), "the pause is a deadline, not a tick");
    h.advance(std::time::Duration::from_millis(100));
    h.settle().await;
    assert!(analyses(&h).is_empty(), "not yet: the text has not been still long enough");
    h.advance(std::time::Duration::from_millis(250));
    h.settle().await;
    assert_eq!(analyses(&h), ["select * from film"], "once, after the pause");
    h.advance(std::time::Duration::from_millis(500));
    h.settle().await;
    assert_eq!(analyses(&h).len(), 1, "and no more");
    assert_eq!(h.app.next_deadline(), None, "nothing is left to wake for");
}

#[tokio::test]
async fn an_open_popup_and_the_marks_share_one_analysis_of_the_same_statement_text() {
    let fake = crate::core::FakeCore::new();
    fake.script_catalogue(pagila());
    let mut h = editing_on(fake).await;
    frame_rows(&mut h.app, 100, 30);
    super::complete_scope_tests::put(&mut h, "select t| from film f");
    h.press("ctrl+space");
    h.settle().await;
    assert!(h.app.ui.complete.is_open());
    h.type_text("i");
    h.settle().await;
    h.advance(PAUSE);
    h.settle().await;
    h.advance(PAUSE);
    h.settle().await;
    assert_eq!(analyses(&h), ["select ti from film f"], "one call for what the popup and the colours both read");
}

#[tokio::test]
async fn working_out_a_statement_again_with_the_same_text_and_schema_is_a_lookup() {
    let (sql, refs) = title_and_nope();
    let mut h = with_analysis(sql, refs).await;
    let marked = h.app.ui.marks.statements_marked();
    assert!(marked >= 1, "worked out once");
    let asked = analyses(&h).len();
    let computed = h.app.ui.marks.computed();
    // An edit and its undoing: the same text again.
    h.press("end");
    h.type_text(" ");
    h.press("backspace");
    h.advance(PAUSE);
    h.settle().await;
    assert!(h.app.ui.marks.computed() > computed, "the text changed, so the marks were looked at again");
    assert_eq!(h.app.ui.marks.statements_marked(), marked, "but nothing was worked out again");
    assert_eq!(analyses(&h).len(), asked, "and nothing asked");
    assert_eq!(h.app.ui.marks.unresolved_count(), 1);
}

#[tokio::test]
async fn the_unresolved_and_column_roles_have_their_own_colour_in_every_theme() {
    use super::theme::Palette;
    for name in Palette::NAMES {
        let p = Palette::by_name(name).unwrap();
        assert_ne!(p.color(Role::SynColumn), p.color(Role::SynIdentifier), "{name}");
        assert_ne!(p.color(Role::SynUnresolved), p.color(Role::SynIdentifier), "{name}");
        assert_ne!(p.color(Role::SynColumn), p.color(Role::SynUnresolved), "{name}");
    }
    assert_eq!(Palette::default_theme().color(Role::SynColumn), Palette::default_theme().color(Role::Mauve));
    assert_eq!(Palette::default_theme().color(Role::SynUnresolved), Palette::default_theme().color(Role::Red));
}
