//! Layer 2: the acceptance tests of slice 05 (run a statement), run on `App`
//! with a `FakeCore` behind the shell's command runner.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::connections_tests::conn;
use super::query::{Outcome, RunState};
use super::requests::RequestId;
use super::testing::*;
use super::theme;
use super::view::{NEWLINE_MARK, NULL_MARK};
use super::*;
use crate::core::{
    Call, Cell, CellKind, Column, ConnectionId, CoreErrorKind, Cursor, DatabasePath, DbKind, FakeCore, ResultSet, RunId,
    Scripted, SessionId, SqlPosition, TabId,
};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

/// A started harness over the Postgres connections `names`.
pub(super) async fn started(connections: &[&str]) -> Harness {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(connections.iter().map(|n| conn(n, DbKind::Postgres)).collect()));
    let mut h = Harness::start(fake);
    h.settle().await;
    h
}

pub(super) fn cid(name: &str) -> ConnectionId {
    ConnectionId(format!("id-{name}"))
}

/// The tab's first session.
pub(super) fn first_session() -> SessionId {
    SessionId { tab: TabId(1), serial: 1 }
}

pub(super) fn dbp(c: &str, db: &str) -> DatabasePath {
    DatabasePath { connection: cid(c), database: db.into() }
}

/// `local pg` opened in the sidebar, showing databases `edge` and `pagila`,
/// with the cursor on the connection.
pub(super) async fn opened() -> Harness {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h
}

/// `opened`, with `pagila` given to the tab (`j j u`) and the editor focused.
pub(super) async fn on_pagila() -> Harness {
    let mut h = opened().await;
    h.press("j j u ctrl+g l");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    h
}

/// The run commands among `commands`.
pub(super) fn runs(commands: &[Command]) -> Vec<(RequestId, RunId, crate::core::RunRequest)> {
    commands
        .iter()
        .filter_map(|c| match c {
            Command::Core(id, CoreCall::Run(run, request)) => Some((*id, *run, request.clone())),
            _ => None,
        })
        .collect()
}

/// The editor's title row as drawn.
pub(super) fn title(h: &mut Harness) -> String {
    frame_rows(&mut h.app, 114, 30).into_iter().find(|r| r.contains("editor")).unwrap_or_default()
}

pub(super) fn mode_bar(h: &mut Harness) -> String {
    frame_rows(&mut h.app, 114, 30).pop().unwrap()
}

pub(super) fn notice_text(h: &Harness) -> String {
    h.app.ui.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default()
}

pub(super) fn key(h: &mut Harness, code: KeyCode, mods: KeyModifiers) -> Vec<Command> {
    let fx = h.app.update(Event::Key(KeyEvent::new(code, mods)), base());
    let commands = fx.commands.clone();
    h.run(fx);
    commands
}

// ---- the editor ---------------------------------------------------------------

#[tokio::test]
async fn typing_in_the_focused_editor_inserts_text_and_no_sidebar_key_fires() {
    let mut h = started(&["local pg", "other"]).await;
    h.press("ctrl+g l");
    assert_eq!(h.app.ui.focus, PaneId::Editor);
    let cursor_before = h.app.ui.sidebar.cursor;
    // `e`, `d`, `r`, `u`, `j`, `G` are sidebar keys; here they are letters.
    let mut commands = Vec::new();
    for c in "select e, d, r, u, j\nfrom G".chars() {
        let code = if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) };
        let mods = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        commands.extend(key(&mut h, code, mods));
    }
    assert_eq!(h.app.ui.editor.text(), "select e, d, r, u, j\nfrom G");
    assert!(commands.is_empty(), "{commands:?}");
    assert!(h.app.ui.overlay.is_none(), "`e` did not open the edit form");
    assert_eq!(h.app.ui.sidebar.cursor, cursor_before);
}

