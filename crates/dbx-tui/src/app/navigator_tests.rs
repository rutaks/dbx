//! Layer 2: the acceptance tests of slice 21 (the navigator), run on `App`
//! with a `FakeCore` behind the shell's command runner.

use std::time::Duration;

use super::complete_tests::{pagila, relation};
use super::connections_tests::conn;
use super::keys::Action;
use super::navigator::PAUSE;
use super::overlay::{Overlay, OverlayAction};
use super::run_tests::dbp;
use super::testing::*;
use super::tree::{NodeId, RowKind};
use super::*;
use crate::core::{
    Call, CandidateKind, Catalogue, DbKind, FakeCore, SchemaPath, Scripted, SearchKind, Table, TableKind, TablePath,
};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

fn table(name: &str) -> Table {
    Table { name: name.into(), kind: TableKind::Table }
}

fn catalogue_with(extra: &[&str]) -> Catalogue {
    let mut c = pagila();
    for name in extra {
        c.relations.push(relation("public", name, CandidateKind::Table));
    }
    c
}

/// `local pg` opened, its databases listed, nothing below them loaded; the
/// fake knows `public` with these tables.
fn fake_with(tables: &[&str], catalogue: Catalogue) -> FakeCore {
    let fake = FakeCore::new();
    fake.script_catalogue(catalogue);
    fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    fake.script_schemas(Scripted::ok(vec!["public".into()]));
    fake.script_tables(Scripted::ok(tables.iter().map(|t| table(t)).collect()));
    fake.script_columns(Scripted::ok(vec![crate::app::complete_tests::column("film_id", "integer", true)]));
    fake
}

async fn started(fake: FakeCore) -> Harness {
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h
}

/// The tab runs on `pagila`, which and `public` are open in the sidebar with
/// these tables loaded.
async fn loaded(tables: &[&str], catalogue: Catalogue) -> Harness {
    let mut h = started(fake_with(tables, catalogue)).await;
    h.press("j j u");
    h.press("enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    h
}

async fn world() -> Harness {
    loaded(&["film", "film_actor", "film_category", "actor"], pagila()).await
}

/// The sidebar cursor on `pagila`, the tab with no database, nothing loaded
/// below it.
async fn collapsed(tables: &[&str], catalogue: Catalogue) -> Harness {
    let mut h = started(fake_with(tables, catalogue)).await;
    h.press("j j");
    h
}

fn screen(h: &mut Harness) -> String {
    frame_text(&mut h.app, 114, 36).join("\n")
}

fn navigator(h: &Harness) -> &super::navigator::NavigatorOverlay {
    match &h.app.ui.overlay {
        Some(Overlay::Navigator(n)) => n,
        other => panic!("the navigator is not open: {other:?}"),
    }
}

/// The names the list holds, as drawn (`schema.name`).
fn labels(h: &Harness) -> Vec<String> {
    navigator(h).items.iter().map(|i| i.label()).collect()
}

fn film() -> TablePath {
    TablePath {
        schema: SchemaPath { database: dbp("local pg", "pagila"), schema: "public".into() },
        table: "film".into(),
    }
}

/// The global searches the fake has been asked.
fn searches(h: &Harness) -> Vec<String> {
    h.fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Complete(r) if r.global => Some(r.prefix),
            _ => None,
        })
        .collect()
}

fn list_calls(h: &Harness) -> Vec<Call> {
    h.fake
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Databases(_) | Call::Schemas(_) | Call::Tables(_) | Call::Columns(_)))
        .collect()
}

fn cursor_node(h: &Harness) -> Option<NodeId> {
    h.app.ui.sidebar.rows.get(h.app.ui.sidebar.cursor).map(|r| r.node.clone())
}

fn button(h: &Harness, action: OverlayAction) -> ratatui::layout::Rect {
    h.app.ui.overlay_buttons.iter().find(|b| b.action == action).unwrap_or_else(|| panic!("no button {action:?}")).rect
}

