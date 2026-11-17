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
        }
    }
}

/// The prefix key and the keys that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinds {
    pub prefix: KeyCombo,
    bindings: Vec<(KeyCombo, Action)>,
}

impl Default for Keybinds {
    fn default() -> Self {
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
                (plain('z'), Action::Zoom),
                (plain('q'), Action::Quit),
            ],
        }
    }
}

impl Keybinds {
    /// The action bound to `key` after the prefix.
    pub fn lookup(&self, key: &KeyCombo) -> Option<Action> {
        self.bindings.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The key bound to `action`, as shown, if any.
    pub fn key_for(&self, action: Action) -> Option<String> {
        self.bindings.iter().find(|(_, a)| *a == action).map(|(k, _)| k.to_string())
    }

    /// Replaces every key bound to `action` with `key`. Returns false, and
    /// changes nothing, if `key` does not parse.
    pub fn rebind(&mut self, action: Action, key: &str) -> bool {
        let Some(combo) = KeyCombo::parse(key) else { return false };
        self.bindings.retain(|(k, a)| *a != action && *k != combo);
        self.bindings.push((combo, action));
        true
    }

    /// `(keys, description)` pairs for the mode bar, from the live bindings.
    pub fn hints(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        for (key, action) in &self.bindings {
            match out.iter_mut().find(|(_, label)| *label == action.label()) {
                Some((keys, _)) => {
                    keys.push('/');
                    keys.push_str(&key.to_string());
                }
                None => out.push((key.to_string(), action.label())),
            }
        }
        out
    }
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
