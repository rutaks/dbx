//! Layer 2: the acceptance tests of slice 12 for live and postfix templates,
//! run on `App` with a `FakeCore` behind the shell's command runner.

use super::complete_popup::ItemKind;
use super::complete_scope_tests::{put, with_keys};
use super::complete_tests::*;
use super::run_tests::mode_bar;
use crate::core::Cursor;
use crate::harness::Harness;

fn caret(h: &Harness) -> (usize, usize) {
    let c = h.app.ui.editor.cursor();
    (c.row, c.col)
}

fn templates(h: &Harness) -> Vec<String> {
    h.app.ui.complete.popup.items().iter().filter(|i| i.kind == ItemKind::Template).map(|i| i.name.clone()).collect()
}

/// Types `abbr` at an empty statement and expands it with `tab`.
async fn expanded(h: &mut Harness, abbr: &str) {
    type_and_settle(h, abbr).await;
    h.press("tab");
    h.settle().await;
}

// ---- live templates ---------------------------------------------------------------

#[tokio::test]
async fn sel_then_tab_expands_with_the_caret_on_the_table_variable_and_tab_moves_on() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "sel").await;
    assert_eq!(text(&h), "select * from ;");
    assert_eq!(caret(&h), (0, 14), "on the table variable");
    assert!(h.app.ui.template.active());
    type_and_settle(&mut h, "fil").await;
    assert!(visible(&h), "a variable that names a table opens the normal table completion");
    assert_eq!(names(&h)[0], "film");
    h.press("tab");
    assert_eq!(text(&h), "select * from film;", "tab accepts the table; a template adds no alias of its own");
    assert!(h.app.ui.template.active());
    h.press("tab");
    assert_eq!(caret(&h), (0, 19), "the last tab puts the caret at the template's end");
    assert!(!h.app.ui.template.active());
    assert_eq!(text(&h), "select * from film;");
}

#[tokio::test]
async fn esc_leaves_template_mode_and_keeps_the_text() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "sel").await;
    type_and_settle(&mut h, "fi").await;
    assert!(visible(&h));
    h.press("esc");
    assert!(!visible(&h) && h.app.ui.template.active(), "the first esc closes only the popup");
    h.press("esc");
    assert!(!h.app.ui.template.active());
    assert_eq!(text(&h), "select * from fi;");
}

#[tokio::test]
async fn a_template_the_word_is_exactly_comes_first_highlighted_with_its_preview_and_enter_and_tab_both_expand_it() {
    for key in ["enter", "tab"] {
        let mut h = editing(pagila()).await;
        type_and_settle(&mut h, "del").await;
        let first = h.app.ui.complete.popup.items()[0].clone();
        assert_eq!((first.name.as_str(), first.kind), ("del", ItemKind::Template), "{:?}", names(&h));
        assert_eq!(h.app.ui.complete.popup.selected_item().map(|i| i.name.clone()), Some("del".into()), "highlighted");
        assert!(first.detail.as_deref().is_some_and(|d| d.starts_with("delete from")), "a preview of the expansion: {first:?}");
        let screen = super::testing::frame_rows(&mut h.app, 114, 30).join("\n");
        assert!(screen.contains("template") && screen.contains("delete from"), "{screen}");
        h.press(key);
        assert_eq!(text(&h), "delete from  where ;", "{key}");
        assert!(h.app.ui.template.active(), "{key}");
    }
    // A template that is the only match is shown too, and enter expands it.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "selw").await;
    assert!(visible(&h));
    h.press("enter");
    assert_eq!(text(&h), "select * from   where .;");
    // One more character: back to keywords.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sel").await;
    assert_eq!(names(&h)[0], "sel");
    type_and_settle(&mut h, "e").await;
    assert!(templates(&h).is_empty() && names(&h)[0] == "select", "{:?}", names(&h));
    h.press("enter");
    assert_eq!(text(&h), "select");
}

#[tokio::test]
async fn the_mode_bar_shows_the_expansion_keys_from_the_live_bindings_while_a_template_is_highlighted() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "del").await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("tab/enter expand") && !bar.contains("replace"), "{bar}");
    h.app.ui.keys.rebind(super::keys::Action::ExpandTemplate, "ctrl+j");
    assert!(mode_bar(&mut h).contains("ctrl+j expand"));
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "sele").await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("enter insert") && !bar.contains("expand"), "{bar}");
}

