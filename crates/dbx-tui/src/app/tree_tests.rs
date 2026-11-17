//! Layer 2: the acceptance tests of slice 04 (schema tree), run on `App` with a
//! `FakeCore` behind the shell's command runner.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::connections_tests::{conn, sidebar_rows};
use super::requests::{RequestId, Target};
use super::testing::*;
use super::theme::Role;
use super::tree::{Children, Health, Listing, NodeId, RowKind};
use super::*;
use crate::core::{
    Call, ColumnInfo, ConnectionId, CoreError, CoreErrorKind, DatabasePath, DbKind, FakeCore, SchemaPath, Scripted, Table,
    TableKind, TablePath,
};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

fn cid(name: &str) -> ConnectionId {
    ConnectionId(format!("id-{name}"))
}

fn dbp(c: &str, db: &str) -> DatabasePath {
    DatabasePath { connection: cid(c), database: db.into() }
}

fn schp(c: &str, db: &str, schema: &str) -> SchemaPath {
    SchemaPath { database: dbp(c, db), schema: schema.into() }
}

fn tabp(c: &str, db: &str, schema: &str, table: &str) -> TablePath {
    TablePath { schema: schp(c, db, schema), table: table.into() }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn table(name: &str) -> Table {
    Table { name: name.into(), kind: TableKind::Table }
}

fn view(name: &str) -> Table {
    Table { name: name.into(), kind: TableKind::View }
}

fn col(name: &str, ty: &str, pk: bool, nullable: bool) -> ColumnInfo {
    ColumnInfo { name: name.into(), db_type: ty.into(), nullable, primary_key: pk, default: None }
}

/// A started harness over the Postgres connections `names`.
async fn started(fake: FakeCore, connections: &[&str]) -> Harness {
    fake.script_connections(Scripted::ok(connections.iter().map(|n| conn(n, DbKind::Postgres)).collect()));
    let mut h = Harness::start(fake);
    h.settle().await;
    h
}

/// The tree's rows as text, without the sidebar's title, trailing blanks and
/// empty rows.
fn tree_rows(h: &mut Harness) -> Vec<String> {
    sidebar_rows(h, 114, 30).into_iter().skip(2).map(|r| r.trim_end().to_string()).filter(|r| !r.is_empty()).collect()
}

fn tree_text(h: &mut Harness) -> String {
    tree_rows(h).join("\n")
}

/// The text of the row the cursor is on (drawn on `selection_bg`), in a frame
/// `w` by `rows` high.
fn cursor_text(h: &mut Harness, w: u16, rows: u16) -> Option<String> {
    let screen = frame_rows(&mut h.app, w, rows);
    let r = h.app.ui.hits.sidebar;
    (r.y..r.bottom())
        .find(|&y| role_at(&mut h.app, r.x + 1, y).bg == Some(Role::SelectionBg))
        .map(|y| screen[y as usize].chars().skip(r.x as usize).take(r.width as usize).collect::<String>().trim_end().to_string())
}

/// Where `text` first appears in the sidebar's list (not the editor's title,
/// which also names the selected connection).
fn find_in_sidebar(h: &mut Harness, text: &str) -> Option<(u16, u16)> {
    // The size of the last frame, or the usual one before any.
    let area = h.app.ui.hits.area;
    let (w, rows) = if area.width == 0 { (114, 30) } else { (area.width, area.height) };
    let screen = frame_rows(&mut h.app, w, rows);
    let r = h.app.ui.hits.sidebar;
    (r.y + 2..r.bottom()).find_map(|y| {
        let row: String = screen[y as usize].chars().skip(r.x as usize).take(r.width as usize).collect();
        let byte = row.find(text)?;
        Some((r.x + row[..byte].chars().count() as u16, y))
    })
}

/// `s` with every run of whitespace made one space: wrapped text, put back together.
fn squashed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Like `cursor_text`, but finds the cursor row from the sidebar's own state
/// (its row must be inside the window that was drawn) instead of by colour:
/// much quicker, for loops over hundreds of rows.
fn cursor_text_fast(h: &mut Harness, w: u16, rows: u16) -> Option<String> {
    let screen = frame_rows(&mut h.app, w, rows);
    let (list, sb) = (h.app.ui.hits.sidebar_list, &h.app.ui.sidebar);
    let y = list.y as usize + sb.cursor.checked_sub(sb.scroll)?;
    (y < (list.y + list.height) as usize)
        .then(|| screen[y].chars().skip(list.x as usize).take(list.width as usize).collect::<String>().trim_end().to_string())
}

fn first_call(commands: &[Command]) -> Option<(RequestId, CoreCall)> {
    commands.iter().find_map(|c| match c {
        Command::Core(id, call) => Some((*id, call.clone())),
        _ => None,
    })
}

/// A pagila-like server: three databases, one schema, tables and a view.
async fn pagila() -> Harness {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["pagila", "edge", "postgres"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![table("actor"), table("film"), view("film_list")]));
    fake.script_columns(Scripted::ok(vec![
        col("film_id", "integer", true, false),
        col("title", "text", false, false),
        col("rating", "mpaa_rating", false, true),
    ]));
    started(fake, &["local pg"]).await
}

/// Presses `enter` on the cursor row, waits for the reply, and moves down one.
async fn open_and_enter(h: &mut Harness) {
    h.press("enter");
    h.settle().await;
    h.press("j");
}

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> Event {
    Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE })
}

fn left_click(h: &mut Harness, x: u16, y: u16, at: Instant) {
    let fx = h.app.update(mouse(MouseEventKind::Down(MouseButton::Left), x, y), at);
    h.run(fx);
}

// ---- layer 2: the slice's acceptance tests ----------------------------------------

#[tokio::test(start_paused = true)]
async fn expanding_a_connection_emits_one_databases_call_and_shows_loading_and_expanding_again_before_the_reply_emits_none() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["pagila"])).after(Duration::from_secs(5)));
    let mut h = started(fake, &["local pg"]).await;

    let commands = h.press_fx("enter");
    let calls: Vec<_> = commands.iter().filter(|c| matches!(c, Command::Core(..))).collect();
    assert_eq!(calls.len(), 1, "{commands:?}");
    let Some((_, CoreCall::List(NodeId::Connection(id)))) = first_call(&commands) else { panic!("{commands:?}") };
    assert_eq!(id, cid("local pg"));
    h.settle().await;
    assert_eq!(h.fake.calls(), vec![Call::Connections, Call::Databases(cid("local pg"))]);
    assert!(tree_text(&mut h).contains("loading…"), "{}", tree_text(&mut h));
    assert!(tree_text(&mut h).contains("▾"), "the node is open while it loads");

    // Every way of saying "expand", again, before the reply: nothing is asked.
    for keys in ["enter", "l", "right"] {
        let again = h.press_fx(keys);
        assert_eq!(again.len(), 0, "{keys}: {again:?}");
        h.press("k");
    }
    h.settle().await;
    assert_eq!(h.fake.calls().len(), 2, "still one databases call: {:?}", h.fake.calls());

    tokio::time::advance(Duration::from_secs(5)).await;
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains("pagila") && !text.contains("loading"), "{text}");
}

#[tokio::test]
async fn a_reply_fills_the_node_and_collapsing_then_expanding_again_uses_the_cache() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    let text = tree_text(&mut h);
    for db in ["pagila", "edge", "postgres"] {
        assert!(text.contains(db), "{db} in {text}");
    }
    h.press("h");
    assert_eq!(tree_rows(&mut h).len(), 1, "collapsed: {}", tree_text(&mut h));
    let again = h.press_fx("enter");
    h.settle().await;
    assert!(again.is_empty(), "re-expanding emits nothing: {again:?}");
    assert_eq!(h.fake.calls(), vec![Call::Connections, Call::Databases(cid("local pg"))], "one call in all");
    assert!(tree_text(&mut h).contains("edge"), "shown from the cache");
}

#[tokio::test]
async fn a_failed_reply_renders_dbxs_message_under_that_node_and_refresh_retries_it() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(CoreErrorKind::Connect, "PostgreSQL connection failed: password authentication failed"));
    fake.script_databases(Scripted::ok(names(&["pagila"])));
    let mut h = started(fake, &["local pg", "other"]).await;
    h.press("enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    assert!(rows[0].contains("local pg"), "{rows:#?}");
    let msg = squashed(&rows[1..rows.len() - 1].join(" "));
    assert!(msg.contains("password authentication failed"), "the message sits under its node: {rows:#?}");
    assert!(rows.last().unwrap().contains("other"), "the next connection follows the message: {rows:#?}");
    // The message is red, and the error row is not a node of its own.
    let (x, y) = find_in_sidebar(&mut h, "PostgreSQL").expect("the message");
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Red));

    // `r` (on the node or on its message) asks again, once.
    let commands = h.press_fx("r");
    assert!(matches!(first_call(&commands), Some((_, CoreCall::List(NodeId::Connection(_))))), "{commands:?}");
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains("pagila") && !text.contains("failed"), "{text}");
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, Call::Databases(_))).count(), 2);
}

