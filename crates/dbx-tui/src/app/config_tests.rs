//! Layer 2: the acceptance tests of slice 22 (the config file), run on `App`.
//! The file is given as text; where it is read from is the shell's business
//! (`shell/config.rs`, with its own tests).

use ratatui::style::Modifier;

use super::config::{self, Problem};
use super::grid_tests::{focused, text_set, where_is};
use super::keys::{Action, KeyCombo, Keybinds};
use super::overlay::Overlay;
use super::run_tests::{mode_bar, notice_text, on_pagila, opened, runs};
use super::settings::{RunAtCaret, Settings};
use super::testing::*;
use super::theme::{Palette, Role};
use crate::app::format::{CommaPosition, KeywordCase};
use crate::core::DEFAULT_ROW_CAP;
use crate::harness::Harness;

const PATH: &str = "/scratch/config/dbpane/config.toml";

/// Reads `text` as the config file and gives it to the app, as the shell does
/// at startup. Returns the problems.
fn apply(h: &mut Harness, text: &str) -> Vec<Problem> {
    apply_with_env(h, text, None)
}

fn apply_with_env(h: &mut Harness, text: &str, env_theme: Option<&str>) -> Vec<Problem> {
    let loaded = config::parse(text);
    let problems = loaded.problems.clone();
    h.app.apply_config(loaded, Some(PATH.into()), true, env_theme.map(str::to_string));
    problems
}

fn screen(h: &mut Harness) -> String {
    frame_text(&mut h.app, 120, 40).join("\n")
}

fn help_for(h: &mut Harness, search: &str) -> String {
    h.press("ctrl+g ?");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Help(_))));
    h.type_text(search);
    screen(h)
}

fn all_default() -> config::Config {
    config::parse("").config
}

// ---- the acceptance tests ----------------------------------------------------------

#[tokio::test]
async fn a_new_prefix_runs_with_it_the_old_prefix_does_not_and_the_mode_bar_shows_it() {
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[keys]\nprefix = \"ctrl+b\"\n");
    assert!(problems.is_empty(), "{problems:?}");
    h.type_text("select 1");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("ctrl+b") && !bar.contains("ctrl+g"), "{bar}");
    assert_eq!(runs(&h.press_fx("ctrl+b r")).len(), 1, "ctrl+b r runs");
    assert!(runs(&h.press_fx("ctrl+g r")).is_empty(), "ctrl+g is not the prefix any more");
}

#[tokio::test]
async fn a_rebound_editor_key_works_the_old_keys_do_not_and_the_help_shows_the_new_one() {
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[keys.editor]\nduplicate = \"ctrl+shift+d\"\n");
    assert!(problems.is_empty(), "{problems:?}");
    h.app.ui.tab.editor.set_text("select 1");
    h.press("ctrl+shift+d");
    assert_eq!(h.app.ui.tab.editor.lines().len(), 2, "the new key duplicates the line");
    for old in ["cmd+d", "ctrl+d", "ctrl+g d"] {
        h.app.ui.tab.editor.set_text("select 1");
        h.press(old);
        assert_eq!(h.app.ui.tab.editor.lines().len(), 1, "{old} no longer duplicates");
    }
    let shown = help_for(&mut h, "duplicate");
    assert!(shown.contains("ctrl+shift+d"), "{shown}");
    assert!(!shown.contains("cmd+d") && !shown.contains("ctrl+g d"), "{shown}");
}

#[tokio::test]
async fn an_unknown_key_on_line_3_starts_the_client_says_so_and_the_help_lists_it() {
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[theme]\nname = \"catppuccin\"\nfoo = 1\n");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!((problems[0].line, problems[0].column), (3, 1));
    assert!(problems[0].message.contains("foo") && problems[0].message.contains("unknown"), "{}", problems[0].message);
    let notice = notice_text(&h);
    assert!(notice.contains("1 problem") && notice.contains("ctrl+g ?"), "{notice}");
    assert!(h.app.ui.notice.as_ref().is_some_and(|n| n.failure));
    let shown = help_for(&mut h, "");
    assert!(shown.contains("line 3, column 1") && shown.contains("foo"), "{shown}");
    assert!(shown.contains(PATH), "the help says which file it read: {shown}");
}