#[tokio::test]
async fn templates_are_offered_only_where_a_statement_starts() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select * from sel").await;
    assert!(templates(&h).is_empty(), "{:?}", names(&h));
    h.press("tab");
    assert!(!h.app.ui.template.active());
    assert!(!text(&h).contains("select * from select * from"), "{}", text(&h));
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select 1; del").await;
    assert_eq!(templates(&h), ["del"], "the next statement starts after the semicolon");
    // Not after anything else: `drop table ins` + tab is not rewritten.
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "drop table ins").await;
    assert!(templates(&h).is_empty(), "{:?}", names(&h));
    h.press("tab");
    assert!(text(&h).starts_with("drop table ins") && !text(&h).contains("insert"), "{}", text(&h));
    assert!(!h.app.ui.template.active());
    // A comment before the word is not a statement.
    let mut h = editing(pagila()).await;
    h.app.ui.editor.set_text("-- note\n/* x */ ");
    h.press("ctrl+end");
    type_and_settle(&mut h, "del").await;
    h.press("tab");
    assert!(text(&h).ends_with("delete from  where ;"), "{}", text(&h));
}

#[tokio::test]
async fn selw_shares_its_alias_between_its_places_and_shift_tab_goes_back() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "selw").await;
    assert_eq!(text(&h), "select * from   where .;");
    assert_eq!(caret(&h), (0, 14));
    h.press("tab");
    assert_eq!(caret(&h), (0, 15), "the alias variable");
    h.press("shift+tab");
    assert_eq!(caret(&h), (0, 14), "back to the table");
    type_and_settle(&mut h, "film").await;
    h.press("tab");
    assert_eq!(text(&h), "select * from film f where f.;", "accepting the table fills the alias");
    h.press("tab");
    assert_eq!(h.app.ui.editor.selection(), Some((Cursor { row: 0, col: 19 }, Cursor { row: 0, col: 20 })), "a filled variable is selected: typing replaces it");
    h.type_text("x");
    assert_eq!(text(&h), "select * from film x where x.;", "typing in one place types in both");
    h.press("tab");
    assert_eq!(caret(&h), (0, 29), "after the dot: where the template ends");
    assert!(!h.app.ui.template.active());
}

#[tokio::test]
async fn typing_in_the_first_place_of_a_shared_variable_fills_the_other() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "selw").await;
    h.press("tab");
    h.type_text("ab");
    assert_eq!(text(&h), "select * from  ab where ab.;");
    assert_eq!(caret(&h), (0, 17));
}

#[tokio::test]
async fn moving_the_caret_out_of_the_template_leaves_template_mode_and_keeps_the_text() {
    let mut h = editing(pagila()).await;
    h.app.ui.editor.set_text("select 1;\nsel");
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "select 1;\nselect * from ;");
    assert!(h.app.ui.template.active());
    h.press("left");
    assert!(h.app.ui.template.active(), "inside the template");
    h.press("up");
    assert!(!h.app.ui.template.active());
    assert_eq!(text(&h), "select 1;\nselect * from ;");
}

#[tokio::test]
async fn the_whole_expansion_is_one_undo_step() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "sel").await;
    assert_eq!(text(&h), "select * from ;");
    h.press("ctrl+u");
    assert_eq!(text(&h), "sel", "one undo gives back what was typed");
    assert!(!h.app.ui.template.active());
    h.press("ctrl+r");
    assert_eq!(text(&h), "select * from ;");
}

#[tokio::test]
async fn an_undo_or_a_redo_leaves_template_mode_and_then_undoes_without_being_mirrored_back() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "selw").await;
    h.press("tab");
    h.type_text("ab");
    assert_eq!(text(&h), "select * from  ab where ab.;");
    assert!(h.app.ui.template.active());
    h.press("ctrl+u");
    assert!(!h.app.ui.template.active(), "an undo ends template mode");
    // From here undo is the editor's own: it walks back to what was typed.
    for _ in 0..30 {
        h.press("ctrl+u");
    }
    assert_eq!(text(&h), "", "undo does not oscillate: it gets back to the empty buffer");
    for _ in 0..30 {
        h.press("ctrl+r");
    }
    assert_eq!(text(&h), "select * from  ab where ab.;", "and redo walks forward");
    assert!(!h.app.ui.template.active());
    // A redo also leaves the mode.
    let mut h = editing(pagila()).await;
    expanded(&mut h, "selw").await;
    h.press("tab");
    h.type_text("a");
    h.press("ctrl+u");
    h.press("ctrl+r");
    assert!(!h.app.ui.template.active());
}

#[tokio::test]
async fn ins_fills_the_column_list_once_the_table_is_accepted() {
    let mut h = editing(pagila()).await;
    expanded(&mut h, "ins").await;
    assert_eq!(text(&h), "insert into  () values ();");
    assert_eq!(caret(&h), (0, 12));
    type_and_settle(&mut h, "film").await;
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "insert into film (film_id, title, rating) values ();");
    h.press("tab");
    h.press("tab");
    assert_eq!(caret(&h), (0, 50), "the end is inside the values parentheses");
}

