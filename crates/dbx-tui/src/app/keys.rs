//! Key combinations, actions, and the bindings between them.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key and its modifiers, normalised so `Q` and `shift+q` compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        // Shift is already in the character of a printable key: fold it in, so
        // `shift+q` is `Q` and differs from `q`.
        match code {
            KeyCode::Char(c) if mods.contains(KeyModifiers::SHIFT) => {
                Self { code: KeyCode::Char(c.to_ascii_uppercase()), mods: mods - KeyModifiers::SHIFT }
            }
            _ => Self { code, mods },
        }
    }

    pub fn from_event(e: &KeyEvent) -> Self {
        Self::new(e.code, e.modifiers)
    }

    /// Parses `"ctrl+g"`, `"alt+enter"`, `"esc"`, `"q"`, `"f5"`.
    pub fn parse(s: &str) -> Option<Self> {
        // The key is whatever follows the last `+`, except that a `+` right
        // after the separator is the plus key itself: "+", "ctrl++".
        let (mod_names, last) = if s == "+" {
            ("", "+")
        } else if let Some(front) = s.strip_suffix("++").filter(|f| !f.is_empty()) {
            (front, "+")
        } else {
            match s.rsplit_once('+') {
                Some(("", _)) => return None,
                Some((front, key)) => (front, key),
                None => ("", s),
            }
        };
        let mut mods = KeyModifiers::NONE;
        if !mod_names.is_empty() {
            for m in mod_names.split('+') {
                mods |= match m {
                    "ctrl" => KeyModifiers::CONTROL,
                    "alt" => KeyModifiers::ALT,
                    "shift" => KeyModifiers::SHIFT,
                    _ => return None,
                };
            }
        }
        let code = match last {
            "esc" => KeyCode::Esc,
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "backtab" => KeyCode::BackTab,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            f if f.len() > 1 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
                KeyCode::F(f[1..].parse().ok()?)
            }
            c if c.chars().count() == 1 => KeyCode::Char(c.chars().next()?),
            _ => return None,
        };
        Some(Self::new(code, mods))
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "f{n}"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Where an action's keys are read: after the prefix, or directly by the
/// sidebar when it has focus (it accepts no text, so it needs no prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Prefix,
    /// Read directly, whichever pane has focus, before the pane sees the key
    /// (`f5`, `alt+enter`): keys no pane uses for anything else.
    Direct,
    Sidebar,
    /// Read by the results pane when it has focus, and by the cell viewer
    /// (neither takes text, so neither needs the prefix).
    Results,
    /// Read by the resize mode, which takes every key while it is on.
    Resize,
    /// Read by the editor while the completion popup is open, before the
    /// editor itself sees the key.
    Complete,
}

/// A named thing the client can do, to which keys are bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    FocusLeft,
    FocusDown,
    FocusUp,
    FocusRight,
    Zoom,
    NewConnection,
    Quit,
    /// Press the prefix key. Has no binding of its own: the prefix is a key
    /// (`Keybinds::prefix`); this is what a click on its hint does.
    OpenPrefix,
    /// Run the selection, or the statement under the cursor, in the query tab.
    Run,
    /// The same, from a direct key (`f5`, `alt+enter`).
    RunDirect,
    /// Cancel the tab's run; while a cancel is waiting to land, offer to drop
    /// the tab's session.
    Cancel,
    /// The same, from a direct key (`ctrl+c`), which works only while the
    /// editor or the results pane has focus and otherwise does nothing.
    CancelDirect,
    // The sidebar's own keys.
    MoveDown,
    MoveUp,
    Expand,
    Collapse,
    PageDown,
    PageUp,
    First,
    Last,
    Refresh,
    EditConnection,
    RemoveConnection,
    /// Make the database under the cursor the one the query tab runs on.
    UseTarget,
    // The results pane's own keys (and the cell viewer's).
    GridLeft,
    GridRight,
    GridUp,
    GridDown,
    GridPageUp,
    GridPageDown,
    GridFirstCol,
    GridLastCol,
    GridFirstRow,
    GridLastRow,
    /// Open the cell under the cursor in the viewer.
    OpenCell,
    /// Copy the cell (or, in the viewer, its raw value), or the whole row.
    CopyCell,
    CopyRow,
    /// Fetch more rows of a truncated result.
    LoadMore,
    /// In the viewer: JSON as pretty-printed, or as stored.
    ToggleRaw,
    /// Enter the resize mode (after the prefix).
    ResizeMode,
    // The resize mode's own keys: the dividers move one step.
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    ResizeDone,
    /// Open the completion popup by hand (a direct key, in the editor).
    CompleteOpen,
    // The completion popup's own keys, read while it is open.
    /// Insert the selected suggestion at the caret.
    CompleteInsert,
    /// Replace the rest of the word to the right of the caret with it.
    CompleteReplace,
    CompleteUp,
    CompleteDown,
    CompletePageUp,
    CompletePageDown,
    CompleteClose,
}

