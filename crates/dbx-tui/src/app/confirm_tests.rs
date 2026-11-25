//! Layer 2: the acceptance tests of slice 17 (confirmations), run on `App`
//! with a `FakeCore` behind the shell's command runner. Whether a statement
//! needs confirming is the adapter's to decide (`core/dbx.rs`, `tests/live.rs`);
//! here the Core answers "needs confirmation" and the app must ask.

use super::connections_tests::conn;
use super::grid_tests::{focused, truncated_set};
use super::overlay::Overlay;
use super::query::RunState;
use super::run_tests::{mode_bar, notice_text, on_pagila, runs, title};
use super::testing::*;
use super::theme::Role;
use super::*;
use crate::core::{Assessment, Confirmation, CoreError, DbKind, FakeCore, Reason, RunRequest, Scripted, TabId};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

/// The adapter's answer for `request`, as `DbxCore` would build it: the
/// statement is the whole selection when there is one.
pub(super) fn needs(request: &RunRequest, production: bool, reasons: &[Reason]) -> CoreError {
    let statement = match request.selection {
        Some((a, b)) => crate::core::text_between(&request.buffer, a, b),
        None => request.buffer.clone(),
    };
    CoreError::needs_confirmation(Assessment {
        production,
        reasons: reasons.to_vec(),
        named: Vec::new(),
        target_unknown: false,
        connection_name: "local pg".into(),
        confirmation: Confirmation {
            statement,
            connection: request.connection.clone(),
            database: request.database.clone().unwrap_or_default(),
            session: request.session,
        },
    })
}

/// Feeds the one run in `commands` the answer "needs confirmation", and lets
/// the box's first moments pass (a `y` right after it opens is ignored).
pub(super) fn ask(h: &mut Harness, commands: &[Command], production: bool, reasons: &[Reason]) -> RunRequest {
    let request = ask_at_once(h, commands, production, reasons);
    h.advance(std::time::Duration::from_millis(300));
    request
}

/// As `ask`, leaving the clock where the box opened.
fn ask_at_once(h: &mut Harness, commands: &[Command], production: bool, reasons: &[Reason]) -> RunRequest {
    let sent = runs(commands);
    assert_eq!(sent.len(), 1, "one run: {commands:?}");
    let (id, _, request) = sent[0].clone();
    let fx = h.app.update(Event::Core(id, CoreReply::Ran(Err(needs(&request, production, reasons)))), h.now());
    h.run(fx);
    request
}

/// The overlay's box as text.
fn the_box(h: &mut Harness) -> String {
    let rows = frame_text(&mut h.app, 114, 30);
    let top = rows.iter().position(|r| r.contains('┌')).expect("a box");
    let bottom = rows.iter().rposition(|r| r.contains('└')).expect("a box");
    rows[top..=bottom].join("\n")
}

fn confirming(h: &Harness) -> bool {
    matches!(h.app.ui.overlay, Some(Overlay::ConfirmRun(_)))
}

