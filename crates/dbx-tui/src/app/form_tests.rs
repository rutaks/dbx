//! Layer 2: the acceptance tests of slice 03 (connection form), run on `App`
//! with a `FakeCore` behind the shell's command runner.

use std::time::Duration;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::connections_tests::{conn, cursor_row, listed, sidebar_rows};
use super::testing::*;
use super::theme::Role;
use super::{Command, CoreCall, Event};
use crate::core::{ConnectionDraft, ConnectionId, CoreErrorKind, DbKind, FakeCore, Scripted};
use crate::harness::Harness;

const PASSWORD: &str = "pw-Zq8!never-shown-4471";
const W: u16 = 100;
const H: u16 = 34;

async fn empty() -> Harness {
    let mut h = Harness::start(FakeCore::empty());
    h.settle().await;
    h
}

fn screen(h: &mut Harness) -> Vec<String> {
    frame_rows(&mut h.app, W, H)
}

/// The text inside the form's box, one string per row, without the border.
/// Panics if no form is on screen.
fn form_rows(h: &mut Harness) -> Vec<String> {
    let rows = screen(h);
    let top = rows.iter().position(|r| r.contains('┌')).unwrap_or_else(|| panic!("no box on screen:\n{}", rows.join("\n")));
    let chars = |r: &str| r.chars().collect::<Vec<_>>();
    let left = chars(&rows[top]).iter().position(|&c| c == '┌').unwrap();
    let right = chars(&rows[top]).iter().position(|&c| c == '┐').unwrap();
    let bottom = (top + 1..rows.len()).find(|&y| chars(&rows[y]).get(left) == Some(&'└')).expect("the box's bottom");
    (top..=bottom).map(|y| chars(&rows[y])[left + 1..right].iter().collect()).collect()
}

/// The form row for a field, by its label; `(screen_y, text after the label column)`.
fn field_row(h: &mut Harness, label: &str) -> (usize, String) {
    let screen_rows = screen(h);
    let rows = form_rows(h);
    let top = screen_rows.iter().position(|r| r.contains('┌')).unwrap();
    rows.iter()
        .enumerate()
        .find(|(_, r)| r.trim_start().starts_with(label))
        .map(|(i, r)| (top + i, r.trim_start()[label.len()..].to_string()))
        .unwrap_or_else(|| panic!("no `{label}` row in\n{}", rows.join("\n")))
}

fn form_text(h: &mut Harness) -> String {
    form_rows(h).join("\n")
}

fn click_at(h: &mut Harness, x: u16, y: u16) -> Vec<Command> {
    let ev = |kind| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    let down = h.app.update(Event::Mouse(ev(MouseEventKind::Down(MouseButton::Left))), base());
    let up = h.app.update(Event::Mouse(ev(MouseEventKind::Up(MouseButton::Left))), base());
    let commands = [down.commands.clone(), up.commands.clone()].concat();
    h.run(down);
    h.run(up);
    commands
}

/// Opens the new-connection form and fills the fields that need text.
fn fill_new(h: &mut Harness, name: &str, host: &str) {
    h.press("ctrl+g n");
    h.type_text(name);
    h.press("tab");
    h.type_text(host);
}

fn saved(h: &mut Harness) -> Option<ConnectionDraft> {
    h.press_fx("enter").into_iter().find_map(|c| match c {
        Command::Core(_, CoreCall::SaveConnection(d)) => Some(d),
        _ => None,
    })
}

// ---- opening and moving around ------------------------------------------------

#[tokio::test]
async fn prefix_then_n_opens_the_form_with_every_field_and_the_keys_that_work_in_it() {
    let mut h = empty().await;
    h.press("ctrl+g");
    let bar = screen(&mut h)[(H - 1) as usize].clone();
    assert!(bar.contains("n new"), "prefix mode offers the key: {bar:?}");
    h.press("n");
    assert!(h.app.ui.overlay.is_some());
    let text = form_text(&mut h);
    assert!(text.contains("New connection"), "{text}");
    for label in ["name", "host", "port", "user", "password", "database", "tls", "production"] {
        field_row(&mut h, label);
    }
    assert!(field_row(&mut h, "port").1.contains("5432"), "port starts at Postgres's default");
    for button in ["ctrl+t test", "↵ save", "esc cancel", "tab next field"] {
        assert!(text.contains(button), "{button} in\n{text}");
    }
    // The mode bar offers nothing the overlay would ignore.
    let bar = screen(&mut h)[(H - 1) as usize].clone();
    assert!(!bar.contains("ctrl+g"), "{bar:?}");
}

#[tokio::test]
async fn tab_moves_between_fields_and_typed_text_lands_in_the_focused_field_only() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    h.type_text("alpha");
    h.press("tab");
    h.type_text("db.example.com");
    h.press("tab tab");
    h.type_text("app");
    assert!(field_row(&mut h, "name").1.contains("alpha"));
    assert!(!field_row(&mut h, "name").1.contains("db.example") && !field_row(&mut h, "name").1.contains("app"));
    assert!(field_row(&mut h, "host").1.contains("db.example.com"));
    assert!(field_row(&mut h, "user").1.contains("app"));
    assert!(!field_row(&mut h, "port").1.contains("app"), "the port row was skipped over, not typed into");

    // shift+tab and the up arrow go back; the order wraps in both directions.
    h.press("backtab");
    h.type_text("x");
    assert!(field_row(&mut h, "port").1.contains("5432x"), "{:?}", field_row(&mut h, "port"));
    h.press("up");
    h.type_text("y");
    assert!(field_row(&mut h, "host").1.contains("db.example.comy"));
    h.press("up up");
    h.type_text("z");
    assert!(field_row(&mut h, "production").1.contains("[ ]"), "wrapped to the last row, a checkbox, which takes no text");
    assert!(!form_text(&mut h).contains('z'));
    h.press("tab");
    h.type_text("w");
    assert!(field_row(&mut h, "name").1.contains("alphaw"), "and forward past the last row to the first");
}