#[tokio::test]
async fn a_direct_key_taken_from_cancel_is_reported_and_the_users_key_runs() {
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[keys.direct]\nrun = \"ctrl+c\"\n");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].line, 2);
    let said = &problems[0].message;
    assert!(said.contains("ctrl+c") && said.contains("direct.cancel"), "{said}");
    h.type_text("select 1");
    assert_eq!(runs(&h.press_fx("ctrl+c")).len(), 1, "the user's key runs");
    assert_eq!(h.app.ui.keys.all_keys(Action::CancelDirect), ["ctrl+f2", "cmd+f2"], "cancel lost only that key");
    assert_eq!(h.app.ui.keys.all_keys(Action::RunDirect), ["ctrl+c"]);
    let shown = help_for(&mut h, "direct.cancel");
    assert!(shown.contains("ctrl+f2") && shown.contains("line 2"), "{shown}");
}

#[tokio::test]
async fn confirm_dangerous_false_reaches_run_requests_and_confirm_production_is_unknown() {
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[run]\nconfirm_dangerous = false\nconfirm_production = false\nrow_cap = 250\n");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].line, 3);
    assert!(problems[0].message.contains("unknown") && problems[0].message.contains("confirm_production"), "{}", problems[0].message);
    h.type_text("select 1");
    let sent = runs(&h.press_fx("f5"));
    assert!(!sent[0].2.confirm_dangerous, "confirm_dangerous = false goes with the run");
    assert_eq!(sent[0].2.row_cap, 250);
}

#[tokio::test]
async fn in_the_terminal_theme_the_sidebar_cursor_row_and_the_grid_cursor_are_reversed() {
    let mut h = opened().await;
    assert!(apply(&mut h, "[theme]\nname = \"terminal\"\n").is_empty());
    assert_eq!(h.app.ui.theme, Palette::terminal());
    frame_rows(&mut h.app, 114, 30);
    let r = h.app.ui.hits.sidebar;
    let cursor_y = (r.y..r.bottom()).find(|&y| role_at(&mut h.app, r.x + 1, y).bg == Some(Role::SelectionBg)).expect("a cursor row");
    let reversed = |h: &mut Harness, x, y| style_at(&mut h.app, x, y).2.contains(Modifier::REVERSED);
    assert!(reversed(&mut h, r.x + 1, cursor_y), "the cursor row is reversed");
    assert!(reversed(&mut h, r.right() - 3, cursor_y), "all of it");
    assert!(!reversed(&mut h, r.x + 1, cursor_y + 1), "the next row is not");

    let mut h = focused(text_set(3, 3)).await;
    apply(&mut h, "[theme]\nname = \"terminal\"\n");
    let (x, y) = where_is(&mut h, "r0c0").expect("the first cell");
    assert!(reversed(&mut h, x, y), "the grid cursor cell is reversed");
    let (x1, y1) = where_is(&mut h, "r0c1").expect("its neighbour");
    assert!(!reversed(&mut h, x1, y1), "its neighbour is not");
    let (x2, y2) = where_is(&mut h, "r1c0").expect("the row below");
    assert!(!reversed(&mut h, x2, y2), "nor is the row below");
}

#[test]
fn an_unclosed_string_leaves_everything_default_with_one_problem_on_its_line() {
    let loaded = config::parse("[run]\nrow_cap = 5\n[theme]\nname = \"terminal\n[keys]\nprefix = \"ctrl+b\"\n");
    assert_eq!(loaded.config, all_default(), "nothing from the file is used");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert_eq!(loaded.problems[0].line, 4, "{:?}", loaded.problems);
}