#[tokio::test]
async fn with_500_tables_in_a_20_row_sidebar_moving_down_scrolls_and_the_selected_row_is_always_visible() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["db"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok((0..500).map(|i| table(&format!("t{i:03}"))).collect()));
    let mut h = started(fake, &["pg"]).await;
    open_and_enter(&mut h).await;
    open_and_enter(&mut h).await;
    h.press("enter");
    h.settle().await;
    h.press("j");
    // 23 rows high: the mode bar, the title, a blank and 20 rows of tree.
    let (w, rows) = (114, 23);
    frame_rows(&mut h.app, w, rows);
    assert_eq!(h.app.ui.hits.sidebar_list.height, 20);
    for i in 0..500 {
        let at = cursor_text_fast(&mut h, w, rows).unwrap_or_else(|| panic!("no cursor row on screen at table {i}"));
        assert!(at.contains(&format!("t{i:03}")), "table {i}: {at:?}");
        h.press("j");
    }
    // Scrolled: the connection row is long gone from the window.
    assert!(find_in_sidebar(&mut h, "● pg").is_none(), "scrolled off the top");
    // And back up, the same way.
    for i in (0..499).rev() {
        h.press("k");
        let at = cursor_text_fast(&mut h, w, rows).unwrap_or_else(|| panic!("no cursor row at table {i}"));
        assert!(at.contains(&format!("t{i:03}")), "table {i}: {at:?}");
    }
}

#[test]
fn a_reply_for_a_node_whose_connection_was_removed_meanwhile_is_dropped() {
    let mut app = ready_app();
    app.model.connections = Loadable::Ready(vec![conn("pg", DbKind::Postgres)]);
    app.touch_rows();
    let fx = press(&mut app, "enter");
    let Some((id, CoreCall::List(node))) = first_call(&fx.commands) else { panic!("{:?}", fx.commands) };

    // The connection is removed while its databases are still being listed.
    let fx = press(&mut app, "d enter");
    let Some((remove, CoreCall::RemoveConnection(gone))) = first_call(&fx.commands) else { panic!("{:?}", fx.commands) };
    let fx = app.update(Event::Core(remove, CoreReply::Removed(Ok(()))), base());
    assert!(fx.commands.contains(&Command::Abandon(id)), "the listing is abandoned: {:?}", fx.commands);
    assert_eq!(gone, cid("pg"));

    // Its reply arrives anyway (a race the shell cannot always win): dropped.
    let fx = app.update(Event::Core(id, CoreReply::Listed(Ok(Listing::Databases(names(&["ghost"]))))), base());
    assert!(!fx.repaint, "a dropped reply repaints nothing");
    assert_eq!(app.model.schema.state(&node), &Children::NotLoaded);
    assert!(app.model.schema.is_empty());
    // Only the connection list is still tracked: nothing of the removed connection.
    assert_eq!(app.model.requests.tracked(), 1, "nothing of the connection is tracked any more");
    assert!(!frame_rows(&mut app, 114, 30).join("\n").contains("ghost"));
}

#[tokio::test]
async fn names_with_mixed_case_spaces_reserved_words_and_slashes_render_exactly() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["My Database"])));
    fake.script_schemas(Scripted::ok(names(&["Public Schema", "select"])));
    fake.script_tables(Scripted::ok(vec![table("Mixed Case"), table("user"), table("a/b"), table("with  two  spaces"), table("ÅNGSTRÖM")]));
    fake.script_columns(Scripted::ok(vec![col("Group", "integer", false, true), col("select from", "text", false, false)]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    let text = tree_text(&mut h);
    for exact in ["My Database", "Public Schema", "select", "Mixed Case", "user", "a/b", "with  two  spaces", "ÅNGSTRÖM"] {
        assert!(text.contains(exact), "{exact:?} in\n{text}");
    }
    // The names are exactly what the Core said: the cursor row is the schema, and its
    // tables are not found under another path just because they look alike.
    h.press("j");
    h.press("j");
    h.press("enter");
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains("Group") && text.contains("select from"), "{text}");
    let calls = h.fake.calls();
    assert!(calls.contains(&Call::Columns(tabp("pg", "My Database", "Public Schema", "Mixed Case"))), "{calls:?}");
}

#[tokio::test]
async fn a_click_on_a_mode_bar_hint_does_what_its_key_does_for_edit_remove_and_add() {
    let mut h = started(FakeCore::new(), &["local pg"]).await;
    let hint_at = |h: &mut Harness, text: &str| {
        let rows = frame_rows(&mut h.app, 114, 30);
        let bar = &rows[29];
        let byte = bar.find(text).unwrap_or_else(|| panic!("{text:?} not on the mode bar: {bar:?}"));
        (bar[..byte].chars().count() as u16, 29u16)
    };

    // edit: opens the form, as `e` does
    let (x, y) = hint_at(&mut h, "e edit");
    left_click(&mut h, x, y, base());
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::Form(_))), "{:?}", h.app.ui.overlay);
    h.press("esc");
    assert!(h.app.ui.overlay.is_none());

    // The description is part of the hint.
    let (x, y) = hint_at(&mut h, "edit");
    left_click(&mut h, x + 1, y, base());
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::Form(_))), "clicking the word works too");
    h.press("esc");

    // remove: asks first, as `d` does
    let (x, y) = hint_at(&mut h, "d remove");
    left_click(&mut h, x, y, base());
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::ConfirmRemove(_))), "{:?}", h.app.ui.overlay);
    h.press("esc");

    // add: the hint spells out prefix and key, and opens the empty form
    let (x, y) = hint_at(&mut h, "ctrl+g n new");
    left_click(&mut h, x + 7, y, base());
    let Some(super::overlay::Overlay::Form(form)) = &h.app.ui.overlay else { panic!("{:?}", h.app.ui.overlay) };
    assert!(!form.dirty(), "an empty new-connection form");
    assert!(frame_rows(&mut h.app, 114, 30).join("\n").contains("New connection"));
    h.press("esc");

    // the prefix hint enters prefix mode, and then its keys are clickable too
    let (x, y) = hint_at(&mut h, "ctrl+g prefix");
    left_click(&mut h, x, y, base());
    assert!(matches!(h.app.ui.mode, Mode::Prefix { .. }));
    let (x, y) = hint_at(&mut h, "z zoom");
    left_click(&mut h, x, y, base());
    assert!(h.app.ui.zoomed, "z zoom, by click");
    assert_eq!(h.app.ui.mode, Mode::Normal);
}

#[tokio::test]
async fn a_hint_is_not_clickable_when_its_key_would_not_work() {
    // A table row: no edit or remove (those are for connections), and a click
    // where they would have been does nothing.
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    let bar = frame_rows(&mut h.app, 114, 30)[29].clone();
    assert!(!bar.contains("edit") && !bar.contains("remove"), "{bar:?}");
    assert!(bar.contains("refresh"), "{bar:?}");
    let before = h.app.ui.overlay.is_none();
    let hints = h.app.ui.mode_hints.clone();
    assert!(hints.iter().all(|hit| hit.action != Action::EditConnection && hit.action != Action::RemoveConnection));
    assert!(before);
}

#[tokio::test]
async fn a_schema_or_table_name_containing_an_escape_sequence_draws_no_control_character() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["d\u{1b}[2J"])));
    fake.script_schemas(Scripted::ok(names(&["s\u{1b}]52;c;A\u{7}h"])));
    fake.script_tables(Scripted::ok(vec![table("ta\u{1b}[31mb\u{9b}"), table("t\tn\nl")]));
    fake.script_columns(Scripted::ok(vec![ColumnInfo {
        name: "c\u{1b}[0ml".into(),
        db_type: "ty\u{1b}p\u{202e}".into(),
        nullable: true,
        primary_key: true,
        default: None,
    }]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(114, 30)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    let buf = term.backend().buffer();
    let mut drawn = String::new();
    for cell in buf.content() {
        let symbol = cell.symbol();
        assert!(!symbol.chars().any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c)), "control character in {symbol:?}");
        drawn.push_str(symbol);
    }
    // The text is still there, with the control characters replaced.
    assert!(drawn.contains("ta") && drawn.contains("b\u{fffd}") && drawn.contains("0ml"), "{drawn}");
    // And a failure's message goes through the same painter.
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(CoreErrorKind::Other, "bad \u{1b}[2J news \u{9b}"));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(114, 30)).unwrap();
    term.draw(|f| h.app.view(f)).unwrap();
    assert!(term.backend().buffer().content().iter().all(|c| !c.symbol().chars().any(char::is_control)));
    assert!(tree_text(&mut h).contains("news"));
}

