//! Layers 2 and 3: the acceptance tests of slice 11 (autocomplete basics), run
//! on `App` with a `FakeCore` behind the shell's command runner. The fake
//! database answers completion by content (`Catalogue`), as the real one does.

use std::time::Duration;

use ratatui::layout::Rect;

use super::complete_popup::ItemKind;
use super::run_tests::dbp;
use super::testing::*;
use super::theme::Role;
use super::*;
use crate::core::{
    Call, CandidateKind, Catalogue, ColumnInfo, CompletionRequest, DbKind, FakeCore, Scripted, SearchKind, Table, TableKind,
};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

pub(super) fn column(name: &str, db_type: &str, primary_key: bool) -> ColumnInfo {
    ColumnInfo { name: name.into(), db_type: db_type.into(), nullable: !primary_key, primary_key, default: None }
}

pub(super) fn relation(schema: &str, name: &str, kind: CandidateKind) -> (String, String, CandidateKind) {
    (schema.into(), name.into(), kind)
}

/// A small Pagila: the tables the acceptance tests name, a view, a table with
/// a space and capitals in its name, and a few functions.
pub(super) fn pagila() -> Catalogue {
    use CandidateKind::{Table as T, View as V};
    Catalogue {
        current_schema: Some("public".into()),
        schemas: vec!["public".into(), "staff".into()],
        relations: vec![
            relation("public", "actor", T),
            relation("public", "fa_log", T),
            relation("public", "film", T),
            relation("public", "film_actor", T),
            relation("public", "film_category", T),
            relation("public", "film_list", V),
            relation("public", "Order Items", T),
            relation("staff", "badge", T),
        ],
        functions: vec![
            ("pg_catalog".into(), "length".into(), "integer".into()),
            ("pg_catalog".into(), "lower".into(), "text".into()),
            ("public".into(), "film_in_stock".into(), "integer".into()),
        ],
        columns: vec![
            (
                "public".into(),
                "film".into(),
                vec![column("film_id", "integer", true), column("title", "text", false), column("rating", "mpaa_rating", false)],
            ),
            ("public".into(), "film_actor".into(), vec![column("actor_id", "integer", false), column("film_id", "integer", false)]),
        ],
        foreign_keys: Vec::new(),
        delay: Duration::ZERO,
    }
}

/// `local pg` with `pagila` given to the tab and the editor focused, over a
/// fake that holds `catalogue`.
pub(super) async fn editing(catalogue: Catalogue) -> Harness {
    let fake = FakeCore::new();
    fake.script_catalogue(catalogue);
    editing_on(fake).await
}

pub(super) async fn editing_on(fake: FakeCore) -> Harness {
    fake.script_connections(Scripted::ok(vec![super::connections_tests::conn("local pg", DbKind::Postgres)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press("j j u ctrl+g l");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    h
}

/// Types `text`, then lets the replies of whatever the typing asked for arrive.
pub(super) async fn type_and_settle(h: &mut Harness, text: &str) {
    h.type_text(text);
    h.settle().await;
}

pub(super) fn names(h: &Harness) -> Vec<String> {
    h.app.ui.tab.complete.popup.items().iter().map(|i| i.name.clone()).collect()
}

/// The names that are not live templates (slice 12 lists those at a
/// statement's start too).
pub(super) fn keywords(h: &Harness) -> Vec<String> {
    h.app.ui.tab.complete.popup.items().iter().filter(|i| i.kind != ItemKind::Template).map(|i| i.name.clone()).collect()
}

pub(super) fn visible(h: &Harness) -> bool {
    h.app.completion_visible()
}

pub(super) fn text(h: &Harness) -> String {
    h.app.ui.tab.editor.text()
}

/// The searches for schemas and tables, which are what a table position asks
/// for (the functions asked for as other words are typed are counted apart).
fn name_searches(h: &Harness) -> Vec<CompletionRequest> {
    searches(h).into_iter().filter(|r| r.kind != SearchKind::Function).collect()
}

pub(super) fn searches(h: &Harness) -> Vec<CompletionRequest> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            // The search across schemas (for a name that two have) is counted apart.
            Call::Complete(r) if !r.global => Some(r),
            _ => None,
        })
        .collect()
}

const KEYWORDS_AFTER_FROM: [&str; 2] = ["lateral", "only"];

// ---- the popup opens, and what enter and tab do ----------------------------------

#[tokio::test]
async fn typing_sele_opens_the_popup_with_select_first_and_enter_leaves_select_in_the_editor() {
    // (`sel` itself is a live template's abbreviation: there, enter expands it.)
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sele").await;
    assert!(visible(&h));
    assert_eq!(names(&h)[0], "select");
    assert_eq!(text(&h), "sele", "listing it changed nothing");
    h.press("enter");
    assert_eq!(text(&h), "select", "enter inserts at the caret");
    assert!(!visible(&h), "and the popup is closed");
    assert_eq!(h.app.ui.tab.editor.cursor(), crate::core::Cursor { row: 0, col: 6 });
}

#[tokio::test]
async fn enter_with_the_popup_open_is_not_a_line_break_and_with_it_closed_it_is() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sele").await;
    h.press("enter");
    assert_eq!(text(&h), "select");
    h.press("enter");
    assert_eq!(text(&h), "select\n", "the popup is closed: enter is the editor's");
}

#[tokio::test]
async fn tab_replaces_the_rest_of_the_word_to_the_right_and_enter_inserts_before_it() {
    for (key, want) in [("tab", "select * from film f"), ("enter", "select * from filmm_x")] {
        let mut h = editing(pagila()).await;
        h.app.ui.tab.editor.set_text("select * from film_x");
        h.press("left left left");
        assert!(!visible(&h), "moving the caret opens nothing");
        h.press("ctrl+space");
        h.settle().await;
        assert!(names(&h).contains(&"film".to_string()), "{:?}", names(&h));
        let at = names(&h).iter().position(|n| n == "film").unwrap();
        h.app.ui.tab.complete.popup.select(at);
        h.press(key);
        assert_eq!(text(&h), want, "{key}");
    }
}

