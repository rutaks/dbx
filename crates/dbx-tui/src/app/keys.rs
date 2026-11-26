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
                mods |= match m.to_ascii_lowercase().as_str() {
                    "ctrl" => KeyModifiers::CONTROL,
                    "alt" => KeyModifiers::ALT,
                    "shift" => KeyModifiers::SHIFT,
                    "cmd" | "super" => KeyModifiers::SUPER,
                    _ => return None,
                };
            }
        }
        // Names are read in any case; a single character keeps its own.
        let named = last.to_ascii_lowercase();
        let code = match named.as_str() {
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
            _ if last.chars().count() == 1 => KeyCode::Char(last.chars().next()?),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Dump the database or schema under the cursor to a `.sql` file; while
    /// a dump runs, show its progress.
    Dump,
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
    /// Write the result to a file (CSV, JSON, Excel, Markdown, SQL INSERTs),
    /// or copy it to the clipboard as text.
    Export,
    CopyAs,
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
    /// The history overlay (slice 18): after the prefix, and from a direct
    /// key (IntelliJ's console history, `ctrl+alt+e`).
    History,
    HistoryDirect,
    /// The navigator overlay (slice 21): go to a table or view by typing part
    /// of its name. After the prefix, and from a direct key (IntelliJ's go to
    /// class).
    Navigator,
    NavigatorDirect,
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
            Action::Dump => "dump",
            Action::GridLeft | Action::GridRight | Action::GridUp | Action::GridDown => "move",
            Action::GridPageUp | Action::GridPageDown => "page",
            Action::GridFirstCol => "first col",
            Action::GridLastCol => "last col",
            Action::GridFirstRow => "first row",
            Action::GridLastRow => "last row",
            Action::OpenCell => "view",
            Action::CopyCell => "copy",
            Action::CopyRow => "copy row",
            Action::Export => "export",
            Action::CopyAs => "copy as",
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
            Action::History | Action::HistoryDirect => "history",
            Action::Navigator | Action::NavigatorDirect => "go to",
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
            | Action::History
            | Action::Navigator
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
            | Action::HistoryDirect
            | Action::NavigatorDirect
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
            | Action::Export
            | Action::CopyAs
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
                (plain('e'), Action::History),
                (plain('p'), Action::Navigator),
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
                // IntelliJ's console history. `ctrl+alt+e` first: it reaches
                // the program in more terminals than Cmd does.
                (key("ctrl+alt+e"), Action::HistoryDirect),
                (key("cmd+alt+e"), Action::HistoryDirect),
                // IntelliJ's go to class (it finds tables here). Not
                // `ctrl+n`, its Windows and Linux key: the completion popup
                // and the editor read it for "down".
                (key("cmd+o"), Action::NavigatorDirect),
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
                (key("D"), Action::Dump),
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
                (key("e"), Action::Export),
                (key("C"), Action::CopyAs),
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
    fn table_ref(&self, t: Tbl) -> &Vec<(KeyCombo, Action)> {
        match t {
            Tbl::AfterPrefix => &self.bindings,
            Tbl::EditorPrefix => &self.editor_prefix,
            Tbl::Direct => &self.direct,
            Tbl::Sidebar => &self.sidebar,
            Tbl::Results => &self.results,
            Tbl::Resize => &self.resize,
            Tbl::Complete => &self.complete,
            Tbl::Template => &self.template,
            Tbl::Editor => &self.editor,
        }
    }

    fn table_mut(&mut self, t: Tbl) -> &mut Vec<(KeyCombo, Action)> {
        match t {
            Tbl::AfterPrefix => &mut self.bindings,
            Tbl::EditorPrefix => &mut self.editor_prefix,
            Tbl::Direct => &mut self.direct,
            Tbl::Sidebar => &mut self.sidebar,
            Tbl::Results => &mut self.results,
            Tbl::Resize => &mut self.resize,
            Tbl::Complete => &mut self.complete,
            Tbl::Template => &mut self.template,
            Tbl::Editor => &mut self.editor,
        }
    }

    /// Every key that does two things in a set of scopes read together, and
    /// the prefix key counted as doing `OpenPrefix` wherever it is read. None
    /// for the defaults (a test), and none after a config file is applied.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut out: Vec<Conflict> = Vec::new();
        for set in READ_TOGETHER {
            let mut entries: Vec<(KeyCombo, Action)> = Vec::new();
            if set.prefix_read {
                entries.push((self.prefix, Action::OpenPrefix));
            }
            for t in set.tables {
                entries.extend(self.table_ref(*t).iter().copied());
            }
            for (i, (key, first)) in entries.iter().enumerate() {
                for (other, second) in &entries[i + 1..] {
                    if key == other && first != second && !template_pair(*first, *second) {
                        let conflict = Conflict { key: *key, first: *first, second: *second };
                        if !out.contains(&conflict) {
                            out.push(conflict);
                        }
                    }
                }
            }
        }
        out
    }

    /// Replaces the keys of `action` with `chords`, in the place of its first
    /// key so its hint keeps its place in the bar. A key another action has in
    /// a scope read at the same time is taken from it (and returned), unless
    /// that action is one of `claimed` (set earlier from the config file), the
    /// key is the prefix, or it is a plain character in a place that takes
    /// text: then nothing changes and the reason is returned.
    pub fn set_keys(&mut self, action: Action, chords: &[Chord], claimed: &[Action]) -> Result<Vec<Taken>, Refused> {
        let mut placed: Vec<(Tbl, KeyCombo)> = Vec::new();
        for chord in chords {
            let (table, key) = match (action.scope(), chord) {
                (Scope::Prefix, Chord::Direct(k) | Chord::AfterPrefix(k)) => (Tbl::AfterPrefix, *k),
                (Scope::Editor, Chord::AfterPrefix(k)) => (Tbl::EditorPrefix, *k),
                (_, Chord::AfterPrefix(_)) => return Err(Refused::NoPrefixKeys),
                (scope, Chord::Direct(k)) => (Tbl::of(scope), *k),
            };
            // The popups and the editor's own table read enter, tab and the
            // arrows on purpose (the editor's defaults do); the direct keys
            // are read before the text, so they must leave those to it.
            let edits = table == Tbl::Direct;
            if table.reads_text() && (if edits { is_typing(&key) } else { is_typing_char(&key) }) {
                return Err(Refused::Typing(key));
            }
            if !placed.contains(&(table, key)) {
                placed.push((table, key));
            }
        }
        let mut next = self.clone();
        let own: &[Tbl] = match action.scope() {
            Scope::Editor => &[Tbl::Editor, Tbl::EditorPrefix],
            scope => &[Tbl::of(scope)],
        };
        for &t in own {
            let table = next.table_mut(t);
            let at = table.iter().position(|(_, a)| *a == action);
            table.retain(|(_, a)| *a != action);
            let at = at.unwrap_or(table.len()).min(table.len());
            let new: Vec<(KeyCombo, Action)> = placed.iter().filter(|(pt, _)| *pt == t).map(|(_, key)| (*key, action)).collect();
            table.splice(at..at, new);
        }
        let mut taken: Vec<(Tbl, Taken)> = Vec::new();
        for &(t, key) in &placed {
            for set in READ_TOGETHER.iter().filter(|s| s.tables.contains(&t)) {
                if set.prefix_read && key == next.prefix {
                    return Err(Refused::IsPrefix(key));
                }
                for &u in set.tables {
                    for (k, other) in next.table_ref(u) {
                        if *k != key || *other == action || (t == Tbl::Complete && u == Tbl::Complete && template_pair(action, *other)) {
                            continue;
                        }
                        if claimed.contains(other) {
                            return Err(Refused::Claimed { key, by: *other });
                        }
                        let found = (u, Taken { key, from: *other });
                        if !taken.contains(&found) {
                            taken.push(found);
                        }
                    }
                }
            }
        }
        for (u, t) in &taken {
            next.table_mut(*u).retain(|(k, a)| !(*k == t.key && *a == t.from));
        }
        *self = next;
        Ok(taken.into_iter().map(|(_, t)| t).collect())
    }

    /// Makes `key` the prefix. Any action that had it in a scope where the
    /// prefix is read loses it (the user's key wins), and is returned.
    pub fn set_prefix(&mut self, key: KeyCombo) -> Result<Vec<Taken>, Refused> {
        // Only a function key (or one with ctrl, alt or cmd) can be the prefix:
        // esc, enter, tab and the like do their own work.
        if is_typing(&key) || (key.mods.difference(KeyModifiers::SHIFT).is_empty() && !matches!(key.code, KeyCode::F(_))) {
            return Err(Refused::Typing(key));
        }
        let mut taken: Vec<(Tbl, Taken)> = Vec::new();
        for set in READ_TOGETHER.iter().filter(|s| s.prefix_read) {
            for &u in set.tables {
                for (k, a) in self.table_ref(u) {
                    let found = (u, Taken { key, from: *a });
                    if *k == key && !taken.contains(&found) {
                        taken.push(found);
                    }
                }
            }
        }
        for (u, t) in &taken {
            self.table_mut(*u).retain(|(k, a)| !(*k == t.key && *a == t.from));
        }
        self.prefix = key;
        Ok(taken.into_iter().map(|(_, t)| t).collect())
    }

    /// Every action's keys as the config file spells them, scope by scope, each
    /// action once in the order of its first key: `(scope, action, keys)`.
    /// Keys after the prefix are written `prefix+x`.
    pub fn entries(&self) -> Vec<(Scope, Action, Vec<String>)> {
        let mut out = Vec::new();
        for scope in Scope::ALL {
            let tables: &[Tbl] = match scope {
                Scope::Editor => &[Tbl::Editor, Tbl::EditorPrefix],
                other => &[Tbl::of(other)],
            };
            let mut actions: Vec<Action> = Vec::new();
            for &t in tables {
                for (_, a) in self.table_ref(t) {
                    if !actions.contains(a) {
                        actions.push(*a);
                    }
                }
            }
            for action in actions {
                let keys = tables
                    .iter()
                    .flat_map(|&t| {
                        let after = matches!(t, Tbl::EditorPrefix | Tbl::AfterPrefix);
                        self.table_ref(t).iter().filter(move |(_, a)| *a == action).map(move |(k, _)| if after { format!("prefix+{k}") } else { k.to_string() })
                    })
                    .collect();
                out.push((scope, action, keys));
            }
        }
        out
    }

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
        use Action as A;
        let row = |label: &'static str, actions: &[Action]| {
            let mut bindings = self.bindings_of(actions);
            // Nine keys that differ in one character are one row's worth.
            if actions.len() == 9 && matches!(actions[0], A::GoToTab(_)) {
                bindings = vec![Binding { text: format!("{} 1-9", self.prefix), needs_protocol: self.prefix.needs_protocol() }];
            }
            HelpRow { label, name: help_name(actions), actions: actions.to_vec(), bindings }
        };
        // Keys the overlays read themselves, which no binding table holds.
        let fixed = |label: &'static str, keys: &[&str]| HelpRow {
            label,
            name: String::new(),
            actions: Vec::new(),
            bindings: keys.iter().map(|k| Binding { text: (*k).to_string(), needs_protocol: KeyCombo::parse(k).is_some_and(|c| c.needs_protocol()) }).collect(),
        };
        let run_key = self.key_for(A::RunDirect).unwrap_or_else(|| "f5".into());
        let run = vec![
            row("run", &[A::RunDirect, A::Run]),
            row("cancel", &[A::CancelDirect, A::Cancel]),
            row("complete statement", &[A::FinishStatementDirect, A::FinishStatement]),
            row("complete", &[A::CompleteOpen]),
            row("history", &[A::HistoryDirect, A::History]),
            row("go to table or view", &[A::NavigatorDirect, A::Navigator]),
            row("expand *", &[A::ExpandStar]),
            row("next error", &[A::NextUnresolved]),
            row("previous error", &[A::PrevUnresolved]),
        ];
        let edit: Vec<_> = EDIT_ACTIONS.iter().map(|a| row(a.label(), &[*a])).collect();
        let panes = vec![
            row("focus left", &[A::FocusLeft]),
            row("focus down", &[A::FocusDown]),
            row("focus up", &[A::FocusUp]),
            row("focus right", &[A::FocusRight]),
            row("zoom", &[A::Zoom]),
            row("resize", &[A::ResizeMode]),
            row("new connection", &[A::NewConnection]),
            row("quit", &[A::Quit]),
            row("help", &[A::Help]),
        ];
        let tabs = vec![
            row("new tab", &[A::NewTab]),
            row("close tab", &[A::CloseTabDirect, A::CloseTab]),
            row("next tab", &[A::NextTabDirect, A::NextTab]),
            row("previous tab", &[A::PrevTabDirect, A::PrevTab]),
            row("go to tab 1-9", &(1..=9).map(A::GoToTab).collect::<Vec<_>>()),
        ];
        let sidebar = vec![
            row("move down", &[A::MoveDown]),
            row("move up", &[A::MoveUp]),
            row("open node", &[A::Expand]),
            row("close node, or parent", &[A::Collapse]),
            row("page down", &[A::PageDown]),
            row("page up", &[A::PageUp]),
            row("top", &[A::First]),
            row("bottom", &[A::Last]),
            row("reload node", &[A::Refresh]),
            row("edit connection", &[A::EditConnection]),
            row("remove connection", &[A::RemoveConnection]),
            row("use for the tab", &[A::UseTarget]),
            row("open table", &[A::OpenTable]),
            row("dump database or schema", &[A::Dump]),
        ];
        let results = vec![
            row("move left", &[A::GridLeft]),
            row("move right", &[A::GridRight]),
            row("move up", &[A::GridUp]),
            row("move down", &[A::GridDown]),
            row("page up", &[A::GridPageUp]),
            row("page down", &[A::GridPageDown]),
            row("first column", &[A::GridFirstCol]),
            row("last column", &[A::GridLastCol]),
            row("first row", &[A::GridFirstRow]),
            row("last row", &[A::GridLastRow]),
            row("view cell", &[A::OpenCell]),
            row("copy cell", &[A::CopyCell]),
            row("copy row", &[A::CopyRow]),
            row("export to a file", &[A::Export]),
            row("copy as", &[A::CopyAs]),
            row("load more rows", &[A::LoadMore]),
            row("next page", &[A::NextPage]),
            row("previous page", &[A::PrevPage]),
            row("count rows", &[A::CountRows]),
            row("raw or pretty (viewer)", &[A::ToggleRaw]),
        ];
        let resize = vec![
            row("sidebar narrower", &[A::ResizeLeft]),
            row("sidebar wider", &[A::ResizeRight]),
            row("split up", &[A::ResizeUp]),
            row("split down", &[A::ResizeDown]),
            row("leave resize mode", &[A::ResizeDone]),
        ];
        let completion = vec![
            row("insert", &[A::CompleteInsert]),
            row("replace the word", &[A::CompleteReplace]),
            row("expand template", &[A::ExpandTemplate]),
            row("move down", &[A::CompleteDown]),
            row("move up", &[A::CompleteUp]),
            row("page down", &[A::CompletePageDown]),
            row("page up", &[A::CompletePageUp]),
            row("close", &[A::CompleteClose]),
        ];
        let template = vec![
            row("next variable", &[A::TemplateNext]),
            row("previous variable", &[A::TemplatePrev]),
            row("leave template", &[A::TemplateDone]),
        ];
        // `esc`, and the key that opened it, which closes it too.
        let mut navigator_close = fixed("close", &["esc"]);
        navigator_close.bindings.extend(self.bindings_of(&[A::NavigatorDirect]));
        let navigator = vec![
            fixed("move", &["down", "up", "ctrl+n", "ctrl+p", "pagedown", "pageup"]),
            fixed("reveal in the sidebar", &["enter"]),
            fixed("open its rows", &["ctrl+o"]),
            navigator_close,
        ];
        let history = vec![
            fixed("move", &["down", "up", "ctrl+n", "ctrl+p", "pagedown", "pageup"]),
            fixed("put in the editor", &["enter"]),
            fixed("run again", &[run_key.as_str()]),
            fixed("this connection or all", &["tab"]),
            fixed("scroll the statement", &["shift+down", "shift+up"]),
            fixed("close", &["esc"]),
        ];
        let export = vec![
            fixed("next or previous row", &["tab", "shift+tab", "down", "up"]),
            fixed("pick a format", &["h", "l", "left", "right", "1-5"]),
            fixed("empty or NULL (CSV)", &["space"]),
            fixed("write the file", &["enter"]),
            fixed("replace the file", &["y"]),
            fixed("cancel", &["esc"]),
        ];
        let dump = vec![
            fixed("next or previous row", &["tab", "shift+tab", "down", "up"]),
            fixed("pick content or scope", &["h", "l", "left", "right", "1-3"]),
            fixed("toggle scope or drop", &["space"]),
            fixed("start the dump", &["enter"]),
            fixed("replace the file", &["y"]),
            fixed("cancel the dump", &["c"]),
            fixed("close", &["esc"]),
        ];
        let confirm = vec![
            fixed("confirm a run or quit", &["y"]),
            fixed("confirm a removal", &["enter"]),
            fixed("decline", &["esc"]),
        ];
        let section = |title: &'static str, rows: Vec<HelpRow>| HelpSection { title, rows };
        vec![
            section("Run", run),
            section("Edit", edit),
            section("Panes", panes),
            section("Tabs", tabs),
            section("Sidebar", sidebar),
            section("Results", results),
            section("Resize mode (prefix R)", resize),
            section("Completion popup", completion),
            section("Template mode", template),
            section("Go to overlay", navigator),
            section("History overlay", history),
            section("Export overlay", export),
            section("Dump overlay", dump),
            section("Confirmations", confirm),
        ]
    }

    /// Every action that any key is bound to, in any scope (the prefix's
    /// editing keys too): what the help must list.
    pub fn bound_actions(&self) -> Vec<Action> {
        let mut out: Vec<Action> = Vec::new();
        for (_, a) in self
            .bindings
            .iter()
            .chain(&self.editor_prefix)
            .chain(&self.direct)
            .chain(&self.sidebar)
            .chain(&self.results)
            .chain(&self.resize)
            .chain(&self.complete)
            .chain(&self.template)
            .chain(&self.editor)
        {
            if !out.contains(a) {
                out.push(*a);
            }
        }
        out
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

/// A key that does two things in scopes read at the same time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: KeyCombo,
    pub first: Action,
    pub second: Action,
}

