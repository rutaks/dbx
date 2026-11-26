//! Layer 2: the acceptance tests of slice 21 (the help), run on `App`.

use super::help::filter;
use super::keys::{Action, Keybinds};
use super::overlay::Overlay;
use super::run_tests::on_pagila;
use super::testing::*;
use super::{Capabilities, PaneId};
use crate::harness::Harness;

fn screen(h: &mut Harness) -> String {
    frame_text(&mut h.app, 120, 40).join("\n")
}

async fn helping() -> Harness {
    let mut h = on_pagila().await;
    h.app.ui.caps = Capabilities { keyboard_protocol: true };
    h.press("ctrl+g ?");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Help(_))));
    h
}

fn labels_matching(keys: &Keybinds, search: &str) -> Vec<&'static str> {
    filter(keys.help(), search).into_iter().flat_map(|s| s.rows).map(|r| r.label).collect()
}

// ---- every key is in the help ----------------------------------------------------

#[test]
fn every_action_bound_in_any_scope_is_in_the_help() {
    let keys = Keybinds::default();
    let listed: Vec<Action> = keys.help().into_iter().flat_map(|s| s.rows).flat_map(|r| r.actions).collect();
    let missing: Vec<String> =
        keys.bound_actions().into_iter().filter(|a| !listed.contains(a)).map(|a| a.qualified_name()).collect();
    assert!(missing.is_empty(), "bound, and not in the help: {missing:?}");
    // The check can fail: with an action left out, it is named.
    let mut sections = keys.help();
    sections[0].rows.retain(|r| !r.actions.contains(&Action::RunDirect));
    let listed: Vec<Action> = sections.into_iter().flat_map(|s| s.rows).flat_map(|r| r.actions).collect();
    assert!(!listed.contains(&Action::RunDirect));
}

#[test]
fn every_row_shows_all_the_keys_bound_to_its_actions_and_the_config_name() {
    let keys = Keybinds::default();
    for row in keys.help().into_iter().flat_map(|s| s.rows) {
        if row.actions.is_empty() {
            assert!(row.name.is_empty() && !row.bindings.is_empty(), "{}: an overlay's own keys", row.label);
            continue;
        }
        assert!(!row.name.is_empty(), "{}: names the config key", row.label);
        // Each action's keys are all there (nine tab keys are one `1-9`).
        if !matches!(row.actions[0], Action::GoToTab(_)) {
            let want: Vec<String> = keys.bindings_of(&row.actions).into_iter().map(|b| b.text).collect();
            let got: Vec<String> = row.bindings.iter().map(|b| b.text.clone()).collect();
            assert_eq!(got, want, "{}", row.label);
        }
    }
}

#[test]
fn an_action_is_named_by_its_scope_and_the_name_the_config_file_will_use() {
    assert_eq!(Action::RunDirect.qualified_name(), "direct.run");
    assert_eq!(Action::Run.qualified_name(), "prefix.run");
    assert_eq!(Action::Duplicate.qualified_name(), "editor.duplicate");
    assert_eq!(Action::OpenTable.qualified_name(), "sidebar.open_table");
    assert_eq!(Action::CopyCell.qualified_name(), "results.copy_cell");
    assert_eq!(Action::GoToTab(3).qualified_name(), "prefix.go_to_tab_3");
    assert_eq!(Action::FinishStatementDirect.qualified_name(), "direct.finish_statement");
    assert_eq!(Action::NavigatorDirect.qualified_name(), "direct.navigator");
    assert_eq!(Action::CompleteInsert.qualified_name(), "complete.complete_insert");
    // No two actions of one scope share a name: the config file could not tell them apart.
    let keys = Keybinds::default();
    let mut seen: Vec<String> = Vec::new();
    for a in keys.bound_actions() {
        let name = a.qualified_name();
        assert!(!seen.contains(&name), "{name} names two actions");
        seen.push(name);
    }
}

// ---- the overlay -----------------------------------------------------------------

#[tokio::test]
async fn the_help_shows_the_config_name_next_to_the_keys_and_covers_every_scope() {
    let mut h = helping().await;
    let mut all = String::new();
    for _ in 0..30 {
        all.push_str(&screen(&mut h));
        all.push('\n');
        h.press("pagedown");
    }
    let row = all.lines().find(|l| l.contains("editor.duplicate")).expect("the name is shown");
    assert!(row.contains("cmd+d") && row.contains("ctrl+g d"), "{row}");
    for title in [
        "Sidebar",
        "Results",
        "Resize mode",
        "Completion popup",
        "Template mode",
        "Go to overlay",
        "History overlay",
        "Export overlay",
        "Dump overlay",
        "Confirmations",
    ] {
        assert!(all.contains(title), "{title} is a section of the help");
    }
    // The navigator's own keys and the prefix and direct keys that open it.
    assert!(all.contains("ctrl+o") && all.contains("ctrl+g p") && all.contains("cmd+o"), "{all}");
}

