//! Layer 2: the acceptance tests of slice 12 (autocomplete in context), the
//! scope and data half: what is in scope at the caret, whole join conditions,
//! aliases, column lists, schema qualification on collisions and expanding `*`.
//! Run on `App` with a `FakeCore` behind the shell's command runner.

use super::complete_popup::ItemKind;
use super::complete_tests::*;
use super::run_tests::mode_bar;
use super::testing::frame_rows;
use crate::core::{Call, CandidateKind, Catalogue, Cursor, ForeignKey, References, Span, TablePath, TableReference};
use crate::harness::Harness;

pub(super) fn fk(name: &str, column: &str, table: &str, ref_column: &str) -> ForeignKey {
    ForeignKey { name: name.into(), column: column.into(), ref_schema: Some("public".into()), ref_table: table.into(), ref_column: ref_column.into() }
}

/// `pagila` with the foreign keys of `film_actor`, and a table whose columns
/// are names that start like keywords.
pub(super) fn with_keys() -> Catalogue {
    let mut cat = pagila();
    cat.foreign_keys.push((
        "public".into(),
        "film_actor".into(),
        vec![fk("film_actor_film_id_fkey", "film_id", "film", "film_id"), fk("film_actor_actor_id_fkey", "actor_id", "actor", "actor_id")],
    ));
    cat.relations.push(relation("public", "codes", CandidateKind::Table));
    cat.columns.push(("public".into(), "codes".into(), vec![column("id", "integer", true), column("descr", "text", false)]));
    cat
}

/// Sets the text of the editor from `sql` with `|` marking the caret, and
/// puts the caret there.
pub(super) fn put(h: &mut Harness, sql: &str) {
    let rows: Vec<&str> = sql.split('\n').collect();
    let (row, col) = rows.iter().enumerate().find_map(|(r, l)| l.find('|').map(|i| (r, l[..i].chars().count()))).expect("a caret");
    h.app.ui.tab.editor.set_text(&sql.replacen('|', "", 1));
    for _ in row..rows.len() - 1 {
        h.press("up");
    }
    h.press("home");
    for _ in 0..col {
        h.press("right");
    }
    assert_eq!(h.app.ui.tab.editor.cursor(), Cursor { row, col });
}

async fn complete_at(h: &mut Harness, sql: &str) {
    put(h, sql);
    h.press("ctrl+space");
    h.settle().await;
}

/// (name, owner, insert) of the items of one kind.
fn of_kind(h: &Harness, kind: ItemKind) -> Vec<(String, Option<String>, String)> {
    h.app.ui.tab.complete.popup.items().iter().filter(|i| i.kind == kind).map(|i| (i.name.clone(), i.owner.clone(), i.insert.clone())).collect()
}

fn col(name: &str, owner: &str, insert: &str) -> (String, Option<String>, String) {
    (name.to_string(), Some(owner.to_string()), insert.to_string())
}

fn position(h: &Harness, name: &str, owner: &str) -> usize {
    h.app.ui.tab.complete.popup.items().iter().position(|i| i.name == name && i.owner.as_deref() == Some(owner)).unwrap_or_else(|| panic!("{name} of {owner} in {:?}", names(h)))
}

// ---- scope ----------------------------------------------------------------------

#[tokio::test]
async fn the_select_list_offers_the_columns_of_every_table_in_scope_each_with_its_alias() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select | from film f join film_actor fa on f.film_id = fa.film_id").await;
    let cols = of_kind(&h, ItemKind::Column);
    for want in [
        col("film_id", "f", "f.film_id"),
        col("film_id", "fa", "fa.film_id"),
        col("title", "f", "title"),
        col("actor_id", "fa", "actor_id"),
    ] {
        assert!(cols.contains(&want), "{want:?} in {cols:?}");
    }
    // Accepting the one of the second table inserts it qualified.
    let at = position(&h, "film_id", "fa");
    h.app.ui.tab.complete.popup.select(at);
    h.press("tab");
    assert!(text(&h).starts_with("select fa.film_id from film f"), "{}", text(&h));
}