// ---- opening and searching ----------------------------------------------------

#[tokio::test]
async fn prefix_p_opens_the_navigator_and_fil_shows_the_loaded_names_before_any_reply_then_the_reply_adds_the_rest() {
    let mut h = world().await;
    h.press("ctrl+g p");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Navigator(_))));
    h.type_text("fil");
    // Before any reply, and before the pause: the tree's own names.
    assert!(searches(&h).is_empty(), "nothing is asked of the database yet");
    let shown = screen(&mut h);
    for name in ["public.film", "public.film_actor", "public.film_category"] {
        assert!(shown.contains(name), "{name}:\n{shown}");
    }
    assert!(!shown.contains("film_list"), "not loaded, not shown yet:\n{shown}");
    assert_eq!(labels(&h), ["public.film", "public.film_actor", "public.film_category"], "the shortest first");
    // The kind and the place are in each row.
    let row = shown.lines().find(|l| l.contains("public.film_category")).unwrap();
    assert!(row.contains("table") && row.contains("local pg / pagila"), "{row}");
    // After the pause the database is asked, and its answer adds what the
    // tree does not hold, after what was shown.
    h.press("down");
    let before = navigator(&h).selected_item().cloned();
    h.advance(PAUSE);
    h.settle().await;
    assert_eq!(searches(&h), ["fil"]);
    assert_eq!(labels(&h), ["public.film", "public.film_actor", "public.film_category", "public.film_list"]);
    let shown = screen(&mut h);
    assert!(shown.lines().any(|l| l.contains("public.film_list") && l.contains("view")), "{shown}");
    assert_eq!(navigator(&h).selected_item(), before.as_ref(), "the highlighted row did not move");
}

#[tokio::test]
async fn cmd_o_opens_it_too_and_ctrl_n_is_not_a_direct_key_because_the_popup_and_the_editor_read_it() {
    let mut h = world().await;
    h.press("cmd+o");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Navigator(_))));
    h.press("cmd+o");
    assert!(h.app.ui.overlay.is_none(), "the same key closes it");
    let keys = &h.app.ui.keys;
    assert_eq!(keys.lookup_direct(&super::keys::KeyCombo::parse("ctrl+n").unwrap()), None);
    assert_eq!(
        keys.lookup_complete(&super::keys::KeyCombo::parse("ctrl+n").unwrap(), false),
        Some(Action::CompleteDown)
    );
    assert_eq!(keys.lookup(&super::keys::KeyCombo::parse("p").unwrap()), Some(Action::Navigator));
}

#[tokio::test(start_paused = true)]
async fn a_stale_reply_for_fi_is_dropped_and_repaints_nothing() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("fi");
    // Ask for `fi`, keeping the request to answer it late by hand.
    let now = h.now() + PAUSE;
    let fx = h.app.update(Event::Timer, now);
    let Some(Command::Core(old, CoreCall::Complete(asked))) =
        fx.commands.iter().find(|c| matches!(c, Command::Core(_, CoreCall::Complete(r)) if r.global)).cloned()
    else {
        panic!("a search for fi: {:?}", fx.commands)
    };
    assert_eq!(asked.prefix, "fi");
    assert!(asked.global && asked.kind == SearchKind::Relation && asked.schema.is_none());
    // The text changes: the search is no longer wanted, and is let go of.
    let key = |c| {
        Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        ))
    };
    let fx = h.app.update(key('l'), now);
    assert!(fx.commands.contains(&Command::Abandon(old)), "{:?}", fx.commands);
    let shown = labels(&h);
    // Its answer lands anyway: nothing changes.
    let stale = crate::core::Completion {
        candidates: vec![crate::core::Candidate {
            name: "fiction".into(),
            kind: CandidateKind::Table,
            schema: Some("public".into()),
            detail: None,
        }],
        incomplete: false,
    };
    let fx = h.app.update(Event::Core(old, CoreReply::Completed(Ok(stale))), now);
    assert!(!fx.repaint && fx.commands.is_empty(), "a stale reply repaints nothing");
    assert_eq!(labels(&h), shown);
    assert!(!screen(&mut h).contains("fiction"));
}