#[tokio::test]
async fn searching_the_help_for_copy_lists_copy_cell_copy_row_copy_and_copy_as() {
    let keys = Keybinds::default();
    let mut found = labels_matching(&keys, "copy");
    found.sort();
    assert_eq!(found, ["copy", "copy as", "copy cell", "copy row"]);
    // Typed in the overlay, the list is those four and nothing else.
    let mut h = helping().await;
    h.type_text("copy");
    let shown = screen(&mut h);
    for label in ["copy cell", "copy row", "copy as"] {
        assert!(shown.contains(label), "{label}:\n{shown}");
    }
    assert!(shown.lines().any(|l| l.contains("editor.copy") && l.contains("cmd+c")), "{shown}");
    assert!(!shown.contains("duplicate") && !shown.contains("export to a file"), "{shown}");
}

#[tokio::test]
async fn searching_by_key_or_by_name_narrows_the_list() {
    let keys = Keybinds::default();
    let by_key = labels_matching(&keys, "ctrl+g t");
    assert!(by_key.contains(&"new tab"), "{by_key:?}");
    assert!(!by_key.contains(&"copy cell"));
    let by_name = labels_matching(&keys, "open_table");
    assert_eq!(by_name, ["open table"]);
    // Several words must all be found, in any of the label, the name and the keys.
    assert_eq!(labels_matching(&keys, "sidebar dump"), ["dump database or schema"]);
    assert!(labels_matching(&keys, "no such action").is_empty());
    let mut h = helping().await;
    h.type_text("zzzz");
    let shown = screen(&mut h);
    assert!(shown.contains("No key, name or action matches \"zzzz\""), "{shown}");
}

#[tokio::test]
async fn letters_go_to_the_search_the_arrows_scroll_and_the_list_keeps_its_place_on_a_new_search() {
    let mut h = helping().await;
    h.type_text("jk");
    let Some(Overlay::Help(help)) = &h.app.ui.overlay else { panic!() };
    assert_eq!(help.search.text(), "jk", "typed, not scrolled");
    assert_eq!(help.scroll, 0);
    h.press("backspace backspace");
    h.press("pagedown");
    let Some(Overlay::Help(help)) = &h.app.ui.overlay else { panic!() };
    assert!(help.scroll > 0, "the page key scrolls");
    h.type_text("r");
    let Some(Overlay::Help(help)) = &h.app.ui.overlay else { panic!() };
    assert_eq!(help.scroll, 0, "a new search starts at the top");
    h.press("esc");
    let Some(Overlay::Help(help)) = &h.app.ui.overlay else { panic!("the first esc only clears the search") };
    assert_eq!(help.search.text(), "");
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(h.app.ui.focus, PaneId::Editor);
}

// ---- review fixes ---------------------------------------------------------------------

#[test]
fn a_query_with_a_space_that_is_a_whole_binding_matches_only_those_rows() {
    let keys = Keybinds::default();
    assert_eq!(labels_matching(&keys, "ctrl+g t"), ["new tab"]);
    // Words that are no one binding still match in any order, across the columns.
    assert_eq!(labels_matching(&keys, "sidebar dump"), ["dump database or schema"]);
}

#[tokio::test]
async fn the_first_esc_clears_a_search_and_the_second_closes() {
    let mut h = helping().await;
    h.type_text("copy");
    h.press("esc");
    let Some(Overlay::Help(help)) = &h.app.ui.overlay else { panic!("closed with text typed") };
    assert_eq!(help.search.text(), "");
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
}

/// Every row's label and every one of its keys are on one screenful or another.
async fn assert_keys_whole(w: u16, rows: u16) {
    let mut h = helping().await;
    let keys = Keybinds::default();
    let mut pages: Vec<String> = Vec::new();
    for _ in 0..80 {
        pages.push(frame_text(&mut h.app, w, rows).join("\n"));
        h.press("pagedown");
    }
    let all = pages.join("\n");
    for row in keys.help().into_iter().flat_map(|s| s.rows) {
        let line =
            all.lines().find(|l| l.contains(row.label) && row.bindings.iter().all(|b| l.contains(b.text.as_str())));
        let continued = row.bindings.iter().all(|b| all.lines().any(|l| l.contains(b.text.as_str())));
        assert!(
            line.is_some() || continued,
            "{w}x{rows}: `{}` is cut: {:?}",
            row.label,
            row.bindings.iter().map(|b| &b.text).collect::<Vec<_>>()
        );
        for b in &row.bindings {
            assert!(all.contains(b.text.as_str()), "{w}x{rows}: key {} of `{}` is not shown", b.text, row.label);
        }
    }
}

#[tokio::test]
async fn at_80x24_and_60x20_no_key_is_cut_off() {
    assert_keys_whole(80, 24).await;
    assert_keys_whole(60, 20).await;
}

#[tokio::test]
async fn a_narrow_help_drops_the_config_name_and_a_wide_one_keeps_it() {
    let mut h = helping().await;
    h.type_text("duplicate");
    let narrow = frame_text(&mut h.app, 80, 24).join("\n");
    assert!(
        !narrow.contains("editor.duplicate") && narrow.contains("ctrl+g d") && narrow.contains("cmd+d"),
        "{narrow}"
    );
    let wide = frame_text(&mut h.app, 120, 30).join("\n");
    assert!(wide.contains("editor.duplicate"), "{wide}");
}