/// `pagila` in the editor's tab, `sql` typed, and its run answered "needs
/// confirmation" by the fake.
async fn asked_for(sql: &str) -> (Harness, RunRequest) {
    let mut h = on_pagila().await;
    h.type_text(sql);
    let commands = h.press_fx("f5");
    let request = runs(&commands)[0].2.clone();
    h.fake.script_run(Scripted::fail(needs(&request, true, &[Reason::Writes])));
    // The fake answers the run the shell started.
    for _ in 0..50 {
        h.settle().await;
        if confirming(&h) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    h.advance(std::time::Duration::from_millis(300));
    (h, request)
}

#[tokio::test]
async fn a_y_in_the_first_300_ms_is_ignored_key_and_button_alike() {
    let ms = std::time::Duration::from_millis;
    let mut h = on_pagila().await;
    h.type_text(DELETE);
    let commands = h.press_fx("f5");
    ask_at_once(&mut h, &commands, true, &[Reason::Writes]);
    assert!(confirming(&h));
    // Typing that carries on past the reply: a `y` lands at once, and again
    // just before the limit.
    assert!(h.press_fx("y").is_empty(), "ignored at once");
    h.advance(ms(200));
    assert!(h.press_fx("y").is_empty(), "ignored at 200 ms");
    let _ = frame_rows(&mut h.app, 114, 30);
    let yes = h.app.ui.overlay_buttons.iter().find(|b| b.action == super::overlay::OverlayAction::ConfirmRunYes).expect("a y button").rect;
    let t = h.now();
    assert!(click_at(&mut h.app, yes, t).commands.is_empty(), "the button too");
    assert!(confirming(&h), "still asked");
    h.advance(ms(100));
    assert_eq!(runs(&h.press_fx("y")).len(), 1, "after 300 ms it confirms");
}

const DELETE: &str = "delete from t where id = 1";

// ---- the overlay ----------------------------------------------------------------

#[tokio::test]
async fn a_run_answered_needs_confirmation_opens_a_red_box_naming_the_target_the_reason_and_the_statement() {
    let (mut h, _) = asked_for(DELETE).await;
    assert!(confirming(&h), "{:?}", h.app.ui.overlay);
    let text = the_box(&mut h);
    assert!(text.contains("Run on production?"), "{text}");
    assert!(text.contains("local pg") && text.contains("pagila"), "names the connection and database: {text}");
    assert!(text.contains("it writes data"), "says why: {text}");
    assert!(text.contains(DELETE), "shows the statement: {text}");
    assert!(text.contains("y run") && text.contains("esc cancel"), "{text}");
    let (x, y) = find_text(&mut h.app, 114, 30, "┌").expect("a corner");
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Red), "the border is red");
    let tab = &h.app.model.tabs[0];
    assert_eq!(tab.run, RunState::Idle, "the tab stays idle");
    assert!(tab.outcome.is_none(), "no result changes");
}

#[tokio::test]
async fn a_dangerous_statement_outside_production_is_titled_as_such() {
    let mut h = on_pagila().await;
    h.type_text("delete from t");
    let commands = h.press_fx("f5");
    ask(&mut h, &commands, false, &[Reason::Dangerous]);
    let text = the_box(&mut h);
    assert!(text.contains("Run a dangerous statement?"), "{text}");
    assert!(text.contains("dbx flags it dangerous"), "{text}");
}

#[tokio::test]
async fn a_use_a_switched_session_and_an_explain_analyze_say_why_in_their_own_words() {
    for (sql, reasons, words) in [
        ("use prod", vec![Reason::SwitchesDatabase], "switches the session's database; later statements in this tab will be confirmed"),
        (DELETE, vec![Reason::Writes, Reason::SessionSwitched], "an earlier USE in this tab switched its database"),
        ("explain analyze delete from t where id = 1", vec![Reason::ExplainRuns, Reason::Writes], "EXPLAIN ANALYZE runs the statement it explains"),
    ] {
        let mut h = on_pagila().await;
        h.type_text(sql);
        let commands = h.press_fx("f5");
        ask(&mut h, &commands, true, &reasons);
        let text = the_box(&mut h).replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(text.contains(words), "{words}: {text}");
        assert!(!text.contains("it writes data") || reasons.contains(&Reason::Writes), "{text}");
    }
}

#[tokio::test]
async fn esc_runs_nothing_and_says_not_run() {
    let (mut h, _) = asked_for(DELETE).await;
    let sent = h.press_fx("esc");
    assert!(sent.is_empty(), "{sent:?}");
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(notice_text(&h), "Not run.");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    assert_eq!(h.app.ui.tab.editor.text(), DELETE, "the text is kept");
}

#[tokio::test]
async fn enter_does_nothing_and_y_sends_one_run_confirming_exactly_that_statement() {
    let (mut h, request) = asked_for(DELETE).await;
    assert!(h.press_fx("enter").is_empty(), "enter cannot confirm");
    assert!(confirming(&h), "still asked");
    let sent = runs(&h.press_fx("y"));
    assert_eq!(sent.len(), 1);
    let again = &sent[0].2;
    let yes = again.confirmation.clone().expect("carries the yes");
    assert_eq!(yes.statement, DELETE);
    assert_eq!((yes.connection.clone(), yes.database.as_str(), yes.session), (request.connection.clone(), "pagila", request.session));
    let (a, b) = again.selection.expect("the whole statement is selected");
    assert_eq!(crate::core::text_between(&again.buffer, a, b), DELETE, "the confirmed text, not what is under the caret");
    assert_eq!((again.connection.clone(), again.database.clone(), again.session), (request.connection, request.database, request.session));
    assert!(h.app.ui.overlay.is_none());
    assert!(h.app.model.tabs[0].run.in_flight().is_some());
}

