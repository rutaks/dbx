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
            // A letter with shift is the capital. Any other character with
            // shift alone is already the character typed (`?`); with Ctrl, Alt
            // or Cmd as well, shift is part of the chord (`ctrl+shift+/` is not
            // `ctrl+/`).
            KeyCode::Char(c) if mods.contains(KeyModifiers::SHIFT) && c.is_alphabetic() => {
                Self { code: KeyCode::Char(c.to_ascii_uppercase()), mods: mods - KeyModifiers::SHIFT }
            }
            KeyCode::Char(c) if mods.contains(KeyModifiers::SHIFT) && mods == KeyModifiers::SHIFT => {
                Self { code: KeyCode::Char(c), mods: KeyModifiers::NONE }
            }
            // `shift+tab` is one key, which terminals send as `BackTab` with
            // or without shift set.
            KeyCode::Tab | KeyCode::BackTab if mods.contains(KeyModifiers::SHIFT) => Self { code: KeyCode::BackTab, mods: mods - KeyModifiers::SHIFT },
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
                    "cmd" | "super" => KeyModifiers::SUPER,
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
        if self.mods.contains(KeyModifiers::SUPER) {
            f.write_str("cmd+")?;
        }
        // A capital with a modifier is shift held with the letter.
        let shifted_letter = matches!(self.code, KeyCode::Char(c) if c.is_ascii_uppercase()) && !self.mods.is_empty();
        if self.mods.contains(KeyModifiers::SHIFT) || shifted_letter {
            f.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(c) if shifted_letter => write!(f, "{}", c.to_ascii_lowercase()),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::BackTab => f.write_str("shift+tab"),
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

impl KeyCombo {
    /// Whether a terminal without the kitty keyboard protocol cannot tell this
    /// key from another: Cmd never arrives; Ctrl with Enter, Backspace or a
    /// punctuation key, and Ctrl with Shift and a letter, send what a plainer
    /// key sends; Shift+Enter is Enter. A judgment from how legacy terminals
    /// encode keys (`^M` is Enter, `^H` is Backspace, `^_` is `ctrl+/`).
    pub fn needs_protocol(&self) -> bool {
        let m = self.mods;
        if m.contains(KeyModifiers::SUPER) {
            return true;
        }
        let ctrl = m.contains(KeyModifiers::CONTROL);
        match self.code {
            KeyCode::Enter => ctrl || m.contains(KeyModifiers::SHIFT),
            KeyCode::Backspace | KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => ctrl,
            KeyCode::Char(' ') => false,
            KeyCode::Char(c) => ctrl && (c.is_ascii_uppercase() || !c.is_ascii_alphabetic() || matches!(c, 'm' | 'i' | 'h' | 'j')),
            _ => false,
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
    /// Read by the editor while a live template is being filled in, before the
    /// editor itself sees the key.
    Template,
    /// Read by the focused editor, after the completion popup and template mode
    /// have had the key.
    Editor,
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
    /// The same, from a direct key (`f5`).
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
    /// Open the table or view under the cursor in a tab of its own, paged.
    OpenTable,
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
    /// In a browse tab: the next page of the table, the previous one, and
    /// the count of its rows.
    NextPage,
    PrevPage,
    CountRows,
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
    /// The intention actions (`alt+enter`, as in IntelliJ): on a select list's
    /// `*` the caret is on, replace it with the columns it stands for; where
    /// no action applies, nothing happens, and no hint is shown. It is not a
    /// run key.
    ExpandStar,
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
    /// Expand the live template the popup has highlighted (`enter` and `tab`
    /// by default: what is highlighted is what you get). Read before the
    /// popup's own keys, and only while a template is highlighted.
    ExpandTemplate,
    // The keys of a live template being filled in.
    /// Move to the next variable; from the last, to the template's end.
    TemplateNext,
    TemplatePrev,
    /// Leave template mode with the text as it is.
    TemplateDone,
    /// Put the caret on the next / previous name the loaded schema does not
    /// have (direct keys, shown only while there is one).
    NextUnresolved,
    PrevUnresolved,
    /// Close the open parentheses of the statement under the caret and add its
    /// semicolon: from a direct key, and after the prefix.
    FinishStatementDirect,
    FinishStatement,
    /// The key help overlay (after the prefix).
    Help,
    // The editor's own actions (slice 13). Each is bound in the editor's table
    // to IntelliJ's macOS key (Cmd), its Windows/Linux key (Ctrl) and, where
    // one exists, a key every terminal delivers; the prefix has a few more.
    Duplicate,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
    MoveStatementUp,
    MoveStatementDown,
    LineComment,
    BlockComment,
    ExtendSelection,
    ShrinkSelection,
    SelectAll,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Find,
    FindNext,
    FindPrev,
    GotoLine,
    NewLineAfter,
    NewLineBefore,
    JoinLines,
    ToggleCase,
    Reformat,
    ReformatAll,
    Indent,
    Unindent,
    WordLeft,
    WordRight,
    WordLeftSelect,
    WordRightSelect,
    DeleteWordBack,
    DeleteWordForward,
    LineStart,
    LineEnd,
    LineStartSelect,
    LineEndSelect,
    TextStart,
    TextEnd,
    TextStartSelect,
    TextEndSelect,
    MatchingBrace,
    // The query tabs (slice 15b): after the prefix, and the same from direct
    // keys (IntelliJ's, where a terminal delivers them).
    /// Open a tab on the active tab's target, and make it active.
    NewTab,
    /// Close the active tab, asking first if it has text or a run.
    CloseTab,
    NextTab,
    PrevTab,
    /// Make tab `n` (counted from 1, as the bar numbers them) active.
    GoToTab(u8),
    CloseTabDirect,
    NextTabDirect,
    PrevTabDirect,
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
            Action::OpenTable => "browse",
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
            Action::NextPage => "next page",
            Action::PrevPage => "prev page",
            Action::CountRows => "count",
            Action::ToggleRaw => "raw",
            Action::ResizeMode => "resize",
            Action::ResizeLeft | Action::ResizeRight => "sidebar",
            Action::ResizeUp | Action::ResizeDown => "split",
            Action::ResizeDone => "done",
            Action::CompleteOpen => "complete",
            Action::ExpandStar => "expand *",
            Action::CompleteInsert => "insert",
            Action::CompleteReplace => "replace",
            Action::CompleteUp | Action::CompleteDown => "move",
            Action::CompletePageUp | Action::CompletePageDown => "page",
            Action::CompleteClose => "close",
            Action::ExpandTemplate => "expand",
            Action::TemplateNext => "next",
            Action::TemplatePrev => "prev",
            Action::TemplateDone => "done",
            Action::NextUnresolved => "next error",
            Action::PrevUnresolved => "prev error",
            Action::FinishStatementDirect | Action::FinishStatement => "finish",
            Action::Help => "help",
            Action::Duplicate => "duplicate",
            Action::DeleteLine => "delete line",
            Action::MoveLineUp => "move line up",
            Action::MoveLineDown => "move line down",
            Action::MoveStatementUp => "move statement up",
            Action::MoveStatementDown => "move statement down",
            Action::LineComment => "line comment",
            Action::BlockComment => "block comment",
            Action::ExtendSelection => "extend selection",
            Action::ShrinkSelection => "shrink selection",
            Action::SelectAll => "select all",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::Cut => "cut",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::Find => "find",
            Action::FindNext => "find next",
            Action::FindPrev => "find previous",
            Action::GotoLine => "go to line",
            Action::NewLineAfter => "new line after",
            Action::NewLineBefore => "new line before",
            Action::JoinLines => "join lines",
            Action::ToggleCase => "toggle case",
            Action::Reformat => "reformat",
            Action::ReformatAll => "reformat all",
            Action::Indent => "indent",
            Action::Unindent => "unindent",
            Action::WordLeft => "word left",
            Action::WordRight => "word right",
            Action::WordLeftSelect => "select word left",
            Action::WordRightSelect => "select word right",
            Action::DeleteWordBack => "delete word back",
            Action::DeleteWordForward => "delete word forward",
            Action::LineStart => "line start",
            Action::LineEnd => "line end",
            Action::LineStartSelect => "select to line start",
            Action::LineEndSelect => "select to line end",
            Action::TextStart => "text start",
            Action::TextEnd => "text end",
            Action::TextStartSelect => "select to text start",
            Action::TextEndSelect => "select to text end",
            Action::MatchingBrace => "matching brace",
            Action::NewTab => "new tab",
            Action::CloseTab | Action::CloseTabDirect => "close tab",
            Action::NextTab | Action::NextTabDirect => "next tab",
            Action::PrevTab | Action::PrevTabDirect => "previous tab",
            Action::GoToTab(_) => "go to tab",
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
            | Action::FinishStatement
            | Action::Help
            | Action::NewTab
            | Action::CloseTab
            | Action::NextTab
            | Action::PrevTab
            | Action::GoToTab(_)
            | Action::OpenPrefix => Scope::Prefix,
            Action::ResizeLeft | Action::ResizeRight | Action::ResizeUp | Action::ResizeDown | Action::ResizeDone => Scope::Resize,
            Action::RunDirect
            | Action::CancelDirect
            | Action::CompleteOpen
            | Action::ExpandStar
            | Action::NextUnresolved
            | Action::PrevUnresolved
            | Action::FinishStatementDirect
            | Action::CloseTabDirect
            | Action::NextTabDirect
            | Action::PrevTabDirect => Scope::Direct,
            Action::TemplateNext | Action::TemplatePrev | Action::TemplateDone => Scope::Template,
            Action::CompleteInsert
            | Action::CompleteReplace
            | Action::CompleteUp
            | Action::CompleteDown
            | Action::CompletePageUp
            | Action::CompletePageDown
            | Action::ExpandTemplate
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
            | Action::NextPage
            | Action::PrevPage
            | Action::CountRows
            | Action::ToggleRaw => Scope::Results,
            Action::Duplicate
            | Action::DeleteLine
            | Action::MoveLineUp
            | Action::MoveLineDown
            | Action::MoveStatementUp
            | Action::MoveStatementDown
            | Action::LineComment
            | Action::BlockComment
            | Action::ExtendSelection
            | Action::ShrinkSelection
            | Action::SelectAll
            | Action::Undo
            | Action::Redo
            | Action::Cut
            | Action::Copy
            | Action::Paste
            | Action::Find
            | Action::FindNext
            | Action::FindPrev
            | Action::GotoLine
            | Action::NewLineAfter
            | Action::NewLineBefore
            | Action::JoinLines
            | Action::ToggleCase
            | Action::Reformat
            | Action::ReformatAll
            | Action::Indent
            | Action::Unindent
            | Action::WordLeft
            | Action::WordRight
            | Action::WordLeftSelect
            | Action::WordRightSelect
            | Action::DeleteWordBack
            | Action::DeleteWordForward
            | Action::LineStart
            | Action::LineEnd
            | Action::LineStartSelect
            | Action::LineEndSelect
            | Action::TextStart
            | Action::TextEnd
            | Action::TextStartSelect
            | Action::TextEndSelect
            | Action::MatchingBrace => Scope::Editor,
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
    template: Vec<(KeyCombo, Action)>,
    editor: Vec<(KeyCombo, Action)>,
    /// Prefix keys for editor actions. Active, but not in the prefix mode's
    /// mode bar (they only work in the editor): the help overlay lists them.
    editor_prefix: Vec<(KeyCombo, Action)>,
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
                (plain(';'), Action::FinishStatement),
                (plain('?'), Action::Help),
                (plain('t'), Action::NewTab),
                (plain('T'), Action::CloseTab),
                (plain(']'), Action::NextTab),
                (plain('['), Action::PrevTab),
                (plain('1'), Action::GoToTab(1)),
                (plain('2'), Action::GoToTab(2)),
                (plain('3'), Action::GoToTab(3)),
                (plain('4'), Action::GoToTab(4)),
                (plain('5'), Action::GoToTab(5)),
                (plain('6'), Action::GoToTab(6)),
                (plain('7'), Action::GoToTab(7)),
                (plain('8'), Action::GoToTab(8)),
                (plain('9'), Action::GoToTab(9)),
            ],
            direct: vec![
                // `f5` first: it reaches the program in every terminal, so it is
                // the one the mode bar shows. Cmd is IntelliJ's macOS key, Ctrl
                // its Windows and Linux one.
                (key("f5"), Action::RunDirect),
                (key("cmd+enter"), Action::RunDirect),
                (key("ctrl+enter"), Action::RunDirect),
                // With nothing running and a selection, `ctrl+c` copies.
                (key("ctrl+c"), Action::CancelDirect),
                (key("ctrl+f2"), Action::CancelDirect),
                (key("cmd+f2"), Action::CancelDirect),
                (key("ctrl+space"), Action::CompleteOpen),
                // The intention actions: where the caret is on a select list's
                // `*`, it expands it; anywhere else it does nothing. It never runs.
                (key("alt+enter"), Action::ExpandStar),
                (key("f2"), Action::NextUnresolved),
                (key("shift+f2"), Action::PrevUnresolved),
                // Terminals differ in which of the two they can send.
                (key("cmd+shift+enter"), Action::FinishStatementDirect),
                (key("ctrl+shift+enter"), Action::FinishStatementDirect),
                // IntelliJ's tab keys. kitty keeps `cmd+w`, `cmd+shift+]`,
                // `cmd+shift+[`, `ctrl+tab` and `ctrl+shift+tab` for its own
                // tabs unless its config releases them; the prefix keys
                // always work. Not `alt+left`/`alt+right`: word movement.
                (key("ctrl+f4"), Action::CloseTabDirect),
                (key("cmd+w"), Action::CloseTabDirect),
                (key("cmd+shift+]"), Action::NextTabDirect),
                (key("ctrl+tab"), Action::NextTabDirect),
                (key("cmd+shift+["), Action::PrevTabDirect),
                (key("ctrl+shift+tab"), Action::PrevTabDirect),
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
                (key("o"), Action::OpenTable),
                (key("f4"), Action::OpenTable),
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
                (key("]"), Action::NextPage),
                (key("["), Action::PrevPage),
                (key("#"), Action::CountRows),
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
                // Where a template is highlighted these are read first.
                (key("tab"), Action::ExpandTemplate),
                (key("enter"), Action::ExpandTemplate),
            ],
            template: vec![
                (key("tab"), Action::TemplateNext),
                (key("shift+tab"), Action::TemplatePrev),
                (key("esc"), Action::TemplateDone),
            ],
            editor: {
                // (action, keys): IntelliJ's macOS key (Cmd), its Windows and
                // Linux key (Ctrl), then a fallback every terminal delivers.
                // Where a Windows/Linux key is something we own (`ctrl+g` is
                // the prefix, `ctrl+c` cancels) that binding is left out.
                let table: &[(Action, &[&str])] = &[
                    (Action::Duplicate, &["cmd+d", "ctrl+d"]),
                    (Action::DeleteLine, &["cmd+backspace", "ctrl+y"]),
                    (Action::MoveLineUp, &["alt+shift+up"]),
                    (Action::MoveLineDown, &["alt+shift+down"]),
                    (Action::MoveStatementUp, &["cmd+shift+up", "ctrl+shift+up"]),
                    (Action::MoveStatementDown, &["cmd+shift+down", "ctrl+shift+down"]),
                    (Action::LineComment, &["cmd+/", "ctrl+/"]),
                    (Action::BlockComment, &["cmd+alt+/", "ctrl+shift+/"]),
                    (Action::ExtendSelection, &["ctrl+w", "alt+up"]),
                    (Action::ShrinkSelection, &["ctrl+shift+w", "alt+down"]),
                    (Action::SelectAll, &["cmd+a", "ctrl+a"]),
                    (Action::Undo, &["cmd+z", "ctrl+z"]),
                    (Action::Redo, &["cmd+shift+z", "ctrl+shift+z"]),
                    (Action::Cut, &["cmd+x", "ctrl+x"]),
                    (Action::Copy, &["cmd+c"]),
                    (Action::Paste, &["cmd+v", "ctrl+v"]),
                    (Action::Find, &["cmd+f", "ctrl+f"]),
                    (Action::FindNext, &["cmd+g", "f3"]),
                    (Action::FindPrev, &["cmd+shift+g", "shift+f3"]),
                    (Action::GotoLine, &["cmd+l"]),
                    (Action::NewLineAfter, &["shift+enter"]),
                    (Action::NewLineBefore, &["cmd+alt+enter", "ctrl+alt+enter"]),
                    (Action::JoinLines, &["ctrl+shift+j"]),
                    (Action::ToggleCase, &["cmd+shift+u", "ctrl+shift+u"]),
                    (Action::Reformat, &["cmd+alt+l", "ctrl+alt+l"]),
                    (Action::Indent, &["tab"]),
                    (Action::Unindent, &["shift+tab"]),
                    (Action::WordLeft, &["alt+left", "ctrl+left"]),
                    (Action::WordRight, &["alt+right", "ctrl+right"]),
                    (Action::WordLeftSelect, &["alt+shift+left", "ctrl+shift+left"]),
                    (Action::WordRightSelect, &["alt+shift+right", "ctrl+shift+right"]),
                    (Action::DeleteWordBack, &["alt+backspace", "ctrl+backspace"]),
                    (Action::DeleteWordForward, &["alt+delete", "ctrl+delete"]),
                    (Action::LineStart, &["home", "cmd+left"]),
                    (Action::LineEnd, &["end", "cmd+right"]),
                    (Action::LineStartSelect, &["shift+home", "cmd+shift+left"]),
                    (Action::LineEndSelect, &["shift+end", "cmd+shift+right"]),
                    (Action::TextStart, &["cmd+up", "cmd+home", "ctrl+home"]),
                    (Action::TextEnd, &["cmd+down", "cmd+end", "ctrl+end"]),
                    (Action::TextStartSelect, &["cmd+shift+home", "ctrl+shift+home"]),
                    (Action::TextEndSelect, &["cmd+shift+end", "ctrl+shift+end"]),
                    (Action::MatchingBrace, &["ctrl+m", "ctrl+shift+m"]),
                ];
                table.iter().flat_map(|(a, keys)| keys.iter().map(move |k| (key(k), *a))).collect()
            },
            editor_prefix: [
                ('d', Action::Duplicate),
                ('x', Action::DeleteLine),
                ('/', Action::LineComment),
                ('*', Action::BlockComment),
                ('w', Action::ExtendSelection),
                ('W', Action::ShrinkSelection),
                ('a', Action::SelectAll),
                ('u', Action::Undo),
                ('U', Action::Redo),
                ('f', Action::Find),
                ('g', Action::GotoLine),
                ('J', Action::JoinLines),
                ('~', Action::ToggleCase),
                ('=', Action::Reformat),
                ('+', Action::ReformatAll),
                ('%', Action::MatchingBrace),
                ('o', Action::NewLineAfter),
                ('O', Action::NewLineBefore),
            ]
            .into_iter()
            .map(|(c, a)| (plain(c), a))
            .collect(),
        }
    }
}