#[tokio::test]
async fn space_toggles_a_checkbox_only_when_it_has_focus_and_text_fields_take_a_space() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    h.press("space");
    assert!(field_row(&mut h, "name").1.contains(' '), "a space typed into the name");
    h.press("backtab");
    assert!(field_row(&mut h, "production").1.contains("[ ]"));
    h.press("space");
    assert!(field_row(&mut h, "production").1.contains("[x]"));
    h.press("backtab");
    assert!(field_row(&mut h, "tls").1.contains("[ ]"));
    h.press("space");
    assert!(field_row(&mut h, "tls").1.contains("[x]"));
    h.press("space");
    assert!(field_row(&mut h, "tls").1.contains("[ ]"));
}

#[tokio::test]
async fn the_password_field_renders_bullets_never_the_text_and_pasting_is_masked_too() {
    let mut h = empty().await;
    h.press("ctrl+g n tab tab tab tab");
    h.type_text("pw-Zq8");
    let row = field_row(&mut h, "password").1;
    assert!(row.contains("••••••"), "{row:?}");
    assert!(!row.contains("Zq8"), "{row:?}");
    let fx = h.app.update(Event::Paste("!never-shown-4471\n".into()), base());
    h.run(fx);
    let all = screen(&mut h).join("\n");
    assert!(!all.contains("never-shown") && !all.contains("pw-Zq8"), "the password is nowhere on screen:\n{all}");
    assert!(field_row(&mut h, "password").1.contains(&"•".repeat(PASSWORD.chars().count())), "all of it, pasted newline dropped");
    assert!(!format!("{:?}", h.app).contains("never-shown"), "and nowhere in the app's Debug output");
    // The Save command carries it, because that is its job; its Debug does not.
    h.press("shift+tab");
    let fx = crate::app::testing::press(&mut h.app, "enter");
    assert!(!format!("{:?}", fx.commands).contains("never-shown"));
}

#[tokio::test]
async fn pasting_into_a_text_field_inserts_one_line_at_the_cursor() {
    let mut h = empty().await;
    h.press("ctrl+g n tab");
    h.type_text("db");
    h.press("left");
    let fx = h.app.update(Event::Paste("1.example\r\n.com\n".into()), base());
    assert!(fx.repaint);
    assert!(field_row(&mut h, "host").1.contains("d1.example .comb"), "{:?}", field_row(&mut h, "host"));
}

// ---- validation -----------------------------------------------------------------

#[tokio::test]
async fn saving_with_an_empty_name_or_host_shows_the_error_on_that_field_and_emits_no_command() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    let commands = h.press_fx("enter");
    assert!(commands.is_empty(), "{commands:?}");
    let (name_y, _) = field_row(&mut h, "name");
    let rows = screen(&mut h);
    assert!(rows[name_y + 1].contains("Name is required."), "the error sits under the name field: {:?}", rows[name_y + 1]);
    let (host_y, _) = field_row(&mut h, "host");
    assert!(screen(&mut h)[host_y + 1].contains("Host is required."));
    assert!(!form_text(&mut h).contains("Port must"), "the port is fine");
    assert_eq!(h.fake.calls().len(), 1, "only the first connection list was asked of the Core");

    // Typing in a field clears its own error and no other.
    h.type_text("a");
    assert!(!form_text(&mut h).contains("Name is required."));
    assert!(form_text(&mut h).contains("Host is required."));
    h.press("tab");
    h.type_text("h");
    assert!(!form_text(&mut h).contains("Host is required."));
    assert!(saved(&mut h).is_some(), "now it can be saved");
}

#[tokio::test]
async fn a_port_that_is_not_a_number_from_1_to_65535_is_an_error_on_the_port_field() {
    for bad in ["", "abc", "0", "65536", "-1", "80.5"] {
        let mut h = empty().await;
        fill_new(&mut h, "n", "h");
        h.press("tab");
        h.press("ctrl+u");
        h.type_text(bad);
        assert!(saved(&mut h).is_none(), "{bad:?}");
        let (y, _) = field_row(&mut h, "port");
        assert!(screen(&mut h)[y + 1].contains("Port must be a number from 1 to 65535."), "{bad:?}");
    }
    let mut h = empty().await;
    fill_new(&mut h, "n", "h");
    h.press("tab ctrl+u");
    h.type_text("65535");
    assert_eq!(saved(&mut h).unwrap().port, Some(65535));
}

// ---- test connection ------------------------------------------------------------------

