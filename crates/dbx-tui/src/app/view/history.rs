//! The history overlay: a search field, the entries newest first (when,
//! where, whether it worked, the first line), and the highlighted statement
//! in full below. Everything in it was written by another program, so every
//! piece of text goes through the painter.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{draw_scrollbar, duration_text, form, overlays, panel, scrolled, Drawn, Painter};
use crate::app::history::{HistoryOverlay, Load};
use crate::app::keys::{Action, Keybinds};
use crate::app::overlay::{OverlayAction, OverlayButton};
use crate::app::theme::Role;
use crate::core::{DatabasePath, HistoryEntry};

/// Where the active tab runs: what an entry that ran elsewhere is told apart by.
pub(super) struct Here {
    pub path: DatabasePath,
    /// The connection's name.
    pub name: String,
}

impl Here {
    fn label(&self) -> String {
        label(&self.name, &self.path.database)
    }

    /// Whether `e` ran somewhere other than here. An entry an old desktop
    /// wrote has no connection id: it is told by its name.
    fn elsewhere(&self, e: &HistoryEntry) -> bool {
        let connection = if e.connection.0.is_empty() { e.connection_name != self.name } else { e.connection != self.path.connection };
        connection || e.database != self.path.database
    }
}

/// `connection / database`.
fn label(connection: &str, database: &str) -> String {
    if database.is_empty() {
        connection.to_string()
    } else {
        format!("{connection} / {database}")
    }
}

const WHEN_W: u16 = 16;
const WHERE_MAX: u16 = 26;

pub(super) fn draw(h: &mut HistoryOverlay, keys: &Keybinds, here: Option<&Here>, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 110.min(area.width.saturating_sub(4)).max(40.min(area.width));
    let height = 36.min(area.height.saturating_sub(2)).max(12.min(area.height));
    let rect = panel::centred(area, width, height);
    let inner = panel::panel(p, buf, rect, "History", Role::Accent, Role::Accent);
    if inner.width < 24 || inner.height < 8 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pi = p.within(inner);
    let mut drawn = Drawn::default();

    // The search field, and under it what the list is of.
    let label_w = 8;
    pi.put(buf, inner.x, inner.y, "search", pi.bold(Role::Accent).bg(bg));
    let input = Rect::new(inner.x + label_w, inner.y, inner.width.saturating_sub(label_w), 1);
    form::draw_input(&pi, buf, input, &mut h.search, true, None);
    let scope = if h.all { "all connections".to_string() } else { format!("this connection ({})", h.here.as_ref().map_or("", |(_, n)| n.as_str())) };
    let count = match &h.load {
        Load::Ready if !h.entries.is_empty() => format!("  ·  {} of {}", super::thousands(h.entries.len()), super::thousands(h.total)),
        _ => String::new(),
    };
    let x = pi.put(buf, inner.x, inner.y + 1, &format!("{scope}{count}"), pi.fg(Role::Overlay0).bg(bg));
    if let Some(why) = &h.more_failed {
        pi.put(buf, x + 2, inner.y + 1, &format!("the next page could not be read: {why}"), pi.fg(Role::Red).bg(bg));
    } else if h.load == Load::Searching || h.more_loading {
        let text = if h.more_loading { "loading more…" } else { "searching…" };
        pi.put(buf, x + 2, inner.y + 1, text, pi.fg(Role::Yellow).bg(bg));
    }

    // The foot: scroll hint on the left, the buttons on the right.
    let foot = inner.bottom() - 1;
    let body_top = inner.y + 3;
    let body = foot.saturating_sub(body_top + 1);
    let list_h = (body * 11 / 20).max(3).min(body.saturating_sub(3));
    let sep = body_top + list_h;
    let preview_top = sep + 1;
    let preview_h = foot.saturating_sub(preview_top + 1);

    // The list.
    let list = Rect::new(inner.x, body_top, inner.width, list_h);
    h.rows = list_h as usize;
    h.top = scrolled(h.top.min(h.entries.len().saturating_sub(h.rows)), h.selected, h.rows);
    draw_list(h, here, p, buf, list, &mut drawn, keys);

    // The divider, then the statement.
    let line = pi.fg(Role::SurfaceDim).bg(bg);
    for dx in 0..inner.width {
        pi.put(buf, inner.x + dx, sep, "─", line);
    }
    h.preview_rows = preview_h as usize;
    let lines = preview_lines(h, here, keys, inner.width.saturating_sub(2) as usize);
    h.preview_scroll = h.preview_scroll.min(lines.len().saturating_sub(h.preview_rows));
    for (i, (text, role, bold)) in lines.iter().skip(h.preview_scroll).take(h.preview_rows).enumerate() {
        let style = if *bold { pi.bold(*role) } else { pi.fg(*role) };
        pi.put(buf, inner.x + 1, preview_top + i as u16, text, style.bg(bg));
    }
    let scrolls = lines.len() > h.preview_rows;
    if scrolls && h.preview_rows > 0 {
        let note = format!("lines {}-{} of {}   shift+↑↓ scroll", h.preview_scroll + 1, (h.preview_scroll + h.preview_rows).min(lines.len()), lines.len());
        pi.put(buf, inner.x, foot, &note, pi.fg(Role::Overlay0).bg(bg));
    }

    // The buttons carry the live keys.
    let run_key = keys.key_for(Action::RunDirect).unwrap_or_else(|| "f5".into());
    let mut specs: Vec<(String, String, OverlayAction)> = vec![("↵".into(), "insert".into(), OverlayAction::HistoryInsert), (run_key, "run".into(), OverlayAction::HistoryRun)];
    if h.can_toggle() {
        specs.push(("tab".into(), if h.all { "this connection" } else { "all connections" }.into(), OverlayAction::HistoryScope));
    }
    specs.push(("esc".into(), "close".into(), OverlayAction::Dismiss));
    drawn.buttons.extend(buttons(&pi, buf, inner, foot, &specs));
    drawn
}