/// A key as the config file gives it: read directly in the action's place, or
/// after the prefix (`prefix+d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    Direct(KeyCombo),
    AfterPrefix(KeyCombo),
}

impl Chord {
    /// `"ctrl+d"`, or `"prefix+d"` for a key after the prefix.
    pub fn parse(s: &str) -> Option<Self> {
        match s.strip_prefix("prefix+") {
            Some(rest) => KeyCombo::parse(rest).map(Chord::AfterPrefix),
            None => KeyCombo::parse(s).map(Chord::Direct),
        }
    }
}

/// Why a set of keys was not given to an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The action is not one a key after the prefix does.
    NoPrefixKeys,
    /// A plain character where the key would be read before the text sees it.
    Typing(KeyCombo),
    /// The key is the prefix.
    IsPrefix(KeyCombo),
    /// The key is an action the config file already gave it to.
    Claimed { key: KeyCombo, by: Action },
}

/// A key an action lost because the user's binding won it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taken {
    pub key: KeyCombo,
    pub from: Action,
}

/// The tables of keys, by where they are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tbl {
    AfterPrefix,
    EditorPrefix,
    Direct,
    Sidebar,
    Results,
    Resize,
    Complete,
    Template,
    Editor,
}

impl Tbl {
    fn of(scope: Scope) -> Tbl {
        match scope {
            Scope::Prefix => Tbl::AfterPrefix,
            Scope::Direct => Tbl::Direct,
            Scope::Sidebar => Tbl::Sidebar,
            Scope::Results => Tbl::Results,
            Scope::Resize => Tbl::Resize,
            Scope::Complete => Tbl::Complete,
            Scope::Template => Tbl::Template,
            Scope::Editor => Tbl::Editor,
        }
    }