#[test]
fn the_config_command_output_read_back_gives_exactly_the_defaults() {
    let text = config::default_file();
    let loaded = config::parse(&text);
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(loaded.config.keys, Keybinds::default());
    assert_eq!(loaded.config.settings, Settings::default());
    assert_eq!(loaded.config.row_cap, DEFAULT_ROW_CAP);
    assert_eq!(Palette::from_setting(loaded.config.theme.as_deref()), Palette::default_theme());
    // It is a commented file listing every action with its keys.
    assert!(text.lines().filter(|l| l.trim_start().starts_with('#')).count() > 20, "commented");
    let defaults = Keybinds::default();
    for action in defaults.bound_actions() {
        let line = format!("{} = ", action.config_name());
        assert!(text.contains(&line), "{} is in the file", action.qualified_name());
    }
    // The file can fail: change one default and it no longer reads back as the defaults.
    let changed = text.replacen("prefix = \"ctrl+g\"", "prefix = \"ctrl+b\"", 1);
    assert!(changed != text, "the prefix is printed as it is");
    assert!(config::parse(&changed).config.keys != Keybinds::default());
}

// ---- what the file can set -------------------------------------------------------------

#[test]
fn the_editor_section_sets_every_editor_setting() {
    let loaded = config::parse(
        "[editor]\nalias_on_table = false\nauto_pair = false\nsmart_enter = false\nsmart_home = false\n\
         run_at_caret = \"smallest\"\nhard_tabs = true\ntab_width = 2\nformat_keyword_case = \"upper\"\n\
         format_indent = 3\nformat_comma = \"leading\"\n",
    );
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    let s = loaded.config.settings;
    assert!(!s.alias_on_table && !s.auto_pair && !s.smart_enter && !s.smart_home && s.hard_tabs);
    assert_eq!((s.run_at_caret, s.tab_width, s.format_indent), (RunAtCaret::Smallest, 2, 3));
    assert_eq!((s.format_keyword_case, s.format_comma), (KeywordCase::Upper, CommaPosition::Leading));
    for (value, want) in [("ask", RunAtCaret::Ask), ("statement", RunAtCaret::Statement)] {
        let s = config::parse(&format!("[editor]\nrun_at_caret = \"{value}\"\n")).config.settings;
        assert_eq!(s.run_at_caret, want);
    }
}

#[test]
fn a_wrong_value_keeps_that_settings_default_names_the_choices_and_spares_the_rest() {
    let loaded = config::parse(
        "[editor]\nrun_at_caret = \"sometimes\"\ntab_width = 0\nhard_tabs = \"yes\"\nauto_pair = false\n[run]\nrow_cap = \"lots\"\n",
    );
    let lines: Vec<usize> = loaded.problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, [2, 3, 4, 7], "{:?}", loaded.problems);
    assert!(loaded.problems[0].message.contains("ask") && loaded.problems[0].message.contains("smallest"), "{}", loaded.problems[0].message);
    let s = loaded.config.settings;
    assert_eq!((s.run_at_caret, s.tab_width, s.hard_tabs), (RunAtCaret::Ask, 4, false));
    assert!(!s.auto_pair, "the valid one in the same section still applies");
    assert_eq!(loaded.config.row_cap, DEFAULT_ROW_CAP);
    for bad in ["0", "100001", "-5", "2.5"] {
        let loaded = config::parse(&format!("[run]\nrow_cap = {bad}\n"));
        assert_eq!(loaded.problems.len(), 1, "{bad}: {:?}", loaded.problems);
        assert_eq!(loaded.config.row_cap, DEFAULT_ROW_CAP, "{bad}");
    }
    assert_eq!(config::parse("[run]\nrow_cap = 100000\n").config.row_cap, 100_000);
}

#[test]
fn an_unknown_section_scope_or_action_is_a_problem_on_its_line() {
    let loaded = config::parse("top = 1\n[nope]\nx = 1\n[keys.nowhere]\na = \"b\"\n[keys.editor]\nfrobnicate = \"ctrl+b\"\n[keys]\nother = \"x\"\n");
    let at: Vec<usize> = loaded.problems.iter().map(|p| p.line).collect();
    assert_eq!(at, [1, 2, 4, 7, 9], "{:#?}", loaded.problems);
    assert!(loaded.problems[3].message.contains("frobnicate") && loaded.problems[3].message.contains("editor"), "{}", loaded.problems[3].message);
    assert_eq!(loaded.config, all_default());
}