/// The list's rows, or what to say in their place.
fn draw_list(h: &HistoryOverlay, here: Option<&Here>, p: &Painter, buf: &mut Buffer, list: Rect, drawn: &mut Drawn, keys: &Keybinds) {
    let bg = p.theme.color(Role::PanelBg);
    let pl = p.within(list);
    if h.entries.is_empty() {
        let (lines, role): (Vec<String>, Role) = match &h.load {
            Load::Searching => (vec!["searching…".into()], Role::Overlay0),
            Load::Failed(why) => (
                vec![format!("History could not be read: {why}"), "Type to search again, or esc to close.".into()],
                Role::Red,
            ),
            Load::Ready => {
                let mut lines = Vec::new();
                let typed = h.search.text().trim();
                if typed.is_empty() {
                    let run = keys.key_for(Action::RunDirect).unwrap_or_else(|| "f5".into());
                    lines.push(format!("Nothing here yet. Run a statement with {run} and it is saved here, and shown in the desktop app's history too."));
                } else {
                    lines.push(format!("Nothing matches \"{typed}\"."));
                }
                if h.can_toggle() && !h.all {
                    lines.push("Press tab to look in all connections.".into());
                }
                (lines, Role::Subtext0)
            }
        };
        let wrapped = lines.iter().flat_map(|l| panel::wrap(l, list.width as usize));
        for (y, line) in (list.y..list.bottom()).zip(wrapped) {
            pl.put(buf, list.x, y, &line, pl.fg(role).bg(bg));
        }
        return;
    }
    // The widest "where" among the rows shown, so the first line lines up.
    let shown = h.entries.iter().skip(h.top).take(h.rows);
    let where_w = shown.map(|e| super::drawn_width(&crate::core::display_safe(&label(&e.connection_name, &e.database))) as u16).max().unwrap_or(0).min(WHERE_MAX);
    let bar = h.entries.len() > h.rows && list.width > 6;
    let width = if bar { list.width - 1 } else { list.width };
    for (row, (i, e)) in (list.y..).zip(h.entries.iter().enumerate().skip(h.top).take(h.rows)) {
        let selected = i == h.selected;
        let base = if selected {
            Style::default().bg(p.theme.color(Role::SelectionBg)).add_modifier(p.theme.modifier(Role::SelectionBg))
        } else {
            Style::default().bg(bg)
        };
        let rect = Rect::new(list.x, row, width, 1);
        let pr = pl.within(rect);
        pr.fill(buf, base);
        let when = pr.within(Rect::new(list.x, row, WHEN_W.min(width), 1));
        when.put(buf, list.x, row, &e.when, base.fg(p.theme.color(Role::Subtext0)));
        let mut x = list.x + WHEN_W + 2;
        let elsewhere = here.is_some_and(|h| h.elsewhere(e));
        let role = if elsewhere { Role::Peach } else { Role::Overlay0 };
        pr.within(Rect::new(x, row, where_w, 1)).put(buf, x, row, &label(&e.connection_name, &e.database), base.fg(p.theme.color(role)));
        x += where_w + 2;
        let (glyph, role) = if e.success { ("✓", Role::Green) } else { ("✗", Role::Red) };
        pr.put(buf, x, row, glyph, base.fg(p.theme.color(role)));
        x += 3;
        let first = e.sql.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        pr.put(buf, x, row, first, base.fg(p.theme.color(Role::Text)));
        drawn.buttons.push(OverlayButton { rect, action: OverlayAction::HistoryPick(i) });
    }
    if bar {
        let track = Rect::new(list.right() - 1, list.y, 1, list.height);
        draw_scrollbar(p, buf, track, h.entries.len(), h.top);
    }
}