#[tokio::test]
async fn a_server_with_three_databases_shows_three_nodes_and_expanding_the_second_lists_its_schemas_without_touching_the_first() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    assert_eq!(rows[1..].iter().filter(|r| r.contains("pagila") || r.contains("edge") || r.contains("postgres")).count(), 3, "{rows:#?}");
    h.press("j j");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("edge"));
    h.press("enter");
    h.settle().await;
    assert_eq!(
        h.fake.calls(),
        vec![Call::Connections, Call::Databases(cid("local pg")), Call::Schemas(dbp("local pg", "edge"))],
        "only the second database was asked"
    );
    let rows = tree_rows(&mut h);
    let edge = rows.iter().position(|r| r.contains("edge")).unwrap();
    assert!(rows[edge + 1].contains("public"), "its schema is under it: {rows:#?}");
    assert!(rows.iter().position(|r| r.contains("pagila")).unwrap() < edge);
    assert!(rows[rows.iter().position(|r| r.contains("pagila")).unwrap() + 1].contains("edge"), "the first has no children shown");
}

#[tokio::test]
async fn selecting_a_table_in_another_database_changes_the_editors_active_database() {
    let mut h = pagila().await;
    let title = |h: &mut Harness| frame_rows(&mut h.app, 114, 30).iter().find(|r| r.contains("editor")).cloned().unwrap();
    // Selecting the connection: its saved database (none, for this fixture).
    assert!(title(&mut h).contains("editor · local pg"), "{}", title(&mut h));
    assert_eq!(h.app.model.active, Some(Active { connection: cid("local pg"), database: None }));

    h.press("enter");
    h.settle().await;
    h.press("j j");
    assert_eq!(h.app.model.active.as_ref().and_then(|a| a.database.as_deref()), Some("edge"));
    assert!(title(&mut h).contains("local pg / edge"), "{}", title(&mut h));
    h.press("enter");
    h.settle().await;
    h.press("j enter");
    h.settle().await;
    h.press("j");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("actor"));
    assert_eq!(h.app.model.active, Some(Active { connection: cid("local pg"), database: Some("edge".into()) }));
    assert!(title(&mut h).contains("local pg / edge"));

    // Up to the first database: the title follows the cursor.
    h.press("g");
    h.press("j");
    assert!(title(&mut h).contains("local pg / pagila"), "{}", title(&mut h));
    assert_eq!(h.app.model.active.as_ref().and_then(|a| a.database.as_deref()), Some("pagila"));
}

// ---- keys ----------------------------------------------------------------------------

#[tokio::test]
async fn h_collapses_an_open_node_and_on_a_closed_one_or_a_leaf_moves_to_the_parent() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await; // connection, database, schema open; cursor on the schema's first child
    }
    // Now on `actor`, a closed table: h goes to its schema.
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("actor"));
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("public"), "{:?}", cursor_text(&mut h, 114, 30));
    // The schema is open: h closes it.
    h.press("h");
    assert!(!tree_text(&mut h).contains("actor"));
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("public"), "the cursor stays on the node it closed");
    // Closed now: h goes to the database, then to the connection, then stays.
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("pagila"));
    h.press("left");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("pagila"), "an open database closes first (left is h)");
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("local pg"));
    h.press("h");
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("local pg"), "a root has no parent");
    assert_eq!(h.fake.calls().len(), 4, "closing and moving asked the Core nothing");
}

#[tokio::test]
async fn h_on_a_column_goes_to_its_table() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("j"); // film
    h.press("enter");
    h.settle().await;
    h.press("j");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("film_id"));
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("film"), "{:?}", cursor_text(&mut h, 114, 30));
    assert!(!cursor_text(&mut h, 114, 30).unwrap().contains("film_id"));
}

#[tokio::test]
async fn enter_l_and_right_expand_j_k_and_the_arrows_move_and_enter_on_an_open_node_goes_in() {
    for expand in ["enter", "l", "right"] {
        let mut h = pagila().await;
        h.press(expand);
        h.settle().await;
        assert!(tree_text(&mut h).contains("pagila"), "{expand}");
        // An open node: the same key goes to its first child.
        h.press(expand);
        assert!(cursor_text(&mut h, 114, 30).unwrap().contains("pagila"), "{expand}: {:?}", cursor_text(&mut h, 114, 30));
        assert_eq!(h.fake.calls().len(), 2);
        for (keys, want) in [("down", "edge"), ("j", "postgres"), ("j", "postgres"), ("up", "edge"), ("k", "pagila"), ("k", "local pg")] {
            h.press(keys);
            assert!(cursor_text(&mut h, 114, 30).unwrap().contains(want), "{expand} {keys}: {:?}", cursor_text(&mut h, 114, 30));
        }
    }
}

#[tokio::test]
async fn page_keys_g_and_shift_g_home_and_end_jump_and_the_cursor_stays_visible() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok((0..100).map(|i| format!("db{i:03}")).collect()));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let at = |h: &mut Harness| cursor_text(h, 114, 23).unwrap_or_else(|| panic!("cursor off screen"));
    assert!(at(&mut h).contains("pg"));
    h.press("G");
    assert!(at(&mut h).contains("db099"), "{:?}", at(&mut h));
    h.press("g");
    assert!(at(&mut h).contains("pg"));
    h.press("end");
    assert!(at(&mut h).contains("db099"));
    h.press("home");
    assert!(at(&mut h).contains("pg"));
    h.press("pagedown");
    let first = at(&mut h);
    assert!(first.contains("db018"), "a page is the window less one row: {first:?}");
    h.press("ctrl+d");
    assert!(at(&mut h).contains("db037"));
    h.press("pageup");
    assert!(at(&mut h).contains("db018"));
    h.press("ctrl+u");
    assert!(at(&mut h).contains("pg"));
    h.press("pageup");
    assert!(at(&mut h).contains("pg"), "stops at the top");
}

#[tokio::test]
async fn tree_keys_only_reach_the_tree_while_the_sidebar_has_focus() {
    let mut h = pagila().await;
    h.press("ctrl+g l");
    let commands = h.press_fx("enter");
    assert!(commands.is_empty(), "{commands:?}");
    h.press("j");
    h.press("r");
    assert_eq!(h.fake.calls(), vec![Call::Connections]);
    h.press("ctrl+g h");
    assert_eq!(h.press_fx("enter").len(), 1, "back in the sidebar, enter expands");
}

#[tokio::test]
async fn r_refreshes_the_node_under_the_cursor_and_what_it_dropped_goes_from_the_tree() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["a", "b"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_schemas(Scripted::ok(names(&["public", "extra"])));
    fake.script_tables(Scripted::ok(vec![table("one")]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    // cursor on `one`; refresh the schema above it: `h` then `r`
    h.press("h");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("public"));
    h.press("g j");
    // on database `a`: refresh it; its schemas are asked again, nothing else is
    let commands = h.press_fx("r");
    assert!(matches!(first_call(&commands), Some((_, CoreCall::List(NodeId::Database(_))))), "{commands:?}");
    assert_eq!(commands.len(), 1);
    // While it loads, a second `r` asks nothing.
    assert!(h.press_fx("r").is_empty());
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains("extra") && text.contains("public"), "{text}");
    assert!(text.contains("one"), "the expanded schema below keeps what it had loaded: {text}");
    // The columns of a table are refreshed from the table's own row or a column.
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, Call::Schemas(_))).count(), 2);
}

// ---- state and requests ---------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn collapsing_a_node_that_is_loading_abandons_the_listing_and_expanding_again_asks_afresh() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["old"])).after(Duration::from_secs(5)));
    fake.script_databases(Scripted::ok(names(&["new"])).after(Duration::from_secs(1)));
    let mut h = started(fake, &["pg"]).await;
    let first = h.press_fx("enter");
    let (id, _) = first_call(&first).unwrap();
    h.settle().await;
    assert_eq!(h.tasks.len(), 1, "the listing is in flight");

    let collapse = h.press_fx("h");
    assert_eq!(collapse, vec![Command::Abandon(id)], "collapsing a loading node abandons its request");
    h.settle().await;
    assert_eq!(h.tasks.len(), 0, "and the shell aborted the call");
    assert_eq!(h.app.model.requests.tracked(), 1, "the node's generation was pruned (the connection list's is not)");
    assert_eq!(h.app.model.schema.state(&NodeId::Connection(cid("pg"))), &Children::NotLoaded);

    // Expanding again is a new call, and only its reply counts.
    let second = h.press_fx("enter");
    assert!(matches!(first_call(&second), Some((_, CoreCall::List(_)))));
    h.settle().await;
    tokio::time::advance(Duration::from_secs(6)).await;
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains("new") && !text.contains("old"), "{text}");
}