fn tested(commands: &[Command]) -> Option<ConnectionDraft> {
    commands.iter().find_map(|c| match c {
        Command::Core(_, CoreCall::TestConnection(d)) => Some(d.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn test_emits_a_test_command_with_what_was_typed_and_the_form_stays_open_on_failure_and_success() {
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::err(CoreErrorKind::Connect, "password authentication failed for user \"postgres\""));
    h.fake.script_test_connection(Scripted::ok(()));
    fill_new(&mut h, "local pg", "127.0.0.1");
    h.press("tab tab");
    h.type_text("postgres");
    h.press("tab");
    h.type_text(PASSWORD);
    h.press("tab");
    h.type_text("pagila");

    let commands = h.press_fx("ctrl+t");
    let draft = tested(&commands).expect("a test command");
    assert_eq!(
        (draft.name.as_str(), draft.host.as_str(), draft.port, draft.username.as_str(), draft.database.as_deref()),
        ("local pg", "127.0.0.1", Some(5432), "postgres", Some("pagila"))
    );
    assert_eq!((draft.id, draft.kind, draft.password.as_deref()), (None, DbKind::Postgres, Some(PASSWORD)));
    assert!(!format!("{commands:?}").contains("never-shown"), "no secret in the command's Debug output");
    assert!(saved_calls(&h).is_empty(), "testing saves nothing");

    assert!(form_text(&mut h).contains("testing…"), "{}", form_text(&mut h));
    h.settle().await;
    let text = form_text(&mut h);
    assert!(text.contains("password authentication failed for user"), "dbx's message is shown: {text}");
    assert!(!text.contains("testing…") && !text.contains("never-shown"));
    assert!(h.app.ui.overlay.is_some(), "the form stays open after a failure");

    h.press("ctrl+t");
    h.settle().await;
    let text = form_text(&mut h);
    assert!(text.contains("Connection successful"), "{text}");
    assert!(!text.contains("password authentication failed"), "the old failure is replaced");
    assert!(h.app.ui.overlay.is_some(), "and after a success");
}

fn saved_calls(h: &Harness) -> Vec<crate::core::Call> {
    h.fake.calls().into_iter().filter(|c| matches!(c, crate::core::Call::SaveConnection(_))).collect()
}

#[tokio::test]
async fn a_test_needs_a_host_and_a_port_but_not_a_name() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    assert!(tested(&h.press_fx("ctrl+t")).is_none());
    assert!(form_text(&mut h).contains("Host is required."));
    assert!(!form_text(&mut h).contains("Name is required."), "a connection can be tried before it is named");
    // Focus went to the first field with a problem: the host.
    h.type_text("h");
    assert!(tested(&h.press_fx("ctrl+t")).is_some());
}

#[tokio::test(start_paused = true)]
async fn while_a_test_runs_the_form_shows_testing_and_typing_and_esc_still_work() {
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::ok(()).after(Duration::from_secs(30)));
    fill_new(&mut h, "n", "h");
    h.press("ctrl+t");
    h.settle().await;
    assert!(form_text(&mut h).contains("testing…"));
    h.press("tab");
    h.type_text("ok");
    assert!(field_row(&mut h, "port").1.contains("5432ok"), "typing is not blocked");
    assert!(!form_text(&mut h).contains("testing…"), "an edit makes the pending result stale, so it is no longer awaited");

    // The reply for the edited-away test, arriving later, changes nothing.
    tokio::time::advance(Duration::from_secs(31)).await;
    h.settle().await;
    assert!(!form_text(&mut h).contains("Connection successful"), "stale reply dropped");

    // esc is not blocked either (the form has changes, so it asks first).
    h.press("esc");
    assert!(form_text(&mut h).contains("Discard changes?"));
    h.press("enter");
    assert!(h.app.ui.overlay.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_test_reply_for_a_form_that_was_closed_meanwhile_is_dropped() {
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::err(CoreErrorKind::Connect, "late failure").after(Duration::from_secs(10)));
    fill_new(&mut h, "n", "h");
    h.press("ctrl+t");
    h.settle().await;
    h.press("esc enter");
    assert!(h.app.ui.overlay.is_none(), "closed");
    // A second form is opened before the first one's reply lands.
    h.press("ctrl+g n");
    tokio::time::advance(Duration::from_secs(11)).await;
    h.settle().await;
    assert!(h.app.ui.overlay.is_some());
    assert!(!form_text(&mut h).contains("late failure"), "the new form never saw the old reply");
    let fx = crate::app::testing::press(&mut h.app, "esc");
    assert!(fx.commands.is_empty());
}

// ---- saving ---------------------------------------------------------------------------

#[tokio::test]
async fn save_emits_a_save_command_and_on_success_closes_the_form_and_the_connection_appears() {
    let mut h = empty().await;
    let mut created = conn("local pg", DbKind::Postgres);
    created.host = "127.0.0.1".into();
    h.fake.script_save_connection(Scripted::ok(created.clone()));
    fill_new(&mut h, "local pg", "127.0.0.1");
    let draft = saved(&mut h).expect("a save command");
    assert_eq!((draft.name.as_str(), draft.host.as_str(), draft.port, draft.id.clone()), ("local pg", "127.0.0.1", Some(5432), None));
    assert!(form_text(&mut h).contains("saving…"));
    h.settle().await;
    assert!(h.app.ui.overlay.is_none(), "closed on success");
    let sidebar = sidebar_rows(&mut h, W, H).join("\n");
    assert!(sidebar.contains("local pg") && sidebar.contains("postgres"), "{sidebar}");
    assert!(!sidebar.contains("No connections yet"), "{sidebar}");
    assert!(cursor_row(&mut h).unwrap_or_default().contains("local pg"), "the cursor is on the new connection");
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, crate::core::Call::Connections)).count(), 1, "no reload needed");
}

#[tokio::test]
async fn a_new_connection_is_listed_in_name_order_and_the_cursor_goes_to_it() {
    let mut h = listed(&["alpha", "zulu"]).await;
    h.fake.script_save_connection(Scripted::ok(conn("Mike", DbKind::Postgres)));
    fill_new(&mut h, "Mike", "h");
    saved(&mut h);
    h.settle().await;
    let rows = sidebar_rows(&mut h, W, H);
    let names: Vec<&str> = ["alpha", "Mike", "zulu"].into_iter().collect();
    let at: Vec<usize> = names.iter().map(|n| rows.iter().position(|r| r.contains(n)).unwrap()).collect();
    assert!(at[0] < at[1] && at[1] < at[2], "{rows:#?}");
    assert!(cursor_row(&mut h).unwrap_or_default().contains("Mike"));
}

#[tokio::test]
async fn a_failed_save_keeps_the_form_open_with_the_message_and_a_key_error_reads_as_one() {
    let mut h = empty().await;
    h.fake.script_save_connection(Scripted::err(CoreErrorKind::Key, "The key for existing encrypted data is missing (ENCRYPTED_DATA_KEY_MISSING). Set DBX_SECRET_KEY_FILE."));
    h.fake.script_save_connection(Scripted::err(CoreErrorKind::Store, "could not write the store"));
    fill_new(&mut h, "n", "h");
    saved(&mut h);
    h.settle().await;
    let text = form_text(&mut h);
    assert!(h.app.ui.overlay.is_some(), "still open");
    assert!(text.contains("ENCRYPTED_DATA_KEY_MISSING"), "{text}");
    assert!(text.contains("Not saved"), "it says nothing was saved: {text}");
    assert!(!text.contains("saving…"));
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("No connections yet"), "nothing was added");
    // Fixing and retrying works; a different failure replaces the message.
    saved(&mut h);
    h.settle().await;
    let text = form_text(&mut h);
    assert!(text.contains("could not write the store") && !text.contains("ENCRYPTED_DATA_KEY_MISSING"), "{text}");
}

