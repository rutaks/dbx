//! Layer 2: the acceptance tests of slice 01, run on `App` with no I/O.

use std::time::Duration;

use ratatui::layout::Rect;

use super::testing::*;
use super::theme::{Palette, Role};
use super::*;
use crate::core::{CoreError, CoreErrorKind};

const SIDEBAR: &str = "connections";

fn titles_visible(rows: &[String]) -> bool {
    let all = rows.join("\n");
    all.contains(SIDEBAR) && all.contains("editor") && all.contains("results")
}

#[test]
fn frame_shows_three_titles_and_the_mode_bar_at_100x30() {
    let mut app = ready_app();
    let rows = frame_rows(&mut app, 100, 30);
    assert!(titles_visible(&rows), "{rows:#?}");
    assert!(rows[29].contains("SIDEBAR"), "mode bar: {:?}", rows[29]);
    assert!(rows[29].contains("ctrl+g"), "the prefix key is on screen: {:?}", rows[29]);
}

#[test]
fn frame_renders_without_panicking_at_40x10_and_at_any_tiny_size() {
    let mut app = ready_app();
    let rows = frame_rows(&mut app, 40, 10);
    assert_eq!(rows.len(), 10);
    for w in 0..=45u16 {
        for h in 0..=12u16 {
            for zoom in [false, true] {
                let mut app = ready_app();
                app.ui.zoomed = zoom;
                let _ = frame_rows(&mut app, w, h);
            }
        }
    }
}

#[test]
fn sidebar_shows_the_empty_state_when_there_are_no_connections() {
    let mut app = ready_app();
    let rows = frame_rows(&mut app, 100, 30).join("\n");
    assert!(rows.contains("No connections yet"), "{rows}");
}

#[test]
fn sidebar_says_loading_until_the_connection_list_arrives() {
    let mut app = App::new();
    app.start();
    let rows = frame_rows(&mut app, 100, 30).join("\n");
    assert!(rows.contains("loading"), "{rows}");
}

#[test]
fn a_failed_connection_list_is_shown_in_the_sidebar() {
    let mut app = App::new();
    let fx = app.start();
    let Some(Command::Core(id, _)) = fx.commands.into_iter().next() else { panic!() };
    let err = CoreError::new(CoreErrorKind::Store, "store is locked");
    app.update(Event::Core(id, CoreReply::Connections(Err(err))), base());
    let rows = frame_rows(&mut app, 100, 30).join("\n");
    assert!(rows.contains("store is locked"), "{rows}");
}

#[test]
fn prefix_then_l_moves_focus_from_sidebar_to_editor() {
    let mut app = ready_app();
    assert_eq!(app.ui.focus, PaneId::Sidebar);
    let (x, y) = find_text(&mut app, 100, 30, "editor").unwrap();
    assert_eq!(role_at(&mut app, x, y).fg, Some(Role::Overlay0));

    let fx = press(&mut app, "ctrl+g l");
    assert!(fx.repaint);
    assert_eq!(app.ui.focus, PaneId::Editor);
    assert_eq!(app.ui.mode, Mode::Normal);
    // The focused pane is marked by its title role, since panes have no border.
    assert_eq!(role_at(&mut app, x, y).fg, Some(Role::Accent));
    let rows = frame_rows(&mut app, 100, 30);
    assert!(rows[29].contains("EDITOR") && !rows[29].contains("SIDEBAR"), "{:?}", rows[29]);
}

#[test]
fn focus_keys_walk_the_three_panes() {
    let mut app = ready_app();
    press(&mut app, "ctrl+g l ctrl+g j");
    assert_eq!(app.ui.focus, PaneId::Results);
    press(&mut app, "ctrl+g k");
    assert_eq!(app.ui.focus, PaneId::Editor);
    press(&mut app, "ctrl+g h");
    assert_eq!(app.ui.focus, PaneId::Sidebar);
    press(&mut app, "ctrl+g k ctrl+g h");
    assert_eq!(app.ui.focus, PaneId::Sidebar, "no pane further left or up");
}

#[test]
fn prefix_then_z_zooms_the_focused_pane_and_again_restores() {
    let mut app = ready_app();
    press(&mut app, "ctrl+g l");
    frame_rows(&mut app, 100, 30);
    let normal = app.ui.hits.editor;
    assert!(app.ui.hits.sidebar.width > 0);

    press(&mut app, "ctrl+g z");
    let rows = frame_rows(&mut app, 100, 30);
    let main = Rect::new(0, 0, 100, 29);
    assert_eq!(app.ui.hits.editor, main);
    assert_eq!(app.ui.hits.sidebar.area(), 0);
    assert_eq!(app.ui.hits.results.area(), 0);
    assert!(!rows.join("\n").contains("results"));
    assert!(rows[29].contains("EDITOR"));

    press(&mut app, "ctrl+g z");
    frame_rows(&mut app, 100, 30);
    assert_eq!(app.ui.hits.editor, normal);
}