#[tokio::test]
async fn inside_a_subquery_only_its_tables_are_offered_and_the_outer_ones_after_them() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from film f where f.film_id in (select | from film_actor fa)").await;
    let cols = of_kind(&h, ItemKind::Column);
    assert!(cols.contains(&col("actor_id", "fa", "actor_id")), "{cols:?}");
    assert!(cols.contains(&col("title", "f", "f.title")), "correlated: the outer table follows, and its columns are qualified: {cols:?}");
    assert!(position(&h, "actor_id", "fa") < position(&h, "title", "f"));
    assert!(position(&h, "film_id", "fa") < position(&h, "film_id", "f"), "the inner one first");

    // A derived table does not see the tables beside it.
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from film f join (select | from film_actor fa) s on true").await;
    let cols = of_kind(&h, ItemKind::Column);
    assert!(cols.iter().all(|c| c.1.as_deref() == Some("fa")), "{cols:?}");
}

#[tokio::test]
async fn a_column_of_an_outer_query_is_always_inserted_qualified_and_so_is_any_column_while_a_source_is_not_loaded() {
    let mut h = editing(with_keys()).await;
    // film's columns are known; film_actor's are on their way and do not come.
    complete_at(&mut h, "select * from film f where f.|").await;
    assert_eq!(names(&h), ["film_id", "title", "rating"]);
    h.press("esc");
    h.fake.set_catalogue_delay(std::time::Duration::from_secs(600));
    put(&mut h, "select * from film f where exists (select 1 from film_actor fa where |");
    h.press("ctrl+space");
    h.settle().await;
    let cols = of_kind(&h, ItemKind::Column);
    assert!(cols.contains(&col("film_id", "f", "f.film_id")), "an outer column is qualified, and `film_id` alone would be fa's: {cols:?}");
    assert!(cols.iter().all(|c| c.2.contains('.')), "{cols:?}");
    let at = position(&h, "film_id", "f");
    h.app.ui.tab.complete.popup.select(at);
    h.press("enter");
    assert_eq!(text(&h), "select * from film f where exists (select 1 from film_actor fa where f.film_id");
    // At the same level: while a source's columns are not known, none is bare.
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from film f where f.|").await;
    h.press("esc");
    h.fake.set_catalogue_delay(std::time::Duration::from_secs(600));
    put(&mut h, "select | from film f join film_actor fa on true");
    h.press("ctrl+space");
    h.settle().await;
    let cols = of_kind(&h, ItemKind::Column);
    assert!(cols.contains(&col("title", "f", "f.title")), "{cols:?}");
    // Once everything is loaded, an unshared column at the innermost level is bare again.
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select | from film f").await;
    assert!(of_kind(&h, ItemKind::Column).contains(&col("title", "f", "title")));
}

#[tokio::test]
async fn outside_a_subquery_its_alias_and_select_list_names_are_offered() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select | from (select film_id as id, title from film) s").await;
    let cols = of_kind(&h, ItemKind::Column);
    assert!(cols.contains(&col("id", "s", "id")) && cols.contains(&col("title", "s", "title")), "{cols:?}");
    assert!(!cols.iter().any(|c| c.0 == "film_id"), "{cols:?}");
    complete_at(&mut h, "select s.| from (select film_id as id, title from film) s").await;
    assert_eq!(names(&h), ["id", "title"]);
}

#[tokio::test]
async fn a_cte_is_a_table_after_its_definition_and_its_select_list_names_are_its_columns() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "with c as (select film_id, title from film) select * from |").await;
    let ctes: Vec<String> = h.app.ui.tab.complete.popup.items().iter().filter(|i| i.kind == ItemKind::Cte).map(|i| i.name.clone()).collect();
    assert_eq!(ctes, ["c"], "{:?}", names(&h));
    complete_at(&mut h, "with c as (select * from |) select * from c").await;
    assert!(!names(&h).contains(&"c".to_string()), "not inside its own definition: {:?}", names(&h));
    complete_at(&mut h, "with c as (select film_id, title from film) select c.|").await;
    assert_eq!(names(&h), ["film_id", "title"]);
    complete_at(&mut h, "with c as (select film_id, title from film) select | from c").await;
    assert!(of_kind(&h, ItemKind::Column).contains(&col("title", "c", "title")));
}