#[tokio::test]
async fn the_prefix_and_the_run_keys_never_reach_the_editor() {
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    h.type_text("x");
    h.press("ctrl+g");
    assert!(matches!(h.app.ui.mode, Mode::Prefix { .. }), "ctrl+g is the prefix, not the widget's");
    h.press("esc");
    h.press("f5 alt+enter");
    assert_eq!(h.app.ui.editor.text(), "x", "no newline from alt+enter, nothing from f5");
}

#[tokio::test]
async fn ctrl_c_is_taken_before_the_editor_so_its_copy_never_runs() {
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    h.type_text("select 1");
    h.press("shift+left shift+left");
    let selected = h.app.ui.editor.selection();
    assert!(selected.is_some());
    h.press("ctrl+c");
    // The widget's copy would have ended the selection.
    assert_eq!(h.app.ui.editor.selection(), selected);
}

#[tokio::test]
async fn a_paste_into_the_editor_turns_every_line_ending_into_one_newline() {
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    let fx = h.app.update(Event::Paste("select 1\r\nfrom a\rwhere b\n;\u{1b}[31m".into()), base());
    assert!(fx.repaint);
    assert_eq!(h.app.ui.editor.text(), "select 1\nfrom a\nwhere b\n;\u{fffd}[31m", "control characters are replaced");
}

#[tokio::test]
async fn a_paste_while_the_sidebar_has_focus_does_not_reach_the_editor() {
    let mut h = started(&["local pg"]).await;
    h.app.update(Event::Paste("select 1".into()), base());
    assert_eq!(h.app.ui.editor.text(), "");
}

// ---- the tab's target -----------------------------------------------------------

#[tokio::test]
async fn with_no_database_given_to_the_tab_run_shows_a_notice_and_emits_no_command() {
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    h.type_text("select 1");
    for keys in ["f5", "alt+enter", "ctrl+g r"] {
        h.app.ui.notice = None;
        let commands = h.press_fx(keys);
        assert!(commands.is_empty(), "{keys}: {commands:?}");
        let said = notice_text(&h);
        assert!(said.contains("u"), "{keys}: the notice names the key that gives the tab a database: {said:?}");
    }
    let screen = frame_rows(&mut h.app, 114, 30).join("\n");
    assert!(screen.contains(&notice_text(&h)[..20]), "the notice is drawn: {screen}");
}

#[tokio::test]
async fn u_on_a_database_gives_it_to_the_tab_and_the_editor_title_names_it() {
    let mut h = opened().await;
    assert!(!title(&mut h).contains("local pg"), "no target yet: {}", title(&mut h));
    let commands = h.press_fx("j j u");
    assert!(commands.is_empty(), "nothing to close yet: {commands:?}");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    assert!(title(&mut h).contains("local pg / pagila"), "{}", title(&mut h));
}

#[tokio::test]
async fn moving_the_sidebar_cursor_after_the_tab_has_its_database_does_not_change_where_the_next_run_goes() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    h.press("ctrl+g h k"); // the cursor onto `edge`
    assert!(title(&mut h).contains("local pg / pagila"), "{}", title(&mut h));
    let sent = runs(&h.press_fx("f5"));
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].2.connection.clone(), sent[0].2.database.clone()), (cid("local pg"), Some("pagila".to_string())));
}

#[tokio::test]
async fn u_on_a_connection_uses_its_saved_database_and_without_one_says_to_pick_a_database() {
    let fake = FakeCore::new();
    let mut saved = conn("saved", DbKind::Postgres);
    saved.database = Some("pagila".into());
    fake.script_connections(Scripted::ok(vec![conn("bare", DbKind::Postgres), saved]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("u");
    assert_eq!(h.app.model.tabs[0].target, None, "`bare` has no database of its own");
    assert!(h.app.ui.note.is_some() || h.app.ui.notice.is_some(), "and says so");
    h.press("j u");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("saved", "pagila")));
}

// ---- running --------------------------------------------------------------------