    /// Whether a key here is read before the text it may be part of: a plain
    /// character bound here could not be typed.
    fn reads_text(self) -> bool {
        matches!(self, Tbl::Direct | Tbl::Editor | Tbl::Complete | Tbl::Template)
    }
}

/// A set of tables read for the same key press (design section 4's routing),
/// and whether the prefix key is read first. A key bound twice within a set
/// does one of two things, so it is a conflict.
struct ReadTogether {
    tables: &'static [Tbl],
    prefix_read: bool,
}

const READ_TOGETHER: &[ReadTogether] = &[
    ReadTogether { tables: &[Tbl::Direct, Tbl::Sidebar], prefix_read: true },
    ReadTogether { tables: &[Tbl::Direct, Tbl::Results], prefix_read: true },
    ReadTogether { tables: &[Tbl::Direct, Tbl::Editor], prefix_read: true },
    // The popup is read before the editor, so the two do not meet.
    ReadTogether { tables: &[Tbl::Direct, Tbl::Complete], prefix_read: true },
    ReadTogether { tables: &[Tbl::Direct, Tbl::Template], prefix_read: true },
    ReadTogether { tables: &[Tbl::AfterPrefix, Tbl::EditorPrefix], prefix_read: false },
    ReadTogether { tables: &[Tbl::Resize], prefix_read: false },
];