impl Action {
    /// What the mode bar says the key does. Actions with the same label share
    /// one hint (`h/j/k/l focus`).
    pub fn label(self) -> &'static str {
        match self {
            Action::FocusLeft | Action::FocusDown | Action::FocusUp | Action::FocusRight => "focus",
            Action::Zoom => "zoom",
            Action::NewConnection => "new",
            Action::Quit => "quit",
            Action::OpenPrefix => "prefix",
            Action::Run | Action::RunDirect => "run",
            Action::Cancel | Action::CancelDirect => "cancel",
            Action::MoveDown | Action::MoveUp => "move",
            Action::Expand => "expand",
            Action::Collapse => "collapse",
            Action::PageDown | Action::PageUp => "page",
            Action::First => "top",
            Action::Last => "bottom",
            Action::Refresh => "refresh",
            Action::EditConnection => "edit",
            Action::RemoveConnection => "remove",
            Action::UseTarget => "use",
            Action::GridLeft | Action::GridRight | Action::GridUp | Action::GridDown => "move",
            Action::GridPageUp | Action::GridPageDown => "page",
            Action::GridFirstCol => "first col",
            Action::GridLastCol => "last col",
            Action::GridFirstRow => "first row",
            Action::GridLastRow => "last row",
            Action::OpenCell => "view",
            Action::CopyCell => "copy",
            Action::CopyRow => "copy row",
            Action::LoadMore => "more",
            Action::ToggleRaw => "raw",
            Action::ResizeMode => "resize",
            Action::ResizeLeft | Action::ResizeRight => "sidebar",
            Action::ResizeUp | Action::ResizeDown => "split",
            Action::ResizeDone => "done",
            Action::CompleteOpen => "complete",
            Action::CompleteInsert => "insert",
            Action::CompleteReplace => "replace",
            Action::CompleteUp | Action::CompleteDown => "move",
            Action::CompletePageUp | Action::CompletePageDown => "page",
            Action::CompleteClose => "close",
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Action::FocusLeft
            | Action::FocusDown
            | Action::FocusUp
            | Action::FocusRight
            | Action::Zoom
            | Action::NewConnection
            | Action::Quit
            | Action::Run
            | Action::Cancel
            | Action::ResizeMode
            | Action::OpenPrefix => Scope::Prefix,
            Action::ResizeLeft | Action::ResizeRight | Action::ResizeUp | Action::ResizeDown | Action::ResizeDone => Scope::Resize,
            Action::RunDirect | Action::CancelDirect | Action::CompleteOpen => Scope::Direct,
            Action::CompleteInsert
            | Action::CompleteReplace
            | Action::CompleteUp
            | Action::CompleteDown
            | Action::CompletePageUp
            | Action::CompletePageDown
            | Action::CompleteClose => Scope::Complete,
            Action::GridLeft
            | Action::GridRight
            | Action::GridUp
            | Action::GridDown
            | Action::GridPageUp
            | Action::GridPageDown
            | Action::GridFirstCol
            | Action::GridLastCol
            | Action::GridFirstRow
            | Action::GridLastRow
            | Action::OpenCell
            | Action::CopyCell
            | Action::CopyRow
            | Action::LoadMore
            | Action::ToggleRaw => Scope::Results,
            _ => Scope::Sidebar,
        }
    }
}

/// The prefix key and the keys that follow it, and the sidebar's own keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinds {
    pub prefix: KeyCombo,
    bindings: Vec<(KeyCombo, Action)>,
    direct: Vec<(KeyCombo, Action)>,
    sidebar: Vec<(KeyCombo, Action)>,
    results: Vec<(KeyCombo, Action)>,
    resize: Vec<(KeyCombo, Action)>,
    complete: Vec<(KeyCombo, Action)>,
}