#[tokio::test]
async fn the_buttons_are_clickable() {
    let (mut h, _) = asked_for(DELETE).await;
    let _ = frame_rows(&mut h.app, 114, 30);
    let yes = h.app.ui.overlay_buttons.iter().find(|b| b.action == super::overlay::OverlayAction::ConfirmRunYes).expect("a y button").rect;
    let t = h.now();
    let fx = click_at(&mut h.app, yes, t);
    assert_eq!(runs(&fx.commands).len(), 1, "a click on y runs it");
    let (mut h, _) = asked_for(DELETE).await;
    let _ = frame_rows(&mut h.app, 114, 30);
    let no = h.app.ui.overlay_buttons.iter().find(|b| b.action == super::overlay::OverlayAction::ConfirmRunNo).expect("an esc button").rect;
    let t = h.now();
    let fx = click_at(&mut h.app, no, t);
    assert!(fx.commands.is_empty());
    assert_eq!(notice_text(&h), "Not run.");
}

#[tokio::test]
async fn a_long_statement_scrolls_inside_the_box() {
    let mut h = on_pagila().await;
    let long: String = (0..60).map(|i| format!("update t set a = {i} where id = {i};\n")).collect();
    let sql = format!("begin;\n{long}");
    let request = RunRequest {
        connection: crate::core::ConnectionId("id-local pg".into()),
        database: Some("pagila".into()),
        session: h.app.model.tabs[0].session(),
        buffer: sql.clone(),
        cursor: crate::core::Cursor::default(),
        selection: None,
        row_cap: 10,
        confirm_dangerous: true,
        confirmation: None,
    };
    h.app.model.tabs[0].run = RunState::Running { run: crate::core::RunId(99), request: h.app.model.requests.begin(super::requests::Target::Run(TabId(1))), started: h.now() };
    let id = match h.app.model.tabs[0].run {
        RunState::Running { request, .. } => request,
        _ => unreachable!(),
    };
    let fx = h.app.update(Event::Core(id, CoreReply::Ran(Err(needs(&request, true, &[Reason::Writes])))), h.now());
    h.run(fx);
    assert!(the_box(&mut h).contains("update t set a = 0 where id = 0"));
    assert!(!the_box(&mut h).contains("update t set a = 59 where id = 59"));
    h.press("G");
    assert!(the_box(&mut h).contains("update t set a = 59 where id = 59"), "{}", the_box(&mut h));
    h.press("g");
    assert!(the_box(&mut h).contains("update t set a = 0 where id = 0"));
}

// ---- every path ----------------------------------------------------------------