#[tokio::test]
async fn nothing_is_asked_while_typing_goes_on_and_closing_lets_go_of_the_search() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("f");
    h.advance(Duration::from_millis(150));
    h.type_text("i");
    h.advance(Duration::from_millis(150));
    assert!(searches(&h).is_empty(), "the pause restarts with each key");
    h.advance(Duration::from_millis(60));
    h.settle().await;
    assert_eq!(searches(&h), ["fi"], "one search, for what was typed");
    // A search in flight when the overlay closes is abandoned.
    h.type_text("l");
    let now = h.now() + PAUSE;
    let fx = h.app.update(Event::Timer, now);
    let asked = fx.commands.iter().any(|c| matches!(c, Command::Core(_, CoreCall::Complete(r)) if r.global));
    assert!(asked);
    let fx = h.app.update(
        Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )),
        now,
    );
    assert!(fx.commands.iter().any(|c| matches!(c, Command::Abandon(_))), "{:?}", fx.commands);
    assert!(h.app.ui.overlay.is_none());
}

// ---- where it looks ---------------------------------------------------------------

#[tokio::test]
async fn with_no_database_on_the_tab_it_looks_in_the_one_under_the_sidebar_cursor() {
    let mut h = collapsed(&["film", "actor"], pagila()).await;
    assert_eq!(h.app.model.tabs[0].target, None);
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    let asked: Vec<_> = h
        .fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Complete(r) if r.global => Some(r.at),
            _ => None,
        })
        .collect();
    assert_eq!(asked, [dbp("local pg", "pagila")]);
    assert!(labels(&h).contains(&"public.film".to_string()), "{:?}", labels(&h));
}

#[tokio::test]
async fn with_no_database_anywhere_it_says_to_give_the_tab_one_with_the_live_key() {
    let mut h = started(fake_with(&["film"], pagila())).await;
    // The cursor is on the connection, which has no database of its own.
    h.press("ctrl+g p");
    let shown = screen(&mut h);
    assert!(shown.contains("Give it one first") && shown.contains("press u"), "{shown}");
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    assert!(h.app.ui.keys.rebind(Action::UseTarget, "x"));
    h.press("ctrl+g p");
    let shown = screen(&mut h);
    assert!(shown.contains("press x") && !shown.contains("press u"), "the live key: {shown}");
    // Typing does nothing, and asks nothing.
    h.type_text("film");
    h.advance(PAUSE);
    assert!(searches(&h).is_empty());
}

// ---- revealing --------------------------------------------------------------------