#[tokio::test]
async fn after_from_the_list_holds_tables_and_views_and_schemas_and_only_the_keywords_valid_there() {
    let mut h = editing(pagila()).await;
    h.type_text("select * from ");
    h.press("ctrl+space");
    h.settle().await;
    let listed = names(&h);
    for want in ["actor", "film", "film_list", "Order Items", "public", "staff"] {
        assert!(listed.iter().any(|n| n == want), "{want} in {listed:?}");
    }
    for item in h.app.ui.tab.complete.popup.items() {
        if item.kind == ItemKind::Keyword {
            assert!(KEYWORDS_AFTER_FROM.contains(&item.name.as_str()), "keyword {} is not valid after from", item.name);
        }
    }
    for not in ["select", "where", "film_id", "count"] {
        assert!(!listed.iter().any(|n| n == not), "{not} in {listed:?}");
    }
    // The kinds are told apart.
    let kind_of = |name: &str| h.app.ui.tab.complete.popup.items().iter().find(|i| i.name == name).map(|i| i.kind);
    assert_eq!(kind_of("film"), Some(ItemKind::Table));
    assert_eq!(kind_of("film_list"), Some(ItemKind::View));
    assert_eq!(kind_of("public"), Some(ItemKind::Schema));
}

#[tokio::test]
async fn after_a_schema_and_a_dot_only_the_tables_of_that_schema_are_listed() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from public.").await;
    assert!(visible(&h), "the dot opens the popup");
    let listed = names(&h);
    assert!(listed.contains(&"film".to_string()) && listed.contains(&"actor".to_string()), "{listed:?}");
    assert!(!listed.contains(&"badge".to_string()), "a table of another schema: {listed:?}");
    assert!(h.app.ui.tab.complete.popup.items().iter().all(|i| matches!(i.kind, ItemKind::Table | ItemKind::View)), "{listed:?}");
    type_and_settle(&mut h, "fi").await;
    assert_eq!(names(&h), ["film", "film_list", "film_actor", "film_category"], "narrowed by what is typed, shorter first");
}

#[tokio::test]
async fn a_dot_after_an_alias_lists_that_tables_columns_with_their_types() {
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select f. from film f");
    h.press("home");
    for _ in 0..9 {
        h.press("right");
    }
    assert_eq!(h.app.ui.tab.editor.cursor().col, 9, "just after `f.`");
    h.press("ctrl+space");
    h.settle().await;
    let items = h.app.ui.tab.complete.popup.items();
    let listed: Vec<(&str, Option<&str>)> = items.iter().map(|i| (i.name.as_str(), i.detail.as_deref())).collect();
    assert_eq!(listed, [("film_id", Some("integer")), ("title", Some("text")), ("rating", Some("mpaa_rating"))]);
    assert!(items.iter().all(|i| i.kind == ItemKind::Column));
    assert!(visible(&h));
    // And on screen: the name, the type and the kind.
    let rows = frame_rows(&mut h.app, 114, 30);
    let row = rows.iter().find(|r| r.contains("film_id") && r.contains("integer")).expect("a popup row");
    assert!(row.contains("column"), "{row}");
}

#[tokio::test]
async fn typing_a_dot_after_an_alias_opens_its_columns_at_once() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film_actor as fa where fa.").await;
    assert_eq!(names(&h), ["actor_id", "film_id"]);
    type_and_settle(&mut h, "fi").await;
    assert_eq!(names(&h), ["film_id"]);
    h.press("enter");
    assert!(text(&h).ends_with("where fa.film_id"), "{}", text(&h));
}

#[tokio::test]
async fn a_name_that_is_neither_an_alias_nor_a_schema_is_tried_as_a_table_of_the_current_schema() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select film.").await;
    assert_eq!(names(&h), ["film_id", "title", "rating"]);
}

#[tokio::test]
async fn the_initials_of_a_name_match_and_rank_below_a_name_that_starts_with_the_letters() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fa").await;
    let listed = names(&h);
    assert_eq!(listed, ["fa_log", "film_actor"], "fa_log starts with fa; film_actor is fa by initials");
    let tiers: Vec<_> = h.app.ui.tab.complete.popup.items().iter().map(|i| i.matched.clone()).collect();
    assert_eq!(tiers, [vec![0, 1], vec![0, 5]], "the matched characters are what is emphasised");
}

#[tokio::test]
async fn nothing_opens_inside_a_string_or_a_comment() {
    for sql in ["select 'fi", "select 'it''s fi", "select 1 -- fi", "select 1 /* fi", "select 'a', 'fi"] {
        let mut h = editing(pagila()).await;
        type_and_settle(&mut h, sql).await;
        assert!(!visible(&h), "{sql}: {:?}", names(&h));
        assert!(!h.app.ui.tab.complete.is_open(), "{sql}");
    }
    // The key does not open one there either.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select 'fi").await;
    h.press("ctrl+space");
    h.settle().await;
    assert!(!visible(&h));
    // Past the closing quote it is a name again.
    type_and_settle(&mut h, "' fi").await;
    assert!(visible(&h), "{:?}", names(&h));
}

#[tokio::test]
async fn nothing_opens_on_a_number_or_before_a_word_has_a_letter_in_it() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select 12").await;
    assert!(!visible(&h));
    type_and_settle(&mut h, " ").await;
    assert!(!visible(&h), "a space is not an identifier character");
}

#[tokio::test]
async fn a_name_typed_in_full_that_is_all_there_is_to_offer_opens_no_popup_so_enter_stays_a_line_break() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film_category").await;
    assert!(!visible(&h), "{:?}", names(&h));
    h.press("enter");
    assert_eq!(text(&h), "select * from film_category\n");
    // The key still lists it.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film_category").await;
    h.press("ctrl+space");
    h.settle().await;
    assert_eq!(names(&h), ["film_category"]);
}

#[tokio::test]
async fn a_word_too_long_to_be_a_name_is_not_matched_against_anything() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, &format!("select {}", "a".repeat(300))).await;
    assert!(!h.app.ui.tab.complete.is_open());
    assert!(h.tasks.is_empty());
    let longest = searches(&h).iter().map(|r| r.prefix.len()).max().unwrap_or(0);
    assert!(longest <= 128, "the Core was asked about a {longest}-character word");
}

// ---- esc, and moving the selection --------------------------------------------------