#[tokio::test(start_paused = true)]
async fn while_saving_the_form_is_inert_so_nothing_typed_can_be_lost_and_esc_does_nothing() {
    let mut h = empty().await;
    h.fake.script_save_connection(Scripted::ok(conn("n", DbKind::Postgres)).after(Duration::from_secs(5)));
    fill_new(&mut h, "n", "h");
    assert!(saved(&mut h).is_some());
    h.settle().await;
    assert!(saved(&mut h).is_none(), "already saving: a second enter saves nothing");
    let before = form_text(&mut h);
    assert!(before.contains("saving…"), "{before}");
    // The hint line says what is going on instead of offering keys that do nothing.
    assert!(!before.contains("esc cancel") && !before.contains("tab next field"), "{before}");

    h.type_text("ab");
    h.press("tab backspace");
    assert!(!field_row(&mut h, "host").1.contains("hab"), "typing is locked while saving");
    let fx = h.app.update(Event::Paste("pasted".into()), base());
    h.run(fx);
    assert!(!form_text(&mut h).contains("pasted"));
    let rows = screen(&mut h);
    let (x, y) = (rows.iter().find_map(|r| r.find("host")).map(|b| b as u16).unwrap(), field_row(&mut h, "tls").0 as u16);
    click_at(&mut h, x + 14, y);
    assert!(field_row(&mut h, "tls").1.contains("[ ]"), "a click does not toggle a checkbox either");

    h.press("esc");
    assert!(h.app.ui.overlay.is_some(), "esc does nothing while saving");
    assert!(!screen(&mut h).join("\n").contains("Discard changes?"));
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    assert!(h.app.ui.overlay.is_none(), "closed by the reply");
    assert_eq!(saved_calls(&h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_save_that_fails_leaves_the_form_open_with_the_error_and_the_form_works_again() {
    let mut h = empty().await;
    h.fake.script_save_connection(Scripted::err(CoreErrorKind::Store, "disk full").after(Duration::from_secs(5)));
    fill_new(&mut h, "n", "h");
    saved(&mut h);
    h.settle().await;
    h.press("esc");
    assert!(h.app.ui.overlay.is_some(), "esc did nothing while saving");
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    let text = form_text(&mut h);
    assert!(h.app.ui.overlay.is_some() && text.contains("Not saved. disk full"), "{text}");
    assert!(text.contains("esc cancel"), "keys are back: {text}");
    h.type_text("!");
    assert!(field_row(&mut h, "host").1.contains("h!") || form_text(&mut h).contains('!'), "typing works again");
    h.press("esc");
    assert!(form_text(&mut h).contains("Discard changes?"), "and esc asks, as before");
}

#[tokio::test(start_paused = true)]
async fn while_a_save_is_pending_the_connection_cannot_be_edited_or_removed() {
    let mut h = listed(&["alpha"]).await;
    h.fake.script_save_connection(Scripted::ok(conn("beta", DbKind::Postgres)).after(Duration::from_secs(5)));
    h.press("e ctrl+u");
    h.type_text("beta");
    saved(&mut h);
    h.settle().await;
    // The form cannot be closed, so the sidebar's e and d are out of reach.
    h.press("esc e d delete");
    assert!(form_text(&mut h).contains("saving…"));
    assert!(!screen(&mut h).join("\n").contains("Remove connection"));
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, crate::core::Call::RemoveConnection(_))).count(), 0);
}

// ---- editing ------------------------------------------------------------------------

#[tokio::test]
async fn e_in_the_sidebar_edits_the_connection_under_the_cursor_without_loading_its_password() {
    let mut h = listed(&["alpha", "beta"]).await;
    h.press("j e");
    let text = form_text(&mut h);
    assert!(text.contains("Edit connection"), "{text}");
    assert!(field_row(&mut h, "name").1.contains("beta"));
    assert!(field_row(&mut h, "host").1.contains("localhost"));
    assert!(field_row(&mut h, "port").1.contains("5432"));
    assert!(field_row(&mut h, "user").1.contains("postgres"));
    let password = field_row(&mut h, "password").1;
    assert!(password.contains("(unchanged)"), "the stored password is not shown, not even as bullets: {password:?}");
    assert!(!password.contains('•'));

    // Nothing changed: esc closes without asking. Then change the name and save.
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());
    h.press("e");
    h.press("ctrl+u");
    h.type_text("gamma");
    let draft = saved(&mut h).expect("save");
    assert_eq!(draft.id, Some(ConnectionId("id-beta".into())), "an edit names the connection it edits");
    assert_eq!((draft.name.as_str(), draft.password), ("gamma", None), "a blank password means unchanged");
}

#[tokio::test]
async fn an_edited_connection_stays_in_place_in_the_list_and_keeps_the_cursor() {
    let mut h = listed(&["alpha", "beta", "gamma"]).await;
    h.press("j e ctrl+u");
    h.type_text("zeta");
    let mut renamed = conn("zeta", DbKind::Postgres);
    renamed.id = ConnectionId("id-beta".into());
    h.fake.script_save_connection(Scripted::ok(renamed));
    saved(&mut h);
    h.settle().await;
    let rows = sidebar_rows(&mut h, W, H).join("\n");
    assert!(!rows.contains("beta") && rows.contains("zeta"), "{rows}");
    assert_eq!(rows.matches('○').count(), 3, "still three connections: {rows}");
    assert!(cursor_row(&mut h).unwrap_or_default().contains("zeta"), "the cursor follows the connection to its new place");
}