impl Default for Keybinds {
    fn default() -> Self {
        let key = |s: &str| KeyCombo::parse(s).expect("a default key parses");
        let plain = |c| KeyCombo::new(KeyCode::Char(c), KeyModifiers::NONE);
        Self {
            // Not ctrl+b: this client will often run inside herdr or tmux.
            prefix: KeyCombo::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            bindings: vec![
                (plain('h'), Action::FocusLeft),
                (plain('j'), Action::FocusDown),
                (plain('k'), Action::FocusUp),
                (plain('l'), Action::FocusRight),
                (plain('n'), Action::NewConnection),
                (plain('r'), Action::Run),
                (plain('c'), Action::Cancel),
                (plain('z'), Action::Zoom),
                (plain('q'), Action::Quit),
                (plain('R'), Action::ResizeMode),
            ],
            direct: vec![
                (key("f5"), Action::RunDirect),
                (key("alt+enter"), Action::RunDirect),
                (key("ctrl+c"), Action::CancelDirect),
                (key("ctrl+space"), Action::CompleteOpen),
            ],
            sidebar: vec![
                (key("j"), Action::MoveDown),
                (key("down"), Action::MoveDown),
                (key("k"), Action::MoveUp),
                (key("up"), Action::MoveUp),
                (key("enter"), Action::Expand),
                (key("l"), Action::Expand),
                (key("right"), Action::Expand),
                (key("h"), Action::Collapse),
                (key("left"), Action::Collapse),
                (key("pagedown"), Action::PageDown),
                (key("ctrl+d"), Action::PageDown),
                (key("pageup"), Action::PageUp),
                (key("ctrl+u"), Action::PageUp),
                (key("g"), Action::First),
                (key("home"), Action::First),
                (key("G"), Action::Last),
                (key("end"), Action::Last),
                (key("r"), Action::Refresh),
                (key("e"), Action::EditConnection),
                (key("d"), Action::RemoveConnection),
                (key("delete"), Action::RemoveConnection),
                (key("u"), Action::UseTarget),
            ],
            results: vec![
                (key("h"), Action::GridLeft),
                (key("left"), Action::GridLeft),
                (key("l"), Action::GridRight),
                (key("right"), Action::GridRight),
                (key("k"), Action::GridUp),
                (key("up"), Action::GridUp),
                (key("j"), Action::GridDown),
                (key("down"), Action::GridDown),
                (key("pageup"), Action::GridPageUp),
                (key("ctrl+u"), Action::GridPageUp),
                (key("pagedown"), Action::GridPageDown),
                (key("ctrl+d"), Action::GridPageDown),
                (key("home"), Action::GridFirstCol),
                (key("0"), Action::GridFirstCol),
                (key("end"), Action::GridLastCol),
                (key("$"), Action::GridLastCol),
                (key("g"), Action::GridFirstRow),
                (key("G"), Action::GridLastRow),
                (key("enter"), Action::OpenCell),
                (key("y"), Action::CopyCell),
                (key("Y"), Action::CopyRow),
                (key("m"), Action::LoadMore),
                (key("r"), Action::ToggleRaw),
            ],
            resize: vec![
                (key("h"), Action::ResizeLeft),
                (key("left"), Action::ResizeLeft),
                (key("l"), Action::ResizeRight),
                (key("right"), Action::ResizeRight),
                (key("k"), Action::ResizeUp),
                (key("up"), Action::ResizeUp),
                (key("j"), Action::ResizeDown),
                (key("down"), Action::ResizeDown),
                (key("esc"), Action::ResizeDone),
                (key("enter"), Action::ResizeDone),
            ],
            complete: vec![
                (key("enter"), Action::CompleteInsert),
                (key("tab"), Action::CompleteReplace),
                (key("down"), Action::CompleteDown),
                (key("ctrl+n"), Action::CompleteDown),
                (key("up"), Action::CompleteUp),
                (key("ctrl+p"), Action::CompleteUp),
                (key("pagedown"), Action::CompletePageDown),
                (key("pageup"), Action::CompletePageUp),
                (key("esc"), Action::CompleteClose),
            ],
        }
    }
}