#[tokio::test]
async fn run_emits_exactly_one_run_command_with_the_buffer_and_cursor_and_with_a_selection_the_selection() {
    let mut h = on_pagila().await;
    h.type_text("select 1;\nselect 2;");
    h.press("up");
    let sent = runs(&h.press_fx("f5"));
    assert_eq!(sent.len(), 1, "exactly one");
    let request = &sent[0].2;
    assert_eq!(request.buffer, "select 1;\nselect 2;");
    assert_eq!(request.cursor, Cursor { row: 0, col: 9 });
    assert_eq!(request.selection, None);
    assert_eq!(request.session.tab, TabId(1));
    assert_eq!(request.row_cap, 1_000);

    // The run finishes; then select `select` on the second row and run that.
    let reply = Event::Core(sent[0].0, CoreReply::Ran(Ok(rows_of(&[]))));
    let fx = h.app.update(reply, base());
    h.run(fx);
    h.press("down home shift+right shift+right shift+right shift+right shift+right shift+right");
    let sent = runs(&h.press_fx("ctrl+g r"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2.selection, Some((Cursor { row: 1, col: 0 }, Cursor { row: 1, col: 6 })));
    assert_ne!(sent[0].1, RunId(0));
}

#[tokio::test]
async fn while_a_run_is_in_flight_a_second_run_in_the_tab_is_refused_with_a_notice() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    assert_eq!(runs(&h.press_fx("f5")).len(), 1);
    let again = h.press_fx("f5");
    assert!(runs(&again).is_empty(), "{again:?}");
    assert!(notice_text(&h).contains("running"), "{:?}", notice_text(&h));
}

#[tokio::test]
async fn while_running_the_mode_bar_shows_elapsed_time_that_advances_with_now() {
    let mut h = on_pagila().await;
    h.type_text("select pg_sleep(9)");
    press_at(&mut h.app, "f5", base());
    assert!(matches!(h.app.model.tabs[0].run, RunState::Running { .. }));
    assert!(mode_bar(&mut h).contains("running 0s"), "{}", mode_bar(&mut h));
    // The shell wakes at the next whole second, not on a fixed tick.
    assert_eq!(h.app.next_deadline(), Some(base() + Duration::from_secs(1)));
    let fx = h.app.update(Event::Timer, base() + Duration::from_millis(3_200));
    assert!(fx.repaint);
    assert!(mode_bar(&mut h).contains("running 3s"), "{}", mode_bar(&mut h));
    assert_eq!(h.app.next_deadline(), Some(base() + Duration::from_secs(4)));
    // Typing goes on meanwhile.
    h.type_text(" x");
    assert!(h.app.ui.editor.text().ends_with(" x"));
}

#[tokio::test(start_paused = true)]
async fn the_run_happens_off_the_ui_thread_and_its_rows_arrive_as_a_reply() {
    let mut h = on_pagila().await;
    h.fake.script_run(Scripted::ok(rows_of(&[&["1"]])).after(Duration::from_secs(2)));
    h.type_text("select 1");
    h.press("f5");
    h.settle().await;
    assert!(matches!(h.app.model.tabs[0].run, RunState::Running { .. }), "not back yet");
    h.press("ctrl+g h j"); // navigation is live while it runs
    tokio::time::advance(Duration::from_secs(2)).await;
    h.settle().await;
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    assert!(matches!(&h.app.model.tabs[0].outcome, Some(Outcome::Rows(r)) if r.rows.len() == 1));
    assert!(h.fake.calls().contains(&Call::Run(RunId(1))));
}