#[tokio::test]
async fn esc_closes_the_popup_and_leaves_the_text_and_the_next_esc_is_not_swallowed() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sel").await;
    assert!(visible(&h));
    let given = h.app.ui.tab.editor.keys_given();
    h.press("esc");
    assert!(!visible(&h) && !h.app.ui.tab.complete.is_open());
    assert_eq!(text(&h), "sel", "the text is untouched");
    assert_eq!(h.app.ui.tab.editor.keys_given(), given, "the first esc was the popup's");
    h.press("esc");
    assert_eq!(h.app.ui.tab.editor.keys_given(), given + 1, "the next esc went on to the editor");
    assert_eq!(text(&h), "sel");
}

#[tokio::test]
async fn esc_with_a_completion_that_has_nothing_to_show_yet_is_still_the_editors() {
    let fake = FakeCore::new();
    let mut slow = pagila();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select * from fi");
    assert!(h.app.ui.tab.complete.is_open() && !visible(&h), "waiting for the Core, nothing to list yet");
    let given = h.app.ui.tab.editor.keys_given();
    h.press("esc");
    assert!(!h.app.ui.tab.complete.is_open());
    assert_eq!(h.app.ui.tab.editor.keys_given(), given + 1, "nothing was shown, so nothing was swallowed");
}

#[tokio::test]
async fn up_and_down_and_ctrl_n_and_ctrl_p_move_the_selection_and_stop_at_the_ends() {
    let mut h = editing(pagila()).await;
    h.type_text("select * from ");
    h.press("ctrl+space");
    h.settle().await;
    let len = names(&h).len();
    assert!(len > 3);
    h.press("up");
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 0, "up at the first item stays");
    h.press("down down");
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 2);
    h.press("ctrl+n");
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 3);
    h.press("ctrl+p ctrl+p");
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 1);
    for _ in 0..len + 3 {
        h.press("down");
    }
    assert_eq!(h.app.ui.tab.complete.popup.selected(), len - 1, "down at the last item stays");
    assert_eq!(text(&h), "select * from ", "none of it reached the text");
    h.press("pageup");
    assert!(h.app.ui.tab.complete.popup.selected() < len - 1);
}

#[tokio::test]
async fn a_list_longer_than_the_popup_scrolls() {
    let mut cat = pagila();
    for i in 0..25 {
        cat.relations.push(relation("public", &format!("zz{i:02}"), CandidateKind::Table));
    }
    let mut h = editing(cat).await;
    h.type_text("select * from zz");
    h.settle().await;
    assert_eq!(names(&h).len(), 25);
    assert_eq!(h.app.ui.tab.complete.popup.rows(), super::complete_popup::MAX_ROWS);
    for _ in 0..14 {
        h.press("down");
    }
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 14);
    assert_eq!(h.app.ui.tab.complete.popup.top(), 5, "the window followed the selection");
    let rows = frame_rows(&mut h.app, 114, 30);
    assert!(rows.iter().any(|r| r.contains("zz14")), "the selected row is on screen");
    assert!(!rows.iter().any(|r| r.contains("zz00")), "the first rows scrolled out");
    assert!(rows.iter().any(|r| r.contains("▕")), "a scrollbar");
}

#[tokio::test]
async fn any_other_key_goes_to_the_editor_and_the_list_follows_the_text() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    assert_eq!(names(&h), ["film", "film_list", "film_actor", "film_category"]);
    type_and_settle(&mut h, "lm_").await;
    assert_eq!(names(&h), ["film_list", "film_actor", "film_category"]);
    h.press("backspace backspace");
    h.settle().await;
    assert_eq!(text(&h), "select * from fil");
    assert!(names(&h).contains(&"film".to_string()));
    // Leaving the word closes it.
    h.type_text(" ");
    assert!(!h.app.ui.tab.complete.is_open());
    // Moving the caret out of the word closes it too.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    assert!(visible(&h));
    h.press("home");
    assert!(!h.app.ui.tab.complete.is_open(), "the caret left the word");
}

// ---- quoting ------------------------------------------------------------------------

#[tokio::test]
async fn a_table_named_with_capitals_and_a_space_is_inserted_quoted_and_others_as_stored() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from Ord").await;
    assert_eq!(names(&h), ["Order Items"]);
    h.press("enter");
    assert_eq!(text(&h), "select * from \"Order Items\" oi");

    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film_c").await;
    h.press("enter");
    assert_eq!(text(&h), "select * from film_category fc", "names are inserted exactly as stored");
}

#[tokio::test]
async fn a_name_started_with_a_quote_is_replaced_whole() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from \"Or").await;
    assert_eq!(names(&h), ["Order Items"]);
    h.press("enter");
    assert_eq!(text(&h), "select * from \"Order Items\" oi");
}

#[tokio::test]
async fn a_keyword_is_inserted_in_the_case_the_user_is_typing_in() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "SELE").await;
    assert_eq!(names(&h)[0], "select");
    h.press("enter");
    assert_eq!(text(&h), "SELECT");
}

// ---- what the Core is asked, and when --------------------------------------------------

#[tokio::test]
async fn what_the_core_gave_is_kept_and_asked_again_only_after_that_part_of_the_tree_is_refreshed() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    let first = name_searches(&h);
    assert_eq!(first.iter().filter(|r| r.kind == SearchKind::Relation).count(), 1, "{first:?}");
    assert_eq!(first.iter().filter(|r| r.kind == SearchKind::Schema).count(), 1);
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, Call::CurrentSchema(_))).count(), 1);
    let relation = first.iter().find(|r| r.kind == SearchKind::Relation).unwrap();
    assert_eq!((relation.prefix.as_str(), relation.schema.as_deref()), ("", Some("public")), "the whole schema, once, so every kind of match works");

    // More typing, a new statement: the same answers, no new calls.
    type_and_settle(&mut h, "lm").await;
    h.press("esc");
    h.type_text("; select * from ac");
    h.settle().await;
    assert_eq!(names(&h), ["actor", "film_actor"], "by prefix, then anywhere in the name");
    assert_eq!(name_searches(&h), first, "nothing more was asked");

    // Refreshing the database node in the sidebar drops it.
    h.press("ctrl+g h");
    h.press("r");
    h.press("ctrl+g l");
    h.type_text("; select * from ac");
    h.settle().await;
    assert!(name_searches(&h).len() > first.len(), "asked again after the refresh: {:?}", name_searches(&h));
}