#[test]
fn prefix_then_an_unbound_key_returns_to_normal_mode_with_no_command() {
    let mut app = ready_app();
    let fx = press(&mut app, "ctrl+g x");
    assert!(fx.commands.is_empty());
    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.ui.focus, PaneId::Sidebar);
    assert_eq!(app.next_deadline(), None);
}

#[test]
fn esc_backs_out_of_prefix_mode() {
    let mut app = ready_app();
    press(&mut app, "ctrl+g");
    assert!(matches!(app.ui.mode, Mode::Prefix { .. }));
    press(&mut app, "esc");
    assert_eq!(app.ui.mode, Mode::Normal);
}

#[test]
fn prefix_mode_expires_after_its_timeout_when_now_is_advanced() {
    let mut app = ready_app();
    press_at(&mut app, "ctrl+g", base());
    let deadline = app.next_deadline().expect("prefix mode sets a deadline");
    assert_eq!(deadline, base() + app.ui.prefix_timeout);

    let early = app.update(Event::Timer, deadline - Duration::from_millis(1));
    assert!(!early.repaint);
    assert!(matches!(app.ui.mode, Mode::Prefix { .. }));

    let late = app.update(Event::Timer, deadline);
    assert!(late.repaint, "the mode bar changes back");
    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.next_deadline(), None);
}

#[test]
fn a_key_after_the_timeout_is_not_taken_as_a_prefix_command() {
    let mut app = ready_app();
    press_at(&mut app, "ctrl+g", base());
    let late = base() + app.ui.prefix_timeout + Duration::from_secs(1);
    press_at(&mut app, "l", late);
    assert_eq!(app.ui.focus, PaneId::Sidebar);
}

#[test]
fn prefix_then_q_emits_quit() {
    let mut app = ready_app();
    let fx = press(&mut app, "ctrl+g q");
    assert_eq!(fx.commands, vec![Command::Quit]);
}

#[test]
fn an_event_that_changes_nothing_does_not_repaint() {
    let mut app = ready_app();
    assert!(!app.update(Event::Timer, base()).repaint);
    assert!(!press(&mut app, "x").repaint);
    assert!(!press(&mut app, "ctrl+c").repaint);
    frame_rows(&mut app, 100, 30);
    let bar = app.ui.hits.mode_bar;
    assert!(!click(&mut app, bar).repaint);
    let focused = app.ui.hits.sidebar;
    assert!(!click(&mut app, focused).repaint, "clicking the focused pane changes nothing");
    assert!(app.update(Event::Resize(80, 24), base()).repaint);
}

#[test]
fn the_focused_title_is_accent_the_others_overlay0_and_a_theme_changes_only_colours() {
    let positions = |app: &mut App| {
        ["connections", "editor", "results"].map(|t| find_text(app, 100, 30, t).expect(t))
    };
    for (focus_keys, focused) in [("", 0), ("ctrl+g l", 1), ("ctrl+g l ctrl+g j", 2)] {
        let mut app = ready_app();
        press(&mut app, focus_keys);
        let at = positions(&mut app);
        let roles: Vec<_> = at.iter().map(|&(x, y)| role_at(&mut app, x, y).fg).collect();
        for (i, role) in roles.iter().enumerate() {
            let want = if i == focused { Role::Accent } else { Role::Overlay0 };
            assert_eq!(*role, Some(want), "title {i}, focus {focused}");
        }

        // Switch theme: the colours drawn change, the role assignment does not.
        let (x, y) = at[focused];
        let colour = |app: &mut App| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
            term.draw(|f| app.view(f)).unwrap();
            term.backend().buffer()[(x, y)].fg
        };
        let before = colour(&mut app);
        app.ui.theme = Palette::terminal();
        let after = colour(&mut app);
        assert_ne!(before, after);
        assert_eq!(before, Palette::default_theme().color(Role::Accent));
        assert_eq!(after, Palette::terminal().color(Role::Accent));
        assert_eq!(positions(&mut app), at);
    }
}

#[test]
fn the_sidebar_is_a_filled_column_with_one_separator_and_one_shared_divider() {
    let mut app = ready_app();
    let rows = frame_rows(&mut app, 100, 30);
    let sep = app.ui.hits.sidebar_separator;
    assert_eq!(sep.width, 1);
    assert_eq!(sep.x, app.ui.hits.sidebar.right());
    assert_eq!(role_at(&mut app, sep.x, 3).fg, Some(Role::SurfaceDim));
    assert_eq!(role_at(&mut app, 2, 20).bg, Some(Role::SidebarBg), "the sidebar is filled");
    assert!(rows[3].chars().nth(sep.x as usize) == Some('│'));
    let divider_row = app.ui.hits.results.y as usize;
    assert!(rows[divider_row].contains('─'));
    assert_eq!(rows.iter().filter(|r| r.contains('─')).count(), 1, "one divider only");
    assert!(!rows.iter().any(|r| r.contains('┌') || r.contains('└')), "no boxes");
}