/// Expanding a template and the popup's own keys share keys on purpose: the
/// expansion is read first, and only while a template is highlighted.
fn template_pair(a: Action, b: Action) -> bool {
    (a == Action::ExpandTemplate || b == Action::ExpandTemplate) && a.scope() == Scope::Complete && b.scope() == Scope::Complete
}

/// A character with no modifier but shift: typed text.
fn is_typing_char(key: &KeyCombo) -> bool {
    matches!(key.code, KeyCode::Char(_)) && key.mods.difference(KeyModifiers::SHIFT).is_empty()
}

/// A key the text is typed or edited with: a character, or enter, tab, the
/// arrows, home, end, backspace, delete and page keys, with no modifier but
/// shift.
fn is_typing(key: &KeyCombo) -> bool {
    let bare = key.mods.difference(KeyModifiers::SHIFT).is_empty();
    match key.code {
        KeyCode::Char(_) => bare,
        // The keys the text moves and deletes by, which the editor and the
        // bars read as they are (with shift, to select).
        KeyCode::Enter
        | KeyCode::Tab
        | KeyCode::BackTab
        | KeyCode::Backspace
        | KeyCode::Delete
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown => bare,
        _ => false,
    }
}

impl Scope {
    /// Every scope, in the order the config file lists them.
    pub const ALL: [Scope; 8] =
        [Scope::Prefix, Scope::Direct, Scope::Sidebar, Scope::Results, Scope::Resize, Scope::Complete, Scope::Template, Scope::Editor];

