//! The first-run welcome, in the results pane while no connection is saved:
//! what dbpane is, which store it uses and where, and the keys that add a
//! connection, open help and quit. Keys come from the live bindings, and a
//! hint whose key is not bound is not shown.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{panel, Painter};
use crate::app::keys::{Action, Keybinds};
use crate::app::theme::Role;
use crate::core::{display_safe, StoreInfo};

/// Draws the welcome inside `area`, from its top row. Nothing outside it is
/// touched, and what does not fit is left off from the bottom.
pub(super) fn draw(keys: &Keybinds, store: Option<&StoreInfo>, p: &Painter, buf: &mut Buffer, area: Rect) {
    let width = area.width as usize;
    // The commands behind the prefix key: spelled out, as in the mode bar.
    let command = |action: Action| keys.key_for(action).map(|k| format!("{} {k}", keys.prefix));

    let mut rows: Vec<(String, Role, bool)> = vec![
        ("Welcome to dbpane".into(), Role::Accent, true),
        ("A terminal database client, built on dbx's core.".into(), Role::Subtext0, false),
        (String::new(), Role::Subtext0, false),
    ];
    if let Some(store) = store {
        rows.extend(panel::wrap(&format!("Store: {}", store.describe()), width).into_iter().map(|l| (l, Role::Subtext0, false)));
        let mut rest = display_safe(&store.path.display().to_string());
        while !rest.is_empty() && width > 2 {
            let (head, tail) = panel::split_at_width(&rest, width - 2);
            rows.push((format!("  {head}"), Role::Overlay0, false));
            rest = tail;
        }
        rows.push((String::new(), Role::Subtext0, false));
    }
    if let Some(key) = command(Action::NewConnection) {
        rows.push((format!("Add your first connection: {key}"), Role::Text, false));
    }
    if let Some(key) = command(Action::Help) {
        rows.push((format!("Help and all keys: {key}"), Role::Subtext0, false));
    }
    if let Some(key) = command(Action::Quit) {
        rows.push((format!("Quit: {key}"), Role::Subtext0, false));
    }

    for (y, (text, role, bold)) in (area.y..area.bottom()).zip(rows) {
        let style = if bold { p.bold(role) } else { p.fg(role) };
        p.put(buf, area.x, y, &text, style);
    }
}
