//! The connection form's state: fields, focus, validation and the draft it
//! builds. Pure (no Core, no drawing). `App` drives it; `view` draws it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::KeyCombo;
use super::text_field::{Edit, TextField};
use crate::core::{Connection, ConnectionDraft, ConnectionId, DbKind};

/// One row of the form, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Host,
    Port,
    User,
    Password,
    Database,
    Tls,
    Production,
}

impl Field {
    pub const ALL: [Field; 8] =
        [Field::Name, Field::Host, Field::Port, Field::User, Field::Password, Field::Database, Field::Tls, Field::Production];

    pub fn label(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::Host => "host",
            Field::Port => "port",
            Field::User => "user",
            Field::Password => "password",
            Field::Database => "database",
            Field::Tls => "tls",
            Field::Production => "production",
        }
    }

    /// Whether the row takes text (the others are checkboxes).
    pub fn is_text(self) -> bool {
        !matches!(self, Field::Tls | Field::Production)
    }
}

/// What the form shows below its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormStatus {
    Idle,
    Testing,
    TestOk,
    TestFailed(String),
    Saving,
    SaveFailed(String),
}

/// What a key did to the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKey {
    /// The form does not use this key.
    Ignored,
    /// Focus or the cursor moved; no value changed.
    Moved,
    /// A value changed.
    Edited,
}

/// The values the form started with, to tell whether anything was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    texts: [String; 5],
    tls: bool,
    production: bool,
}

#[derive(Debug, Clone)]
pub struct ConnectionForm {
    /// Tells this form's replies from an earlier form's.
    pub serial: u64,
    /// The connection being edited, if any.
    pub editing: Option<ConnectionId>,
    /// The kind of server. New connections are Postgres; an edit keeps the
    /// connection's own kind.
    pub kind: DbKind,
    pub name: TextField,
    pub host: TextField,
    pub port: TextField,
    pub user: TextField,
    /// Never loaded from the store: blank means "keep the stored password".
    pub password: TextField,
    pub database: TextField,
    pub tls: bool,
    pub production: bool,
    pub focus: Field,
    pub errors: Vec<(Field, String)>,
    pub status: FormStatus,
    /// The "discard changes?" question is up over the form.
    pub confirm_discard: bool,
    initial: Snapshot,
}

impl ConnectionForm {
    pub fn new_connection(serial: u64) -> Self {
        Self::build(serial, None, DbKind::Postgres, ["", "", "5432", "", ""], false, false)
    }

    /// A form for the connection as the sidebar knows it. The password field
    /// stays empty: the stored password is never loaded into the form.
    pub fn edit(serial: u64, c: &Connection) -> Self {
        let port = c.port.map(|p| p.to_string()).unwrap_or_default();
        let texts = [c.name.as_str(), c.host.as_str(), port.as_str(), c.username.as_str(), c.database.as_deref().unwrap_or("")];
        Self::build(serial, Some(c.id.clone()), c.kind.clone(), texts, c.tls, c.production)
    }

    fn build(serial: u64, editing: Option<ConnectionId>, kind: DbKind, t: [&str; 5], tls: bool, production: bool) -> Self {
        let mut form = ConnectionForm {
            serial,
            editing,
            kind,
            name: TextField::with_text(t[0]),
            host: TextField::with_text(t[1]),
            port: TextField::with_text(t[2]),
            user: TextField::with_text(t[3]),
            password: TextField::masked(),
            database: TextField::with_text(t[4]),
            tls,
            production,
            focus: Field::Name,
            errors: Vec::new(),
            status: FormStatus::Idle,
            confirm_discard: false,
            initial: Snapshot { texts: Default::default(), tls, production },
        };
        form.initial = form.snapshot();
        form
    }

    fn snapshot(&self) -> Snapshot {
        let t = |f: &TextField| f.text().to_string();
        Snapshot {
            texts: [t(&self.name), t(&self.host), t(&self.port), t(&self.user), t(&self.database)],
            tls: self.tls,
            production: self.production,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.editing.is_some() {
            "Edit connection"
        } else {
            "New connection"
        }
    }

    /// Whether leaving would lose something typed.
    pub fn dirty(&self) -> bool {
        !self.password.is_empty() || self.snapshot() != self.initial
    }

    pub fn text_field(&self, field: Field) -> Option<&TextField> {
        Some(match field {
            Field::Name => &self.name,
            Field::Host => &self.host,
            Field::Port => &self.port,
            Field::User => &self.user,
            Field::Password => &self.password,
            Field::Database => &self.database,
            Field::Tls | Field::Production => return None,
        })
    }

    pub fn text_field_mut(&mut self, field: Field) -> Option<&mut TextField> {
        Some(match field {
            Field::Name => &mut self.name,
            Field::Host => &mut self.host,
            Field::Port => &mut self.port,
            Field::User => &mut self.user,
            Field::Password => &mut self.password,
            Field::Database => &mut self.database,
            Field::Tls | Field::Production => return None,
        })
    }

    pub fn error(&self, field: Field) -> Option<&str> {
        self.errors.iter().find(|(f, _)| *f == field).map(|(_, m)| m.as_str())
    }

    /// A line to show under a field that is not an error: input was cut.
    pub fn note(&self, field: Field) -> Option<&'static str> {
        self.text_field(field).filter(|t| t.was_cut()).map(|_| "Input cut at 4096 characters.")
    }