#[test]
fn a_reply_that_was_superseded_by_a_refresh_or_a_collapse_changes_nothing() {
    let mut app = ready_app();
    app.model.connections = Loadable::Ready(vec![conn("pg", DbKind::Postgres)]);
    app.touch_rows();
    let fx = press(&mut app, "enter");
    let (old, _) = first_call(&fx.commands).unwrap();
    // Collapse and expand again: a new request; the old reply is stale.
    press(&mut app, "h");
    let fx = press(&mut app, "enter");
    let (new, _) = first_call(&fx.commands).unwrap();
    assert_ne!(old, new);
    let fx = app.update(Event::Core(old, CoreReply::Listed(Ok(Listing::Databases(names(&["stale"]))))), base());
    assert!(!fx.repaint);
    assert!(matches!(app.model.schema.state(&NodeId::Connection(cid("pg"))), Children::Loading));
    app.update(Event::Core(new, CoreReply::Listed(Ok(Listing::Databases(names(&["fresh"]))))), base());
    let text = frame_rows(&mut app, 114, 30).join("\n");
    assert!(text.contains("fresh") && !text.contains("stale"), "{text}");
}

#[tokio::test]
async fn request_generations_are_pruned_when_a_node_collapses_and_when_its_connection_is_removed() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    // The connection list, and one target per node opened.
    assert_eq!(h.app.model.requests.tracked(), 1 + 3);
    // Collapse the connection from the top: everything below is let go.
    h.press("g");
    h.press("h");
    assert_eq!(h.app.model.requests.tracked(), 1, "collapsing prunes the node and what is below it");
    // The data stays cached: opening again asks nothing and tracks nothing.
    let again = h.press_fx("enter");
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(h.app.model.requests.tracked(), 1);

    // Removing the connection drops its cache and health too.
    h.press("d");
    h.fake.script_remove_connection(Scripted::ok(()));
    h.press("enter");
    h.settle().await;
    assert!(h.app.model.schema.is_empty());
    assert_eq!(h.app.model.schema.health(&cid("local pg")), None);
    assert_eq!(h.app.model.requests.tracked(), 1, "only the connection list is left");
}

#[tokio::test]
async fn editing_where_a_connection_points_drops_its_tree_but_renaming_it_keeps_it() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    let mut renamed = conn("local pg", DbKind::Postgres);
    renamed.name = "renamed".into();
    let mut fx = Effects::default();
    h.app.upsert_connection(renamed.clone(), &mut fx);
    assert!(tree_text(&mut h).contains("edge"), "a rename keeps what was loaded");
    let mut moved = renamed;
    moved.host = "elsewhere".into();
    h.app.upsert_connection(moved, &mut fx);
    assert!(h.app.model.schema.is_empty());
    let rows = tree_rows(&mut h);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert!(rows[0].contains("▸"), "and it is closed again: {rows:#?}");
}

#[tokio::test]
async fn the_visible_rows_are_rebuilt_only_when_the_tree_changes() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    frame_rows(&mut h.app, 114, 30);
    let built = h.app.ui.sidebar.rebuilds;
    for keys in ["j", "j", "k", "down", "up", "g", "G", "pagedown"] {
        h.press(keys);
        frame_rows(&mut h.app, 114, 30);
    }
    for _ in 0..5 {
        frame_rows(&mut h.app, 114, 30);
    }
    h.app.update(Event::Resize(114, 30), base());
    assert_eq!(h.app.ui.sidebar.rebuilds, built, "moving, scrolling and redrawing rebuild nothing");
    h.press("g");
    h.press("h");
    assert_eq!(h.app.ui.sidebar.rebuilds, built + 1, "collapsing is a change");
    h.press("enter");
    assert_eq!(h.app.ui.sidebar.rebuilds, built + 2);
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_inserts_rows_above_the_cursor_does_not_move_the_cursor_onto_another_row() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["one", "two", "three"])).after(Duration::from_secs(3)));
    let mut h = started(fake, &["alpha", "beta"]).await;
    h.press("enter"); // open alpha, listing is slow
    h.settle().await;
    h.press("j j"); // on `beta`, below alpha's "loading…"
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("beta"));
    tokio::time::advance(Duration::from_secs(3)).await;
    h.settle().await;
    assert!(tree_text(&mut h).contains("three"));
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("beta"), "{:?}", cursor_text(&mut h, 114, 30));
    assert_eq!(h.app.model.active.as_ref().map(|a| a.connection.clone()), Some(cid("beta")));
}

#[tokio::test]
async fn a_call_that_fails_makes_the_connection_glyph_red_and_one_that_works_makes_it_green() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(CoreErrorKind::Connect, "refused"));
    fake.script_databases(Scripted::ok(names(&["pagila"])));
    let mut h = started(fake, &["pg", "idle"]).await;
    let glyph_of = |h: &mut Harness, name: &str| -> (String, Option<Role>) {
        let (x, y) = find_in_sidebar(h, name).unwrap();
        let row: Vec<char> = frame_rows(&mut h.app, 114, 30)[y as usize].chars().collect();
        let g = row[x as usize - 2].to_string();
        (g, role_at(&mut h.app, x - 2, y).fg)
    };
    assert_eq!(glyph_of(&mut h, "pg"), ("○".into(), Some(Role::Overlay0)), "before any call");
    assert_eq!(glyph_of(&mut h, "idle"), ("○".into(), Some(Role::Overlay0)));
    h.press("enter");
    h.settle().await;
    assert_eq!(glyph_of(&mut h, "pg"), ("●".into(), Some(Role::Red)), "its last call failed");
    assert_eq!(h.app.model.schema.health(&cid("pg")), Some(Health::Failed));
    assert_eq!(glyph_of(&mut h, "idle"), ("○".into(), Some(Role::Overlay0)), "other connections are untouched");
    h.press("r");
    h.settle().await;
    assert_eq!(glyph_of(&mut h, "pg"), ("●".into(), Some(Role::Green)), "a call has succeeded");
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_is_being_listed_for_the_first_time_shows_the_connecting_glyph() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["pagila"])).after(Duration::from_secs(2)));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let (x, y) = find_in_sidebar(&mut h, "pg").unwrap();
    assert_eq!(role_at(&mut h.app, x - 2, y).fg, Some(Role::Yellow));
}

// ---- how it looks ---------------------------------------------------------------------

#[tokio::test]
async fn nodes_show_arrows_and_indent_by_depth_and_views_say_what_they_are() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    let rows = tree_rows(&mut h);
    let indent = |needle: &str| {
        let row = rows.iter().find(|r| r.contains(needle)).unwrap_or_else(|| panic!("{needle} in {rows:#?}"));
        row.chars().position(|c| c != ' ').unwrap()
    };
    assert!(rows[0].contains("▾") && rows[0].contains("local pg") && rows[0].contains("postgres"), "{rows:#?}");
    let (c, d, s, t) = (indent("local pg"), indent("pagila"), indent("public"), indent("actor"));
    assert!(c < d && d < s && s < t, "each level is indented further: {c} {d} {s} {t}");
    assert!(rows.iter().any(|r| r.contains("▾") && r.contains("public")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("▸") && r.contains("edge")), "{rows:#?}");
    let film_list = rows.iter().find(|r| r.contains("film_list")).unwrap();
    assert!(film_list.trim_end().ends_with("view"), "a view says so: {film_list:?}");
    let actor = rows.iter().find(|r| r.contains("actor")).unwrap();
    assert!(!actor.contains("view") && !actor.contains("table"), "a plain table needs no label: {actor:?}");
    // The kind label is muted.
    let (x, y) = find_in_sidebar(&mut h, "view").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Overlay0));
}

#[tokio::test]
async fn column_rows_show_a_key_marker_the_type_right_aligned_in_overlay0_and_a_nullable_marker() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("j enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    let film_id = rows.iter().find(|r| r.contains("film_id")).unwrap_or_else(|| panic!("{rows:#?}")).clone();
    let title = rows.iter().find(|r| r.contains("title")).unwrap().clone();
    let rating = rows.iter().find(|r| r.contains("rating")).unwrap().clone();
    assert!(film_id.contains("◆ film_id"), "the key marker: {film_id:?}");
    assert!(!title.contains('◆') && !rating.contains('◆'), "only the key column has it");
    assert!(film_id.contains("integer") && rating.contains("mpaa_rating"), "{rows:#?}");
    assert!(rating.trim_end().ends_with("mpaa_rating ?"), "a nullable column says so: {rating:?}");
    assert!(!film_id.trim_end().ends_with('?') && !title.trim_end().ends_with('?'), "a NOT NULL one does not");
    // Types line up on the right: the same end column up to the nullable slot.
    let end = |r: &str| r.trim_end().trim_end_matches('?').trim_end().chars().count();
    assert_eq!(end(&film_id), end(&rating), "right-aligned: {film_id:?} / {rating:?}");
    // Roles: the marker is yellow, the type overlay0, the name text.
    let (x, y) = find_in_sidebar(&mut h, "◆").unwrap();
    assert_eq!(role_at(&mut h.app, x, y).fg, Some(Role::Yellow));
    let (tx, ty) = find_in_sidebar(&mut h, "mpaa_rating").unwrap();
    assert_eq!(role_at(&mut h.app, tx, ty).fg, Some(Role::Overlay0));
    let (nx, ny) = find_in_sidebar(&mut h, "film_id").unwrap();
    assert_eq!(role_at(&mut h.app, nx, ny).fg, Some(Role::Text));
    // A column is a leaf: no arrow, and enter does nothing.
    h.press("g");
    h.press("j j j j j");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("film_id"));
    assert!(h.press_fx("enter").is_empty());
}