#[tokio::test]
async fn the_sidebars_schema_cache_is_used_before_the_core_is_asked() {
    let fake = FakeCore::new();
    fake.script_catalogue(Catalogue { current_schema: Some("public".into()), ..Catalogue::default() });
    fake.script_schemas(Scripted::ok(vec!["public".into()]));
    fake.script_tables(Scripted::ok(vec![
        Table { name: "film".into(), kind: TableKind::Table },
        Table { name: "film_list".into(), kind: TableKind::View },
    ]));
    fake.script_columns(Scripted::ok(vec![column("film_id", "integer", true)]));
    fake.script_connections(Scripted::ok(vec![super::connections_tests::conn("local pg", DbKind::Postgres)]));
    fake.script_databases(Scripted::ok(vec!["pagila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    // Open the connection, the database, the schema (its tables), and the table (its columns).
    h.press("enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    h.press("u ctrl+g l");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    type_and_settle(&mut h, "select * from fi").await;
    assert_eq!(names(&h), ["film", "film_list"], "from the tree");
    assert!(name_searches(&h).is_empty(), "no search for relations: {:?}", name_searches(&h));
    type_and_settle(&mut h, "lm where film.").await;
    assert_eq!(names(&h), ["film_id"], "the table's columns, from the tree");
    assert!(name_searches(&h).is_empty(), "{:?}", name_searches(&h));
    assert_eq!(
        h.fake.calls().iter().filter(|c| matches!(c, Call::Columns(_))).count(),
        1,
        "the columns call the tree made when the table was opened, and no other"
    );
}

#[tokio::test]
async fn a_function_is_searched_for_by_what_is_typed_and_listed_after_the_keywords_and_columns_it_ties_with() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select lo").await;
    assert_eq!(names(&h)[0], "lower", "a prefix match of the shorter name comes first");
    assert!(names(&h).contains(&"locked".to_string()), "keywords are in the list too");
    let found = h.app.ui.tab.complete.popup.items()[0].clone();
    assert_eq!((found.kind, found.detail.as_deref()), (ItemKind::Function, Some("text")));
    let functions: Vec<_> = searches(&h).into_iter().filter(|r| r.kind == SearchKind::Function).collect();
    assert_eq!(functions.iter().map(|r| r.schema.clone().unwrap()).collect::<Vec<_>>(), ["public", "pg_catalog"]);
    assert!(functions.iter().all(|r| r.prefix == "lo"));
}

#[tokio::test(start_paused = true)]
async fn the_common_aggregates_are_listed_without_the_core_because_dbxs_search_does_not_return_them() {
    let fake = FakeCore::new();
    let mut slow = pagila();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select cou");
    assert_eq!(names(&h), ["count"], "from the client's own list, with the Core still thinking");
    let count = &h.app.ui.tab.complete.popup.items()[0];
    assert_eq!(count.kind, ItemKind::Function);
    // Not offered where only a table is.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from cou").await;
    assert!(!visible(&h), "{:?}", names(&h));
}

#[tokio::test]
async fn columns_of_the_tables_in_the_statement_are_listed_with_the_keywords() {
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select ti from film");
    h.press("home");
    for _ in 0..9 {
        h.press("right");
    }
    h.press("ctrl+space");
    h.settle().await;
    assert_eq!(names(&h)[0], "title");
    assert!(h.app.ui.tab.complete.popup.items().iter().any(|i| i.kind == ItemKind::Keyword), "keywords match `ti`: {:?}", names(&h));
}

#[tokio::test]
async fn a_kind_other_than_postgres_gets_keywords_and_the_core_is_never_asked() {
    let mut h = editing(pagila()).await;
    h.app.model.connections = Loadable::Ready(vec![super::connections_tests::conn("local pg", DbKind::MySql)]);
    // The tab's connection is now a MySQL one.
    h.app.ui.tab.editor.set_text("");
    let before = h.fake.calls().len();
    type_and_settle(&mut h, "sele").await;
    assert_eq!(names(&h)[0], "select");
    type_and_settle(&mut h, "ct * from fi").await;
    assert!(h.app.ui.tab.complete.popup.items().iter().all(|i| i.kind == ItemKind::Keyword), "{:?}", names(&h));
    assert_eq!(h.fake.calls().len(), before, "nothing was asked of the Core");
}

#[tokio::test]
async fn a_failing_core_leaves_the_keywords_and_is_not_asked_again_on_every_key() {
    let fake = FakeCore::new(); // no catalogue: every completion call is an error
    let mut h = editing_on(fake).await;
    type_and_settle(&mut h, "select * from film where ti").await;
    let asked = h.fake.calls().len();
    assert!(visible(&h), "keywords are still offered");
    // Keywords, and the functions the client knows without the Core.
    assert!(h.app.ui.tab.complete.popup.items().iter().all(|i| i.kind == ItemKind::Keyword || (i.kind == ItemKind::Function && i.detail.is_none())));
    type_and_settle(&mut h, "tl").await;
    type_and_settle(&mut h, "e").await;
    assert_eq!(h.fake.calls().len(), asked, "a failure is not retried while the part of the tree is not refreshed");
}

// ---- a slow Core never blocks typing -----------------------------------------------------

#[tokio::test(start_paused = true)]
async fn while_a_call_is_in_flight_typed_characters_appear_at_once_and_the_list_refilters_from_what_is_cached() {
    let fake = FakeCore::new();
    let mut slow = pagila();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("s");
    assert!(visible(&h), "the keywords are listed already");
    assert!(h.tasks.is_empty(), "one letter asks the Core nothing");
    h.type_text("el");
    assert_eq!(text(&h), "sel");
    assert_eq!(keywords(&h), ["select"]);
    assert!(!h.tasks.is_empty(), "from two letters on the Core is asked for functions, and has not answered");
    h.type_text("e");
    assert_eq!(text(&h), "sele", "the character is in the text at once");
    assert_eq!(keywords(&h), ["select"], "and the list refiltered from the keywords");
    h.type_text("ct le");
    assert_eq!(text(&h), "select le");
    assert!(visible(&h), "keywords that match `le`: {:?}", names(&h));
    assert!(!h.tasks.is_empty(), "the Core was asked for functions, and has not answered");
    h.type_text("n");
    assert_eq!(text(&h), "select len");
    assert!(!visible(&h), "nothing cached matches `len` yet");
    assert!(h.app.ui.tab.complete.is_open(), "but it is waiting");
    for _ in 0..2 {
        h.settle().await;
        tokio::time::advance(Duration::from_secs(5)).await;
    }
    h.settle().await;
    assert_eq!(names(&h), ["length"], "the reply filled the popup in");
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_arrives_after_the_caret_left_the_word_does_not_open_or_change_a_popup() {
    let fake = FakeCore::new();
    let mut slow = pagila();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select * from fi");
    assert!(h.app.ui.tab.complete.is_open() && !visible(&h));
    // The caret leaves the word before the reply comes.
    h.type_text(" ");
    assert!(!h.app.ui.tab.complete.is_open());
    h.settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    let repainted = h.repaints;
    h.settle().await;
    assert_eq!(h.repaints, repainted, "the reply changed nothing on screen, so it repainted nothing");
    assert!(h.app.ui.tab.complete.popup.is_empty(), "no popup was opened");
    assert!(!visible(&h) && !h.app.ui.tab.complete.is_open());
    assert_eq!(text(&h), "select * from fi ");
    assert!(!h.app.model.completions.is_empty(), "what it said was kept for later");
}

#[tokio::test(start_paused = true)]
async fn a_reply_for_a_word_the_caret_has_moved_to_another_of_changes_nothing() {
    let fake = FakeCore::new();
    let mut slow = pagila();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select * from fi");
    h.type_text(" where ti");
    let anchor = h.app.ui.tab.complete.anchor();
    assert_eq!(anchor, Some(crate::core::Cursor { row: 0, col: 23 }), "the second word");
    for _ in 0..3 {
        h.settle().await;
        tokio::time::advance(Duration::from_secs(5)).await;
    }
    h.settle().await;
    // The popup is for `ti`, not `fi`.
    for item in h.app.ui.tab.complete.popup.items() {
        assert!(item.name.to_lowercase().contains("ti"), "{item:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_longer_word_that_needs_a_new_search_makes_the_older_reply_stale() {
    // A schema too big to list in one answer: the app asks by prefix.
    let fake = FakeCore::new();
    let mut cat = pagila();
    for i in 0..1100 {
        cat.relations.push(relation("public", &format!("zz{i:04}"), CandidateKind::Table));
    }
    cat.delay = Duration::from_secs(5);
    fake.script_catalogue(cat);
    let mut h = editing_on(fake).await;
    h.type_text("select * from ");
    h.press("ctrl+space");
    for _ in 0..2 {
        h.settle().await;
        tokio::time::advance(Duration::from_secs(5)).await;
    }
    h.settle().await;
    let first = searches(&h).into_iter().filter(|r| r.kind == SearchKind::Relation).count();
    assert_eq!(first, 1);
    h.type_text("zz");
    h.type_text("00");
    // Each longer word asked afresh, because the list held was cut short.
    h.settle().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    h.settle().await;
    let prefixes: Vec<_> = searches(&h).into_iter().filter(|r| r.kind == SearchKind::Relation).map(|r| r.prefix).collect();
    // `z`, `zz` and `zz0` were asked for and abandoned before they ran.
    assert_eq!(prefixes, ["", "zz00"], "{prefixes:?}");
    tokio::time::advance(Duration::from_secs(5)).await;
    h.settle().await;
    assert!(h.app.ui.tab.complete.popup.items().iter().all(|i| i.name.starts_with("zz00")), "{:?}", names(&h).len());
}

// ---- how it looks and the mouse ---------------------------------------------------------------

#[tokio::test]
async fn the_mode_bar_shows_the_popups_keys_while_it_is_open_and_not_otherwise() {
    let mut h = editing(pagila()).await;
    let bar = |h: &mut Harness| frame_rows(&mut h.app, 114, 30).pop().unwrap();
    let closed = bar(&mut h);
    assert!(closed.contains("f5 run") && !closed.contains("tab replace"), "{closed}");
    assert!(closed.contains("ctrl+space complete"), "the key that opens it is offered in the editor: {closed}");
    type_and_settle(&mut h, "sele").await;
    let open = bar(&mut h);
    for hint in ["enter insert", "tab replace", "down/up move", "esc close"] {
        assert!(open.contains(hint), "{hint} in {open}");
    }
    assert!(!open.contains("f5 run"));
    h.press("esc");
    assert!(bar(&mut h).contains("f5 run"));
}

#[tokio::test]
async fn a_popup_hint_in_the_mode_bar_does_what_its_key_does() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sele").await;
    frame_rows(&mut h.app, 114, 30);
    let hit = h.app.ui.mode_hints.iter().find(|m| m.action == Action::CompleteInsert).copied().expect("an insert hint");
    let fx = click(&mut h.app, hit.rect);
    h.run(fx);
    assert_eq!(text(&h), "select");
}

#[tokio::test]
async fn the_popup_is_drawn_below_the_word_on_the_popup_background_with_the_kind_on_the_right() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    let rows = frame_rows(&mut h.app, 114, 30);
    let popup = h.app.ui.hits.popup;
    assert!(!popup.is_empty());
    let text_area = h.app.ui.hits.editor_text;
    assert_eq!(popup.y, text_area.y + 1, "directly below the caret's row");
    assert_eq!(popup.x + 1, text_area.x + 14, "its names start where the word does");
    let first: String = rows[popup.y as usize].chars().skip(popup.x as usize).take(popup.width as usize).collect();
    assert!(first.contains("film") && first.trim_end().ends_with("table"), "{first:?}");
    let list = rows.iter().find(|r| r.contains("film_list")).unwrap();
    assert!(list.contains("view"), "{list}");
    // Backgrounds: the popup's fill, and the selected row in the selection style.
    assert_eq!(role_at(&mut h.app, popup.x + popup.width - 2, popup.y + 1).bg, Some(Role::PopupBg));
    assert_eq!(role_at(&mut h.app, popup.x + popup.width - 2, popup.y).bg, Some(Role::SelectionBg));
    // The screen behind it is not dimmed: it is not an overlay.
    assert!(!dimmed_at(&mut h.app, 0, 0));
    assert!(!dimmed_at(&mut h.app, text_area.x, text_area.y));
    // The matched characters are emphasised.
    let (_, _, modifier) = style_at(&mut h.app, popup.x + 1, popup.y);
    assert!(modifier.contains(ratatui::style::Modifier::BOLD), "the f of film");
}

#[tokio::test]
async fn the_popup_goes_above_the_word_when_there_is_no_room_below_and_is_clipped_to_the_terminal() {
    let mut h = editing(pagila()).await;
    h.app.ui.layout.editor = Some(940);
    let lines: Vec<String> = (1..=24).map(|i| format!("select {i};")).collect();
    h.app.ui.tab.editor.set_text(&format!("{}\nselect * from ", lines.join("\n")));
    h.press("ctrl+space");
    h.settle().await;
    frame_rows(&mut h.app, 114, 30);
    let popup = h.app.ui.hits.popup;
    // The editor scrolled to keep the caret in view.
    let caret_row = h.app.ui.hits.editor_text.y + (24 - h.app.ui.tab.editor.scroll_top().0) as u16;
    assert!(!popup.is_empty());
    assert_eq!(popup.bottom(), caret_row, "above the caret's row, ending where it begins");
    assert!(popup.bottom() <= 30 && popup.right() <= 114);

    // In a terminal that is too small for either side, it is cut to fit.
    let mut small = editing(pagila()).await;
    small.app.ui.tab.editor.set_text("select * from ");
    small.press("ctrl+space");
    small.settle().await;
    let rows = frame_rows(&mut small.app, 60, 8);
    assert_eq!(rows.len(), 8);
    let popup = small.app.ui.hits.popup;
    assert!(popup.bottom() <= 8 && popup.right() <= 60, "{popup:?}");
    assert!(small.app.ui.tab.complete.popup.selected() >= small.app.ui.tab.complete.popup.top());
}

#[tokio::test]
async fn a_click_on_an_item_accepts_it_and_a_click_elsewhere_closes_the_popup() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    frame_rows(&mut h.app, 114, 30);
    let popup = h.app.ui.hits.popup;
    let third = Rect { x: popup.x + 2, y: popup.y + 2, width: 1, height: 1 };
    assert_eq!(h.app.ui.tab.complete.popup.items()[2].name, "film_actor");
    let fx = click(&mut h.app, third);
    h.run(fx);
    assert_eq!(text(&h), "select * from film_actor fa");
    assert!(!visible(&h));

    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    frame_rows(&mut h.app, 114, 30);
    let text_area = h.app.ui.hits.editor_text;
    let fx = click(&mut h.app, Rect { x: text_area.x + 2, y: text_area.y, width: 1, height: 1 });
    h.run(fx);
    assert!(!visible(&h) && !h.app.ui.tab.complete.is_open());
    assert_eq!(text(&h), "select * from fi", "the click only moved the caret");
}

#[tokio::test]
async fn the_wheel_over_the_popup_scrolls_its_list_and_not_the_editor() {
    let mut cat = pagila();
    for i in 0..25 {
        cat.relations.push(relation("public", &format!("zz{i:02}"), CandidateKind::Table));
    }
    let mut h = editing(cat).await;
    h.type_text("select * from zz");
    h.settle().await;
    frame_rows(&mut h.app, 114, 30);
    let popup = h.app.ui.hits.popup;
    let (column, row) = (popup.x + 3, popup.y + 1);
    let ev = |kind| crossterm::event::MouseEvent { kind, column, row, modifiers: crossterm::event::KeyModifiers::NONE };
    let scroll = h.app.update(Event::Mouse(ev(crossterm::event::MouseEventKind::ScrollDown)), base());
    assert!(scroll.repaint);
    assert_eq!(h.app.ui.tab.complete.popup.top(), 3);
    assert_eq!(h.app.ui.tab.complete.popup.selected(), 0, "the selection stayed where it was");
    assert_eq!(h.app.ui.tab.editor.scroll_top(), (0, 0));
}

#[tokio::test]
async fn a_popup_does_not_survive_the_editor_losing_focus_or_a_paste() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sel").await;
    h.press("ctrl+g");
    assert!(!h.app.ui.tab.complete.is_open(), "the prefix key closes it");
    h.press("esc");
    type_and_settle(&mut h, "e").await;
    assert!(visible(&h));
    let fx = h.app.update(Event::Paste("x".into()), base());
    h.run(fx);
    assert!(!h.app.ui.tab.complete.is_open());
}

#[tokio::test]
async fn moving_the_focus_to_another_pane_closes_the_popup() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sel").await;
    assert!(visible(&h));
    h.press("ctrl+g h");
    assert!(!h.app.ui.tab.complete.is_open());
}

// ---- enter is a line break when the word is already typed in full ------------------------

/// `pagila` and a table whose columns are names (and keywords) that are also
/// the beginning of other names.
fn with_events() -> Catalogue {
    let mut cat = pagila();
    cat.relations.push(relation("public", "events", CandidateKind::Table));
    cat.columns.push((
        "public".into(),
        "events".into(),
        vec![column("id", "integer", true), column("identity", "text", false), column("end_date", "date", false)],
    ));
    cat
}

#[tokio::test]
async fn enter_after_a_keyword_typed_in_full_is_a_line_break_whatever_the_case() {
    for word in ["WHERE", "where", "Where"] {
        let mut h = editing(pagila()).await;
        type_and_settle(&mut h, &format!("select 1 {word}")).await;
        h.press("enter");
        assert_eq!(text(&h), format!("select 1 {word}\n"), "{word}: nothing was inserted, and the line broke");
        assert!(!visible(&h));
    }
}

#[tokio::test]
async fn enter_after_a_word_that_is_also_the_start_of_other_names_is_a_line_break() {
    for (sql, other) in [("select * from events where end", "end_date"), ("select * from events where id", "identity"), ("select * from events where or", "order")] {
        let mut h = editing(with_events()).await;
        type_and_settle(&mut h, sql).await;
        assert!(visible(&h) && names(&h).iter().any(|n| n == other), "{sql}: {:?}", names(&h));
        h.press("enter");
        assert_eq!(text(&h), format!("{sql}\n"), "{sql}");
        assert!(!visible(&h));
    }
}

#[tokio::test]
async fn enter_accepts_when_the_selection_was_moved_and_tab_accepts_the_exact_match() {
    let mut h = editing(with_events()).await;
    type_and_settle(&mut h, "select * from events where id").await;
    h.press("down");
    assert_eq!(h.app.ui.tab.complete.popup.selected_item().unwrap().name, "identity");
    h.press("enter");
    assert_eq!(text(&h), "select * from events where identity", "the user chose it");

    // Moved off the exact match and back: chosen, so `enter` accepts it (the
    // same text) and is not a line break.
    let mut h = editing(with_events()).await;
    type_and_settle(&mut h, "select * from events where id").await;
    h.press("down up");
    h.press("enter");
    assert_eq!(text(&h), "select * from events where id");
    assert!(!visible(&h));

    // `tab` is the key that accepts, whatever is typed.
    let mut h = editing(with_events()).await;
    type_and_settle(&mut h, "select * from events where end").await;
    h.press("tab");
    assert_eq!(text(&h), "select * from events where end", "the same text, and no line break");
    assert!(!visible(&h));
}

#[tokio::test]
async fn a_name_typed_in_full_in_quotes_is_a_line_break_too() {
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select * from \"Order Items\"");
    h.press("left");
    h.press("ctrl+space");
    h.settle().await;
    assert!(visible(&h));
    h.press("enter");
    assert_eq!(text(&h), "select * from \"Order Items\n\"", "the editor's enter, at the caret");
}