#[tokio::test]
async fn dbxs_analysis_is_used_when_it_parses_and_the_tokens_when_it_does_not() {
    let sql = "select t from film f";
    let mut h = editing(with_keys()).await;
    h.fake.script_analysis(
        sql,
        References {
            tables: vec![TableReference {
                name: "film".into(),
                schema: None,
                alias: Some("g".into()),
                span: Span { start: Cursor { row: 0, col: 14 }, end: Cursor { row: 0, col: 18 } },
                scope: 0,
            }],
            scopes: vec![(0, None)],
            ..References::default()
        },
    );
    complete_at(&mut h, "select t| from film f").await;
    let asked = |h: &Harness| h.fake.calls().iter().filter(|c| matches!(c, Call::Analyze(_))).count();
    assert_eq!(asked(&h), 0, "completion never asks for an analysis: the colours do, once the text is still");
    assert!(of_kind(&h, ItemKind::Column).iter().any(|c| c.1.as_deref() == Some("f")), "the tokens' alias until then");
    let pause = std::time::Duration::from_millis(250);
    h.advance(pause);
    h.settle().await;
    assert!(of_kind(&h, ItemKind::Column).iter().any(|c| c.1.as_deref() == Some("g")), "dbx's alias, once it has answered: {:?}", of_kind(&h, ItemKind::Column));
    assert_eq!(asked(&h), 1);
    // The same text is not asked about again.
    h.press("ctrl+space");
    h.advance(pause);
    h.settle().await;
    assert_eq!(asked(&h), 1, "kept for the text");
    // Text that does not parse: the tokens.
    complete_at(&mut h, "select t| from film f join").await;
    h.advance(pause);
    h.settle().await;
    assert!(of_kind(&h, ItemKind::Column).iter().any(|c| c.1.as_deref() == Some("f")));
    // Nothing to resolve: nothing asked, however much is typed.
    let before = asked(&h);
    h.app.ui.tab.editor.set_text("");
    type_and_settle(&mut h, "select 1 + 1").await;
    h.advance(pause);
    h.settle().await;
    assert_eq!(asked(&h), before);
}

// ---- join conditions --------------------------------------------------------------

#[tokio::test]
async fn after_on_the_first_suggestion_is_the_foreign_key_condition_between_the_tables() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from film f join film_actor fa on |").await;
    let first = &h.app.ui.tab.complete.popup.items()[0];
    assert_eq!((first.name.as_str(), first.kind), ("f.film_id = fa.film_id", ItemKind::Condition), "{:?}", names(&h));
    let keys: Vec<TablePath> = h.fake.calls().into_iter().filter_map(|c| if let Call::ForeignKeys(t) = c { Some(t) } else { None }).collect();
    assert!(keys.iter().any(|t| t.table == "film_actor"), "{keys:?}");
    // The other direction: the key is on the earlier table.
    complete_at(&mut h, "select * from film_actor fa join film f on |").await;
    assert_eq!(names(&h)[0], "fa.film_id = f.film_id");
    h.press("enter");
    assert_eq!(text(&h), "select * from film_actor fa join film f on fa.film_id = f.film_id");
    // Kept like columns: no second ask for the same table.
    let again = h.fake.calls().iter().filter(|c| matches!(c, Call::ForeignKeys(t) if t.table == "film_actor")).count();
    assert_eq!(again, 1);
}

// ---- aliases ----------------------------------------------------------------------

#[tokio::test]
async fn accepting_a_table_after_from_adds_an_alias_made_unique_in_the_statement() {
    let mut h = editing(with_keys()).await;
    type_and_settle(&mut h, "select * from fil").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    h.press("enter");
    assert_eq!(text(&h), "select * from film f");
    type_and_settle(&mut h, " join fil").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    h.press("enter");
    assert_eq!(text(&h), "select * from film f join film f2");
    // Initials of the underscore-separated words.
    let mut h = editing(with_keys()).await;
    type_and_settle(&mut h, "select * from film_a").await;
    h.press("enter");
    assert_eq!(text(&h), "select * from film_actor fa");
    h.press("ctrl+z");
    assert_eq!(text(&h), "select * from film_a", "one undo step");
}

#[tokio::test]
async fn no_alias_when_the_option_is_off_when_one_was_typed_or_the_table_is_completed_elsewhere() {
    let mut h = editing(with_keys()).await;
    h.app.model.settings.alias_on_table = false;
    type_and_settle(&mut h, "select * from fil").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    h.press("enter");
    assert_eq!(text(&h), "select * from film");

    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from fil| f").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    h.press("tab");
    assert_eq!(text(&h), "select * from film f", "the user's alias stays the only one");

    for sql in ["insert into fil|", "update fil|"] {
        let mut h = editing(with_keys()).await;
        complete_at(&mut h, sql).await;
        h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
        h.press("enter");
        assert_eq!(text(&h), sql.replace("fil|", "film"), "{sql}");
    }
}