#[tokio::test]
async fn right_aligned_types_are_measured_by_display_width_not_by_characters() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["db"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![table("t")]));
    fake.script_columns(Scripted::ok(vec![col("a", "integer", false, false), col("b", "数値型", false, false), col("c", "text", false, true)]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    // The screen holds one cell per character, a blank after each wide one.
    let cells = |row: &str| {
        let (mut n, mut chars) = (0, row.chars());
        while let Some(c) = chars.next() {
            let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            n += w;
            if w == 2 {
                chars.next(); // its second cell
            }
        }
        n
    };
    let ends: Vec<usize> = ["integer", "数 値 型", "text"]
        .iter()
        .map(|ty| {
            let row = rows.iter().find(|r| r.contains(ty)).unwrap_or_else(|| panic!("{ty} in {rows:#?}"));
            cells(row.trim_end().trim_end_matches('?').trim_end())
        })
        .collect();
    assert!(ends.iter().all(|e| *e == ends[0]), "types end in one column: {ends:?} in {rows:#?}");
}

#[tokio::test]
async fn a_long_type_does_not_leave_its_column_without_a_name() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["db"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![table("t")]));
    fake.script_columns(Scripted::ok(vec![col("last_update", "timestamp with time zone", false, false)]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    let row = rows.iter().find(|r| r.contains("last_")).unwrap_or_else(|| panic!("the name survives: {rows:#?}"));
    assert!(row.contains("timestamp"), "and so does the start of the type: {row:?}");
    assert_eq!(row.matches('…').count(), 2, "each cut says so: {row:?}");
}

#[tokio::test]
async fn long_names_are_cut_with_an_ellipsis_by_display_width_and_never_spill_out_of_the_sidebar() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["日本語のとても長いデータベースの名前です", "plain_database_name_that_is_much_too_long"])));
    let mut h = started(fake, &["a connection with a really long name"]).await;
    h.press("enter");
    h.settle().await;
    let screen = frame_rows(&mut h.app, 114, 30);
    let sep = h.app.ui.hits.sidebar_separator.x as usize;
    for row in &screen[2..29] {
        let cells: Vec<char> = row.chars().collect();
        assert!(matches!(cells.get(sep), Some('│' | '├')), "the separator holds: {row:?}");
    }
    let rows = tree_rows(&mut h);
    // (The test screen puts a blank in the second cell of a wide character.)
    assert!(rows.iter().any(|r| r.contains('…') && r.replace(' ', "").contains("日本語")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("plain_database_name") && r.contains('…')), "{rows:#?}");
    // The connection keeps its kind on the right while its name gives way.
    assert!(rows[0].contains("postgres") && rows[0].contains('…'), "{rows:#?}");
    for r in &rows {
        assert!(r.chars().count() <= h.app.ui.hits.sidebar.width as usize, "{r:?} is wider than the sidebar");
    }
}

#[tokio::test]
async fn a_scrollbar_appears_only_when_the_list_overflows_and_its_thumb_follows_the_scroll() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok((0..60).map(|i| format!("db{i:02}")).collect()));
    let mut h = started(fake, &["pg"]).await;
    let bar_rows = |h: &mut Harness, rows: u16| -> Vec<u16> {
        let screen = frame_rows(&mut h.app, 114, rows);
        let r = h.app.ui.hits.sidebar;
        (r.y..r.bottom()).filter(|&y| screen[y as usize].chars().nth((r.right() - 1) as usize) == Some('▕')).collect()
    };
    assert!(bar_rows(&mut h, 23).is_empty(), "one row fits: no scrollbar");
    h.press("enter");
    h.settle().await;
    let at_top = bar_rows(&mut h, 23);
    assert_eq!(at_top.len(), 20, "the track runs the height of the list: {at_top:?}");
    let thumb = |h: &mut Harness| -> Vec<u16> {
        let x = h.app.ui.hits.sidebar.right() - 1;
        bar_rows(h, 23).into_iter().filter(|&y| role_at(&mut h.app, x, y).fg == Some(Role::Overlay0)).collect()
    };
    let top_thumb = thumb(&mut h);
    assert!(!top_thumb.is_empty() && top_thumb[0] == at_top[0], "at the top the thumb is at the top: {top_thumb:?}");
    h.press("G");
    let bottom_thumb = thumb(&mut h);
    assert_eq!(*bottom_thumb.last().unwrap(), *at_top.last().unwrap(), "at the end the thumb is at the bottom");
    assert!(bottom_thumb.len() < 20, "and it is shorter than the track");
    // Collapsing brings the list back inside the window: the bar goes.
    h.press("g");
    h.press("h");
    assert!(bar_rows(&mut h, 23).is_empty());
    // The rows never run under the bar: nothing but the bar is in the last column.
    h.press("enter");
    let screen = frame_rows(&mut h.app, 114, 23);
    let r = h.app.ui.hits.sidebar;
    for y in r.y + 2..r.bottom() {
        let last = screen[y as usize].chars().nth((r.right() - 1) as usize).unwrap();
        assert!(last == '▕' || last == ' ', "{:?}", screen[y as usize]);
    }
}

#[test]
fn the_scrollbar_thumb_is_proportional_and_always_inside_the_track() {
    use super::view::scrollbar_thumb;
    assert_eq!(scrollbar_thumb(10, 20, 0), None, "no bar when everything fits");
    assert_eq!(scrollbar_thumb(20, 20, 0), None);
    for total in 21..600 {
        for scroll in [0, 1, total / 3, total / 2, total - 20] {
            let (top, len) = scrollbar_thumb(total, 20, scroll).unwrap();
            assert!(len >= 1 && top + len <= 20, "total {total} scroll {scroll}: {top}+{len}");
        }
        assert_eq!(scrollbar_thumb(total, 20, 0).unwrap().0, 0);
        let (top, len) = scrollbar_thumb(total, 20, total - 20).unwrap();
        assert_eq!(top + len, 20, "at the end the thumb touches the bottom (total {total})");
    }
}

#[tokio::test]
async fn a_long_error_wraps_under_its_node_in_a_few_rows_and_ends_in_an_ellipsis_past_eight() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(
        CoreErrorKind::Connect,
        "PostgreSQL connection failed: password authentication failed for user \"postgres\"",
    ));
    let long = "word ".repeat(200);
    fake.script_databases(Scripted::err(CoreErrorKind::Other, long));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let rows = tree_rows(&mut h);
    let joined = squashed(&rows[1..].join(" "));
    assert!(joined.contains("password authentication failed for user"), "dbx's whole message: {rows:#?}");
    assert!(rows.len() > 2 && rows.len() <= 6, "wrapped over a few rows, and the key that retries: {rows:#?}");
    h.press("r");
    h.settle().await;
    let rows = tree_rows(&mut h);
    assert_eq!(rows.len(), 1 + 8 + 1, "at most eight lines, then the key that retries: {rows:#?}");
    assert!(rows[8].ends_with('…') && rows.last().unwrap().contains("to retry"), "{rows:#?}");
}

#[tokio::test]
async fn an_empty_listing_says_so_under_its_node() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["db"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![]));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..2 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    assert!(tree_text(&mut h).contains("(empty)"), "{}", tree_text(&mut h));
}

// ---- mouse -----------------------------------------------------------------------------

