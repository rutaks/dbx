//! What the user can switch. Defaults now; the config file that sets them comes
//! with a later slice.

/// The client's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Accepting a table after `from` or `join` in the completion popup adds
    /// an alias made of the initials of its name (`film_actor fa`).
    pub alias_on_table: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { alias_on_table: true }
    }
}