#[tokio::test]
async fn a_password_typed_while_editing_is_sent_and_the_edit_keeps_the_connections_other_fields() {
    let fake = FakeCore::new();
    let mut pg = conn("shop", DbKind::Postgres);
    pg.port = Some(5433);
    fake.script_connections(Scripted::ok(vec![pg]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h.press("e tab tab tab tab");
    h.type_text("new-secret");
    let draft = saved(&mut h).unwrap();
    assert_eq!((draft.kind, draft.password.as_deref(), draft.port), (DbKind::Postgres, Some("new-secret"), Some(5433)));
}

// ---- esc and discarding ------------------------------------------------------------

#[tokio::test]
async fn esc_closes_an_untouched_form_and_asks_before_discarding_a_changed_one() {
    let mut h = empty().await;
    h.press("ctrl+g n esc");
    assert!(h.app.ui.overlay.is_none(), "nothing to lose");

    h.press("ctrl+g n");
    h.type_text("half-typed");
    h.press("esc");
    let text = screen(&mut h).join("\n");
    assert!(text.contains("Discard changes?"), "{text}");
    assert!(text.contains("↵ discard") && text.contains("esc keep editing"), "{text}");
    assert!(h.app.ui.overlay.is_some());

    h.press("esc");
    assert!(!screen(&mut h).join("\n").contains("Discard changes?"), "back to the form");
    assert!(field_row(&mut h, "name").1.contains("half-typed"), "what was typed is still there");
    h.type_text("!");
    assert!(field_row(&mut h, "name").1.contains("half-typed!"), "and typing continues where it was");

    h.press("esc enter");
    assert!(h.app.ui.overlay.is_none(), "discarded");
    h.press("ctrl+g n");
    assert!(!field_row(&mut h, "name").1.contains("half-typed"), "the next form starts empty");
}

#[tokio::test]
async fn while_the_discard_question_is_open_other_keys_do_nothing() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    h.type_text("abc");
    h.press("esc");
    h.press("x tab ctrl+t");
    assert!(form_text(&mut h).contains("Discard changes?"));
    assert!(h.fake.calls().len() == 1, "no Core call: {:?}", h.fake.calls());
    h.press("esc");
    assert!(field_row(&mut h, "name").1.contains("abc") && !field_row(&mut h, "name").1.contains("abcx"));
}

#[tokio::test]
async fn a_form_changed_and_changed_back_has_nothing_to_discard() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    h.type_text("a");
    h.press("backspace esc");
    assert!(h.app.ui.overlay.is_none());
}

// ---- removing --------------------------------------------------------------------------

#[tokio::test]
async fn d_asks_for_confirmation_naming_the_connection_and_esc_cancels_and_emits_nothing() {
    let mut h = listed(&["alpha", "beta"]).await;
    h.press("j d");
    let text = screen(&mut h).join("\n");
    assert!(text.contains("Remove connection"), "{text}");
    assert!(text.contains("\"beta\""), "it names the connection: {text}");
    assert!(!text.contains("\"alpha\""), "{text}");
    assert!(text.contains("↵ remove") && text.contains("esc cancel"), "{text}");
    let fx = crate::app::testing::press(&mut h.app, "esc");
    assert!(fx.commands.is_empty(), "{:?}", fx.commands);
    assert!(h.app.ui.overlay.is_none());
    assert!(!h.fake.calls().iter().any(|c| matches!(c, crate::core::Call::RemoveConnection(_))));
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("beta"), "still listed");
}

#[tokio::test]
async fn confirming_a_removal_asks_the_core_and_the_row_disappears_when_it_succeeds() {
    for key in ["d", "delete"] {
        let mut h = listed(&["alpha", "beta", "gamma"]).await;
        h.fake.script_remove_connection(Scripted::ok(()));
        h.press("j");
        h.press(key);
        let commands = h.press_fx("enter");
        assert!(commands.iter().any(|c| matches!(c, Command::Core(_, CoreCall::RemoveConnection(id)) if id.0 == "id-beta")), "{key}: {commands:?}");
        assert!(h.app.ui.overlay.is_none());
        h.settle().await;
        let rows = sidebar_rows(&mut h, W, H).join("\n");
        assert!(!rows.contains("beta") && rows.contains("alpha") && rows.contains("gamma"), "{key}: {rows}");
        assert!(cursor_row(&mut h).is_some(), "the cursor is still on a row");
    }
}

#[tokio::test]
async fn removing_the_last_connection_shows_the_empty_state_and_a_failed_removal_says_why() {
    let mut h = listed(&["only"]).await;
    h.fake.script_remove_connection(Scripted::err(CoreErrorKind::Store, "the store is locked"));
    h.fake.script_remove_connection(Scripted::ok(()));
    h.press("d enter");
    h.settle().await;
    let text = screen(&mut h).join("\n");
    assert!(text.contains("Could not remove connection") && text.contains("the store is locked"), "{text}");
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("only"), "still listed");
    h.press("enter");
    assert!(h.app.ui.overlay.is_none(), "enter closes it");
    h.press("d enter");
    h.settle().await;
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("No connections yet"));
}

#[tokio::test]
async fn d_and_e_do_nothing_outside_the_sidebar_or_without_a_connection() {
    let mut h = listed(&["alpha"]).await;
    h.press("ctrl+g l");
    h.press("d e delete");
    assert!(h.app.ui.overlay.is_none(), "the editor is focused: those are text there");
    let mut empty = empty().await;
    empty.press("d e delete");
    assert!(empty.app.ui.overlay.is_none(), "nothing to edit or remove");
}

// ---- mode bar and empty state ------------------------------------------------------------

#[tokio::test]
async fn the_mode_bar_offers_edit_and_remove_only_where_they_work() {
    let mut h = listed(&["alpha"]).await;
    let bar = |h: &mut Harness| screen(h)[(H - 1) as usize].clone();
    let b = bar(&mut h);
    assert!(b.contains("e edit") && b.contains("d remove"), "{b:?}");
    h.press("ctrl+g l");
    let b = bar(&mut h);
    assert!(!b.contains("edit") && !b.contains("remove"), "the editor has focus: {b:?}");
    let mut none = empty().await;
    let b = bar(&mut none);
    assert!(!b.contains("edit") && !b.contains("remove"), "nothing to edit: {b:?}");
}

#[tokio::test]
async fn the_empty_sidebar_names_the_key_that_adds_a_connection() {
    let mut h = empty().await;
    let text = sidebar_rows(&mut h, W, H).join("\n");
    assert!(text.contains("No connections yet."), "{text}");
    assert!(text.contains("ctrl+g n"), "{text}");
    assert!(!text.contains("dbx app"), "no more sending people to the desktop app: {text}");
    assert!(!text.contains('…'), "the sentence fits the sidebar: {text}");
    h.press("ctrl+g n");
    assert!(h.app.ui.overlay.is_some(), "and that key does it");
}

// ---- the sidebar cursor follows its connection ------------------------------------------