// ---- the alias position ---------------------------------------------------------------------

#[tokio::test]
async fn typing_an_alias_after_a_table_opens_no_popup_and_enter_is_a_line_break_but_the_key_still_lists() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film f").await;
    assert!(!visible(&h) && !h.app.ui.tab.complete.is_open(), "{:?}", names(&h));
    h.press("enter");
    assert_eq!(text(&h), "select * from film f\n");

    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from film o").await;
    assert!(!visible(&h), "`o` is an alias, not `on` or `order`: {:?}", names(&h));
    h.press("ctrl+space");
    h.settle().await;
    assert!(visible(&h), "the key lists what can follow a table");
    assert!(names(&h).contains(&"on".to_string()) && names(&h).contains(&"order".to_string()), "{:?}", names(&h));
}

// ---- a quoted name -----------------------------------------------------------------------------

#[tokio::test]
async fn enter_in_a_closed_quoted_name_replaces_it_through_the_closing_quote() {
    for (sql, want) in [
        ("select * from \"Or\"", "select * from \"Order Items\" oi"),
        ("select * from \"Or\" where x", "select * from \"Order Items\" oi where x"),
        ("select * from \"Ord\" o", "select * from \"Order Items\" o"),
    ] {
        let mut h = editing(pagila()).await;
        h.app.ui.tab.editor.set_text(sql);
        h.press("home");
        for _ in 0..17 {
            h.press("right");
        }
        h.press("ctrl+space");
        h.settle().await;
        assert_eq!(names(&h), ["Order Items"], "{sql}: names only");
        h.press("enter");
        assert_eq!(text(&h), want, "{sql}");
    }
}