#[tokio::test]
async fn giving_the_tab_another_database_mid_run_cancels_the_run_closes_the_old_session_and_drops_its_reply() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    let sent = runs(&h.press_fx("f5"));
    let (request, run) = (sent[0].0, sent[0].1);
    let commands = h.press_fx("ctrl+g h k u"); // `edge`
    assert!(commands.contains(&Command::Cancel(run)), "{commands:?}");
    assert!(commands.contains(&Command::CloseSession(first_session(), dbp("local pg", "pagila"))), "{commands:?}");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "edge")));
    // Until its reply comes, the tab says it may still be running there.
    assert_eq!(h.app.model.tabs[0].abandoned.len(), 1);
    let fx = h.app.update(Event::Core(request, CoreReply::Ran(Ok(rows_of(&[&["1"]])))), base());
    assert!(fx.commands.is_empty(), "stale");
    assert_eq!(h.app.model.tabs[0].outcome, None, "its rows never land");
    assert!(h.app.model.tabs[0].abandoned.is_empty(), "it has ended: the marker goes (the only repaint it causes)");
}

#[tokio::test]
async fn a_newer_run_makes_an_older_reply_stale() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    let first = runs(&h.press_fx("f5"))[0].0;
    // A superseded request (as after a retarget and a new run) is dropped.
    h.app.model.requests.supersede(requests::Target::Run(TabId(1)));
    let fx = h.app.update(Event::Core(first, CoreReply::Ran(Ok(rows_of(&[&["1"]])))), base());
    assert!(!fx.repaint);
    assert_eq!(h.app.model.tabs[0].outcome, None);
}

#[tokio::test]
async fn quitting_closes_the_tabs_session() {
    let mut h = on_pagila().await;
    let commands = h.press_fx("ctrl+g q");
    assert_eq!(commands, vec![Command::CloseSession(first_session(), dbp("local pg", "pagila")), Command::Quit]);
}

#[tokio::test]
async fn quitting_mid_run_cancels_the_run_first() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    let run = runs(&h.press_fx("f5"))[0].1;
    let commands = h.press_fx("ctrl+g q");
    assert_eq!(commands, vec![Command::Cancel(run), Command::CloseSession(first_session(), dbp("local pg", "pagila")), Command::Quit]);
}

#[tokio::test]
async fn collapsing_the_tabs_database_in_the_sidebar_does_not_close_its_session() {
    let mut h = opened().await;
    h.press("j j u");
    let commands = h.press_fx("k h"); // collapse `local pg`
    assert!(!commands.iter().any(|c| matches!(c, Command::CloseSession(..))), "{commands:?}");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
}

#[tokio::test]
async fn removing_the_tabs_connection_takes_its_target_away() {
    let mut h = on_pagila().await;
    h.fake.script_remove_connection(Scripted::ok(()));
    h.press("ctrl+g h g d enter");
    h.settle().await;
    assert_eq!(h.app.model.tabs[0].target, None);
}

// ---- helpers for results ----------------------------------------------------------

pub(super) fn col(name: &str, db_type: &str, kind: CellKind) -> Column {
    Column { name: name.into(), db_type: db_type.into(), kind }
}

/// A one-column text result with these rows.
pub(super) fn rows_of(rows: &[&[&str]]) -> ResultSet {
    ResultSet {
        columns: vec![col("v", "text", CellKind::Text)],
        rows: rows.iter().map(|r| r.iter().map(|v| Cell::Value(v.to_string())).collect()).collect(),
        truncated: false,
        elapsed: Duration::from_millis(12),
        messages: vec![],
        affected: 0,
        ran: "select v".into(),
        rerun: crate::core::Rerun::Proven,
    }
}

/// Runs `select ...` on pagila and answers it with `reply`.
pub(super) async fn ran(reply: Result<ResultSet, crate::core::CoreError>) -> Harness {
    let mut h = on_pagila().await;
    h.type_text("select ...");
    let sent = runs(&h.press_fx("f5"));
    let fx = h.app.update(Event::Core(sent[0].0, CoreReply::Ran(reply)), base() + Duration::from_millis(40));
    h.run(fx);
    h
}

