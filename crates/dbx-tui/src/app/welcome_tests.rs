//! Layer 2: the first-run welcome (slice 24a). With no connection saved, the
//! results pane says what dbpane is, which store it uses and where, and the
//! keys that add a connection, open help and quit. It reads the live key
//! bindings, goes once a connection exists, and is never drawn before the
//! store has answered.

use std::path::PathBuf;
use std::time::Duration;

use super::connections_tests::conn;
use super::keys::Action;
use super::testing::*;
use super::*;
use crate::core::{DbKind, FakeCore, Scripted, StoreInfo, StoreKind, StoreSource};
use crate::harness::Harness;

const OWN_PATH: &str = "/scratch/home/Library/Application Support/dbpane";

fn own_store() -> StoreInfo {
    StoreInfo { kind: StoreKind::Own, source: StoreSource::Auto, path: PathBuf::from(OWN_PATH) }
}

fn screen(app: &mut App) -> String {
    frame_text(app, 120, 35).join("\n")
}

/// An app told the store is empty and which store that is.
fn empty_app() -> App {
    let mut app = ready_app();
    app.ui.store = Some(own_store());
    app
}

#[test]
fn an_empty_store_shows_the_welcome_naming_the_store_and_its_path() {
    let mut app = empty_app();
    let text = screen(&mut app);
    assert!(text.contains("Welcome to dbpane"), "{text}");
    assert!(text.contains("dbpane's own store"), "which store: {text}");
    assert!(text.contains(OWN_PATH), "where it is: {text}");
}

#[test]
fn the_welcome_hints_are_the_live_key_bindings() {
    let mut app = empty_app();
    let text = screen(&mut app);
    for (action, what) in [(Action::NewConnection, "add your first"), (Action::Help, "help"), (Action::Quit, "quit")] {
        let key = format!("{} {}", app.ui.keys.prefix, app.ui.keys.key_for(action).unwrap());
        // The panes, not the mode bar on the last row.
        let line = text.lines().take(34).find(|l| l.contains(&key) && l.to_lowercase().contains(what));
        assert!(line.is_some(), "a {what} hint with {key}: {text}");
    }
    // Rebound, the welcome follows; the old key is no longer promised.
    assert!(app.ui.keys.rebind(Action::NewConnection, "m"));
    let text = screen(&mut app);
    assert!(text.lines().any(|l| l.contains("ctrl+g m") && l.to_lowercase().contains("add your first")), "{text}");
    assert!(!text.contains("ctrl+g n"), "the old key is not shown: {text}");
}

#[test]
fn the_mode_bar_offers_new_help_and_quit_as_clickable_hints_while_the_welcome_shows() {
    let mut app = empty_app();
    let rows = frame_text(&mut app, 120, 35);
    assert!(rows[34].contains("ctrl+g n") && rows[34].contains("new"), "{:?}", rows[34]);
    for action in [Action::NewConnection, Action::Help, Action::Quit] {
        assert!(app.ui.mode_hints.iter().any(|h| h.action == action), "{action:?} is clickable in the bar: {:?}", rows[34]);
    }
    let help = app.ui.mode_hints.iter().find(|h| h.action == Action::Help).copied().unwrap();
    click(&mut app, help.rect);
    assert!(matches!(app.ui.overlay, Some(Overlay::Help(_))), "clicking the hint opens help");
}

#[tokio::test]
async fn with_one_connection_the_welcome_is_gone() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn("local pg", DbKind::Postgres)]));
    let mut h = Harness::start(fake);
    h.app.ui.store = Some(own_store());
    h.settle().await;
    let text = screen(&mut h.app);
    assert!(!text.contains("Welcome to dbpane"), "{text}");
    assert!(text.contains("local pg"), "{text}");
    assert!(text.contains("Results appear here"), "the ordinary empty results hint is back: {text}");
}

#[test]
fn the_welcome_is_not_drawn_while_loading_or_when_the_store_failed() {
    let mut app = App::new();
    app.ui.store = Some(own_store());
    app.start();
    let text = screen(&mut app);
    assert!(text.contains("loading"), "{text}");
    assert!(!text.contains("Welcome to dbpane"), "nothing is known yet: {text}");

    let mut app = App::new();
    app.ui.store = Some(own_store());
    let fx = app.start();
    let Some(Command::Core(id, _)) = fx.commands.into_iter().next() else { panic!() };
    let err = crate::core::CoreError::new(crate::core::CoreErrorKind::Store, "store is locked");
    app.update(Event::Core(id, CoreReply::Connections(Err(err))), base());
    assert!(!screen(&mut app).contains("Welcome to dbpane"), "a failed store is not an empty one");
}

#[tokio::test(start_paused = true)]
async fn the_first_frame_is_drawn_before_the_store_answers() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(Vec::new()).after(Duration::from_secs(8)));
    let mut app_probe = App::new();
    let fx = app_probe.start();
    assert!(fx.repaint, "starting asks for the first frame");

    let mut h = Harness::start(fake);
    h.app.ui.store = Some(own_store());
    // Let the call start (its delay runs from here), then wait.
    h.settle().await;
    tokio::time::advance(Duration::from_secs(7)).await;
    h.settle().await;
    // Seven seconds in (a keychain prompt takes that long): the screen is
    // there, and says it is loading. No reply has been handled.
    assert_eq!(h.repaints, 0, "the store has not answered");
    let text = screen(&mut h.app);
    assert!(text.contains("connections") && text.contains("loading"), "{text}");
    assert!(!text.contains("Welcome to dbpane"), "{text}");
    tokio::time::advance(Duration::from_secs(2)).await;
    h.settle().await;
    assert_eq!(h.repaints, 1, "the answer repaints once");
    assert!(screen(&mut h.app).contains("Welcome to dbpane"));
}

#[test]
fn the_welcome_draws_through_the_filtering_painter_and_fits_any_size() {
    let mut app = ready_app();
    app.ui.store = Some(StoreInfo {
        kind: StoreKind::Named,
        source: StoreSource::Env,
        path: PathBuf::from("/tmp/\u{1b}[31mred\u{7}\u{202e}/store"),
    });
    let rows = frame_rows(&mut app, 120, 35).join("\n");
    assert!(rows.contains("Welcome to dbpane"), "{rows}");
    assert!(!rows.chars().any(|c| c == '\u{1b}' || c == '\u{7}' || c == '\u{202e}'), "control characters never reach the screen");
    for w in 0..=45u16 {
        for h in 0..=14u16 {
            let mut app = empty_app();
            let _ = frame_rows(&mut app, w, h);
        }
    }
}