impl Keybinds {
    /// The action bound to `key` after the prefix.
    pub fn lookup(&self, key: &KeyCombo) -> Option<Action> {
        self.bindings.iter().chain(&self.editor_prefix).find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The editing action the focused editor binds to `key`.
    pub fn lookup_editor(&self, key: &KeyCombo) -> Option<Action> {
        self.editor.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
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

    /// The action a live template being filled in binds to `key`.
    pub fn lookup_template(&self, key: &KeyCombo) -> Option<Action> {
        self.template.iter().find(|(k, _)| k == key).map(|(_, a)| *a)
    }

    /// The action the completion popup binds to `key`. While `template` (the
    /// popup has a live template highlighted) the expansion key is the
    /// template's, whatever else the key does.
    pub fn lookup_complete(&self, key: &KeyCombo, template: bool) -> Option<Action> {
        let own = |a: &Action| (*a == Action::ExpandTemplate) == template;
        self.complete.iter().find(|(k, a)| k == key && own(a)).or_else(|| self.complete.iter().find(|(k, a)| k == key && *a != Action::ExpandTemplate)).map(|(_, a)| *a)
    }

    /// Every key bound to `action` in its scope, as shown, in binding order.
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        self.table(action.scope()).iter().filter(|(_, a)| *a == action).map(|(k, _)| k.to_string()).collect()
    }

    fn table(&self, scope: Scope) -> &[(KeyCombo, Action)] {
        match scope {
            Scope::Results => &self.results,
            Scope::Prefix => &self.bindings,
            Scope::Direct => &self.direct,
            Scope::Sidebar => &self.sidebar,
            Scope::Resize => &self.resize,
            Scope::Complete => &self.complete,
            Scope::Template => &self.template,
            Scope::Editor => &self.editor,
        }
    }

    /// Every key that does `action`, in the order they are listed: its own
    /// table's, then the prefix's, spelled as typed (`ctrl+g d`).
    pub fn all_keys(&self, action: Action) -> Vec<String> {
        self.bindings_of(&[action]).into_iter().map(|b| b.text).collect()
    }

    /// The keys that do any of `actions`, each with whether a terminal without
    /// the keyboard protocol can deliver it: the actions' own tables first,
    /// then the prefix's (`ctrl+g d`).
    pub fn bindings_of(&self, actions: &[Action]) -> Vec<Binding> {
        let mut out = Vec::new();
        for action in actions {
            if action.scope() != Scope::Prefix {
                for (k, a) in self.table(action.scope()) {
                    if a == action {
                        out.push(Binding { text: k.to_string(), needs_protocol: k.needs_protocol() });
                    }
                }
            }
            for (k, a) in self.bindings.iter().chain(&self.editor_prefix) {
                if a == action {
                    out.push(Binding { text: format!("{} {k}", self.prefix), needs_protocol: self.prefix.needs_protocol() || k.needs_protocol() });
                }
            }
        }
        out
    }

    /// Every action with every key that does it, for the help overlay:
    /// sections of (what it does, its keys). Built from the live bindings, so
    /// a rebound key shows as it is.
    pub fn help(&self) -> Vec<HelpSection> {
        let row = |label: &'static str, actions: &[Action]| (label, self.bindings_of(actions));
        let run = vec![
            row("run", &[Action::RunDirect, Action::Run]),
            row("cancel", &[Action::CancelDirect, Action::Cancel]),
            row("complete statement", &[Action::FinishStatementDirect, Action::FinishStatement]),
            row("complete", &[Action::CompleteOpen]),
            row("expand *", &[Action::ExpandStar]),
            row("next error", &[Action::NextUnresolved]),
            row("previous error", &[Action::PrevUnresolved]),
        ];
        let edit: Vec<_> = EDIT_ACTIONS.iter().map(|a| row(a.label(), &[*a])).collect();
        let panes = vec![
            row("focus left", &[Action::FocusLeft]),
            row("focus down", &[Action::FocusDown]),
            row("focus up", &[Action::FocusUp]),
            row("focus right", &[Action::FocusRight]),
            row("zoom", &[Action::Zoom]),
            row("resize", &[Action::ResizeMode]),
            row("new connection", &[Action::NewConnection]),
            row("quit", &[Action::Quit]),
            row("help", &[Action::Help]),
        ];
        let tabs = vec![
            row("new tab", &[Action::NewTab]),
            row("close tab", &[Action::CloseTabDirect, Action::CloseTab]),
            row("next tab", &[Action::NextTabDirect, Action::NextTab]),
            row("previous tab", &[Action::PrevTabDirect, Action::PrevTab]),
            row("go to tab 1-9", &(1..=9).map(Action::GoToTab).collect::<Vec<_>>()),
        ];
        let browse = vec![
            row("open table (sidebar)", &[Action::OpenTable]),
            row("next page (results)", &[Action::NextPage]),
            row("previous page (results)", &[Action::PrevPage]),
            row("count rows (results)", &[Action::CountRows]),
        ];
        vec![
            HelpSection { title: "Run", rows: run },
            HelpSection { title: "Browse", rows: browse },
            HelpSection { title: "Edit", rows: edit },
            HelpSection { title: "Panes", rows: panes },
            HelpSection { title: "Tabs", rows: tabs },
        ]
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
            Scope::Template => &mut self.template,
            Scope::Editor => &mut self.editor,
        };
        // In the place of its first key, so its hint keeps its place in the bar.
        let first = table.iter().position(|(_, a)| *a == action);
        // The keys taken away before that place shift it left.
        let before = first.map_or(0, |at| table[..at].iter().filter(|(k, _)| *k == combo).count());
        table.retain(|(k, a)| *a != action && *k != combo);
        let at = first.map_or(table.len(), |at| (at - before).min(table.len()));
        table.insert(at, (combo, action));
        true
    }