/// The results pane's rows as text (below its title row), trailing blanks cut.
pub(super) fn results_rows(h: &mut Harness) -> Vec<String> {
    let screen = frame_rows(&mut h.app, 114, 30);
    let r = h.app.ui.hits.results;
    screen[(r.y + 1) as usize..r.bottom() as usize]
        .iter()
        .map(|row| row.chars().skip(r.x as usize).take(r.width as usize).collect::<String>().trim_end().to_string())
        .collect()
}

/// Where `text` is in the results pane: screen column and row.
pub(super) fn in_results(h: &mut Harness, text: &str) -> Option<(u16, u16)> {
    let screen = frame_rows(&mut h.app, 114, 30);
    let r = h.app.ui.hits.results;
    (r.y + 1..r.bottom()).find_map(|y| {
        let row: String = screen[y as usize].chars().skip(r.x as usize).take(r.width as usize).collect();
        let byte = row.find(text)?;
        Some((r.x + row[..byte].chars().count() as u16, y))
    })
}

// ---- results ----------------------------------------------------------------------

#[tokio::test]
async fn integers_are_right_aligned_and_null_the_empty_string_and_the_text_null_each_look_different() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("id", "int4", CellKind::Integer), col("label", "text", CellKind::Text), col("v", "text", CellKind::Text)],
        rows: vec![
            vec![v("1"), v("null"), Cell::Null],
            vec![v("22"), v("empty"), v("")],
            vec![v("333"), v("text"), v("NULL")],
        ],
        truncated: false,
        elapsed: Duration::from_millis(12),
        messages: vec![],
        affected: 0,
        ran: "select ...".into(),
        rerun: crate::core::Rerun::Proven,
    };
    let mut h = ran(Ok(set)).await;
    let rows = results_rows(&mut h);
    let header = rows.iter().position(|r| r.contains("label")).expect("a header row");
    assert!(rows[header].contains("id") && rows[header].contains('v'), "{rows:#?}");
    // Right-aligned: the last digit of each id is in the same column.
    // (Each id is looked up with the label after it: the row-number gutter
    // on the left holds digits too.)
    let ends: Vec<u16> = ["1  null", "22  empty", "333  text"].iter().map(|t| {
        let (x, _) = in_results(&mut h, t).unwrap_or_else(|| panic!("{t:?} in {rows:#?}"));
        x + t.split_whitespace().next().unwrap().len() as u16
    }).collect();
    assert!(ends.windows(2).all(|w| w[0] == w[1]), "ids end at {ends:?}: {rows:#?}");
    // NULL is a dimmed glyph; the text NULL is text; the empty string is nothing.
    let (nx, ny) = in_results(&mut h, NULL_MARK).expect("the NULL glyph");
    assert_ne!(NULL_MARK, "NULL");
    assert_eq!(role_at(&mut h.app, nx, ny).fg, Some(theme::Role::Overlay0));
    let (tx, ty) = in_results(&mut h, "NULL").expect("the text NULL");
    assert_eq!(role_at(&mut h.app, tx, ty).fg, Some(theme::Role::Text));
    let empty_row = rows.iter().find(|r| r.contains("empty")).unwrap();
    assert!(!empty_row.contains(NULL_MARK) && !empty_row.contains("NULL"), "{empty_row:?}");
    assert!(mode_bar(&mut h).contains("3 rows  12ms"), "{}", mode_bar(&mut h));
}

#[tokio::test]
async fn a_result_without_column_types_still_renders_left_aligned() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("n", "", CellKind::Other)],
        rows: vec![vec![v("5")], vec![v("12345")]],
        truncated: false,
        elapsed: Duration::from_millis(3),
        messages: vec![],
        affected: 0,
        ran: "select n".into(),
        rerun: crate::core::Rerun::Proven,
    };
    let mut h = ran(Ok(set)).await;
    let (a, _) = in_results(&mut h, "5").unwrap();
    let (b, _) = in_results(&mut h, "12345").unwrap();
    assert_eq!(a, b, "both start in the same column: {:#?}", results_rows(&mut h));
}

