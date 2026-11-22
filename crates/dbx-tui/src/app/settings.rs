//! What the user can switch. Defaults now; the config file that sets them comes
//! with a later slice.

/// What running does when the caret is inside a subquery (IntelliJ's "when
/// caret inside statement, execute").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAtCaret {
    /// Open the statement chooser (the default, as in IntelliJ).
    Ask,
    /// Run the innermost subquery the caret is in.
    Smallest,
    /// Run the whole statement, as before the chooser existed.
    Statement,
}

/// The client's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Accepting a table after `from` or `join` in the completion popup adds
    /// an alias made of the initials of its name (`film_actor fa`).
    pub alias_on_table: bool,
    /// Smart keys, each as IntelliJ's smart-keys page has it, on by default.
    /// Typing `(`, `[`, `'` or `"` inserts the pair; typing the closer moves
    /// over it; `backspace` between an empty pair deletes both; `tab` moves
    /// past a closer; an opener typed over a selection surrounds it.
    pub auto_pair: bool,
    /// `enter` keeps the line's indentation and opens a block between `(` and
    /// `)`.
    pub smart_enter: bool,
    /// `home` toggles between the first non-blank character and column 0.
    pub smart_home: bool,
    /// What running does with the caret inside a subquery.
    pub run_at_caret: RunAtCaret,
    /// `tab` inserts a tab character instead of spaces.
    pub hard_tabs: bool,
    /// The width of an indent step, in columns.
    pub tab_width: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            alias_on_table: true,
            auto_pair: true,
            smart_enter: true,
            smart_home: true,
            run_at_caret: RunAtCaret::Ask,
            hard_tabs: false,
            tab_width: 4,
        }
    }
}