    /// The hints for the mode bar after the prefix, from the live bindings:
    /// keys that do the same kind of thing share one (`h/j/k/l focus`).
    /// With `editor`, the editing keys that follow the prefix are listed too.
    pub fn prefix_hints(&self, editor: bool) -> Vec<Hint> {
        let mut out: Vec<Hint> = Vec::new();
        let editing: &[(KeyCombo, Action)] = if editor { &self.editor_prefix } else { &[] };
        for (key, action) in self.bindings.iter().chain(editing) {
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
    /// the live bindings. With a live template highlighted the keys that take
    /// it are the expansion's.
    pub fn complete_hints(&self, template: bool) -> Vec<Hint> {
        let one = |actions: &[Action], desc: &'static str| Hint {
            keys: actions.iter().filter_map(|a| self.key_for(*a).map(|k| (k, *a))).collect(),
            desc,
        };
        let expand = Hint { keys: self.keys_for(Action::ExpandTemplate).into_iter().map(|k| (k, Action::ExpandTemplate)).collect(), desc: "expand" };
        let mut out = vec![
            if template { expand } else { one(&[Action::CompleteInsert], "insert") },
            if template { Hint { keys: Vec::new(), desc: "replace" } } else { one(&[Action::CompleteReplace], "replace") },
            one(&[Action::CompleteDown, Action::CompleteUp], "move"),
            one(&[Action::CompletePageDown, Action::CompletePageUp], "page"),
            one(&[Action::CompleteClose], "close"),
        ];
        out.retain(|h| !h.keys.is_empty());
        out
    }

    /// The hints for the mode bar while a live template is being filled in.
    pub fn template_hints(&self) -> Vec<Hint> {
        let one = |action: Action, desc: &'static str| Hint { keys: self.key_for(action).map(|k| (k, action)).into_iter().collect(), desc };
        let mut out = vec![one(Action::TemplateNext, "next"), one(Action::TemplatePrev, "prev"), one(Action::TemplateDone, "done")];
        out.retain(|h| !h.keys.is_empty());
        out
    }

    /// `(keys, description)` pairs, as text.
    pub fn hints(&self) -> Vec<(String, &'static str)> {
        self.prefix_hints(false)
            .into_iter()
            .map(|h| (h.keys.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join("/"), h.desc))
            .collect()
    }
}

/// A titled run of rows in the help overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub rows: Vec<(&'static str, Vec<Binding>)>,
}

