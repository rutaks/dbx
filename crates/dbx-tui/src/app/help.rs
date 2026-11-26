//! The help overlay's input (slice 21). The rows come from
//! `Keybinds::help()`, built from the live bindings; here is what typing does
//! to them. A search narrows the rows to those whose label, config name or
//! key contains every word typed; the arrows and page keys scroll, and every
//! printable key goes to the search field (so the list's own keys are not
//! letters).

use crossterm::event::{KeyCode, KeyEvent};

use super::keys::{HelpSection, KeyCombo};
use super::overlay::Overlay;
use super::text_field::Edit;
use super::{App, Effects};

/// The sections with only the rows that answer `search`; a section left with
/// none is dropped. An empty search keeps everything.
pub fn filter(sections: Vec<HelpSection>, search: &str) -> Vec<HelpSection> {
    if search.trim().is_empty() {
        return sections;
    }
    // A query with a space that is one whole key of some row means that key;
    // only without such a row do the words stand apart.
    let whole =
        search.split_whitespace().count() > 1 && sections.iter().flat_map(|s| &s.rows).any(|r| r.matches_whole(search));
    sections
        .into_iter()
        .filter_map(|mut s| {
            s.rows.retain(|r| if whole { r.matches_whole(search) } else { r.matches(search) });
            (!s.rows.is_empty()).then_some(s)
        })
        .collect()
}

impl App {
    pub(super) fn on_help_key(&mut self, key: &KeyEvent, combo: KeyCombo, fx: &mut Effects) {
        let plain = combo.mods.is_empty();
        let Some(Overlay::Help(h)) = &mut self.ui.overlay else { return };
        // The view keeps the scroll in range.
        let page = h.rows.saturating_sub(1).max(1);
        match combo.code {
            // Typed text is not lost to one key: the first `esc` clears the
            // search, the second closes.
            KeyCode::Esc if plain && !h.search.text().is_empty() => {
                h.search.set_text("");
                h.scroll = 0;
                fx.repaint = true;
            }
            KeyCode::Esc if plain => {
                self.ui.overlay = None;
                fx.repaint = true;
            }
            KeyCode::Down if plain => {
                h.scroll += 1;
                fx.repaint = true;
            }
            KeyCode::Up if plain => {
                h.scroll = h.scroll.saturating_sub(1);
                fx.repaint = true;
            }
            KeyCode::PageDown if plain => {
                h.scroll += page;
                fx.repaint = true;
            }
            KeyCode::PageUp if plain => {
                h.scroll = h.scroll.saturating_sub(page);
                fx.repaint = true;
            }
            _ => match h.search.handle_key(key) {
                Edit::Changed => {
                    h.scroll = 0;
                    fx.repaint = true;
                }
                Edit::Moved => fx.repaint = true,
                Edit::Ignored => {}
            },
        }
    }

    pub(super) fn on_help_paste(&mut self, text: &str, fx: &mut Effects) {
        let Some(Overlay::Help(h)) = &mut self.ui.overlay else { return };
        h.search.insert_str(text);
        h.scroll = 0;
        fx.repaint = true;
    }
}