    pub fn from_name(name: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// A titled run of rows in the help overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub rows: Vec<HelpRow>,
}

/// One row of the help: what it does, the name the config file gives it, and
/// every key that does it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    pub label: &'static str,
    /// `scope.action` (`editor.duplicate`, `after_prefix/direct.run`): what
    /// slice 22 reads in the config file. Empty for keys an overlay reads itself,
    /// which no binding table holds and the config does not rebind.
    pub name: String,
    /// The actions the row stands for; none for an overlay's own keys.
    pub actions: Vec<Action>,
    pub bindings: Vec<Binding>,
}

impl HelpRow {
    /// Whether the row answers a search: every word of `search` is in the
    /// label, the name or one of the keys, without regard to case.
    pub fn matches(&self, search: &str) -> bool {
        let hay = self.haystack();
        search.to_lowercase().split_whitespace().all(|word| hay.iter().any(|h| h.contains(word)))
    }

    /// Whether the whole of `search`, spaces and all, is one of the row's keys
    /// (`ctrl+g t`). Case counts here, as it does to the keys: `ctrl+g T` is
    /// another key.
    pub fn matches_whole(&self, search: &str) -> bool {
        let whole = search.split_whitespace().collect::<Vec<_>>().join(" ");
        self.bindings.iter().any(|b| b.text == whole)
    }