#[tokio::test]
async fn a_sql_error_with_a_position_renders_the_message_the_position_and_the_statement_that_ran() {
    let mut e = crate::core::CoreError::new(CoreErrorKind::Sql, "ERROR: syntax error at or near \"form\"");
    e.statement = Some("select title\n  from film form x".into());
    e.position = Some(SqlPosition { line: 2, column: 8 });
    let mut h = ran(Err(e)).await;
    let text = results_rows(&mut h).join("\n");
    assert!(text.contains("syntax error at or near \"form\""), "{text}");
    assert!(text.contains("line 2, column 8"), "{text}");
    assert!(text.contains("select title") && text.contains("from film form x"), "the statement that ran: {text}");
    let (x, y) = in_results(&mut h, "syntax error").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(theme::Role::Red));
    assert!(mode_bar(&mut h).contains("failed"), "{}", mode_bar(&mut h));
}

#[tokio::test]
async fn a_statement_with_no_result_columns_renders_the_rows_it_changed() {
    let set = ResultSet { columns: vec![], rows: vec![], affected: 3, ..rows_of(&[]) };
    let mut h = ran(Ok(set)).await;
    assert!(results_rows(&mut h).join("\n").contains("3 rows affected"), "{:#?}", results_rows(&mut h));
}

#[tokio::test]
async fn an_empty_result_shows_its_header_and_0_rows() {
    let mut h = ran(Ok(rows_of(&[]))).await;
    let rows = results_rows(&mut h);
    assert!(rows.iter().any(|r| r.trim() == "v"), "the header: {rows:#?}");
    assert!(rows.iter().any(|r| r.contains("0 rows")), "{rows:#?}");
}

#[tokio::test]
async fn a_truncated_result_says_plus_after_the_count_in_the_mode_bar() {
    let many: Vec<String> = (0..1000).map(|i| i.to_string()).collect();
    let mut set = rows_of(&[]);
    set.rows = many.iter().map(|v| vec![Cell::Value(v.clone())]).collect();
    set.truncated = true;
    let mut h = ran(Ok(set)).await;
    assert!(mode_bar(&mut h).contains("1,000+ rows  12ms"), "{}", mode_bar(&mut h));
    // Only what fits is drawn: the last row on screen is still in the pane.
    let rows = results_rows(&mut h);
    // (The row-number gutter comes first on each row, then the value, then the scrollbar.)
    assert!(rows.last().unwrap().split_whitespace().rfind(|t| t.chars().all(|c| c.is_ascii_digit())).unwrap().parse::<usize>().unwrap() < 30, "{rows:#?}");
}

#[tokio::test]
async fn cells_show_newlines_as_a_glyph_cut_wide_text_with_an_ellipsis_and_never_draw_control_characters() {
    let mut set = rows_of(&[&["line1\nline2"], &["\u{1b}]52;c;AAAA\u{7}bad"], &[&"日本語".repeat(40)]]);
    set.columns[0].name = "v".into();
    let mut h = ran(Ok(set)).await;
    let text = results_rows(&mut h).join("\n");
    assert!(text.contains(&format!("line1{NEWLINE_MARK}line2")), "{text}");
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'), "{text:?}");
    let wide = results_rows(&mut h).into_iter().find(|r| r.contains('日')).unwrap();
    assert!(wide.ends_with('…'), "{wide:?}");
}

// ---- mode bar hints ---------------------------------------------------------------

pub(super) fn click_hint(h: &mut Harness, text: &str) -> Vec<Command> {
    let bar = mode_bar(h);
    let x = bar.find(text).unwrap_or_else(|| panic!("{text:?} not in {bar:?}"));
    let x = bar[..x].chars().count() as u16;
    let y = h.app.ui.hits.mode_bar.y;
    let fx = click(&mut h.app, ratatui::layout::Rect::new(x, y, 1, 1));
    let commands = fx.commands.clone();
    h.run(fx);
    commands
}