impl Keybinds {
    /// The action bound to `key` after the prefix.
    pub fn lookup(&self, key: &KeyCombo) -> Option<Action> {
        self.bindings.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action bound directly to `key`, read before any pane sees it.
    pub fn lookup_direct(&self, key: &KeyCombo) -> Option<Action> {
        self.direct.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action the sidebar binds to `key`.
    pub fn lookup_sidebar(&self, key: &KeyCombo) -> Option<Action> {
        self.sidebar.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action the results pane (or the cell viewer) binds to `key`.
    pub fn lookup_results(&self, key: &KeyCombo) -> Option<Action> {
        self.results.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action the resize mode binds to `key`.
    pub fn lookup_resize(&self, key: &KeyCombo) -> Option<Action> {
        self.resize.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action the completion popup binds to `key`.
    pub fn lookup_complete(&self, key: &KeyCombo) -> Option<Action> {
        self.complete.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    fn table(&self, scope: Scope) -> &[(KeyCombo, Action)] {
        match scope {
            Scope::Results => &self.results,
            Scope::Prefix => &self.bindings,
            Scope::Direct => &self.direct,
            Scope::Sidebar => &self.sidebar,
            Scope::Resize => &self.resize,
            Scope::Complete => &self.complete,
        }
    }

    /// The key bound to `action`, as shown, if any. When several keys do the
    /// same thing, the first.
    pub fn key_for(&self, action: Action) -> Option<String> {
        self.table(action.scope()).iter().find(|(_, a)| *a == action).map(|(k, _)| k.to_string())
    }

    /// Replaces every key bound to `action` with `key`, taking the key from
    /// whatever else it did in the same scope. Returns false, and changes
    /// nothing, if `key` does not parse.
    pub fn rebind(&mut self, action: Action, key: &str) -> bool {
        let Some(combo) = KeyCombo::parse(key) else { return false };
        let table = match action.scope() {
            Scope::Prefix => &mut self.bindings,
            Scope::Direct => &mut self.direct,
            Scope::Sidebar => &mut self.sidebar,
            Scope::Results => &mut self.results,
            Scope::Resize => &mut self.resize,
            Scope::Complete => &mut self.complete,
        };
        table.retain(|(k, a)| *a != action && *k != combo);
        table.push((combo, action));
        true
    }

    /// The hints for the mode bar after the prefix, from the live bindings:
    /// keys that do the same kind of thing share one (`h/j/k/l focus`).
    pub fn prefix_hints(&self) -> Vec<Hint> {
        let mut out: Vec<Hint> = Vec::new();
        for (key, action) in &self.bindings {
            match out.iter_mut().find(|h| h.desc == action.label()) {
                Some(hint) => hint.keys.push((key.to_string(), *action)),
                None => out.push(Hint { keys: vec![(key.to_string(), *action)], desc: action.label() }),
            }
        }
        out
    }

    /// The hints for the mode bar in the resize mode, from the live bindings:
    /// the sidebar's keys, the split's, and the one that leaves.
    pub fn resize_hints(&self) -> Vec<Hint> {
        let one = |actions: &[Action], desc: &'static str| Hint {
            keys: actions.iter().filter_map(|a| self.key_for(*a).map(|k| (k, *a))).collect(),
            desc,
        };
        let mut out = vec![
            one(&[Action::ResizeLeft, Action::ResizeRight], "sidebar"),
            one(&[Action::ResizeDown, Action::ResizeUp], "split"),
            one(&[Action::ResizeDone], "done"),
        ];
        out.retain(|h| !h.keys.is_empty());
        out
    }

    /// The hints for the mode bar while the completion popup is open, from
    /// the live bindings.
    pub fn complete_hints(&self) -> Vec<Hint> {
        let one = |actions: &[Action], desc: &'static str| Hint {
            keys: actions.iter().filter_map(|a| self.key_for(*a).map(|k| (k, *a))).collect(),
            desc,
        };
        let mut out = vec![
            one(&[Action::CompleteInsert], "insert"),
            one(&[Action::CompleteReplace], "replace"),
            one(&[Action::CompleteDown, Action::CompleteUp], "move"),
            one(&[Action::CompletePageDown, Action::CompletePageUp], "page"),
            one(&[Action::CompleteClose], "close"),
        ];
        out.retain(|h| !h.keys.is_empty());
        out
    }

    /// `(keys, description)` pairs, as text.
    pub fn hints(&self) -> Vec<(String, &'static str)> {
        self.prefix_hints()
            .into_iter()
            .map(|h| (h.keys.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join("/"), h.desc))
            .collect()
    }
}

/// One hint in the mode bar: the keys (each with the action it does, so a click
/// on it can do that) and what they do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub keys: Vec<(String, Action)>,
    pub desc: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_on_a_letter_means_the_capital_and_not_the_lowercase_letter() {
        assert_eq!(KeyCombo::parse("shift+q"), KeyCombo::parse("Q"));
        assert_ne!(KeyCombo::parse("shift+q"), KeyCombo::parse("q"));
        // A terminal that reports a lowercase letter with shift set.
        assert_eq!(KeyCombo::new(KeyCode::Char('q'), KeyModifiers::SHIFT), KeyCombo::parse("Q").unwrap());
        assert_eq!(KeyCombo::parse("Q").unwrap().to_string(), "Q");
    }

    #[test]
    fn a_plus_after_a_modifier_is_the_plus_key_and_a_dangling_plus_is_an_error() {
        let ctrl_plus = KeyCombo::new(KeyCode::Char('+'), KeyModifiers::CONTROL);
        assert_eq!(KeyCombo::parse("ctrl++"), Some(ctrl_plus));
        assert_eq!(ctrl_plus.to_string(), "ctrl++");
        assert_eq!(KeyCombo::parse("+"), Some(KeyCombo::new(KeyCode::Char('+'), KeyModifiers::NONE)));
        for bad in ["ctrl+", "a+", "alt+ctrl+", "++", "+x", "ctrl+ab"] {
            assert_eq!(KeyCombo::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn parses_and_prints_key_names() {
        for s in ["ctrl+g", "alt+enter", "esc", "q", "f5", "+", "space"] {
            assert_eq!(KeyCombo::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(KeyCombo::parse("Q"), KeyCombo::parse("shift+Q"));
        assert_eq!(KeyCombo::parse("hyper+x"), None);
        assert_eq!(KeyCombo::parse(""), None);
    }
}