#[tokio::test]
async fn no_alias_is_added_when_the_statement_already_names_the_table_before_a_dot_and_the_row_shows_the_alias() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select film_actor.actor_id from film_a|").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film_actor").unwrap());
    let row = h.app.ui.tab.complete.popup.selected_item().unwrap().clone();
    assert_eq!(row.alias, None);
    h.press("enter");
    assert_eq!(text(&h), "select film_actor.actor_id from film_actor");
    // Otherwise the alias that will be inserted is in the row.
    let mut h = editing(with_keys()).await;
    type_and_settle(&mut h, "select * from fil").await;
    let screen = frame_rows(&mut h.app, 114, 30).join("\n");
    assert!(screen.contains("film f "), "{screen}");
    // An alias the statement has taken, or a table or CTE it names, is not generated.
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "with f as (select 1) select * from fil|").await;
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    assert_eq!(h.app.ui.tab.complete.popup.selected_item().unwrap().alias.as_deref(), Some("f2"));
}

// ---- column lists -----------------------------------------------------------------

#[tokio::test]
async fn insert_into_a_table_offers_one_item_that_inserts_every_column() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "insert into film (|").await;
    let lists = of_kind(&h, ItemKind::ColumnList);
    assert_eq!(lists.len(), 1, "{:?}", names(&h));
    assert_eq!(h.app.ui.tab.complete.popup.items()[0].kind, ItemKind::ColumnList, "first");
    h.press("enter");
    assert_eq!(text(&h), "insert into film (film_id, title, rating");
}

#[tokio::test]
async fn group_by_and_order_by_offer_the_select_lists_expressions_and_aliases_first() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select title as t, lower(title) from film f group by |").await;
    let first: Vec<(String, ItemKind)> = h.app.ui.tab.complete.popup.items().iter().take(3).map(|i| (i.name.clone(), i.kind)).collect();
    assert_eq!(
        first,
        [("t".to_string(), ItemKind::Select), ("title".to_string(), ItemKind::Select), ("lower(title)".to_string(), ItemKind::Select)]
    );
    complete_at(&mut h, "select film_id, count(*) n from film f order by |").await;
    assert_eq!(h.app.ui.tab.complete.popup.items()[0].kind, ItemKind::Select);
}

// ---- keywords against columns -------------------------------------------------------

#[tokio::test]
async fn a_keyword_that_starts_like_the_word_ranks_before_a_column_only_where_it_is_valid() {
    let mut h = editing(with_keys()).await;
    complete_at(&mut h, "select * from codes order by id des|").await;
    assert_eq!(names(&h)[0], "desc", "valid after an expression: {:?}", names(&h));
    complete_at(&mut h, "select * from codes order by des|").await;
    assert_eq!(names(&h)[0], "descr", "not valid there: {:?}", names(&h));
}

// ---- schema qualification ------------------------------------------------------------

#[tokio::test]
async fn a_name_in_two_schemas_is_inserted_qualified_and_a_name_in_one_bare() {
    let mut cat = with_keys();
    cat.relations.push(relation("archive", "film", CandidateKind::Table));
    let mut h = editing(cat).await;
    type_and_settle(&mut h, "select * from fil").await;
    let films: Vec<Option<String>> = h.app.ui.tab.complete.popup.items().iter().filter(|i| i.name == "film").map(|i| i.owner.clone()).collect();
    assert_eq!(films, [Some("public".to_string()), Some("archive".to_string())], "{:?}", names(&h));
    assert!(!names(&h).contains(&"badge".to_string()), "other schemas are offered only for a name they share");
    h.app.ui.tab.complete.popup.select(names(&h).iter().position(|n| n == "film").unwrap());
    h.press("enter");
    assert_eq!(text(&h), "select * from public.film f");
}

// ---- expanding * -------------------------------------------------------------------

#[tokio::test]
async fn alt_enter_on_a_star_expands_it_in_one_undo_step_and_the_mode_bar_says_so() {
    let mut h = editing(with_keys()).await;
    put(&mut h, "select *| from film f");
    assert!(mode_bar(&mut h).contains("alt+enter expand *"), "{}", mode_bar(&mut h));
    h.press("alt+enter");
    h.settle().await;
    assert_eq!(text(&h), "select f.film_id, f.title, f.rating from film f");
    h.press("ctrl+z");
    assert_eq!(text(&h), "select * from film f");
    // Two tables: qualified by alias.
    put(&mut h, "select |* from film f join film_actor fa on true");
    h.press("alt+enter");
    h.settle().await;
    assert_eq!(text(&h), "select f.film_id, f.title, f.rating, fa.actor_id, fa.film_id from film f join film_actor fa on true");
    // Elsewhere it does nothing, and no hint says otherwise.
    put(&mut h, "select * from film f where f.film_id = 1|");
    assert!(!mode_bar(&mut h).contains("expand"), "{}", mode_bar(&mut h));
    let _ = frame_rows;
}