#[tokio::test]
async fn the_editors_hints_show_only_while_it_has_focus_and_the_use_hint_only_in_the_sidebar() {
    let mut h = opened().await;
    h.press("j");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("u use"), "{bar}");
    assert!(!bar.contains("f5 run"), "{bar}");
    h.press("ctrl+g l");
    let bar = mode_bar(&mut h);
    assert!(bar.contains("f5 run"), "{bar}");
    assert!(!bar.contains("u use") && !bar.contains("e edit") && !bar.contains("r refresh"), "{bar}");
}

#[tokio::test]
async fn clicking_the_use_hint_and_the_run_hint_do_what_their_keys_do() {
    let mut h = opened().await;
    h.press("j j");
    click_hint(&mut h, "u use");
    assert_eq!(h.app.model.tabs[0].target, Some(dbp("local pg", "pagila")));
    h.press("ctrl+g l");
    h.type_text("select 1");
    let sent = runs(&click_hint(&mut h, "f5 run"));
    assert_eq!(sent.len(), 1);
}

#[tokio::test]
async fn a_notice_expires_on_its_own_at_its_deadline() {
    let mut h = started(&["local pg"]).await;
    press_at(&mut h.app, "f5", base());
    let expires = h.app.ui.notice.as_ref().expect("a notice").expires;
    assert_eq!(h.app.next_deadline(), Some(expires));
    assert!(!h.app.update(Event::Timer, expires - Duration::from_millis(1)).repaint);
    assert!(h.app.update(Event::Timer, expires).repaint);
    assert_eq!(h.app.ui.notice, None);
    assert_eq!(h.app.next_deadline(), None);
}

#[tokio::test]
async fn quitting_from_an_overlay_or_on_a_signal_cancels_the_run_and_closes_the_session_too() {
    for how in ["overlay", "signal"] {
        let mut h = on_pagila().await;
        h.type_text("select 1");
        let run = runs(&h.press_fx("f5"))[0].1;
        let commands = if how == "overlay" {
            h.app.ui.overlay = Some(overlay::Overlay::Error(overlay::ErrorOverlay { title: "t".into(), message: "m".into() }));
            h.press_fx("q")
        } else {
            h.app.quit_requested().commands
        };
        assert_eq!(
            commands,
            vec![Command::Cancel(run), Command::CloseSession(first_session(), dbp("local pg", "pagila")), Command::Quit],
            "{how}"
        );
    }
}

#[tokio::test]
async fn a_run_that_worked_names_the_statement_that_ran_in_the_results_pane() {
    for set in [
        ResultSet { ran: "delete from t where x = 1\n  and y = 2".into(), columns: vec![], affected: 1, ..rows_of(&[]) },
        ResultSet { ran: "select v\nfrom tbl2".into(), ..rows_of(&[&["a"]]) },
    ] {
        let first = set.ran.lines().next().unwrap().to_string();
        let mut h = ran(Ok(set)).await;
        let rows = results_rows(&mut h);
        assert!(rows[0].contains(&first), "the first line of what ran heads the pane: {rows:#?}");
        assert!(!rows.join("\n").contains("y = 2") && !rows.join("\n").contains("tbl2"), "only its first line: {rows:#?}");
        let (x, y) = in_results(&mut h, &first).unwrap();
        assert_eq!(role_at(&mut h.app, x, y).fg, Some(theme::Role::Overlay0));
    }
    let long = format!("select '{}'", "x".repeat(300));
    let mut h = ran(Ok(ResultSet { ran: long, ..rows_of(&[&["a"]]) })).await;
    assert!(results_rows(&mut h)[0].ends_with('…'), "{:?}", results_rows(&mut h)[0]);
}