#[test]
fn mode_bar_hints_come_from_the_live_keybindings() {
    let mut app = ready_app();
    press(&mut app, "ctrl+g");
    let bar = frame_rows(&mut app, 100, 30)[29].clone();
    for want in ["PREFIX", "h/j/k/l", "focus", "z", "zoom", "q", "quit", "esc", "cancel"] {
        assert!(bar.contains(want), "{want:?} missing from {bar:?}");
    }
    app.ui.keys.rebind(Action::Quit, "x");
    let bar = frame_rows(&mut app, 100, 30)[29].clone();
    assert!(bar.contains("x quit") && !bar.contains("q quit"), "{bar:?}");
}

#[test]
fn mode_bar_key_names_are_accent_and_descriptions_overlay0() {
    let mut app = ready_app();
    press(&mut app, "ctrl+g");
    let rows = frame_rows(&mut app, 100, 30);
    let row = &rows[29];
    let zoom_at = row.find("z zoom").expect("hint") as u16;
    assert_eq!(role_at(&mut app, zoom_at, 29).fg, Some(Role::Accent));
    assert_eq!(role_at(&mut app, zoom_at + 2, 29).fg, Some(Role::Overlay0));
}

#[test]
fn clicking_a_pane_focuses_it_through_the_hit_map() {
    let mut app = ready_app();
    frame_rows(&mut app, 100, 30);
    let results = app.ui.hits.results;
    assert!(results.width > 0 && results.height > 0);
    let fx = click(&mut app, results);
    assert!(fx.repaint);
    assert_eq!(app.ui.focus, PaneId::Results);
}

#[test]
fn a_reply_with_a_stale_generation_changes_no_state() {
    let mut app = App::new();
    let first = app.start();
    let second = app.start();
    let (Some(Command::Core(old, _)), Some(Command::Core(new, _))) =
        (first.commands.into_iter().next(), second.commands.into_iter().next())
    else {
        panic!("start requests the connection list")
    };
    assert_ne!(old, new);
    let conn = crate::core::Connection {
        id: crate::core::ConnectionId("a".into()),
        name: "stale".into(),
        kind: crate::core::DbKind::Postgres,
        host: "localhost".into(),
        port: Some(5432),
        database: None,
        username: String::new(),
        tls: false,
        production: false,
    };
    let stale = app.update(Event::Core(old, CoreReply::Connections(Ok(vec![conn.clone()]))), base());
    assert!(!stale.repaint);
    assert!(stale.commands.is_empty());
    assert!(matches!(app.model.connections, Loadable::Loading));

    let fresh = app.update(Event::Core(new, CoreReply::Connections(Ok(vec![conn.clone()]))), base());
    assert!(fresh.repaint);
    assert!(matches!(&app.model.connections, Loadable::Ready(c) if c == &vec![conn]));
}

#[test]
fn a_long_connection_list_stays_inside_the_sidebar_and_off_the_mode_bar() {
    use crate::core::{Connection, ConnectionId, DbKind};
    let many: Vec<Connection> = (0..40)
        .map(|i| Connection {
            id: ConnectionId(format!("c{i}")),
            name: format!("conn-{i}-{}", "x".repeat(20)),
            kind: DbKind::Postgres,
            host: "localhost".into(),
            port: None,
            database: None,
            username: String::new(),
            tls: false,
            production: false,
        })
        .collect();
    let mut app = ready_app();
    app.model.connections = Loadable::Ready(many);
    let rows = frame_rows(&mut app, 60, 12);
    assert_eq!(rows[11].trim_end(), " SIDEBAR   ctrl+g prefix  j/k move  e edit  d remove", "only mode bar text on the mode bar row");
    assert!(rows[10].contains("conn-"), "the list fills the sidebar down to its last row");
    // Nothing from the list leaks right of the sidebar either.
    let sep = app.ui.hits.sidebar_separator.x as usize;
    assert!(rows.iter().all(|r| !r.chars().skip(sep + 1).collect::<String>().contains("conn-")));
}

#[test]
fn a_click_or_a_paste_leaves_prefix_mode() {
    for other in ["click", "paste"] {
        let mut app = ready_app();
        frame_rows(&mut app, 100, 30);
        press(&mut app, "ctrl+g");
        let fx = if other == "click" {
            let r = app.ui.hits.sidebar;
            click(&mut app, r)
        } else {
            app.update(Event::Paste("select 1".into()), base())
        };
        assert!(fx.repaint, "{other}: the mode bar changes back");
        assert_eq!(app.ui.mode, Mode::Normal, "{other}");
        let fx = press(&mut app, "q");
        assert!(fx.commands.is_empty(), "{other}: q after it must not quit");
    }
}

#[test]
fn plain_pointer_movement_does_not_end_prefix_mode() {
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let mut app = ready_app();
    press(&mut app, "ctrl+g");
    let moved = MouseEvent { kind: MouseEventKind::Moved, column: 3, row: 3, modifiers: KeyModifiers::NONE };
    let fx = app.update(Event::Mouse(moved), base());
    assert!(!fx.repaint);
    assert!(matches!(app.ui.mode, Mode::Prefix { .. }));
}