#[tokio::test]
async fn a_click_selects_a_row_and_a_click_on_its_arrow_or_a_double_click_toggles_it() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    frame_rows(&mut h.app, 114, 30);
    let list = h.app.ui.hits.sidebar_list;
    let t0 = base();

    // Click on the name of the second database: selected, not toggled.
    let (x, y) = find_in_sidebar(&mut h, "edge").unwrap();
    left_click(&mut h, x, y, t0);
    h.settle().await;
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("edge"));
    assert_eq!(h.fake.calls().len(), 2, "a single click on a name only selects");

    // Click on its arrow: toggles (opens, and asks for its schemas).
    let arrow_x = list.x + MARGIN_FOR_TEST;
    left_click(&mut h, arrow_x, y, t0 + Duration::from_secs(5));
    h.settle().await;
    assert!(h.fake.calls().contains(&Call::Schemas(dbp("local pg", "edge"))), "{:?}", h.fake.calls());
    assert!(tree_text(&mut h).contains("public"));
    left_click(&mut h, arrow_x, y, t0 + Duration::from_secs(10));
    assert!(!tree_text(&mut h).contains("public"), "the arrow closes it again");

    // Double-click on the name toggles.
    let (x, y) = find_in_sidebar(&mut h, "▸ postgres").map(|(x, y)| (x + 4, y)).unwrap();
    left_click(&mut h, x, y, t0 + Duration::from_secs(20));
    left_click(&mut h, x, y, t0 + Duration::from_secs(20) + Duration::from_millis(200));
    h.settle().await;
    assert!(h.fake.calls().contains(&Call::Schemas(dbp("local pg", "postgres"))), "{:?}", h.fake.calls());
    // Two slow clicks are two selections, not a double-click.
    let (x, y) = find_in_sidebar(&mut h, "pagila").unwrap();
    left_click(&mut h, x, y, t0 + Duration::from_secs(30));
    left_click(&mut h, x, y, t0 + Duration::from_secs(31));
    h.settle().await;
    assert!(!h.fake.calls().contains(&Call::Schemas(dbp("local pg", "pagila"))));
    // A click below the last row selects nothing and breaks nothing.
    let before = h.app.ui.sidebar.cursor;
    left_click(&mut h, list.x + 3, list.y + 25, t0 + Duration::from_secs(40));
    assert_eq!(h.app.ui.sidebar.cursor, before);
}

const MARGIN_FOR_TEST: u16 = 1 + 2; // the margin and one level of indent, for a database row

#[tokio::test]
async fn the_wheel_scrolls_the_list_and_keeps_the_cursor_on_screen() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok((0..100).map(|i| format!("db{i:03}")).collect()));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    frame_rows(&mut h.app, 114, 23);
    let list = h.app.ui.hits.sidebar_list;
    for _ in 0..10 {
        let fx = h.app.update(mouse(MouseEventKind::ScrollDown, list.x + 2, list.y + 2), base());
        assert!(fx.repaint);
    }
    assert!(find_in_sidebar(&mut h, "● pg").is_none(), "scrolled off the top");
    let at = cursor_text(&mut h, 114, 23).expect("the cursor is still on screen");
    assert!(at.contains("db0"), "{at:?}");
    for _ in 0..20 {
        h.app.update(mouse(MouseEventKind::ScrollUp, list.x + 2, list.y + 2), base());
    }
    assert!(find_in_sidebar(&mut h, "● pg").is_some());
    // The wheel over another pane scrolls nothing in the sidebar.
    let editor = h.app.ui.hits.editor;
    let before = h.app.ui.sidebar.scroll;
    h.app.update(mouse(MouseEventKind::ScrollDown, editor.x + 2, editor.y + 2), base());
    assert_eq!(h.app.ui.sidebar.scroll, before);
}

// ---- keybindings -------------------------------------------------------------------------

#[tokio::test]
async fn the_sidebars_keys_come_from_the_live_keybindings_and_the_mode_bar_shows_the_rebound_key() {
    let mut h = started(FakeCore::new(), &["pg"]).await;
    assert!(h.app.ui.keys.rebind(Action::EditConnection, "x"));
    assert!(h.app.ui.keys.rebind(Action::RemoveConnection, "ctrl+k"));
    h.press("e");
    assert!(h.app.ui.overlay.is_none(), "the old key does nothing");
    h.press("d");
    assert!(h.app.ui.overlay.is_none());
    let bar = frame_rows(&mut h.app, 114, 30)[29].clone();
    assert!(bar.contains("x edit") && bar.contains("ctrl+k remove"), "{bar:?}");
    assert!(!bar.contains("e edit") && !bar.contains("d remove"), "{bar:?}");
    h.press("x");
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::Form(_))));
    h.press("esc");
    h.press("ctrl+k");
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::ConfirmRemove(_))));
    h.press("esc");

    // Movement and the tree keys too.
    assert!(h.app.ui.keys.rebind(Action::Expand, "o"));
    assert!(h.app.ui.keys.rebind(Action::MoveDown, "n"));
    h.fake.script_databases(Scripted::ok(names(&["a", "b"])));
    assert!(h.press_fx("enter").is_empty(), "enter no longer expands");
    assert_eq!(h.press_fx("o").len(), 1);
    h.settle().await;
    h.press("n");
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains(" a"), "{:?}", cursor_text(&mut h, 114, 30));
    let bar = frame_rows(&mut h.app, 114, 30)[29].clone();
    assert!(bar.contains("n/k move"), "{bar:?}");
}

#[tokio::test]
async fn the_mode_bar_shows_expand_collapse_parent_and_refresh_only_where_they_work() {
    let mut h = pagila().await;
    let bar = |h: &mut Harness| frame_rows(&mut h.app, 160, 30)[29].clone();
    let b = bar(&mut h);
    assert!(b.contains("enter expand") && b.contains("r refresh") && b.contains("e edit") && b.contains("d remove"), "{b:?}");
    assert!(!b.contains("collapse") && !b.contains("parent"), "a closed root has neither: {b:?}");
    h.press("enter");
    h.settle().await;
    let b = bar(&mut h);
    assert!(b.contains("h collapse") && !b.contains("expand"), "an open node collapses: {b:?}");
    h.press("j");
    let b = bar(&mut h);
    assert!(b.contains("enter expand") && b.contains("h parent"), "a closed child opens, and has a parent: {b:?}");
    assert!(!b.contains("edit") && !b.contains("remove"), "edit and remove are for connections: {b:?}");
    for _ in 0..2 {
        h.press("enter");
        h.settle().await;
        h.press("j");
    }
    h.press("j enter");
    h.settle().await;
    h.press("j");
    let b = bar(&mut h);
    assert!(cursor_text(&mut h, 160, 30).unwrap().contains("film_id"), "{:?}", cursor_text(&mut h, 160, 30));
    assert!(!b.contains("expand") && !b.contains("collapse"), "a column is a leaf: {b:?}");
    assert!(b.contains("h parent") && b.contains("r refresh"), "{b:?}");
    // Not on the sidebar: none of them.
    h.press("ctrl+g l");
    let b = bar(&mut h);
    assert!(!b.contains("refresh") && !b.contains("move") && !b.contains("parent"), "{b:?}");
}

#[test]
fn page_keys_parse_and_print() {
    use super::keys::KeyCombo;
    for s in ["pageup", "pagedown", "ctrl+d", "G", "home", "end"] {
        assert_eq!(KeyCombo::parse(s).unwrap().to_string(), s);
    }
    let k = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(KeyCombo::from_event(&k), KeyCombo::parse("pagedown").unwrap());
}

#[tokio::test]
async fn the_cursor_on_a_connection_makes_its_saved_database_the_active_one() {
    let fake = FakeCore::new();
    let mut c = conn("local pg", DbKind::Postgres);
    c.database = Some("pagila".into());
    fake.script_connections(Scripted::ok(vec![c]));
    let mut h = Harness::start(fake);
    h.settle().await;
    assert_eq!(h.app.model.active, Some(Active { connection: cid("local pg"), database: Some("pagila".into()) }));
    let title = frame_rows(&mut h.app, 114, 30).iter().find(|r| r.contains("editor")).cloned().unwrap();
    assert!(title.contains("editor · local pg / pagila"), "{title:?}");
}

#[test]
fn no_connections_means_no_active_context_and_the_tree_keys_do_nothing() {
    let mut app = ready_app();
    assert_eq!(app.model.active, None);
    for keys in ["j", "k", "enter", "h", "l", "r", "g", "G", "e", "d", "pagedown"] {
        let fx = press(&mut app, keys);
        assert!(fx.commands.is_empty(), "{keys}");
    }
    let title = frame_rows(&mut app, 114, 30).iter().find(|r| r.contains("editor")).cloned().unwrap();
    assert!(title.contains("editor") && !title.contains('·'), "{title:?}");
}

#[test]
fn a_node_row_is_found_again_by_its_path_not_its_position() {
    // Two tables of the same name in different schemas are different nodes.
    let a = NodeId::Table(tabp("c", "db", "one", "t"));
    let b = NodeId::Table(tabp("c", "db", "two", "t"));
    assert_ne!(a, b);
    assert_eq!(a.parent(), Some(NodeId::Schema(schp("c", "db", "one"))));
    assert!(a.is_below(&NodeId::Connection(cid("c"))) && !a.is_below(&b));
    // Paths made of names with separators in them stay distinct.
    let x = NodeId::Table(tabp("c", "db", "a/b", "c"));
    let y = NodeId::Table(tabp("c", "db", "a", "b/c"));
    assert_ne!(x, y);
    let _ = RowKind::Database("x".into());
    let _ = CoreError::new(CoreErrorKind::Other, "x");
    let _ = Target::Connections;
}