#[tokio::test]
async fn a_paste_too_large_or_with_a_line_too_long_is_refused_whole_with_a_notice() {
    use super::editor::{LINE_LIMIT, PASTE_LIMIT};
    assert_eq!((PASTE_LIMIT, LINE_LIMIT), (256 * 1024, 16 * 1024));
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    h.type_text("keep");
    let long_line = "x".repeat(LINE_LIMIT + 1);
    let too_big = format!("{}\n", "y".repeat(1000)).repeat(PASTE_LIMIT / 1000 + 1);
    for (what, text) in [("a long line", long_line), ("a large text", too_big)] {
        h.app.ui.notice = None;
        h.app.update(Event::Paste(text), base());
        assert_eq!(h.app.ui.editor.text(), "keep", "{what}: nothing of it is pasted");
        assert!(notice_text(&h).contains("not pasted"), "{what}: {:?}", notice_text(&h));
    }
}

#[tokio::test]
async fn typing_after_the_largest_paste_allowed_stays_quick() {
    use super::editor::{LINE_LIMIT, PASTE_LIMIT};
    let mut h = started(&["local pg"]).await;
    h.press("ctrl+g l");
    // As many lines of the longest length as fit, all on screen-height rows.
    let line = "z".repeat(LINE_LIMIT - 1);
    let text = vec![line; PASTE_LIMIT / LINE_LIMIT].join("\n");
    assert!(text.len() <= PASTE_LIMIT);
    let started = std::time::Instant::now();
    h.app.update(Event::Paste(text), base());
    frame_rows(&mut h.app, 114, 30);
    let paste_took = started.elapsed();
    let started = std::time::Instant::now();
    for _ in 0..10 {
        h.type_text("a");
    }
    let keys_only = started.elapsed() / 10;
    let started = std::time::Instant::now();
    for _ in 0..10 {
        frame_rows(&mut h.app, 114, 30);
    }
    let frame_only = started.elapsed() / 10;
    eprintln!("OBSERVED key alone {keys_only:?}, frame alone {frame_only:?}");
    let started = std::time::Instant::now();
    for _ in 0..10 {
        h.type_text("a");
        frame_rows(&mut h.app, 114, 30);
    }
    let per_key = started.elapsed() / 10;
    eprintln!("OBSERVED largest paste: {paste_took:?}; a key and a frame after it: {per_key:?}");
    // Budgets for this unoptimised test build. The widget walks every visible
    // line whole on each frame; in a release build the same frame (16 lines
    // of 16 KiB) took 10 ms and the key 1 ms. A 1 MB line took over a minute.
    assert!(paste_took < std::time::Duration::from_millis(1_000), "{paste_took:?}");
    assert!(per_key < std::time::Duration::from_millis(250), "{per_key:?}");
}

#[tokio::test]
async fn a_ten_megabyte_cell_costs_a_frame_almost_nothing() {
    let big = "é".repeat(5 * 1024 * 1024); // 10 MB of text
    let mut set = rows_of(&[&["small"]]);
    set.columns.push(col("big", "text", CellKind::Text));
    set.columns.push(col("off", "text", CellKind::Text));
    set.rows = vec![vec![Cell::Value("a".into()), Cell::Value(big.clone()), Cell::Value(format!("{big}\n{big}"))]];
    let mut h = ran(Ok(set)).await;
    frame_rows(&mut h.app, 114, 30);
    // The frame without the big cells, for comparison.
    let mut small = ran(Ok(rows_of(&[&["a"]]))).await;
    let time = |h: &mut Harness| {
        let started = std::time::Instant::now();
        for _ in 0..5 {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(114, 30)).unwrap();
            term.draw(|f| h.app.view(f)).unwrap();
        }
        started.elapsed() / 5
    };
    let (with, without) = (time(&mut h), time(&mut small));
    eprintln!("OBSERVED frame with a 10 MB cell {with:?}, without {without:?}");
    assert!(with.saturating_sub(without) < std::time::Duration::from_millis(5), "{with:?} vs {without:?}");
    assert!(results_rows(&mut h).iter().any(|r| r.contains("ééé") && r.ends_with('…')));
}