#[test]
fn a_key_that_does_not_parse_or_has_the_wrong_type_keeps_the_default_for_that_entry_only() {
    let loaded = config::parse(
        "[keys.editor]\nduplicate = \"ctrl+shift+\"\ndelete_line = 5\nundo = [\"ctrl+j\", \"hyper+x\"]\nredo = \"ctrl+k\"\n",
    );
    let at: Vec<usize> = loaded.problems.iter().map(|p| p.line).collect();
    assert_eq!(at, [2, 3, 4], "{:?}", loaded.problems);
    assert!(loaded.problems[0].message.contains("ctrl+shift+"), "{}", loaded.problems[0].message);
    let keys = &loaded.config.keys;
    let default = Keybinds::default();
    for a in [Action::Duplicate, Action::DeleteLine, Action::Undo] {
        assert_eq!(keys.all_keys(a), default.all_keys(a), "{a:?} keeps its default");
    }
    assert_eq!(keys.all_keys(Action::Redo), ["ctrl+k"], "the entry that was fine applies");
}

#[test]
fn invalid_toml_is_one_problem_with_its_line_and_column_and_everything_default() {
    let loaded = config::parse("[run]\nrow_cap = 5\n[keys\nprefix = \"ctrl+b\"\n");
    assert_eq!(loaded.config, all_default());
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert_eq!(loaded.problems[0].line, 3, "{:?}", loaded.problems);
    assert!(loaded.problems[0].column >= 1);
}

#[test]
fn an_empty_file_and_a_file_of_comments_are_the_defaults_with_no_problem() {
    for text in ["", "\n\n", "# nothing here\n"] {
        let loaded = config::parse(text);
        assert_eq!(loaded.config, all_default());
        assert!(loaded.problems.is_empty());
    }
}

#[test]
fn the_theme_is_named_and_an_unknown_name_lists_the_themes() {
    assert_eq!(config::parse("[theme]\nname = \"intellij-dark\"\n").config.theme.as_deref(), Some("intellij-dark"));
    let loaded = config::parse("[theme]\nname = \"neon\"\n");
    assert_eq!(loaded.problems.len(), 1);
    assert!(loaded.problems[0].message.contains("neon") && loaded.problems[0].message.contains("terminal"), "{}", loaded.problems[0].message);
    assert_eq!(loaded.config.theme, None);
}

#[tokio::test]
async fn the_environment_theme_wins_over_the_file_and_a_bad_one_does_not() {
    let mut h = on_pagila().await;
    apply_with_env(&mut h, "[theme]\nname = \"terminal\"\n", Some("intellij-dark"));
    assert_eq!(h.app.ui.theme, Palette::intellij_dark());
    apply_with_env(&mut h, "[theme]\nname = \"terminal\"\n", Some("nonsense"));
    assert_eq!(h.app.ui.theme, Palette::terminal(), "an unknown name in the environment is ignored");
    apply_with_env(&mut h, "", None);
    assert_eq!(h.app.ui.theme, Palette::default_theme());
}

// ---- keys: the rules --------------------------------------------------------------------

#[test]
fn a_list_sets_every_key_of_the_action_and_prefix_keys_are_written_with_prefix_plus() {
    let keys = config::parse("[keys.editor]\nduplicate = [\"ctrl+shift+d\", \"prefix+D\"]\n[keys.after_prefix]\nquit = [\"x\", \"prefix+X\"]\n").config.keys;
    assert_eq!(keys.all_keys(Action::Duplicate), ["ctrl+shift+d", "ctrl+g D"]);
    assert_eq!(keys.lookup(&KeyCombo::parse("x").unwrap()), Some(Action::Quit));
    assert_eq!(keys.lookup(&KeyCombo::parse("X").unwrap()), Some(Action::Quit), "prefix+ is accepted after the prefix too");
    assert_eq!(keys.lookup(&KeyCombo::parse("q").unwrap()), None, "q no longer quits");
    let none = config::parse("[keys.sidebar]\nrefresh = \"prefix+r\"\n");
    assert_eq!(none.problems.len(), 1, "{:?}", none.problems);
    assert!(none.problems[0].message.contains("prefix"), "{}", none.problems[0].message);
}