#[tokio::test]
async fn after_a_reload_the_sidebar_cursor_stays_on_the_same_connection_by_id() {
    let mut h = listed(&["bravo", "charlie", "delta"]).await;
    h.press("j j");
    assert!(cursor_row(&mut h).unwrap().contains("delta"));
    // Another head added two connections that sort before it.
    h.fake.script_connections(Scripted::ok(vec![
        conn("alpha", DbKind::Postgres),
        conn("bravo", DbKind::Postgres),
        conn("bravo-2", DbKind::Postgres),
        conn("charlie", DbKind::Postgres),
        conn("delta", DbKind::Postgres),
    ]));
    let fx = h.app.start();
    h.run(fx);
    h.settle().await;
    assert!(cursor_row(&mut h).unwrap().contains("delta"), "{:?}", cursor_row(&mut h));

    // The connection under the cursor is gone: the cursor stays on a row.
    h.fake.script_connections(Scripted::ok(vec![conn("alpha", DbKind::Postgres), conn("bravo", DbKind::Postgres)]));
    let fx = h.app.start();
    h.run(fx);
    h.settle().await;
    assert!(cursor_row(&mut h).unwrap().contains("bravo"), "clamped to the last row");
}

// ---- mouse ---------------------------------------------------------------------------------

#[tokio::test]
async fn clicking_a_field_focuses_it_and_places_the_cursor_where_the_click_landed() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    h.type_text("abcdef");
    h.press("tab");
    h.type_text("zz");
    let (y, after) = field_row(&mut h, "name");
    let rows = screen(&mut h);
    let x_text = rows[y].find("abcdef").map(|b| rows[y][..b].chars().count()).expect("name text on screen") as u16;
    assert!(after.contains("abcdef"));
    // Click on the `c`: focus returns to the name field and the cursor sits on `c`.
    click_at(&mut h, x_text + 2, y as u16);
    h.type_text("X");
    assert!(field_row(&mut h, "name").1.contains("abXcdef"), "{:?}", field_row(&mut h, "name"));
    // Clicking past the end of the text puts the cursor at the end.
    let (hy, _) = field_row(&mut h, "host");
    click_at(&mut h, x_text + 30, hy as u16);
    h.type_text("!");
    assert!(field_row(&mut h, "host").1.contains("zz!"), "{:?}", field_row(&mut h, "host"));
    // Clicking a checkbox focuses and toggles it.
    let (ty, _) = field_row(&mut h, "tls");
    click_at(&mut h, x_text + 2, ty as u16);
    assert!(field_row(&mut h, "tls").1.contains("[x]"));
}

#[tokio::test]
async fn the_buttons_work_by_click_as_they_do_by_key() {
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::ok(()));
    h.fake.script_save_connection(Scripted::ok(conn("n", DbKind::Postgres)));
    fill_new(&mut h, "n", "h");
    screen(&mut h);
    let button = |h: &mut Harness, label: &str| {
        let (x, y) = find_text(&mut h.app, W, H, label).unwrap_or_else(|| panic!("{label} on screen"));
        (x + 1, y)
    };
    let (x, y) = button(&mut h, "ctrl+t test");
    let commands = click_at(&mut h, x, y);
    assert!(tested(&commands).is_some(), "click on test: {commands:?}");
    h.settle().await;
    assert!(form_text(&mut h).contains("Connection successful"));
    let (x, y) = button(&mut h, "↵ save");
    let commands = click_at(&mut h, x, y);
    assert!(commands.iter().any(|c| matches!(c, Command::Core(_, CoreCall::SaveConnection(_)))), "{commands:?}");
    h.settle().await;
    assert!(h.app.ui.overlay.is_none());

    // Cancel by click: clean form closes, dirty asks, and the question's buttons click too.
    h.press("ctrl+g n");
    h.type_text("x");
    let (x, y) = button(&mut h, "esc cancel");
    click_at(&mut h, x, y);
    assert!(form_text(&mut h).contains("Discard changes?") || screen(&mut h).join("\n").contains("Discard changes?"));
    let (x, y) = button(&mut h, "↵ discard");
    click_at(&mut h, x, y);
    assert!(h.app.ui.overlay.is_none());
}

#[tokio::test]
async fn a_click_outside_the_form_does_nothing_to_the_panes_behind_it() {
    let mut h = listed(&["alpha", "beta", "gamma"]).await;
    h.press("ctrl+g n");
    h.type_text("keep");
    let focus = h.app.ui.focus;
    let before = cursor_row(&mut h);
    let rows = screen(&mut h);
    // On the third sidebar row: it would move the cursor if the click got through.
    let (x, y) = find_text(&mut h.app, W, H, "gamma").unwrap();
    assert!(!rows[y as usize].contains('┌'));
    click_at(&mut h, x, y);
    click_at(&mut h, W - 2, 3); // over the editor pane
    assert!(h.app.ui.overlay.is_some(), "the form stays open");
    assert_eq!(h.app.ui.focus, focus);
    assert_eq!(cursor_row(&mut h), before);
    assert!(field_row(&mut h, "name").1.contains("keep"));
    h.type_text("!");
    assert!(field_row(&mut h, "name").1.contains("keep!"), "focus is where it was");
}

// ---- look and feel -------------------------------------------------------------------------