/// The editor's actions in the order the help lists them.
pub const EDIT_ACTIONS: [Action; 43] = [
    Action::Duplicate,
    Action::DeleteLine,
    Action::MoveLineUp,
    Action::MoveLineDown,
    Action::MoveStatementUp,
    Action::MoveStatementDown,
    Action::LineComment,
    Action::BlockComment,
    Action::ExtendSelection,
    Action::ShrinkSelection,
    Action::SelectAll,
    Action::Undo,
    Action::Redo,
    Action::Cut,
    Action::Copy,
    Action::Paste,
    Action::Find,
    Action::FindNext,
    Action::FindPrev,
    Action::GotoLine,
    Action::NewLineAfter,
    Action::NewLineBefore,
    Action::JoinLines,
    Action::ToggleCase,
    Action::Reformat,
    Action::ReformatAll,
    Action::Indent,
    Action::Unindent,
    Action::WordLeft,
    Action::WordRight,
    Action::WordLeftSelect,
    Action::WordRightSelect,
    Action::DeleteWordBack,
    Action::DeleteWordForward,
    Action::LineStart,
    Action::LineEnd,
    Action::LineStartSelect,
    Action::LineEndSelect,
    Action::TextStart,
    Action::TextEnd,
    Action::TextStartSelect,
    Action::TextEndSelect,
    Action::MatchingBrace,
];