/// The preview: when and how long, where it ran if not here, why it failed,
/// then the statement whole (spaces kept, long lines cut into pieces).
fn preview_lines(h: &HistoryOverlay, here: Option<&Here>, keys: &Keybinds, width: usize) -> Vec<(String, Role, bool)> {
    let Some(e) = h.selected_entry() else { return Vec::new() };
    let mut out: Vec<(String, Role, bool)> = Vec::new();
    let mut meta = format!("{}  ·  {}", e.when, duration_text(e.elapsed));
    if let Some(n) = e.affected {
        meta.push_str(&format!("  ·  {}", super::affected_text(n)));
    }
    out.push((meta, Role::Overlay0, false));
    if let Some(here) = here.filter(|here| here.elsewhere(e)) {
        let run = keys.key_for(Action::RunDirect).unwrap_or_else(|| "f5".into());
        out.push((format!("Ran on {}.", label(&e.connection_name, &e.database)), Role::Peach, true));
        out.push((format!("This tab runs on {}: {run} runs it here.", here.label()), Role::Peach, false));
    }
    if !e.success {
        let why = e.error.as_deref().unwrap_or("it failed");
        for (i, part) in why.split('\n').enumerate() {
            let part = if i == 0 { format!("✗ {part}") } else { part.to_string() };
            for line in panel::wrap(&part, width) {
                out.push((line, Role::Red, false));
            }
        }
    }
    out.push((String::new(), Role::Text, false));
    for line in overlays::statement_lines(&crate::app::editor::clean_paste(&e.sql), width.max(1)) {
        out.push((line, Role::Text, true));
    }
    out
}

/// `panel::buttons` with labels that are not static (the live run key, the
/// scope the tab flips to).
pub(super) fn buttons(p: &Painter, buf: &mut Buffer, inner: Rect, y: u16, specs: &[(String, String, OverlayAction)]) -> Vec<OverlayButton> {
    let bg = p.theme.color(Role::PanelBg);
    let widths: Vec<usize> = specs.iter().map(|(k, l, _)| k.chars().count() + 1 + l.chars().count()).collect();
    let total: usize = widths.iter().sum::<usize>() + 3 * specs.len().saturating_sub(1);
    let mut x = inner.right().saturating_sub(total as u16).max(inner.x);
    let mut out = Vec::new();
    for ((key, label, action), w) in specs.iter().zip(widths) {
        let start = x;
        x = p.put(buf, x, y, key, p.bold(Role::Accent).bg(bg));
        x = p.put(buf, x + 1, y, label, p.fg(Role::Overlay0).bg(bg));
        out.push(OverlayButton { rect: Rect::new(start, y, w as u16, 1).intersection(inner), action: *action });
        x += 3;
    }
    out
}