#[tokio::test]
async fn the_form_is_a_panel_bg_box_with_an_accent_border_and_the_focused_field_stands_out() {
    use ratatui::style::Modifier;
    let mut h = empty().await;
    h.press("ctrl+g n");
    let rows = screen(&mut h);
    let top = rows.iter().position(|r| r.contains('┌')).unwrap();
    let left = rows[top].chars().position(|c| c == '┌').unwrap() as u16;
    let top = top as u16;
    assert_eq!(role_at(&mut h.app, left, top).fg, Some(Role::Accent), "accent border");
    assert_eq!(role_at(&mut h.app, left + 1, top + 1).bg, Some(Role::PanelBg), "panel_bg inside");
    let title_x = rows[top as usize].find("New connection").map(|b| rows[top as usize][..b].chars().count()).unwrap() as u16;
    assert_eq!(role_at(&mut h.app, title_x, top).fg, Some(Role::Accent), "title in accent");

    // The focused field's label is bold accent; the others are subtext0.
    let (ny, _) = field_row(&mut h, "name");
    let (hy, _) = field_row(&mut h, "host");
    let label_x = |h: &mut Harness, y: usize, label: &str| {
        let r = screen(h);
        r[y].find(label).map(|b| r[y][..b].chars().count()).unwrap() as u16
    };
    let (nx, hx) = (label_x(&mut h, ny, "name"), label_x(&mut h, hy, "host"));
    assert_eq!(role_at(&mut h.app, nx, ny as u16).fg, Some(Role::Accent));
    assert_eq!(role_at(&mut h.app, hx, hy as u16).fg, Some(Role::Subtext0));
    // Input boxes: surface1 under focus, surface0 otherwise. The text cursor is a reversed cell.
    let input_x = nx + 11;
    assert_eq!(role_at(&mut h.app, input_x + 5, ny as u16).bg, Some(Role::Surface1));
    assert_eq!(role_at(&mut h.app, input_x + 5, hy as u16).bg, Some(Role::Surface0));
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    assert!(term.backend().buffer()[(input_x, ny as u16)].modifier.contains(Modifier::REVERSED), "the cursor cell");
    assert!(!term.backend().buffer()[(input_x + 1, ny as u16)].modifier.contains(Modifier::REVERSED));
    assert!(term.backend().buffer()[(0, 0)].modifier.contains(Modifier::DIM), "behind the form is dimmed");

    // Errors are red.
    h.press("enter");
    let (ey, _) = field_row(&mut h, "name");
    let r = screen(&mut h);
    let ex = r[ey + 1].find("Name is required.").map(|b| r[ey + 1][..b].chars().count()).unwrap() as u16;
    assert_eq!(role_at(&mut h.app, ex, (ey + 1) as u16).fg, Some(Role::Red));
}

#[tokio::test]
async fn destructive_questions_have_a_red_border_and_buttons_carry_their_keys() {
    let mut h = listed(&["alpha"]).await;
    h.press("d");
    let rows = screen(&mut h);
    let top = rows.iter().position(|r| r.contains('┌')).unwrap();
    let left = rows[top].chars().position(|c| c == '┌').unwrap() as u16;
    assert_eq!(role_at(&mut h.app, left, top as u16).fg, Some(Role::Red), "remove is destructive");
    let (bx, by) = find_text(&mut h.app, W, H, "↵ remove").unwrap();
    assert_eq!(role_at(&mut h.app, bx, by).fg, Some(Role::Accent));
    assert_eq!(role_at(&mut h.app, bx + 2, by).fg, Some(Role::Overlay0));

    h.press("esc");
    h.press("ctrl+g n");
    h.type_text("x");
    h.press("esc");
    let rows = screen(&mut h);
    // The discard question's box is the second one drawn: its top-left is the last `┌`.
    let (top, left) = rows
        .iter()
        .enumerate()
        .filter_map(|(y, r)| r.chars().collect::<Vec<_>>().iter().rposition(|&c| c == '┌').map(|x| (y, x)))
        .next_back()
        .unwrap();
    assert_eq!(role_at(&mut h.app, left as u16, top as u16).fg, Some(Role::Red), "discarding is destructive too");
}

#[tokio::test]
async fn the_form_and_its_questions_draw_inside_any_terminal_size_without_panicking() {
    for (w, h) in [(0, 0), (1, 1), (10, 3), (20, 8), (30, 12), (40, 16), (60, 20), (100, 40)] {
        let mut app = listed(&["alpha"]).await;
        app.press("ctrl+g n");
        app.press("enter"); // errors on name and host
        app.type_text("a");
        let _ = frame_rows(&mut app.app, w, h);
        app.press("esc");
        let _ = frame_rows(&mut app.app, w, h);
        app.press("esc esc");
        app.press("d");
        let _ = frame_rows(&mut app.app, w, h);
    }
}

// ---- review fixes ---------------------------------------------------------------------

const OSC52: &str = "\u{1b}]52;c;AAAA\u{7}";

/// Every cell of the frame, as symbols.
fn all_symbols(h: &mut Harness) -> Vec<String> {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    let buf = term.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol().to_string()).collect()
}

fn assert_no_controls(h: &mut Harness, what: &str) {
    let bad: Vec<String> = all_symbols(h).into_iter().filter(|s| s.chars().any(char::is_control)).collect();
    assert!(bad.is_empty(), "{what}: control characters reached the screen: {bad:?}");
}

#[tokio::test]
async fn a_server_message_with_terminal_escapes_reaches_no_cell_as_a_control_character() {
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::err(CoreErrorKind::Connect, format!("boom {OSC52} after")));
    fill_new(&mut h, "n", "h");
    h.press("ctrl+t");
    h.settle().await;
    assert!(form_text(&mut h).contains("boom"), "the message is still shown");
    assert_no_controls(&mut h, "test failure");
    h.fake.script_save_connection(Scripted::err(CoreErrorKind::Store, format!("bad {OSC52}")));
    saved(&mut h);
    h.settle().await;
    assert_no_controls(&mut h, "save failure");
}

#[tokio::test]
async fn a_connection_name_and_a_removal_question_with_escapes_draw_without_control_characters() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn(&format!("evil{OSC52}name"), DbKind::Postgres)]));
    let mut h = Harness::start(fake);
    h.settle().await;
    assert_no_controls(&mut h, "sidebar");
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("evil"));
    h.press("d");
    assert!(screen(&mut h).join("\n").contains("Remove connection"));
    assert_no_controls(&mut h, "removal question");
    h.press("esc e");
    assert_no_controls(&mut h, "edit form");
}