#[test]
fn an_empty_list_unbinds_and_the_help_says_unbound() {
    let loaded = config::parse("[keys.editor]\nduplicate = []\n");
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert!(loaded.config.keys.all_keys(Action::Duplicate).is_empty());
    let help = loaded.config.keys.help();
    let row = help.iter().flat_map(|s| &s.rows).find(|r| r.name == "editor.duplicate").expect("the row stays");
    assert!(row.bindings.is_empty());
}

#[tokio::test]
async fn the_help_shows_unbound_for_an_action_with_no_key() {
    let mut h = on_pagila().await;
    apply(&mut h, "[keys.editor]\nduplicate = []\n");
    let shown = help_for(&mut h, "editor.duplicate");
    assert!(shown.contains("unbound"), "{shown}");
}

#[test]
fn a_key_that_is_typing_cannot_be_a_direct_editor_or_prefix_key() {
    for text in ["[keys.direct]\nrun = \"x\"\n", "[keys.editor]\nundo = \"Z\"\n", "[keys]\nprefix = \"x\"\n", "[keys.complete]\ncomplete_up = \"?\"\n"] {
        let loaded = config::parse(text);
        assert_eq!(loaded.problems.len(), 1, "{text}: {:?}", loaded.problems);
        assert!(loaded.problems[0].message.contains("typing") || loaded.problems[0].message.contains("type"), "{}", loaded.problems[0].message);
        assert_eq!(loaded.config.keys, Keybinds::default(), "{text}");
    }
    // Panes that take no text may use plain letters.
    let loaded = config::parse("[keys.sidebar]\nrefresh = \"x\"\n[keys.results]\ncopy_cell = \"Z\"\n[keys.after_prefix]\nrun = \"v\"\n");
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(loaded.config.keys.lookup_sidebar(&KeyCombo::parse("x").unwrap()), Some(Action::Refresh));
    assert_eq!(loaded.config.keys.lookup_sidebar(&KeyCombo::parse("r").unwrap()), None);
}

#[test]
fn two_of_the_users_own_keys_on_one_key_keep_the_first_and_reject_the_second() {
    let loaded = config::parse("[keys.direct]\nrun = \"ctrl+k\"\nhistory = \"ctrl+k\"\n");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert_eq!(loaded.problems[0].line, 3);
    assert!(loaded.problems[0].message.contains("line 2"), "{}", loaded.problems[0].message);
    let default = Keybinds::default();
    assert_eq!(loaded.config.keys.all_keys(Action::RunDirect), ["ctrl+k"]);
    assert_eq!(loaded.config.keys.all_keys(Action::HistoryDirect), default.all_keys(Action::HistoryDirect));
}

#[test]
fn a_key_that_is_the_prefix_cannot_be_given_to_an_action_and_a_new_prefix_takes_its_key() {
    let loaded = config::parse("[keys.direct]\nhistory = \"ctrl+g\"\n");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert!(loaded.problems[0].message.contains("prefix"), "{}", loaded.problems[0].message);
    assert_eq!(loaded.config.keys, Keybinds::default());
    // The prefix is read first whatever its line, and takes `ctrl+u` from the sidebar.
    let loaded = config::parse("[keys.direct]\nhistory = \"ctrl+g\"\n[keys]\nprefix = \"ctrl+u\"\n");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert!(loaded.problems[0].message.contains("sidebar.page_up"), "the prefix took ctrl+u from the sidebar: {}", loaded.problems[0].message);
    assert_eq!(loaded.config.keys.prefix, KeyCombo::parse("ctrl+u").unwrap());
    assert_eq!(loaded.config.keys.all_keys(Action::HistoryDirect), ["ctrl+g"].map(String::from), "ctrl+g is free now");
    assert_eq!(loaded.config.keys.lookup_sidebar(&KeyCombo::parse("ctrl+u").unwrap()), None, "the prefix took it");
    assert_eq!(loaded.config.keys.all_keys(Action::PageUp), ["pageup"]);
}