    fn haystack(&self) -> Vec<String> {
        let unbound = (self.bindings.is_empty() && !self.actions.is_empty()).then(|| "unbound".to_string());
        [self.label.to_lowercase(), self.name.to_lowercase()].into_iter().chain(self.bindings.iter().map(|b| b.text.to_lowercase())).chain(unbound).collect()
    }
}

impl Scope {
    /// The section of the config file the scope's keys are in.
    pub fn name(self) -> &'static str {
        match self {
            // Not `prefix`: `[keys] prefix = "ctrl+g"` is the prefix key itself,
            // and TOML cannot have a value and a table of one name.
            Scope::Prefix => "after_prefix",
            Scope::Direct => "direct",
            Scope::Sidebar => "sidebar",
            Scope::Results => "results",
            Scope::Resize => "resize",
            Scope::Complete => "complete",
            Scope::Template => "template",
            Scope::Editor => "editor",
        }
    }
}

impl Action {
    /// The action's name without its scope, in snake case. A direct key's
    /// action shares the name of the prefix key's (`run` for `RunDirect` and
    /// `Run`): the scope tells them apart.
    pub fn config_name(self) -> String {
        let camel = match self {
            Action::GoToTab(n) => format!("GoToTab{n}"),
            other => format!("{other:?}"),
        };
        let mut out = String::new();
        let mut previous: Option<char> = None;
        for c in camel.chars() {
            let split = (c.is_uppercase() && previous.is_some()) || (c.is_ascii_digit() && previous.is_some_and(|p| !p.is_ascii_digit()));
            if split {
                out.push('_');
            }
            out.extend(c.to_lowercase());
            previous = Some(c);
        }
        match out.strip_suffix("_direct") {
            Some(base) => base.to_string(),
            None => out,
        }
    }