fn notice(h: &Harness) -> String {
    h.app.ui.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default()
}

fn ran(commands: &[crate::app::Command]) -> bool {
    commands.iter().any(|c| matches!(c, crate::app::Command::Core(_, crate::app::CoreCall::Run(..))))
}

#[tokio::test]
async fn alt_enter_is_not_a_run_key_it_expands_a_star_and_elsewhere_does_nothing_and_says_nothing() {
    let mut h = editing(with_keys()).await;
    // Not on a `*`: nothing runs, nothing is edited, nothing is said.
    put(&mut h, "select 1|");
    let commands = h.press_fx("alt+enter");
    assert!(!ran(&commands), "{commands:?}");
    assert_eq!(text(&h), "select 1");
    assert_eq!(notice(&h), "");
    assert!(!mode_bar(&mut h).contains("alt+enter"), "{}", mode_bar(&mut h));
    // On a `*`: it expands, and does not run.
    put(&mut h, "select *| from film f");
    let commands = h.press_fx("alt+enter");
    h.settle().await;
    assert!(!ran(&commands), "{commands:?}");
    assert_eq!(text(&h), "select f.film_id, f.title, f.rating from film f");
    // The run keys are f5 and the prefix `r`.
    put(&mut h, "select 1|");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("f5 run") && !bar.contains("alt+enter"), "{bar}");
    assert!(ran(&h.press_fx("f5")));
    let mut other = editing(with_keys()).await;
    put(&mut other, "select 1|");
    assert!(ran(&other.press_fx("ctrl+g r")));
    // Also from the sidebar, where it ran before.
    let mut h = editing(with_keys()).await;
    put(&mut h, "select 1|");
    h.press("ctrl+g h");
    let commands = h.press_fx("alt+enter");
    assert!(!ran(&commands), "{commands:?}");
}

#[tokio::test]
async fn a_star_is_not_expanded_when_the_columns_it_stands_for_are_not_all_known() {
    for (sql, why) in [
        ("select |* from (select film_id, film_id + 1 from film) s", "no name"),
        ("with c as (select 1) select |* from c", "no name"),
        ("select |* from film join film_actor using (film_id)", "USING or NATURAL"),
        ("select |* from film natural join film_actor", "USING or NATURAL"),
        ("select |* from film f, generate_series(1,3) g", "is not a table"),
        ("select |* from (values (1)) v", "is not a table"),
        ("select |* from film f cross join lateral (select 1) s", "is not a table"),
    ] {
        let mut h = editing(with_keys()).await;
        put(&mut h, sql);
        h.press("alt+enter");
        h.settle().await;
        assert_eq!(text(&h), sql.replace('|', ""), "{sql}");
        let said = notice(&h);
        assert!(said.starts_with("Could not expand *") && said.contains(why), "{sql}: {said:?}");
    }
}

#[tokio::test]
async fn a_star_over_a_subquery_whose_columns_are_all_named_expands_to_them() {
    let mut h = editing(with_keys()).await;
    put(&mut h, "select |* from (select film_id, title as t from film) s");
    h.press("alt+enter");
    h.settle().await;
    assert_eq!(text(&h), "select s.film_id, s.t from (select film_id, title as t from film) s");
    // A qualified star is refused by the same rules.
    put(&mut h, "select s.|* from film f join film_actor using (film_id), (select 1) s");
    h.press("alt+enter");
    assert!(text(&h).contains("s.* from"), "{}", text(&h));
}

#[tokio::test]
async fn the_search_across_schemas_is_made_once_for_the_database_and_kept() {
    let mut h = editing(with_keys()).await;
    type_and_settle(&mut h, "select * from fi").await;
    type_and_settle(&mut h, "l").await;
    let global = |h: &Harness| h.fake.calls().iter().filter(|c| matches!(c, Call::Complete(r) if r.global && r.schema.is_none())).count();
    assert_eq!(global(&h), 1, "every schema, once, with an empty prefix");
}