#[tokio::test]
async fn esc_leaves_the_resize_mode_whatever_the_bindings_say() {
    use crate::app::Mode;
    let mut h = on_pagila().await;
    let problems = apply(&mut h, "[keys.resize]\nresize_done = \"ctrl+shift+f12\"\n");
    assert!(problems.is_empty(), "{problems:?}");
    h.press("ctrl+g R");
    assert_eq!(h.app.ui.mode, Mode::Resize);
    h.press("esc");
    assert_eq!(h.app.ui.mode, Mode::Normal, "esc always gets out");
}

#[test]
fn quit_help_and_leaving_the_resize_mode_cannot_be_unbound() {
    for action in [Action::ResizeDone, Action::Quit, Action::Help] {
        let n = action.qualified_name();
        let (scope, short) = n.split_once('.').unwrap();
        let text = format!("[keys.{scope}]\n{short} = []\n");
        let loaded = config::parse(&text);
        assert_eq!(loaded.problems.len(), 1, "{text}: {:?}", loaded.problems);
        assert!(loaded.problems[0].message.contains("cannot be unbound"), "{}", loaded.problems[0].message);
        assert_eq!(loaded.config.keys.all_keys(action), Keybinds::default().all_keys(action), "{text}: the default stays");
    }
}

#[test]
fn keys_the_text_reads_cannot_be_given_unmodified_and_the_prefix_cannot_be_esc_or_enter() {
    for key in ["enter", "tab", "backspace", "delete", "left", "up", "home", "end", "shift+left", "shift+home", "shift+end"] {
        let text = format!("[keys.direct]\nrun = \"{key}\"\n");
        let loaded = config::parse(&text);
        assert_eq!(loaded.problems.len(), 1, "{key}: {:?}", loaded.problems);
        assert_eq!(loaded.config.keys, Keybinds::default(), "{key}");
    }
    for key in ["esc", "enter", "tab", "backspace", "left", "home"] {
        let loaded = config::parse(&format!("[keys]\nprefix = \"{key}\"\n"));
        assert_eq!(loaded.problems.len(), 1, "{key}: {:?}", loaded.problems);
        assert_eq!(loaded.config.keys, Keybinds::default(), "{key}");
    }
    let ok = config::parse("[keys]\nprefix = \"f9\"\n[keys.results]\ncopy_cell = \"delete\"\n[keys.sidebar]\nrefresh = \"x\"\n");
    assert!(ok.problems.is_empty(), "{:?}", ok.problems);
}

#[test]
fn key_names_are_read_in_any_case_but_a_letter_keeps_its_case() {
    for (written, want) in [("Ctrl+g", "ctrl+g"), ("CMD+Enter", "cmd+enter"), ("Space", "space"), ("ctrl+ESC", "ctrl+esc"), ("Alt+F5", "alt+f5")] {
        assert_eq!(KeyCombo::parse(written), KeyCombo::parse(want), "{written}");
        assert!(KeyCombo::parse(written).is_some(), "{written}");
    }
    assert_ne!(KeyCombo::parse("ctrl+T"), KeyCombo::parse("ctrl+t"));
    assert_ne!(KeyCombo::parse("T"), KeyCombo::parse("t"));
}

#[tokio::test]
async fn a_theme_or_key_name_with_control_characters_is_drawn_replaced_in_the_help() {
    let mut h = on_pagila().await;
    apply(&mut h, "[theme]\nname = \"ev\\u202eil\\u001b[31m\"\n[keys.direct]\nrun = \"ctrl+\\u202e\\u0007x\"\n");
    h.press("ctrl+g ?");
    let shown = screen(&mut h);
    assert!(shown.contains("Config problems"), "{shown}");
    assert!(shown.contains("ev") && shown.contains("il"), "the name is still shown: {shown}");
    assert!(!shown.chars().any(|c| c.is_control() && c != '\n' || ('\u{202a}'..='\u{202e}').contains(&c)), "{shown:?}");
}