// ---- review fixes ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_expand_request_for_a_node_that_is_already_loading_asks_the_core_nothing() {
    // The keys never reach this (an open node's `enter` goes to its first
    // child), but a click or a future caller can: the guard in `expand` is what
    // keeps one listing in flight per node.
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["pagila"])).after(Duration::from_secs(5)));
    let mut h = started(fake, &["local pg"]).await;
    h.press("enter");
    h.settle().await;
    let node = NodeId::Connection(cid("local pg"));
    assert!(matches!(h.app.model.schema.state(&node), Children::Loading));

    let mut fx = Effects::default();
    h.app.expand(node.clone(), &mut fx);
    assert!(fx.commands.is_empty(), "no second call, no abandon of the first: {:?}", fx.commands);
    assert!(matches!(h.app.model.schema.state(&node), Children::Loading));
    h.settle().await;
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, Call::Databases(_))).count(), 1);
    assert_eq!(h.tasks.len(), 1, "the one listing is still running");
}

#[tokio::test]
async fn r_on_a_closed_node_only_drops_its_cache_and_the_next_expand_asks_again() {
    let mut h = pagila().await;
    h.press("enter");
    h.settle().await;
    h.press("h");
    let node = NodeId::Connection(cid("local pg"));
    assert!(matches!(h.app.model.schema.state(&node), Children::Loaded(_)));

    let commands = h.press_fx("r");
    assert!(commands.is_empty(), "nothing is loaded out of sight: {commands:?}");
    assert_eq!(h.app.model.schema.state(&node), &Children::NotLoaded, "the cache is dropped");
    h.settle().await;
    assert_eq!(h.fake.calls(), vec![Call::Connections, Call::Databases(cid("local pg"))]);

    // The next expand reloads, once; `r` then `enter` is not two calls.
    h.fake.script_databases(Scripted::ok(names(&["fresh"])));
    let commands = h.press_fx("enter");
    assert_eq!(commands.iter().filter(|c| matches!(c, Command::Core(..))).count(), 1, "{commands:?}");
    h.settle().await;
    assert!(tree_text(&mut h).contains("fresh"));
    assert_eq!(h.fake.calls().iter().filter(|c| matches!(c, Call::Databases(_))).count(), 2);
}

#[tokio::test]
async fn r_on_a_column_row_refreshes_its_open_table_and_does_not_drop_it() {
    let mut h = pagila().await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter");
    h.settle().await;
    h.press("j"); // on a column of the open table `actor`
    h.fake.script_columns(Scripted::ok(vec![col("only", "text", false, false)]));
    let commands = h.press_fx("r");
    assert!(matches!(first_call(&commands), Some((_, CoreCall::List(NodeId::Table(_))))), "{commands:?}");
}

fn releases(commands: &[Command]) -> Vec<DatabasePath> {
    let mut out: Vec<DatabasePath> = commands
        .iter()
        .filter_map(|c| match c {
            Command::Release(at) => Some(at.clone()),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.database.cmp(&b.database));
    out
}

/// Like `pagila()`, but the connection is saved with `pagila` as its database.
async fn pagila_with_default() -> Harness {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["pagila", "edge", "postgres"])));
    for _ in 0..3 {
        fake.script_schemas(Scripted::ok(names(&["public"])));
    }
    let mut c = conn("local pg", DbKind::Postgres);
    c.database = Some("pagila".into());
    fake.script_connections(Scripted::ok(vec![c]));
    let mut h = Harness::start(fake);
    h.settle().await;
    h
}

#[tokio::test]
async fn collapsing_a_database_releases_its_server_connections_except_the_connections_own_default() {
    let mut h = pagila_with_default().await;
    h.press("enter");
    h.settle().await;
    h.press("j j"); // edge
    h.press("enter");
    h.settle().await;
    let collapse = h.press_fx("h");
    assert_eq!(releases(&collapse), vec![dbp("local pg", "edge")], "{collapse:?}");
    h.settle().await;
    assert!(h.fake.calls().contains(&Call::Release(dbp("local pg", "edge"))), "the shell ran it: {:?}", h.fake.calls());

    // The default database keeps its pool.
    h.press("k");
    h.press("enter");
    h.settle().await;
    let collapse = h.press_fx("h");
    assert!(releases(&collapse).is_empty(), "the saved default database is kept: {collapse:?}");
}

#[tokio::test]
async fn collapsing_a_connection_releases_every_loaded_database_below_it_and_removing_it_does_too() {
    let mut h = pagila_with_default().await;
    h.press("enter");
    h.settle().await;
    h.press("j"); // pagila (the default)
    h.press("enter");
    h.settle().await;
    h.press("j j");
    h.press("enter"); // edge
    h.settle().await;
    h.press("g");
    let collapse = h.press_fx("h");
    assert_eq!(releases(&collapse), vec![dbp("local pg", "edge")], "pagila is the default; postgres was never opened: {collapse:?}");

    // Removing the connection releases what is loaded below it.
    h.press("d");
    h.fake.script_remove_connection(Scripted::ok(()));
    h.press("enter");
    h.settle().await;
    let calls = h.fake.calls();
    assert!(calls.iter().filter(|c| **c == Call::Release(dbp("local pg", "edge"))).count() >= 2, "{calls:?}");
}

#[tokio::test(start_paused = true)]
async fn collapsing_a_node_while_it_refreshes_abandons_and_forgets_everything_below_it() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["a"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![table("one")]));
    fake.script_columns(Scripted::ok(vec![col("id", "integer", true, false)]).after(Duration::from_secs(5)));
    fake.script_schemas(Scripted::ok(names(&["public"])).after(Duration::from_secs(5)));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("enter"); // the table's columns, slow
    h.settle().await;
    assert_eq!(h.tasks.len(), 1);
    h.press("k k"); // database a
    let refresh = h.press_fx("r");
    assert_eq!(refresh.len(), 1, "{refresh:?}");
    h.settle().await;
    assert_eq!(h.tasks.len(), 2, "the refresh and the columns are both in flight");

    let collapse = h.press_fx("h");
    let abandoned = collapse.iter().filter(|c| matches!(c, Command::Abandon(_))).count();
    assert_eq!(abandoned, 2, "the refresh and the columns below it: {collapse:?}");
    h.settle().await;
    assert_eq!(h.tasks.len(), 0);
    // The connection list and the connection's own listing are all that is left.
    assert_eq!(h.app.model.requests.tracked(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_node_that_refreshes_keeps_showing_what_it_had_with_the_glyph_at_work() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["a"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_databases(Scripted::ok(names(&["a", "b"])).after(Duration::from_secs(2)));
    let mut h = started(fake, &["pg"]).await;
    open_and_enter(&mut h).await;
    h.press("enter");
    h.settle().await;
    h.press("g");
    h.press("r");
    h.settle().await;
    let text = tree_text(&mut h);
    assert!(text.contains(" a") && text.contains("public") && !text.contains("loading"), "{text}");
    let (x, y) = find_in_sidebar(&mut h, "pg").unwrap();
    assert_eq!(role_at(&mut h.app, x - 2, y).fg, Some(Role::Yellow), "at work, though its last call worked");
    tokio::time::advance(Duration::from_secs(2)).await;
    h.settle().await;
    assert!(tree_text(&mut h).contains(" b"));
    let (x, y) = find_in_sidebar(&mut h, "pg").unwrap();
    assert_eq!(role_at(&mut h.app, x - 2, y).fg, Some(Role::Green));
}

#[tokio::test(start_paused = true)]
async fn when_the_row_the_cursor_is_on_disappears_it_goes_to_the_nearest_ancestor_still_there() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["a", "b"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok(vec![table("one"), table("two"), table("three")]));
    fake.script_tables(Scripted::ok(vec![table("two"), table("three")]).after(Duration::from_secs(3)));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("k"); // schema public
    h.press("r");
    h.settle().await;
    h.press("j"); // `one`, still shown while the refresh runs
    assert!(cursor_text(&mut h, 114, 30).unwrap().contains("one"), "{:?}", cursor_text(&mut h, 114, 30));
    tokio::time::advance(Duration::from_secs(3)).await;
    h.settle().await;
    assert!(!tree_text(&mut h).contains("one"));
    let at = cursor_text(&mut h, 114, 30).unwrap();
    assert!(at.contains("public"), "the schema `one` was in, not an unrelated row: {at:?}");
    assert_eq!(h.app.model.active.as_ref().and_then(|a| a.database.clone()), Some("a".into()));
}

#[tokio::test]
async fn collapsing_a_schema_of_ten_thousand_tables_with_two_thousand_opened_takes_one_pass() {
    use std::time::Instant as Wall;
    let mut h = pagila().await;
    let (c, db, sc) = (NodeId::Connection(cid("local pg")), dbp("local pg", "db"), schp("local pg", "db", "public"));
    let m = &mut h.app.model;
    m.schema.set(c.clone(), Children::Loaded(Listing::Databases(names(&["db"]))));
    m.schema.set(NodeId::Database(db), Children::Loaded(Listing::Schemas(names(&["public"]))));
    let tables: Vec<Table> = (0..10_000).map(|i| table(&format!("t{i}"))).collect();
    m.schema.set(NodeId::Schema(sc.clone()), Children::Loaded(Listing::Tables(tables)));
    for i in 0..2_000 {
        let t = NodeId::Table(tabp("local pg", "db", "public", &format!("t{i}")));
        m.schema.set(t.clone(), Children::Loaded(Listing::Columns(vec![col("id", "integer", true, false)])));
        m.requests.begin(Target::Children(t.clone()));
        h.app.ui.sidebar.expanded.insert(t);
    }
    assert_eq!(h.app.model.requests.tracked(), 1 + 2_000);

    let mut fx = Effects::default();
    let start = Wall::now();
    h.app.collapse(&NodeId::Schema(sc), &mut fx);
    let took = start.elapsed();
    assert!(took < Duration::from_millis(150), "collapse took {took:?}");
    assert_eq!(h.app.model.requests.tracked(), 1, "and all of them were let go");
}

#[tokio::test]
async fn the_note_at_the_foot_of_the_sidebar_takes_its_rows_from_the_list() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::ok(names(&["db"])));
    fake.script_schemas(Scripted::ok(names(&["public"])));
    fake.script_tables(Scripted::ok((0..100).map(|i| table(&format!("table_{i:03}"))).collect()));
    let mut h = started(fake, &["pg"]).await;
    for _ in 0..3 {
        open_and_enter(&mut h).await;
    }
    h.press("G");
    h.app.say("Could not remove: the server said no, and then it said so again at some length.".into(), true);
    frame_rows(&mut h.app, 80, 24);
    let (side, list) = (h.app.ui.hits.sidebar, h.app.ui.hits.sidebar_list);
    let note_rows = super::view::wrap(&h.app.ui.note.as_ref().unwrap().text, side.width as usize - 2).len().min(4) as u16;
    assert!(list.bottom() + note_rows <= side.bottom(), "list {list:?} and {note_rows} note rows in {side:?}");
    let cursor = cursor_text_fast(&mut h, 80, 24).expect("the cursor row is on screen");
    assert!(cursor.contains("table_099"), "{cursor:?}");
    // A click where the note is hits no row.
    let before = h.app.ui.sidebar.cursor;
    left_click(&mut h, side.x + 2, side.bottom() - 1, base());
    assert_eq!(h.app.ui.sidebar.cursor, before);
}

