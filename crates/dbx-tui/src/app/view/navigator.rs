//! The navigator overlay: a search field, then the tables and views that
//! match, each as `schema.name`, what it is, and where it is. Every name came
//! from a database, so every piece of text goes through the painter.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{draw_scrollbar, drawn_width, form, history, panel, scrolled, Drawn, Painter};
use crate::app::navigator::{NavigatorOverlay, Search};
use crate::app::overlay::{OverlayAction, OverlayButton};
use crate::app::theme::Role;
use crate::core::display_safe;

/// The widest the "where" column grows.
const WHERE_MAX: u16 = 28;
/// The kind column: `matview`, `foreign`.
const KIND_W: u16 = 8;
/// The fewest columns the name gets before the other columns give way.
const MIN_LABEL: u16 = 12;

pub(super) fn draw(n: &mut NavigatorOverlay, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 100.min(area.width.saturating_sub(4)).max(40.min(area.width));
    let height = 28.min(area.height.saturating_sub(2)).max(10.min(area.height));
    let rect = panel::centred(area, width, height);
    let inner = panel::panel(p, buf, rect, "Go to table or view", Role::Accent, Role::Accent);
    if inner.width < 24 || inner.height < 6 {
        panel::too_small(p, buf, rect);
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pi = p.within(inner);
    let mut drawn = Drawn::default();

    // The search field, and under it where it looks and how that is going.
    let label_w = 8;
    pi.put(buf, inner.x, inner.y, "go to", pi.bold(Role::Accent).bg(bg));
    let input = Rect::new(inner.x + label_w, inner.y, inner.width.saturating_sub(label_w), 1);
    form::draw_input(&pi, buf, input, &mut n.search, n.scope.is_some(), None);
    let place = n.scope.as_ref().map(|(_, w)| w.clone());
    let mut x = inner.x;
    if let Some(place) = &place {
        x = pi.put(buf, inner.x, inner.y + 1, place, pi.fg(Role::Overlay0).bg(bg));
    }
    let status = match &n.state {
        Search::Asking => Some(("searching…".to_string(), Role::Yellow)),
        Search::Failed(why) => Some((format!("the database could not be searched: {why}"), Role::Red)),
        Search::Done { more: true } => {
            Some((format!("more than {} names: type more to narrow them", crate::app::navigator::LIMIT), Role::Yellow))
        }
        _ => None,
    };
    if let Some((text, role)) = status {
        pi.put(buf, if place.is_some() { x + 2 } else { x }, inner.y + 1, &text, pi.fg(role).bg(bg));
    }

    let foot = inner.bottom() - 1;
    let list = Rect::new(inner.x, inner.y + 3, inner.width, foot.saturating_sub(inner.y + 4));
    n.rows = list.height as usize;
    n.top = scrolled(n.top.min(n.items.len().saturating_sub(n.rows)), n.selected, n.rows);
    if n.items.is_empty() {
        let (role, lines) = empty_lines(n);
        let wrapped = lines.iter().flat_map(|l| panel::wrap(l, list.width as usize));
        for (y, line) in (list.y..list.bottom()).zip(wrapped) {
            pi.put(buf, list.x, y, &line, pi.fg(role).bg(bg));
        }
    } else {
        draw_list(n, p, buf, list, &mut drawn);
    }

    // The buttons carry the keys the overlay answers to.
    let mut specs: Vec<(String, String, OverlayAction)> = Vec::new();
    if n.selected_item().is_some() {
        specs.push(("↵".into(), "reveal in sidebar".into(), OverlayAction::NavigatorReveal));
        specs.push(("ctrl+o".into(), "open rows".into(), OverlayAction::NavigatorOpen));
    }
    specs.push(("esc".into(), "close".into(), OverlayAction::Dismiss));
    drawn.buttons.extend(history::buttons(&pi, buf, inner, foot, &specs));
    drawn
}

/// What to say where the list would be.
fn empty_lines(n: &NavigatorOverlay) -> (Role, Vec<String>) {
    let Some((_, place)) = &n.scope else {
        return (
            Role::Yellow,
            vec![format!(
                "This tab has no database to look in. Give it one first: put the sidebar cursor on a database and press {} (use), then open this again.",
                n.use_key
            )],
        );
    };
    let typed = n.search.text().trim();
    if typed.is_empty() {
        return (
            Role::Subtext0,
            vec![format!("Type part of a table or view name. What the sidebar has loaded shows at once; all of {place} is searched when you pause.")],
        );
    }
    match &n.state {
        Search::Idle | Search::Asking => (Role::Overlay0, vec!["searching…".into()]),
        Search::Failed(why) => (
            Role::Red,
            vec![format!("{place} could not be searched: {why}"), "Type to search again, or esc to close.".into()],
        ),
        Search::Done { .. } => (Role::Subtext0, vec![format!("Nothing in {place} matches \"{typed}\".")]),
    }
}

fn draw_list(n: &NavigatorOverlay, p: &Painter, buf: &mut Buffer, list: Rect, drawn: &mut Drawn) {
    let bg = p.theme.color(Role::PanelBg);
    let pl = p.within(list);
    let place = n.scope.as_ref().map(|(_, w)| w.as_str()).unwrap_or("");
    let bar = n.items.len() > n.rows && list.width > 6;
    let width = if bar { list.width - 1 } else { list.width };
    // The name matters most: the place takes at most a third, and the place
    // and then the kind give way when the name would have under 12 columns.
    let mut where_w = (drawn_width(&display_safe(place)) as u16).min(WHERE_MAX).min(width / 3);
    let mut kind_w = KIND_W;
    let mut label_w = width.saturating_sub(where_w + kind_w + 4);
    if label_w < MIN_LABEL {
        where_w = 0;
        label_w = width.saturating_sub(kind_w + 3);
    }
    if label_w < MIN_LABEL {
        kind_w = 0;
        label_w = width.saturating_sub(2);
    }
    for (row, (i, item)) in (list.y..).zip(n.items.iter().enumerate().skip(n.top).take(n.rows)) {
        let selected = i == n.selected;
        let base = if selected {
            Style::default().bg(p.theme.color(Role::SelectionBg)).add_modifier(p.theme.modifier(Role::SelectionBg))
        } else {
            Style::default().bg(bg)
        };
        let rect = Rect::new(list.x, row, width, 1);
        pl.within(rect).fill(buf, base);
        let at = list.x + 1;
        p.within(Rect::new(at, row, label_w, 1)).put(buf, at, row, &item.label(), base.fg(p.theme.color(Role::Text)));
        let kx = at + label_w + 1;
        if kind_w > 0 {
            p.within(Rect::new(kx, row, kind_w, 1)).put(
                buf,
                kx,
                row,
                item.kind_label(),
                base.fg(p.theme.color(Role::Subtext0)),
            );
        }
        let wx = kx + kind_w + 1;
        if where_w > 0 {
            p.within(Rect::new(wx, row, where_w, 1)).put(buf, wx, row, place, base.fg(p.theme.color(Role::Overlay0)));
        }
        drawn.buttons.push(OverlayButton { rect, action: OverlayAction::NavigatorPick(i) });
    }
    if bar {
        let track = Rect::new(list.right() - 1, list.y, 1, list.height);
        draw_scrollbar(p, buf, track, n.items.len(), n.top);
    }
}