#[test]
fn the_defaults_collide_nowhere() {
    assert_eq!(Keybinds::default().conflicts(), Vec::new());
}

#[test]
fn a_key_given_to_an_action_in_a_scope_read_at_the_same_time_is_taken_from_the_other_action() {
    // The sidebar's `ctrl+d` (page down) and a direct `ctrl+d`: both are read
    // while the sidebar has focus.
    let loaded = config::parse("[keys.direct]\nhistory = \"ctrl+d\"\n");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert!(loaded.problems[0].message.contains("sidebar.page_down"), "{}", loaded.problems[0].message);
    assert_eq!(loaded.config.keys.all_keys(Action::PageDown), ["pagedown"]);
    assert_eq!(loaded.config.keys.conflicts(), Vec::new(), "what is left collides nowhere");
    // Scopes never read together do not collide: `tab` is the editor's indent
    // and the popup's replace.
    assert!(config::parse("[keys.complete]\ncomplete_replace = \"tab\"\n").problems.is_empty());
    assert!(config::parse("[keys.sidebar]\nrefresh = \"f4\"\n").problems.len() == 1, "f4 is the sidebar's open_table");
}

#[tokio::test]
async fn with_every_action_unbound_the_screen_still_draws_and_keys_do_nothing_harmful() {
    const KEPT: [Action; 3] = [Action::Help, Action::Quit, Action::ResizeDone];
    let mut text = String::new();
    let defaults = Keybinds::default();
    for scope in ["after_prefix", "direct", "sidebar", "results", "resize", "complete", "template", "editor"] {
        text.push_str(&format!("[keys.{scope}]\n"));
        for a in defaults.bound_actions() {
            // The ways out and the help stay: they cannot be unbound.
            if a.qualified_name().starts_with(&format!("{scope}.")) && !KEPT.contains(&a) {
                text.push_str(&format!("{} = []\n", a.config_name()));
            }
        }
    }
    let mut h = on_pagila().await;
    let problems = apply(&mut h, &text);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(Keybinds::default().bound_actions().into_iter().filter(|a| !KEPT.contains(a)).all(|a| h.app.ui.keys.all_keys(a).is_empty()));
    screen(&mut h);
    h.press("ctrl+g");
    screen(&mut h);
    h.press("esc");
    h.type_text("select 1");
    h.press("f5 ctrl+g ? esc");
    for focus in ["ctrl+g h", "ctrl+g j", "enter j k"] {
        h.press(focus);
        screen(&mut h);
    }
    let shown = help_for(&mut h, "run");
    assert!(shown.contains("unbound"), "{shown}");
}

#[tokio::test]
async fn the_notice_says_how_many_problems_and_how_to_read_them() {
    let mut h = on_pagila().await;
    apply(&mut h, "a = 1\nb = 2\n[run]\nrow_cap = 0\n");
    let notice = notice_text(&h);
    assert!(notice.contains("3 problems") && notice.contains("ctrl+g ?"), "{notice}");
    let shown = help_for(&mut h, "");
    for needle in ["line 1", "line 2", "line 4", "row_cap"] {
        assert!(shown.contains(needle), "{needle}: {shown}");
    }
    // A clean file says nothing.
    let mut h = on_pagila().await;
    h.app.ui.notice = None;
    apply(&mut h, "[run]\nrow_cap = 10\n");
    assert!(h.app.ui.notice.is_none());
    let shown = help_for(&mut h, "");
    assert!(shown.contains(PATH) && !shown.contains("problem"), "{shown}");
}

#[tokio::test]
async fn a_search_narrows_the_problems_with_the_keys() {
    let mut h = on_pagila().await;
    apply(&mut h, "alpha = 1\nbeta = 2\n");
    let shown = help_for(&mut h, "beta");
    assert!(shown.contains("line 2, column 1") && !shown.contains("line 1, column 1"), "{shown}");
}