    /// A save is in flight: its values are handed over, so the form takes no
    /// input until the Core answers. (Nothing typed can be lost, and nothing
    /// typed later can be discarded by the form closing on success.)
    pub fn locked(&self) -> bool {
        self.status == FormStatus::Saving
    }

    fn clear_error(&mut self, field: Field) {
        self.errors.retain(|(f, _)| *f != field);
    }

    pub fn toggle(&mut self, field: Field) {
        match field {
            Field::Tls => self.tls = !self.tls,
            Field::Production => self.production = !self.production,
            _ => {}
        }
    }

    fn move_focus(&mut self, step: isize) {
        let at = Field::ALL.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = Field::ALL[(at + step).rem_euclid(Field::ALL.len() as isize) as usize];
    }

    /// Applies a key to the focused row. Enter, esc and the buttons' keys are
    /// the caller's: they never reach here.
    pub fn on_key(&mut self, key: &KeyEvent) -> FormKey {
        if self.locked() {
            return FormKey::Ignored;
        }
        let combo = KeyCombo::from_event(key);
        match combo.code {
            KeyCode::BackTab => {
                self.move_focus(-1);
                return FormKey::Moved;
            }
            KeyCode::Tab if combo.mods.contains(KeyModifiers::SHIFT) => {
                self.move_focus(-1);
                return FormKey::Moved;
            }
            KeyCode::Tab | KeyCode::Down if combo.mods.is_empty() => {
                self.move_focus(1);
                return FormKey::Moved;
            }
            KeyCode::Up if combo.mods.is_empty() => {
                self.move_focus(-1);
                return FormKey::Moved;
            }
            KeyCode::Char(' ') if combo.mods.is_empty() && !self.focus.is_text() => {
                self.toggle(self.focus);
                return FormKey::Edited;
            }
            _ => {}
        }
        let focus = self.focus;
        let Some(field) = self.text_field_mut(focus) else { return FormKey::Ignored };
        match field.handle_key(key) {
            Edit::Ignored => FormKey::Ignored,
            Edit::Moved => FormKey::Moved,
            Edit::Changed => {
                self.clear_error(focus);
                FormKey::Edited
            }
        }
    }

    /// Pastes into the focused text row. False when the row takes no text.
    pub fn paste(&mut self, text: &str) -> bool {
        if self.locked() {
            return false;
        }
        let focus = self.focus;
        let Some(field) = self.text_field_mut(focus) else { return false };
        field.insert_str(text);
        self.clear_error(focus);
        true
    }

    /// A click on a row: focus it, and put the cursor at `column` of its input
    /// or toggle the checkbox.
    pub fn click(&mut self, field: Field, column: u16) -> FormKey {
        if self.locked() {
            return FormKey::Ignored;
        }
        self.focus = field;
        match self.text_field_mut(field) {
            Some(text) => {
                text.click(column);
                FormKey::Moved
            }
            None => {
                self.toggle(field);
                FormKey::Edited
            }
        }
    }

    /// The draft for a test (`for_save` false: a name is not needed) or a
    /// save. When something is wrong, records the message on that field,
    /// moves focus to the first such field and returns `None`.
    pub fn draft(&mut self, for_save: bool) -> Option<ConnectionDraft> {
        self.errors.clear();
        let name = self.name.text().trim().to_string();
        let host = self.host.text().trim().to_string();
        if for_save && name.is_empty() {
            self.errors.push((Field::Name, "Name is required.".into()));
        }
        if host.is_empty() {
            self.errors.push((Field::Host, "Host is required.".into()));
        } else if !host_is_plain(&host) {
            self.errors.push((
                Field::Host,
                "Host must be a name or an address, without a scheme, user, path or options.".into(),
            ));
        }
        let port = self.port.text().trim().parse::<u16>().ok().filter(|p| *p > 0);
        if port.is_none() {
            self.errors.push((Field::Port, "Port must be a number from 1 to 65535.".into()));
        }
        if let Some((field, _)) = self.errors.first() {
            self.focus = *field;
            return None;
        }
        let database = self.database.text().trim();
        Some(ConnectionDraft {
            id: self.editing.clone(),
            name,
            kind: self.kind.clone(),
            host,
            port,
            database: (!database.is_empty()).then(|| database.to_string()),
            username: self.user.text().trim().to_string(),
            password: (!self.password.is_empty()).then(|| self.password.text().to_string()),
            tls: self.tls,
            production: self.production,
        })
    }
}

/// Whether `host` is a bare host name or address (or a list of host:port
/// pairs, which Postgres allows). dbx puts the host into a `postgres://` URL
/// without escaping it, so a scheme, credentials, a path or options typed or
/// pasted here would be read as URL syntax: credentials would show in clear
/// and be stored unencrypted, and `?sslmode=disable` would override the TLS box.
fn host_is_plain(host: &str) -> bool {
    !host.contains("://")
        && !host.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\' | '?' | '#' | '%'))
}