#[tokio::test(start_paused = true)]
async fn a_column_list_that_comes_late_is_written_only_while_its_table_is_still_in_the_variable() {
    use std::time::Duration;
    let mut h = editing(with_keys()).await;
    expanded(&mut h, "ins").await;
    type_and_settle(&mut h, "film").await;
    // The columns of `film` are on their way, slowly.
    h.fake.set_catalogue_delay(Duration::from_secs(5));
    h.press("tab");
    assert_eq!(text(&h), "insert into film () values ();");
    // The user goes back and types another table.
    h.press("shift+tab");
    h.type_text("film_actor");
    h.settle().await;
    assert!(h.app.ui.template.active());
    // film's reply arrives: it is not written under `film_actor`.
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    assert!(!text(&h).contains("rating"), "{}", text(&h));
    assert_eq!(text(&h), "insert into film_actor () values ();");
    // The table that is there now is asked for, and filled in when it comes.
    let asked: Vec<String> = h.fake.calls().into_iter().filter_map(|c| if let crate::core::Call::Columns(t) = c { Some(t.table) } else { None }).collect();
    assert_eq!(asked, ["film", "film_actor"], "{asked:?}");
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    assert_eq!(text(&h), "insert into film_actor (actor_id, film_id) values ();");
}

#[tokio::test]
async fn the_other_templates_expand_as_the_research_gives_them() {
    for (abbr, want, at) in [
        ("selc", "select count(*) from   where .;", 21),
        ("lim", "select * from  limit ;", 14),
        ("upd", "update  set  =  where ;", 7),
        ("tab", "create table  (    );", 13),
        ("view", "create view  as select * from  ;", 12),
    ] {
        let mut h = editing(pagila()).await;
        expanded(&mut h, abbr).await;
        assert_eq!(text(&h), want, "{abbr}");
        assert_eq!(caret(&h), (0, at), "{abbr}");
    }
}

#[tokio::test]
async fn the_mode_bar_shows_template_mode_and_its_live_keys_and_hides_them_after() {
    let mut h = editing(pagila()).await;
    assert!(!mode_bar(&mut h).contains("TEMPLATE"));
    expanded(&mut h, "selw").await;
    let bar = mode_bar(&mut h);
    for want in ["TEMPLATE", "tab next", "shift+tab prev", "esc done"] {
        assert!(bar.contains(want), "{want} in {bar}");
    }
    h.app.ui.keys.rebind(super::keys::Action::TemplateNext, "ctrl+j");
    assert!(mode_bar(&mut h).contains("ctrl+j next"), "from the live bindings");
    h.press("esc");
    let bar = mode_bar(&mut h);
    assert!(!bar.contains("TEMPLATE") && !bar.contains("prev"), "{bar}");
}

// ---- postfix templates ---------------------------------------------------------------

#[tokio::test]
async fn film_dot_from_then_tab_becomes_select_from_film_with_the_caret_after_select() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "film.from").await;
    h.press("tab");
    assert_eq!(text(&h), "select  from film");
    assert_eq!(caret(&h), (0, 7));
    assert!(!h.app.ui.template.active());
}

#[tokio::test]
async fn dot_cfrom_lists_the_tables_columns() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "film.cfrom").await;
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "select film_id, title, rating from film");
}

#[tokio::test]
async fn dot_join_uses_a_foreign_key_when_one_exists() {
    let mut h = editing(with_keys()).await;
    type_and_settle(&mut h, "film_actor.join").await;
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "from film_actor join film f on film_actor.film_id = f.film_id");
    put(&mut h, "film.join|");
    h.press("tab");
    h.settle().await;
    assert_eq!(text(&h), "from film join  on ", "no foreign key: the caret waits for the table");
    assert_eq!(caret(&h), (0, 15));
}

#[tokio::test]
async fn dot_cast_wraps_the_expression_and_leaves_the_caret_before_the_bracket() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "'x'.cast").await;
    h.press("tab");
    assert_eq!(text(&h), "cast('x' as )");
    assert_eq!(caret(&h), (0, 12));
}

#[tokio::test]
async fn a_postfix_expansion_is_one_undo_step() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "film.from").await;
    h.press("tab");
    h.press("ctrl+u");
    assert_eq!(text(&h), "film.from");
}

#[tokio::test]
async fn an_ordinary_dot_word_is_not_a_postfix_template() {
    let mut h = editing(pagila()).await;
    type_and_settle(&mut h, "select film.title").await;
    h.press("tab");
    assert!(text(&h).starts_with("select film.title"), "{}", text(&h));
    assert!(!text(&h).contains("from") && !text(&h).contains("cast"), "nothing was expanded: {}", text(&h));
}