#[tokio::test]
async fn tab_and_enter_with_the_caret_mid_word_in_a_closed_quote_leave_one_closing_quote() {
    for key in ["enter", "tab"] {
        let mut h = editing(pagila()).await;
        h.app.ui.tab.editor.set_text("select * from \"Orders\"");
        h.press("home");
        for _ in 0..17 {
            h.press("right");
        }
        h.press("ctrl+space");
        h.settle().await;
        assert_eq!(names(&h), ["Order Items"]);
        h.press(key);
        assert_eq!(text(&h), "select * from \"Order Items\" oi", "{key}");
        assert_eq!(h.app.ui.tab.editor.cursor().col, 30, "{key}: the caret is after the name and its alias");
    }
}

#[tokio::test]
async fn a_quoted_name_is_offered_names_and_no_keywords_or_functions() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from \"sel").await;
    assert!(!visible(&h), "no table matches `sel`, and `select` is not a name: {:?}", names(&h));
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select \"lo\" from film");
    h.press("home");
    for _ in 0..10 {
        h.press("right");
    }
    h.press("ctrl+space");
    h.settle().await;
    assert!(h.app.ui.tab.complete.popup.items().iter().all(|i| !matches!(i.kind, ItemKind::Keyword | ItemKind::Function)), "{:?}", names(&h));
    h.app.ui.tab.editor.set_text("select * from \"sel\"");
    h.press("left");
    h.press("ctrl+space");
    h.settle().await;
    h.press("enter");
    assert!(!text(&h).contains("select\""), "{}", text(&h));
}