/// One key that does an action, as the help overlay lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub text: String,
    /// A terminal without the keyboard protocol cannot deliver it.
    pub needs_protocol: bool,
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
    fn rebinding_onto_a_key_that_sat_earlier_keeps_the_hint_in_its_place() {
        let mut k = Keybinds::default();
        let order = |k: &Keybinds| k.bindings.iter().map(|(_, a)| *a).collect::<Vec<_>>();
        let mut want = order(&k);
        want.retain(|a| *a != Action::Run);
        assert!(k.rebind(Action::Quit, "r"));
        assert_eq!(order(&k), want, "Run lost its key to Quit; nothing else moved");
        assert_eq!(k.key_for(Action::Quit).as_deref(), Some("r"));
    }

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

    /// Every set of tables read for the same key press (design section 4's
    /// routing): the prefix key and the direct keys with each pane's own, the
    /// direct keys with the completion popup's (with and without a template
    /// highlighted) and with a template's, everything after the prefix, and
    /// the resize mode. A key bound to two actions in one set fails.
    #[test]
    fn no_key_does_two_things_in_any_set_of_scopes_read_together() {
        let k = Keybinds::default();
        let prefix = vec![(k.prefix, Action::OpenPrefix)];
        let popup = |template: bool| -> Vec<(KeyCombo, Action)> {
            let mut out: Vec<(KeyCombo, Action)> = Vec::new();
            for (key, _) in &k.complete {
                if !out.iter().any(|(o, _)| o == key) {
                    out.push((*key, k.lookup_complete(key, template).expect("bound")));
                }
            }
            out
        };
        let with_direct = |own: &[(KeyCombo, Action)]| [prefix.as_slice(), &k.direct, own].concat();
        let sets: Vec<(&str, Vec<(KeyCombo, Action)>)> = vec![
            ("sidebar", with_direct(&k.sidebar)),
            ("results", with_direct(&k.results)),
            ("editor", with_direct(&k.editor)),
            ("popup", with_direct(&popup(false))),
            ("popup on a template", with_direct(&popup(true))),
            ("template", with_direct(&k.template)),
            ("after the prefix", [k.bindings.as_slice(), &k.editor_prefix].concat()),
            ("resize", k.resize.clone()),
        ];
        for (name, set) in sets {
            for (i, (key, action)) in set.iter().enumerate() {
                for (other, also) in &set[i + 1..] {
                    assert!(!(key == other && action != also), "{name}: {key} is bound to both {action:?} and {also:?}");
                }
            }
        }
    }

    #[test]
    fn the_tab_keys_are_bound_where_the_slice_says() {
        let k = Keybinds::default();
        let prefix = |s: &str| k.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(prefix("t"), Some(Action::NewTab));
        assert_eq!(prefix("T"), Some(Action::CloseTab));
        assert_eq!(prefix("]"), Some(Action::NextTab));
        assert_eq!(prefix("["), Some(Action::PrevTab));
        for n in 1..=9u8 {
            assert_eq!(prefix(&n.to_string()), Some(Action::GoToTab(n)));
        }
        let direct = |s: &str| k.lookup_direct(&KeyCombo::parse(s).unwrap());
        for close in ["ctrl+f4", "cmd+w"] {
            assert_eq!(direct(close), Some(Action::CloseTabDirect), "{close}");
        }
        for next in ["cmd+shift+]", "ctrl+tab"] {
            assert_eq!(direct(next), Some(Action::NextTabDirect), "{next}");
        }
        for prev in ["cmd+shift+[", "ctrl+shift+tab"] {
            assert_eq!(direct(prev), Some(Action::PrevTabDirect), "{prev}");
        }
        assert!(KeyCombo::parse("ctrl+shift+tab").unwrap().needs_protocol() && KeyCombo::parse("ctrl+tab").unwrap().needs_protocol());
        assert_eq!(direct("alt+left"), None, "word movement in the editor");
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