#[tokio::test]
async fn load_more_answered_needs_confirmation_asks_and_y_loads_more() {
    let mut h = focused(truncated_set(1_000)).await;
    let commands = h.press_fx("m");
    ask(&mut h, &commands, true, &[Reason::Unproven]);
    assert!(confirming(&h));
    assert!(h.app.model.tabs[0].loading_more.is_none(), "nothing is loading");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    let sent = runs(&h.press_fx("y"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2.row_cap, 2_000, "the same cap");
    assert!(sent[0].2.confirmation.is_some());
    assert!(h.app.model.tabs[0].loading_more.is_some(), "it is load more again");
}

#[tokio::test]
async fn the_statement_chooser_answered_needs_confirmation_asks() {
    let mut h = on_pagila().await;
    h.type_text("delete from t where id in (select id from u)");
    h.press("left left");
    h.press("f5");
    assert!(matches!(h.app.ui.overlay, Some(Overlay::Chooser(_))), "{:?}", h.app.ui.overlay);
    let commands = h.press_fx("enter");
    let request = ask(&mut h, &commands, true, &[Reason::Writes]);
    assert!(confirming(&h));
    let sent = runs(&h.press_fx("y"));
    let (a, b) = request.selection.expect("the chosen range");
    assert_eq!(sent[0].2.confirmation.as_ref().unwrap().statement, crate::core::text_between(&request.buffer, a, b));
}

#[tokio::test]
async fn a_yes_after_the_tab_moved_to_another_session_is_sent_for_the_new_one_and_asked_again() {
    let (mut h, request) = asked_for(DELETE).await;
    // The tab moved on while the question was open (a new session).
    h.app.model.tabs[0].session += 1;
    let commands = h.press_fx("y");
    let sent = runs(&commands);
    assert_eq!(sent.len(), 1);
    let again = &sent[0].2;
    assert_eq!(again.session, h.app.model.tabs[0].session(), "sent where the tab is now");
    let yes = again.confirmation.as_ref().expect("with the old yes");
    assert_ne!(yes.session, again.session, "which cannot match, so the adapter runs nothing");
    assert_eq!(yes.session, request.session);
    // The adapter asks again; so does the app.
    ask(&mut h, &commands, true, &[Reason::Writes, Reason::Changed]);
    assert!(confirming(&h));
    assert!(the_box(&mut h).contains("changed since it"), "{}", the_box(&mut h));
}

#[tokio::test]
async fn a_question_for_a_tab_not_on_screen_is_held_until_it_is() {
    let mut h = on_pagila().await;
    h.type_text(DELETE);
    let commands = h.press_fx("f5");
    h.press("ctrl+g t");
    assert_eq!(h.app.active_tab(), TabId(2));
    ask(&mut h, &commands, true, &[Reason::Writes]);
    assert!(h.app.ui.overlay.is_none(), "not over another tab");
    assert_eq!(h.app.model.tabs[0].run, RunState::Idle);
    h.press("ctrl+g [");
    assert_eq!(h.app.active_tab(), TabId(1));
    assert!(notice_text(&h).contains("Not run"), "{}", notice_text(&h));
}

#[tokio::test]
async fn the_setting_goes_with_every_run() {
    let mut h = on_pagila().await;
    h.type_text("select 1");
    assert!(runs(&h.press_fx("f5"))[0].2.confirm_dangerous, "on by default");
    let mut h = on_pagila().await;
    h.app.model.settings.confirm_dangerous = false;
    h.type_text("select 1");
    assert!(!runs(&h.press_fx("f5"))[0].2.confirm_dangerous);
}

// ---- PROD ------------------------------------------------------------------------

async fn on(connection: crate::core::Connection, keys: &str) -> Harness {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![connection]));
    fake.script_databases(Scripted::ok(vec!["edge".into(), "pagila".into()]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("enter");
    h.settle().await;
    h.press(keys);
    h
}

#[tokio::test]
async fn prod_shows_in_red_in_the_tab_bar_and_the_mode_bar_on_a_production_target_only() {
    let marked = crate::core::Connection { production: true, ..conn("local pg", DbKind::Postgres) };
    let mut h = on(marked, "j j u").await;
    let bar = title(&mut h);
    assert!(bar.contains("PROD"), "{bar}");
    assert!(mode_bar(&mut h).contains("PROD"), "{}", mode_bar(&mut h));
    let (x, y) = find_text(&mut h.app, 114, 30, "PROD").unwrap();
    let roles = role_at(&mut h.app, x, y);
    assert!(roles.fg == Some(Role::Red) || roles.bg == Some(Role::Red), "{roles:?}");

    let only_pagila = crate::core::Connection { production_databases: vec!["pagila".into()], ..conn("local pg", DbKind::Postgres) };
    let mut h = on(only_pagila.clone(), "j j u").await;
    assert!(title(&mut h).contains("PROD"), "pagila is marked");
    let mut h = on(only_pagila, "j u").await;
    assert_eq!(h.app.model.tabs[0].target.as_ref().map(|t| t.database.as_str()), Some("edge"));
    assert!(!title(&mut h).contains("PROD"), "edge is not: {}", title(&mut h));
    assert!(!mode_bar(&mut h).contains("PROD"));

    let mut h = on(conn("local pg", DbKind::Postgres), "j j u").await;
    assert!(!title(&mut h).contains("PROD") && !mode_bar(&mut h).contains("PROD"));
}
