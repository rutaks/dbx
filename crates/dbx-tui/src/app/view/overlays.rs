//! The statement chooser and the key help, two overlays over the dimmed screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{panel, Drawn, Painter};
use crate::app::keys::{HelpRow, Keybinds};
use crate::app::overlay::{Chooser, ConfirmRun, HelpOverlay, Overlay, OverlayAction, OverlayButton};
use crate::core::Reason;
use crate::app::theme::Role;
use crate::app::Capabilities;

/// "Run which?": one row per candidate, the highlighted one in the selection
/// colours, each with a one-line preview of exactly the text it runs.
pub(super) fn draw_chooser(c: &Chooser, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 80.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let rect = panel::centred(area, width, c.items.len() as u16 + 4);
    let inner = panel::panel(p, buf, rect, "Run which?", Role::Accent, Role::Accent);
    if inner.width < 8 || inner.height < 3 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pane = p.within(inner);
    let mut buttons = Vec::new();
    for (i, item) in c.items.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() - 2 {
            break;
        }
        let selected = i == c.selected;
        let base = if selected {
            Style::default().bg(p.theme.color(Role::SelectionBg)).add_modifier(p.theme.modifier(Role::SelectionBg))
        } else {
            Style::default().bg(bg)
        };
        pane.within(Rect { y, height: 1, ..inner }).fill(buf, base);
        let x = pane.put(buf, inner.x, y, item.label, base.fg(p.theme.color(Role::Accent)).add_modifier(ratatui::style::Modifier::BOLD));
        let at = inner.x + 12.max(x - inner.x + 2);
        pane.put(buf, at, y, &item.preview, base.fg(p.theme.color(Role::Text)));
        buttons.push(OverlayButton { rect: Rect { y, height: 1, ..inner }, action: OverlayAction::ChooserPick(i) });
    }
    let specs = Overlay::Chooser(c.clone()).buttons();
    buttons.extend(panel::buttons(p, buf, inner, inner.bottom() - 1, specs));
    Drawn { buttons, fields: Vec::new() }
}

/// A line of the help, as drawn. The keys of a row that do not fit on its
/// line carry on, on lines under it.
enum Line<'a> {
    Title(&'static str),
    /// The row, which of its keys this line has, and whether it has the label.
    Row(&'a HelpRow, std::ops::Range<usize>, bool),
    Blank,
}

/// What a key looks like in the list: one this terminal cannot deliver is in
/// `[brackets]`.
fn key_text(b: &crate::app::keys::Binding, caps: Capabilities) -> String {
    if b.needs_protocol && !caps.keyboard_protocol {
        format!("[{}]", b.text)
    } else {
        b.text.clone()
    }
}

/// The lines of the help, the keys of each row broken into lines of at most
/// `avail` columns (a key too wide for a line has one to itself).
fn lines(sections: &[crate::app::keys::HelpSection], avail: u16, caps: Capabilities) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if i > 0 {
            out.push(Line::Blank);
        }
        out.push(Line::Title(s.title));
        for row in &s.rows {
            let (mut start, mut used, mut first) = (0, 0u16, true);
            for (j, b) in row.bindings.iter().enumerate() {
                let w = super::drawn_width(&key_text(b, caps)) as u16;
                if j > start && used + 2 + w > avail {
                    out.push(Line::Row(row, start..j, first));
                    (start, used, first) = (j, 0, false);
                }
                used += if j > start { 2 } else { 0 } + w;
            }
            out.push(Line::Row(row, start..row.bindings.len(), first));
        }
    }
    out
}

/// How wide the label column is, and the config name's.
const LABEL: u16 = 24;
const NAME: u16 = 32;
/// A terminal narrower than this has no room for the config name column.
const WIDE: u16 = 110;