#[tokio::test]
async fn a_message_in_wide_characters_keeps_the_overlay_border_aligned() {
    use unicode_width::UnicodeWidthStr;
    let mut h = empty().await;
    h.fake.script_test_connection(Scripted::err(CoreErrorKind::Connect, "致命的エラー：パスワード認証に失敗しました。ユーザー「postgres」の接続を確認してください"));
    fill_new(&mut h, "n", "h");
    h.press("ctrl+t");
    h.settle().await;
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(W, H)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let top = (0..H).find(|&y| (0..W).any(|x| buf[(x, y)].symbol() == "┌")).unwrap();
    let left = (0..W).find(|&x| buf[(x, top)].symbol() == "┌").unwrap();
    let right = (0..W).find(|&x| buf[(x, top)].symbol() == "┐").unwrap();
    let bottom = (top..H).find(|&y| buf[(left, y)].symbol() == "└").unwrap();
    let mut wide_seen = false;
    for y in top + 1..bottom {
        assert_eq!(buf[(left, y)].symbol(), "│", "left border, row {y}");
        assert_eq!(buf[(right, y)].symbol(), "│", "right border, row {y}");
        // The cells between the borders, as the terminal would lay them out.
        let mut cols = 0usize;
        let mut covered = 0usize; // cells the terminal fills from the wide glyph before them
        for x in left + 1..right {
            if covered > 0 {
                covered -= 1;
                continue;
            }
            let w = UnicodeWidthStr::width(buf[(x, y)].symbol()).max(1);
            covered = w - 1;
            cols += w;
            wide_seen |= w == 2;
        }
        assert_eq!(cols, (right - left - 1) as usize, "row {y}: {:?}", (left + 1..right).map(|x| buf[(x, y)].symbol().to_string()).collect::<Vec<_>>().concat());
    }
    assert!(wide_seen, "the wide characters are drawn");
}

#[tokio::test(start_paused = true)]
async fn a_failed_removal_does_not_replace_the_overlay_that_is_open_and_is_still_shown() {
    let mut h = listed(&["alpha", "beta"]).await;
    h.fake.script_remove_connection(Scripted::err(CoreErrorKind::Store, "the store is locked").after(Duration::from_secs(5)));
    h.press("d enter");
    h.settle().await;
    h.press("ctrl+g n");
    h.type_text("typed");
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    assert!(form_text(&mut h).contains("New connection"), "the form is still the overlay");
    assert!(field_row(&mut h, "name").1.contains("typed"), "and keeps what was typed");
    let side = sidebar_rows(&mut h, W, H).join("\n");
    assert!(side.contains("Could not remove") || screen(&mut h).join("\n").contains("Could not remove"), "the failure is shown somewhere: {side}");
    // Leaving the form, the failure is still there to read.
    h.press("esc enter");
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("Could not remove"));
    assert!(sidebar_rows(&mut h, W, H).join("\n").contains("alpha"), "alpha is still listed");
}

#[tokio::test]
async fn a_hundred_thousand_characters_pasted_into_a_field_are_cut_at_4_kib_and_draw_in_bounded_work() {
    let mut h = empty().await;
    h.press("ctrl+g n tab");
    let big = "x".repeat(100_000);
    // Work is counted, not timed: the field is cut to 4096 characters, and
    // drawing it reads them a bounded number of times over (never the 100 000).
    let steps = || super::text_field::WINDOW_STEPS.with(|s| s.get());
    let before = steps();
    let fx = h.app.update(Event::Paste(big), base());
    h.run(fx);
    let rows = screen(&mut h);
    let worked = steps() - before;
    assert!(worked <= 4 * 4_096, "paste and render read the field {worked} times for 4096 characters");
    let text = rows.join("\n");
    assert!(text.contains("cut at 4096 characters"), "the cut is shown: {text}");
    h.press("ctrl+a");
    h.type_text("y");
    let before = steps();
    let _ = screen(&mut h);
    let worked = steps() - before;
    assert!(worked <= 4 * 4_096, "a redraw read the field {worked} times");
}

#[tokio::test]
async fn e_edits_postgres_connections_only_and_says_where_to_edit_the_others() {
    let fake = FakeCore::new();
    let mut shop = conn("shop", DbKind::MySql);
    shop.port = Some(3306);
    fake.script_connections(Scripted::ok(vec![conn("alpha", DbKind::Postgres), shop, conn("notes", DbKind::Sqlite)]));
    let mut h = Harness::start(fake);
    h.settle().await;
    let bar = |h: &mut Harness| screen(h)[(H - 1) as usize].clone();
    assert!(bar(&mut h).contains("e edit"), "{:?}", bar(&mut h));
    h.press("j");
    let b = bar(&mut h);
    assert!(!b.contains("e edit") && b.contains("d remove"), "no edit hint on a MySQL row: {b:?}");
    h.press("e");
    assert!(h.app.ui.overlay.is_none(), "no Postgres-shaped form for MySQL");
    let side = sidebar_rows(&mut h, W, H).join("\n");
    assert!(side.contains("dbx app"), "it says where to edit it: {side}");
    h.press("j");
    assert!(!bar(&mut h).contains("e edit"), "nor on SQLite");
    h.press("k k");
    assert!(bar(&mut h).contains("e edit"), "back on Postgres");
    h.press("e");
    assert!(h.app.ui.overlay.is_some());
    h.press("esc j");
    h.fake.script_remove_connection(Scripted::ok(()));
    h.press("d");
    assert!(screen(&mut h).join("\n").contains("\"shop\""), "remove works for every kind");
}

#[tokio::test]
async fn a_host_with_url_syntax_is_an_error_on_the_host_field_and_nothing_is_sent() {
    for bad in ["postgres://app:S3cret@db/shop", "db?sslmode=disable#", "user@db", "db/shop", "db host", "a#b", "a?b", "x://y"] {
        let mut h = empty().await;
        fill_new(&mut h, "n", bad);
        assert!(saved(&mut h).is_none(), "{bad:?} must not save");
        assert!(tested(&h.press_fx("ctrl+t")).is_none(), "{bad:?} must not test");
        let (y, _) = field_row(&mut h, "host");
        let below = screen(&mut h)[y + 1].clone();
        assert!(below.contains("Host must be a name or an address"), "{bad:?}: {below:?}");
        assert!(!below.contains("S3cret"), "the rejected text is not echoed in the message");
    }
    for good in ["db.example.com", "127.0.0.1", "::1", "[::1]", "h1:5432,h2:5433", "db_1.internal-net"] {
        let mut h = empty().await;
        fill_new(&mut h, "n", good);
        assert!(saved(&mut h).is_some(), "{good:?} is a fine host");
    }
}

#[tokio::test]
async fn the_tls_box_says_what_it_does() {
    let mut h = empty().await;
    h.press("ctrl+g n");
    let row = field_row(&mut h, "tls").1;
    assert!(row.contains("require TLS (certificate not verified)"), "{row:?}");
    assert!(!row.contains("encrypt"), "{row:?}");
}