// ---- undo ------------------------------------------------------------------------------------------

#[tokio::test]
async fn one_undo_restores_the_text_from_before_an_acceptance_and_one_redo_puts_it_back() {
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select * from fil_x");
    h.press("left left left");
    h.press("ctrl+space");
    h.settle().await;
    let at = names(&h).iter().position(|n| n == "film").unwrap();
    h.app.ui.tab.complete.popup.select(at);
    h.press("tab");
    assert_eq!(text(&h), "select * from film f");
    h.press("ctrl+z");
    assert_eq!(text(&h), "select * from fil_x", "one undo");
    h.press("ctrl+shift+z");
    assert_eq!(text(&h), "select * from film f", "one redo");
}

// ---- the list under a pending enter -----------------------------------------------------------

/// `pagila` with a short function name that outranks the keywords for `lo`.
fn with_lob() -> Catalogue {
    let mut cat = pagila();
    cat.functions.push(("public".into(), "lob".into(), "integer".into()));
    cat
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_re_ranks_the_list_keeps_the_item_the_user_has_seen_highlighted() {
    let fake = FakeCore::new();
    let mut slow = with_lob();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select lo");
    let drawn = names(&h)[0].clone();
    assert_eq!(drawn, "locked", "the keyword, with the Core still thinking");
    frame_rows(&mut h.app, 114, 30);
    for _ in 0..3 {
        h.settle().await;
        tokio::time::advance(Duration::from_secs(5)).await;
    }
    h.settle().await;
    assert_eq!(names(&h)[0], "lob", "the reply is a better match, and sorts first");
    assert_eq!(h.app.ui.tab.complete.popup.selected_item().unwrap().name, drawn, "but the highlighted item is the one that was drawn");
    h.press("enter");
    assert_eq!(text(&h), "select locked", "enter inserted what the user saw");
}

#[tokio::test(start_paused = true)]
async fn a_reply_before_any_frame_showed_the_list_may_change_the_top_item() {
    let fake = FakeCore::new();
    let mut slow = with_lob();
    slow.delay = Duration::from_secs(5);
    fake.script_catalogue(slow);
    let mut h = editing_on(fake).await;
    h.type_text("select lo");
    assert_eq!(names(&h)[0], "locked");
    for _ in 0..3 {
        h.settle().await;
        tokio::time::advance(Duration::from_secs(5)).await;
    }
    h.settle().await;
    assert_eq!(h.app.ui.tab.complete.popup.selected_item().unwrap().name, "lob", "nothing was drawn, so the best match is the top");
}

// ---- a failing Core ----------------------------------------------------------------------------------

fn notice(h: &Harness) -> Option<String> {
    h.app.ui.notice.as_ref().map(|n| n.text.clone())
}

#[tokio::test]
async fn a_core_error_says_so_once_and_the_key_asks_again() {
    let mut h = editing_on(FakeCore::new()).await; // no catalogue: every completion call is an error
    type_and_settle(&mut h, "select * from film where ti").await;
    let said = notice(&h).expect("a notice");
    assert!(said.starts_with("Completion for ") && said.contains(" failed: ") && said.ends_with("Refresh it in the sidebar to retry."), "{said}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    let asked = h.fake.calls().len();
    type_and_settle(&mut h, "tl").await;
    assert_eq!(h.fake.calls().len(), asked, "not asked again by typing");

    // The Core is well again: the key asks what failed once more.
    h.fake.script_catalogue(pagila());
    h.app.ui.notice = None;
    h.press("ctrl+space");
    h.settle().await;
    assert!(h.fake.calls().len() > asked, "asked again");
    assert!(notice(&h).is_none(), "{:?}", notice(&h));
    h.press("esc");
    h.app.ui.tab.editor.set_text("select * from fi");
    h.press("ctrl+space");
    h.settle().await;
    assert!(names(&h).contains(&"film".to_string()), "{:?}", names(&h));
}

// ---- what is never offered -------------------------------------------------------------------------

#[tokio::test]
async fn a_name_with_a_control_character_is_not_offered() {
    let mut cat = pagila();
    cat.relations.push(relation("public", "bad_ok", CandidateKind::Table));
    cat.relations.push(relation("public", "bad\nname", CandidateKind::Table));
    cat.relations.push(relation("public", "bad\u{7}bell", CandidateKind::Table));
    cat.relations.push(relation("public", "bad\u{85}next", CandidateKind::Table));
    let mut h = editing(cat).await;
    type_and_settle(&mut h, "select * from ba").await;
    assert_eq!(names(&h), ["bad_ok"]);
    h.press("down");
    h.press("enter");
    assert_eq!(h.app.ui.tab.editor.lines().len(), 1, "nothing was split across rows: {:?}", h.app.ui.tab.editor.lines());
}

// ---- work done while typing -----------------------------------------------------------------------

#[tokio::test]
async fn typing_a_word_tokenizes_nothing_between_frames_and_one_frame_does_it_once() {
    let mut h = editing(pagila()).await;
    frame_rows(&mut h.app, 114, 30);
    let base = h.app.ui.tab.syntax.stats().runs;
    h.type_text("selectfilm");
    assert_eq!(h.app.ui.tab.syntax.stats().runs, base, "ten keystrokes between frames: no pass over the buffer");
    frame_rows(&mut h.app, 114, 30);
    assert_eq!(h.app.ui.tab.syntax.stats().runs, base + 1, "the frame took one");
}

#[tokio::test]
async fn a_string_the_windowed_read_cannot_see_closes_the_popup_at_the_next_frame() {
    // The `;` on the row above the caret cuts the window, so the string that
    // began before it is not seen as one: the frame's full tokenization is.
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select 'abc\nmore; text\nsel");
    h.press("ctrl+space");
    h.settle().await;
    frame_rows(&mut h.app, 114, 30);
    assert!(!visible(&h), "{:?}", names(&h));
    // And `enter` before any frame does not put a keyword in the string.
    let mut h = editing(pagila()).await;
    h.app.ui.tab.editor.set_text("select 'abc\nmore; text\nsel");
    h.press("ctrl+space");
    h.press("enter");
    assert!(!text(&h).contains("select\nmore") && !text(&h).ends_with("select"), "{}", text(&h));
}

// ---- search_path -------------------------------------------------------------------------------------

/// Lets the tab's run finish.
async fn ran_to_the_end(h: &mut Harness) {
    for _ in 0..200 {
        h.settle().await;
        if h.app.model.tabs[0].run.in_flight().is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the run did not end");
}

#[tokio::test]
async fn a_run_that_mentions_search_path_drops_the_cached_current_schema_and_others_do_not() {
    let current_schema_calls = |h: &Harness| h.fake.calls().iter().filter(|c| matches!(c, Call::CurrentSchema(_))).count();
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from fi").await;
    assert_eq!(current_schema_calls(&h), 1);
    // An ordinary run leaves it.
    h.fake.script_run(Scripted::ok(super::run_tests::rows_of(&[&["1"]])));
    h.press("f5");
    ran_to_the_end(&mut h).await;
    h.app.ui.tab.editor.set_text("select * from fi");
    h.press("ctrl+space");
    h.settle().await;
    assert_eq!(current_schema_calls(&h), 1, "kept");
    // One that sets the search path drops it.
    h.app.ui.tab.editor.set_text("SET search_path TO staff");
    h.fake.script_run(Scripted::ok(super::run_tests::rows_of(&[])));
    h.press("f5");
    ran_to_the_end(&mut h).await;
    h.app.ui.tab.editor.set_text("select * from fi");
    h.press("ctrl+space");
    h.settle().await;
    assert_eq!(current_schema_calls(&h), 2, "asked again after the run");
}