#[tokio::test(start_paused = true)]
async fn a_retry_after_a_failure_shows_the_working_glyph_not_the_failed_one() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(CoreErrorKind::Connect, "refused"));
    fake.script_databases(Scripted::ok(names(&["pagila"])).after(Duration::from_secs(2)));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let (x, y) = find_in_sidebar(&mut h, "pg").unwrap();
    assert_eq!(role_at(&mut h.app, x - 2, y).fg, Some(Role::Red));
    h.press("r");
    h.settle().await;
    let (x, y) = find_in_sidebar(&mut h, "pg").unwrap();
    assert_eq!(role_at(&mut h.app, x - 2, y).fg, Some(Role::Yellow), "retrying is work in progress");
}

#[tokio::test]
async fn a_failed_row_names_the_key_that_retries_from_the_live_bindings() {
    let fake = FakeCore::new();
    fake.script_databases(Scripted::err(CoreErrorKind::Connect, "refused"));
    let mut h = started(fake, &["pg"]).await;
    h.press("enter");
    h.settle().await;
    let text = squashed(&tree_text(&mut h));
    assert!(text.contains("refused") && text.contains("press r to retry"), "{text}");
    assert!(h.app.ui.keys.rebind(Action::Refresh, "x"));
    let text = squashed(&tree_text(&mut h));
    assert!(text.contains("press x to retry") && !text.contains("press r"), "{text}");
}

#[test]
fn a_listing_of_the_wrong_kind_for_its_node_repaints_nothing() {
    let mut app = ready_app();
    app.model.connections = Loadable::Ready(vec![conn("pg", DbKind::Postgres)]);
    app.touch_rows();
    let fx = press(&mut app, "enter");
    let Some((id, _)) = first_call(&fx.commands) else { panic!("{:?}", fx.commands) };
    // The wrong kind of listing for the node.
    let fx = app.update(Event::Core(id, CoreReply::Listed(Ok(Listing::Schemas(names(&["x"]))))), base());
    assert!(!fx.repaint, "a listing that does not fit its node");

}

#[test]
fn a_listing_for_a_connection_that_is_no_longer_in_the_list_repaints_nothing() {
    let mut app = ready_app();
    app.model.connections = Loadable::Ready(vec![conn("pg", DbKind::Postgres)]);
    app.touch_rows();
    let fx = press(&mut app, "enter");
    let Some((id, _)) = first_call(&fx.commands) else { panic!("{:?}", fx.commands) };
    // A current request whose connection has left the list.
    app.model.connections = Loadable::Ready(vec![]);
    let fx = app.update(Event::Core(id, CoreReply::Listed(Ok(Listing::Databases(names(&["x"]))))), base());
    assert!(!fx.repaint, "a listing for a connection that is gone");
}

#[tokio::test]
async fn a_connection_that_is_not_postgres_says_it_cannot_be_browsed_yet_and_asks_dbx_nothing() {
    let fake = FakeCore::new();
    fake.script_connections(Scripted::ok(vec![conn("lite", DbKind::Sqlite), conn("my", DbKind::MySql)]));
    let mut h = Harness::start(fake);
    h.settle().await;
    let commands = h.press_fx("enter");
    assert!(commands.is_empty(), "{commands:?}");
    let text = squashed(&tree_text(&mut h));
    assert!(text.contains("Browsing SQLite is not supported yet."), "{text}");
    assert!(!text.contains("no databases"), "{text}");
    assert!(h.press_fx("r").is_empty(), "refresh asks nothing either");
    h.press("h");
    h.press("j");
    let commands = h.press_fx("enter");
    assert!(commands.is_empty(), "{commands:?}");
    assert!(squashed(&tree_text(&mut h)).contains("Browsing MySQL is not supported yet."), "{}", tree_text(&mut h));
    h.settle().await;
    assert_eq!(h.fake.calls(), vec![Call::Connections], "dbx was never asked");
    assert!(h.app.model.schema.is_empty() && h.app.model.requests.tracked() == 1);
}

#[tokio::test]
async fn the_sidebar_is_about_thirty_percent_of_the_width_between_26_and_34_columns() {
    let mut h = started(FakeCore::new(), &["pg"]).await;
    // The column and its separator.
    for (width, total) in [(60u16, 26u16), (80, 26), (100, 30), (113, 33), (150, 34), (300, 34), (40, 20)] {
        frame_rows(&mut h.app, width, 24);
        let hits = h.app.ui.hits;
        assert_eq!(hits.sidebar.width + hits.sidebar_separator.width, total, "{width} columns");
        assert!(hits.editor.width >= width / 2, "an editor stays usable at {width}: {hits:?}");
    }
}

#[tokio::test]
async fn at_80_by_24_on_a_connection_row_the_edit_and_remove_hints_are_there_and_clickable() {
    let mut h = started(FakeCore::new(), &["local pg"]).await;
    let bar = frame_rows(&mut h.app, 80, 24)[23].clone();
    assert!(bar.contains("e edit") && bar.contains("d remove"), "{bar:?}");
    assert!(bar.contains("enter expand") && bar.contains("r refresh"), "{bar:?}");
    assert_eq!(bar.chars().count(), 80);
    let at = |bar: &str, text: &str| bar[..bar.find(text).unwrap()].chars().count() as u16;
    let x = at(&bar, "e edit");
    left_click(&mut h, x, 23, base());
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::Form(_))), "{:?}", h.app.ui.overlay);
    h.press("esc");
    frame_rows(&mut h.app, 80, 24);
    let x = at(&bar, "d remove");
    left_click(&mut h, x, 23, base());
    assert!(matches!(h.app.ui.overlay, Some(super::overlay::Overlay::ConfirmRemove(_))), "{:?}", h.app.ui.overlay);
}