/// Every action with all its keys, from the live bindings. A key this
/// terminal cannot deliver is in `[brackets]` and dimmed.
pub(super) fn draw_help(h: &mut HelpOverlay, keys: &Keybinds, caps: Capabilities, p: &Painter, buf: &mut Buffer) -> Drawn {
    let area = buf.area;
    let width = 100.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let height = area.height.saturating_sub(4).max(8.min(area.height));
    let rect = panel::centred(area, width, height);
    let inner = panel::panel(p, buf, rect, "Keys", Role::Accent, Role::Accent);
    if inner.width < 30 || inner.height < 4 {
        panel::too_small(p, buf, rect);
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pane = p.within(inner);
    // The config name is a column only where there is room for it.
    let named = area.width >= WIDE;
    let keys_at = if named { LABEL + NAME } else { LABEL };
    // The search field, then a line saying what the columns are.
    pane.put(buf, inner.x, inner.y, "search", p.bold(Role::Accent).bg(bg));
    let input = Rect::new(inner.x + 8, inner.y, inner.width.saturating_sub(8), 1);
    super::form::draw_input(&pane, buf, input, &mut h.search, true, Some("an action, a name or a key"));
    pane.put(buf, inner.x, inner.y + 1, "does", p.fg(Role::Overlay0).bg(bg));
    if named {
        pane.put(buf, inner.x + LABEL, inner.y + 1, "config name", p.fg(Role::Overlay0).bg(bg));
    }
    pane.put(buf, inner.x + keys_at, inner.y + 1, "keys", p.fg(Role::Overlay0).bg(bg));
    let sections = crate::app::help::filter(keys.help(), h.search.text());
    let all = lines(&sections, inner.width.saturating_sub(keys_at), caps);
    // Two lines on top, the note and the foot below.
    let rows = (inner.height.saturating_sub(4)) as usize;
    h.rows = rows;
    h.scroll = h.scroll.min(all.len().saturating_sub(rows));
    if all.is_empty() {
        let text = format!("No key, name or action matches \"{}\". Clear the search to see everything.", h.search.text().trim());
        for (i, line) in panel::wrap(&text, inner.width as usize).iter().take(rows).enumerate() {
            pane.put(buf, inner.x, inner.y + 2 + i as u16, line, p.fg(Role::Subtext0).bg(bg));
        }
    }
    for (i, line) in all.iter().skip(h.scroll).take(rows).enumerate() {
        let y = inner.y + 2 + i as u16;
        match line {
            Line::Title(t) => {
                pane.put(buf, inner.x, y, t, p.bold(Role::Accent).bg(bg));
            }
            Line::Blank => {}
            Line::Row(row, which, first) => {
                if *first {
                    pane.within(Rect::new(inner.x, y, LABEL - 1, 1)).put(buf, inner.x, y, row.label, p.fg(Role::Subtext0).bg(bg));
                    if named {
                        // A name longer than its column is cut at it, not into the keys.
                        let at = inner.x + LABEL;
                        pane.within(Rect::new(at, y, NAME - 1, 1)).put(buf, at, y, &row.name, p.fg(Role::Overlay0).bg(bg));
                    }
                }
                let mut x = inner.x + keys_at;
                for b in &row.bindings[which.clone()] {
                    let missing = b.needs_protocol && !caps.keyboard_protocol;
                    let style = if missing { p.fg(Role::Overlay0).bg(bg) } else { p.bold(Role::Text).bg(bg) };
                    x = pane.put(buf, x, y, &key_text(b, caps), style) + 2;
                }
            }
        }
    }
    // The foot: what the brackets mean, where in the list, and the keys.
    let foot = inner.bottom() - 1;
    let note = if caps.keyboard_protocol { "" } else { "[keys] need a terminal with the kitty keyboard protocol; this one has none" };
    pane.put(buf, inner.x, foot - 1, note, p.fg(Role::Overlay0).bg(bg));
    if all.len() > rows {
        let text = format!("{}-{} of {}", h.scroll + 1, (h.scroll + rows).min(all.len()), all.len());
        pane.put(buf, inner.x, foot, &text, p.fg(Role::Overlay0).bg(bg));
    }
    let buttons = panel::buttons(p, buf, inner, foot, Overlay::Help(h.clone()).buttons());
    Drawn { buttons, fields: Vec::new() }
}

/// The run confirmation: red, the title saying production or dangerous, then
/// where it runs, why, and the exact statement, scrollable, through the
/// painter. `y run` and `esc cancel` at the foot; `enter` is not a button.
pub(super) fn draw_confirm_run(c: &mut ConfirmRun, p: &Painter, buf: &mut Buffer) -> Drawn {
    let a = &c.assessment;
    let title = if a.production {
        "Run on production?"
    } else if a.reasons.iter().any(|r| *r != Reason::Changed) {
        "Run a dangerous statement?"
    } else {
        "Run this statement?"
    };
    let mut head = format!("On {} / {}.", a.connection_name, a.confirmation.database);
    if !a.named.is_empty() {
        head.push_str(&format!(" It names {}, marked production.", a.named.join(", ")));
    }
    if a.target_unknown {
        head.push_str(" dbx cannot tell which database it writes to, so it counts as production.");
    }
    let why: Vec<&str> = a.reasons.iter().map(|r| r.words()).collect();
    let mut why = format!("Stopped because {}.", why.join("; "));
    if a.production {
        why.push_str(" Production is always confirmed: no setting turns this off.");
    }
    let area = buf.area;
    let width = 90.min(area.width.saturating_sub(4)).max(24.min(area.width));
    let text_w = width.saturating_sub(4) as usize;
    let mut intro = panel::wrap(&head, text_w);
    intro.extend(panel::wrap(&why, text_w));
    let statement = statement_lines(&a.confirmation.statement, text_w.saturating_sub(2).max(1));
    // Border, intro, a blank row, the statement, a blank row, the foot, border.
    let want = intro.len() + statement.len() + 5;
    let height = (want as u16).min(area.height.saturating_sub(2));
    let rect = panel::centred(area, width, height);
    let inner = panel::panel(p, buf, rect, title, Role::Red, Role::Red);
    if inner.width < 8 || inner.height < 4 {
        return Drawn::default();
    }
    let bg = p.theme.color(Role::PanelBg);
    let pane = p.within(inner);
    let mut y = inner.y;
    let foot = inner.bottom() - 1;
    for line in &intro {
        if y + 2 >= foot {
            break;
        }
        pane.put(buf, inner.x, y, line, p.fg(Role::Text).bg(bg));
        y += 1;
    }
    y += 1;
    let rows = foot.saturating_sub(y + 1) as usize;
    c.scroll = c.scroll.min(statement.len().saturating_sub(rows));
    for (i, line) in statement.iter().skip(c.scroll).take(rows).enumerate() {
        pane.put(buf, inner.x + 2, y + i as u16, line, p.bold(Role::Text).bg(bg));
    }
    if statement.len() > rows && rows > 0 {
        let text = format!("lines {}-{} of {}", c.scroll + 1, (c.scroll + rows).min(statement.len()), statement.len());
        pane.put(buf, inner.x, foot, &text, p.fg(Role::Overlay0).bg(bg));
    }
    let buttons = panel::buttons(p, buf, inner, foot, Overlay::ConfirmRun(Box::new(c.clone())).buttons());
    Drawn { buttons, fields: Vec::new() }
}

/// The statement's lines, each cut into pieces of at most `width` cells,
/// spaces kept: nothing of the text is dropped or reflowed.
pub(super) fn statement_lines(text: &str, width: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = crate::core::display_safe(line.trim_end_matches('\r'));
        let (mut piece, mut used) = (String::new(), 0usize);
        for g in line.graphemes(true) {
            let w = super::drawn_width(g);
            if used + w > width && !piece.is_empty() {
                out.push(std::mem::take(&mut piece));
                used = 0;
            }
            piece.push_str(g);
            used += w;
        }
        out.push(piece);
    }
    out
}