    /// `scope.name`, as the config file addresses the action.
    pub fn qualified_name(self) -> String {
        format!("{}.{}", self.scope().name(), self.config_name())
    }
}

/// The names of a row's actions: one `scope.name`, or `scope/scope.name` when
/// the actions are the same thing in several scopes (`after_prefix/direct.run`), or
/// the qualified names side by side when they differ.
fn help_name(actions: &[Action]) -> String {
    let Some(first) = actions.first() else { return String::new() };
    // Nine tab keys are one name.
    if actions.len() == 9 && matches!(first, Action::GoToTab(_)) {
        return "after_prefix.go_to_tab_N".to_string();
    }
    let base = first.config_name();
    if actions.iter().all(|a| a.config_name() == base) {
        let mut scopes: Vec<&str> = actions.iter().map(|a| a.scope().name()).collect();
        scopes.sort();
        scopes.dedup();
        format!("{}.{base}", scopes.join("/"))
    } else {
        actions.iter().map(|a| a.qualified_name()).collect::<Vec<_>>().join(", ")
    }
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
    fn conflicts_finds_a_key_that_does_two_things_where_scopes_are_read_together_and_the_prefix_too() {
        let key = |s: &str| KeyCombo::parse(s).unwrap();
        let mut k = Keybinds::default();
        assert!(k.conflicts().is_empty());
        // `f5` is a direct key, read while the sidebar has focus.
        k.sidebar.push((key("f5"), Action::Refresh));
        assert_eq!(k.conflicts(), [Conflict { key: key("f5"), first: Action::RunDirect, second: Action::Refresh }]);
        // The same key in two scopes never read together is no conflict: `r`
        // is the sidebar's refresh and the results pane's raw toggle, and the
        // popup's `ctrl+d` does not meet the editor's.
        let mut k = Keybinds::default();
        k.complete.push((key("ctrl+d"), Action::CompleteClose));
        assert!(k.conflicts().is_empty());
        // The prefix key counts.
        let mut k = Keybinds::default();
        k.direct.push((k.prefix, Action::HistoryDirect));
        assert_eq!(k.conflicts()[0].first, Action::OpenPrefix);
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