#[tokio::test]
async fn enter_on_film_with_the_tree_collapsed_loads_each_level_in_order_and_ends_on_film_with_the_sidebar_focused() {
    let mut h = collapsed(&["film", "film_actor", "actor"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    assert_eq!(
        labels(&h),
        ["public.film", "public.film_actor", "public.film_category", "public.film_list"],
        "what the database knows"
    );
    let before = list_calls(&h);
    let rev = h.press_fx("enter");
    assert!(h.app.ui.overlay.is_none(), "the overlay closes");
    h.settle().await;
    // The listings, in the order the levels are: schemas of the database,
    // then the tables of the schema. Nothing is listed twice.
    let asked: Vec<Call> = list_calls(&h).into_iter().skip(before.len()).collect();
    let public = SchemaPath { database: dbp("local pg", "pagila"), schema: "public".into() };
    assert_eq!(asked, [Call::Schemas(dbp("local pg", "pagila")), Call::Tables(public)], "{rev:?}");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    assert_eq!(cursor_node(&h), Some(NodeId::Table(film())));
    let row = h.app.ui.sidebar.rows[h.app.ui.sidebar.cursor].clone();
    assert!(matches!(&row.kind, RowKind::Table { name, .. } if name == "film"));
    assert!(h.app.ui.sidebar.expanded.contains(&NodeId::Database(dbp("local pg", "pagila"))));
    assert!(screen(&mut h).contains("film"));
}

#[tokio::test]
async fn a_name_that_is_not_loaded_yet_is_revealed_after_each_level_arrives_and_a_key_in_between_stops_it() {
    let mut h = collapsed(&["film"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    h.press("enter");
    // The first listing is on its way; the user moves on.
    assert!(h.app.ui.reveal.is_some());
    h.press("k");
    assert!(h.app.ui.reveal.is_none(), "a key in the sidebar ends the reveal");
    h.settle().await;
    assert_ne!(cursor_node(&h), Some(NodeId::Table(film())), "the cursor was not taken away");
}

#[tokio::test]
async fn mixed_case_is_revealed_with_its_exact_name() {
    let mut h = collapsed(&["Mixed Case", "film"], catalogue_with(&["Mixed Case"])).await;
    h.press("ctrl+g p");
    h.type_text("mixed");
    h.advance(PAUSE);
    h.settle().await;
    assert_eq!(labels(&h), ["public.Mixed Case"]);
    h.press("enter");
    h.settle().await;
    let want = TablePath { table: "Mixed Case".into(), ..film() };
    assert_eq!(cursor_node(&h), Some(NodeId::Table(want)));
}

#[tokio::test]
async fn a_name_with_control_characters_is_drawn_replaced_and_still_revealed_by_its_exact_name() {
    let nasty = "bad\u{1b}[31mname\u{7}";
    let mut h = loaded(&[nasty, "film"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("bad");
    let shown = screen(&mut h);
    assert!(!shown.chars().any(|c| c.is_control() && c != '\n'), "{shown:?}");
    assert!(shown.contains("public.bad\u{fffd}[31mname\u{fffd}"), "replaced, not dropped:\n{shown}");
    h.press("enter");
    let want = TablePath { table: nasty.into(), ..film() };
    assert_eq!(cursor_node(&h), Some(NodeId::Table(want)));
}

#[tokio::test]
async fn a_level_that_fails_to_load_ends_the_reveal_on_it_and_says_so() {
    let fake = FakeCore::new();
    fake.script_catalogue(pagila());
    fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    fake.script_schemas(Scripted::err(crate::core::CoreErrorKind::Connect, "connection refused"));
    let mut h = started(fake).await;
    h.press("j j");
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    assert!(h.app.ui.reveal.is_none());
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    assert_eq!(cursor_node(&h), Some(NodeId::Database(dbp("local pg", "pagila"))), "on the level that failed");
    assert!(screen(&mut h).contains("connection refused"));
}

// ---- opening rows -------------------------------------------------------------------

#[tokio::test]
async fn ctrl_o_opens_film_in_a_browse_tab() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("film");
    let commands = h.press_fx("ctrl+o");
    h.settle().await;
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.model.tabs.len(), 2, "{commands:?}");
    let tab = h.app.model.tabs.iter().find(|t| t.browse.is_some()).expect("a browse tab");
    assert_eq!(tab.browse.as_ref().unwrap().table, film());
    assert_eq!(tab.target, Some(dbp("local pg", "pagila")));
    assert_eq!(h.app.ui.focus, PaneId::Results);
}

#[tokio::test]
async fn both_actions_are_buttons_and_a_click_on_a_row_highlights_it() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("film_");
    screen(&mut h);
    let shown = screen(&mut h);
    assert!(shown.contains("reveal in sidebar") && shown.contains("open rows"), "{shown}");
    // A click on the second row highlights it.
    let second = button(&h, OverlayAction::NavigatorPick(1));
    click(&mut h.app, second);
    assert_eq!(navigator(&h).selected, 1);
    // Reveal.
    screen(&mut h);
    let reveal = button(&h, OverlayAction::NavigatorReveal);
    let fx = click(&mut h.app, reveal);
    h.run(fx);
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
    assert!(matches!(cursor_node(&h), Some(NodeId::Table(t)) if t.table == "film_category"), "{:?}", cursor_node(&h));
    // Open its rows.
    h.press("ctrl+g p");
    h.type_text("film_cat");
    screen(&mut h);
    let open = button(&h, OverlayAction::NavigatorOpen);
    let fx = click(&mut h.app, open);
    h.run(fx);
    h.settle().await;
    assert!(h.app.model.tabs.iter().any(|t| t.browse.as_ref().is_some_and(|b| b.table.table == "film_category")));
}

// ---- keys inside it ---------------------------------------------------------------

#[tokio::test]
async fn the_arrows_move_the_highlight_and_esc_closes_without_losing_the_editor_text() {
    let mut h = world().await;
    h.press("ctrl+g l");
    h.type_text("select 1");
    h.press("ctrl+g p");
    h.type_text("film");
    assert_eq!(navigator(&h).selected, 0);
    h.press("down");
    h.press("ctrl+n");
    assert_eq!(navigator(&h).selected, 2);
    h.press("ctrl+p");
    h.press("up");
    assert_eq!(navigator(&h).selected, 0);
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.tab.editor.text(), "select 1");
}

// ---- review fixes ---------------------------------------------------------------------

/// A reveal of `film` begun with the sidebar still collapsed, and the focus in
/// the editor, so a reveal that steals it shows.
async fn revealing() -> Harness {
    let mut h = collapsed(&["film"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    h.app.ui.focus = PaneId::Editor;
    h.press("enter");
    assert!(h.app.ui.reveal.is_some());
    h
}

fn pagila_db() -> NodeId {
    NodeId::Database(dbp("local pg", "pagila"))
}

#[tokio::test]
async fn a_click_on_the_loading_nodes_arrow_ends_the_reveal_and_a_listing_landing_later_moves_nothing() {
    let mut h = revealing().await;
    frame_rows(&mut h.app, 114, 30);
    let list = h.app.ui.hits.sidebar_list;
    let at = h.app.ui.sidebar.rows.iter().position(|r| r.is_node() && r.node == pagila_db()).unwrap();
    let rect = ratatui::layout::Rect::new(
        list.x + 1 + 2 * h.app.ui.sidebar.rows[at].depth,
        list.y + (at - h.app.ui.sidebar.scroll) as u16,
        1,
        1,
    );
    let fx = click(&mut h.app, rect);
    h.run(fx);
    assert!(h.app.ui.reveal.is_none(), "the user took over with the mouse");
    // A different listing lands.
    let mut fx = Effects::default();
    h.app.expand(NodeId::Database(dbp("local pg", "edge")), &mut fx);
    h.run(fx);
    h.settle().await;
    h.app.ui.focus = PaneId::Editor;
    let cursor = cursor_node(&h);
    h.settle().await;
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    assert_eq!(cursor_node(&h), cursor);
    assert_ne!(cursor_node(&h), Some(NodeId::Table(film())));
}

#[tokio::test]
async fn a_listing_the_reveal_did_not_wait_for_does_not_continue_it() {
    let mut h = revealing().await;
    let cursor = cursor_node(&h);
    // The node it waits for is collapsed from elsewhere (its listing is let go of).
    let mut fx = Effects::default();
    h.app.collapse(&pagila_db(), &mut fx);
    h.run(fx);
    // A different listing lands.
    let mut fx = Effects::default();
    h.app.expand(NodeId::Database(dbp("local pg", "edge")), &mut fx);
    h.run(fx);
    h.settle().await;
    assert_eq!(h.app.ui.focus, PaneId::Editor, "focus is not stolen");
    assert_eq!(cursor_node(&h), cursor, "the cursor does not move");
    assert!(!h.app.ui.sidebar.expanded.contains(&pagila_db()), "nothing re-expanded");
}

#[tokio::test]
async fn a_tab_switch_or_a_focus_change_ends_the_reveal() {
    let mut h = revealing().await;
    h.press("ctrl+g t");
    assert!(h.app.ui.reveal.is_none(), "a new tab");
    let mut h = revealing().await;
    h.press("ctrl+g j");
    assert!(h.app.ui.reveal.is_none(), "focus moved by key");
}

#[tokio::test]
async fn a_reveal_retries_a_level_once_and_then_stops_on_it() {
    let mut h = collapsed(&["film"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    h.press("enter");
    let asked = |h: &Harness| list_calls(h).len();
    // The level is let go of twice (the cache dropped under it).
    h.app.model.schema.set(pagila_db(), super::tree::Children::NotLoaded);
    let before = asked(&h);
    let mut fx = Effects::default();
    h.app.continue_reveal(&mut fx);
    assert_eq!(fx.commands.iter().filter(|c| matches!(c, Command::Core(..))).count(), 1, "one retry");
    h.run(fx);
    assert!(asked(&h) >= before);
    h.app.model.schema.set(pagila_db(), super::tree::Children::NotLoaded);
    let mut fx = Effects::default();
    h.app.continue_reveal(&mut fx);
    assert!(fx.commands.iter().all(|c| !matches!(c, Command::Core(..))), "no third try: {:?}", fx.commands);
    assert!(h.app.ui.reveal.is_none());
    assert_eq!(cursor_node(&h), Some(pagila_db()), "it stops on that node");
    assert_eq!(h.app.ui.focus, PaneId::Sidebar);
}

#[tokio::test]
async fn a_reveal_through_a_connection_that_cannot_be_browsed_says_so() {
    let mut h = collapsed(&["film"], pagila()).await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    h.settle().await;
    let super::Loadable::Ready(list) = &mut h.app.model.connections else { panic!("connections") };
    let conn = NodeId::Connection(list[0].id.clone());
    list[0].capabilities.browse = false;
    h.app.model.schema.set(conn.clone(), super::tree::Children::NotLoaded);
    h.press("enter");
    assert!(h.app.ui.reveal.is_none());
    let note = h.app.ui.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(note.contains("cannot be browsed"), "{note:?}");
}

#[tokio::test]
async fn a_search_of_an_earlier_navigator_does_not_add_rows_to_the_one_opened_after_it() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("film");
    h.advance(PAUSE);
    assert!(matches!(navigator(&h).state, super::navigator::Search::Asking));
    // Something else took the overlay's place; the navigator is opened again.
    h.app.ui.overlay = None;
    h.press("ctrl+g p");
    h.settle().await;
    assert!(navigator(&h).items.is_empty(), "{:?}", labels(&h));
}

#[tokio::test]
async fn the_navigator_keeps_its_names_visible_at_40x12_and_60x16_and_says_when_it_cannot_be_drawn() {
    let mut h = world().await;
    h.press("ctrl+g p");
    h.type_text("film");
    for (w, rows) in [(40u16, 12u16), (60, 16)] {
        let shown = frame_text(&mut h.app, w, rows).join("\n");
        assert!(shown.contains("public.film"), "{w}x{rows}:\n{shown}");
    }
    let shown = frame_text(&mut h.app, 20, 6).join("\n");
    assert!(shown.contains("too small"), "{shown}");
}

#[tokio::test]
async fn the_help_row_for_closing_the_navigator_shows_every_key_that_does_it() {
    let keys = super::keys::Keybinds::default();
    let row = keys
        .help()
        .into_iter()
        .flat_map(|s| s.rows)
        .find(|r| r.label == "close" && r.bindings.iter().any(|b| b.text == "cmd+o"))
        .map(|r| r.bindings.len());
    assert!(row.is_some(), "a `close` row lists cmd+o");
}
