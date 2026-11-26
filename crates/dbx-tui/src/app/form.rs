//! The connection form's state: fields, focus, validation and the draft it
//! builds. Pure (no Core, no drawing). `App` drives it; `view` draws it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::KeyCombo;
use super::text_field::{Edit, TextField};
use crate::core::{Connection, ConnectionDraft, ConnectionId, DbKind};

/// One row of the form, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The type of database: one of the form's choices.
    Kind,
    Name,
    /// The server's host, or for a file database the path of the file.
    Host,
    Port,
    User,
    Password,
    Database,
    Tls,
    Production,
}

/// The rows of a form for a file database: the type, a name, the file, and
/// the production flag. No host, port, user, password or TLS.
const FILE_FIELDS: [Field; 4] = [Field::Kind, Field::Name, Field::Host, Field::Production];

impl Field {
    pub const ALL: [Field; 9] =
        [Field::Kind, Field::Name, Field::Host, Field::Port, Field::User, Field::Password, Field::Database, Field::Tls, Field::Production];

    pub fn label(self) -> &'static str {
        match self {
            Field::Kind => "type",
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

    /// Whether the row takes text (the others are a choice or checkboxes).
    pub fn is_text(self) -> bool {
        !matches!(self, Field::Kind | Field::Tls | Field::Production)
    }
}

/// One type of connection the form offers, with what the form needs to know
/// of it (from its `Capabilities`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindChoice {
    pub kind: DbKind,
    /// A file database: the form asks for a name and a file.
    pub local_file: bool,
    pub default_port: Option<u16>,
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
    kind: DbKind,
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
    /// The kind of server. New connections start as Postgres and the type row
    /// changes it among `choices`; an edit keeps the connection's own kind.
    pub kind: DbKind,
    /// What the type row offers, in order. An edit offers only its own kind.
    pub choices: Vec<KindChoice>,
    /// The kind in force is a file database: the rows are `FILE_FIELDS`.
    pub file: bool,
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
    /// What was typed for another type, kept for when it is chosen again.
    put_aside: Vec<(DbKind, KindFields)>,
}

/// The rows that belong to one type of connection (`cycle_kind`).
#[derive(Debug, Clone)]
struct KindFields {
    host: TextField,
    port: TextField,
    user: TextField,
    password: TextField,
    database: TextField,
    tls: bool,
}

impl ConnectionForm {
    /// A form for a new connection of `kind`, its port starting at the type's
    /// default (`Capabilities::default_port`).
    pub fn new_connection(serial: u64, kind: DbKind, default_port: Option<u16>) -> Self {
        let port = default_port.map(|p| p.to_string()).unwrap_or_default();
        let choice = KindChoice { kind: kind.clone(), local_file: false, default_port };
        Self::build(serial, None, kind, vec![choice], false, ["", "", port.as_str(), "", ""], false, false)
    }

    /// The types the type row offers (the first is the kind the form is for).
    /// A choice that is the form's kind sets whether it is a file database.
    pub fn with_choices(mut self, choices: Vec<KindChoice>) -> Self {
        if let Some(own) = choices.iter().find(|c| c.kind == self.kind) {
            self.file = own.local_file;
        }
        self.choices = choices;
        self.initial = self.snapshot();
        self
    }

    /// A form for the connection as the sidebar knows it. The password field
    /// stays empty: the stored password is never loaded into the form. It
    /// offers only the connection's own type.
    pub fn edit(serial: u64, c: &Connection) -> Self {
        let port = c.port.map(|p| p.to_string()).unwrap_or_default();
        let texts = [c.name.as_str(), c.host.as_str(), port.as_str(), c.username.as_str(), c.database.as_deref().unwrap_or("")];
        let choice = KindChoice { kind: c.kind.clone(), local_file: c.capabilities.local_file, default_port: c.capabilities.default_port };
        Self::build(serial, Some(c.id.clone()), c.kind.clone(), vec![choice], c.capabilities.local_file, texts, c.tls, c.production)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(serial: u64, editing: Option<ConnectionId>, kind: DbKind, choices: Vec<KindChoice>, file: bool, t: [&str; 5], tls: bool, production: bool) -> Self {
        let mut form = ConnectionForm {
            serial,
            editing,
            kind,
            choices,
            file,
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
            initial: Snapshot { kind: Default::default(), texts: Default::default(), tls, production },
            put_aside: Vec::new(),
        };
        form.initial = form.snapshot();
        form
    }

    /// The rows the form shows now, in tab order.
    pub fn fields(&self) -> &'static [Field] {
        if self.file {
            &FILE_FIELDS
        } else {
            &Field::ALL
        }
    }

    /// What a row is called: the host row of a file database is the file.
    pub fn label(&self, field: Field) -> &'static str {
        if field == Field::Host && self.file {
            "file"
        } else {
            field.label()
        }
    }

    /// The type row can change the type: more than one is offered, and this
    /// is a new connection (an edit keeps its type).
    pub fn can_change_kind(&self) -> bool {
        self.editing.is_none() && self.choices.len() > 1
    }

    /// The next (or previous) type. What the old type's rows held (host or
    /// file, port, user, password, database, TLS) is put aside for that type
    /// and comes back when it is chosen again; a type not chosen before starts
    /// empty, its port at the type's default. The name and production flag
    /// are the connection's, and stay.
    pub fn cycle_kind(&mut self, step: isize) -> bool {
        if !self.can_change_kind() {
            return false;
        }
        let at = self.choices.iter().position(|c| c.kind == self.kind).unwrap_or(0) as isize;
        let next = self.choices[(at + step).rem_euclid(self.choices.len() as isize) as usize].clone();
        let kept = KindFields {
            host: std::mem::take(&mut self.host),
            port: std::mem::take(&mut self.port),
            user: std::mem::take(&mut self.user),
            password: std::mem::replace(&mut self.password, TextField::masked()),
            database: std::mem::take(&mut self.database),
            tls: self.tls,
        };
        self.put_aside.retain(|(kind, _)| *kind != self.kind);
        self.put_aside.push((self.kind.clone(), kept));
        let back = self.put_aside.iter().position(|(kind, _)| *kind == next.kind).map(|i| self.put_aside.remove(i).1);
        let fields = back.unwrap_or_else(|| KindFields {
            host: TextField::new(),
            port: TextField::with_text(&next.default_port.map(|p| p.to_string()).unwrap_or_default()),
            user: TextField::new(),
            password: TextField::masked(),
            database: TextField::new(),
            tls: false,
        });
        self.kind = next.kind;
        self.file = next.local_file;
        self.host = fields.host;
        self.port = fields.port;
        self.user = fields.user;
        self.password = fields.password;
        self.database = fields.database;
        self.tls = fields.tls;
        self.errors.clear();
        true
    }

    fn snapshot(&self) -> Snapshot {
        let t = |f: &TextField| f.text().to_string();
        Snapshot {
            kind: self.kind.clone(),
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
        let typed_for_another = self.put_aside.iter().any(|(_, f)| [&f.host, &f.user, &f.password, &f.database].iter().any(|t| !t.is_empty()));
        !self.password.is_empty() || typed_for_another || self.snapshot() != self.initial
    }

    pub fn text_field(&self, field: Field) -> Option<&TextField> {
        Some(match field {
            Field::Name => &self.name,
            Field::Host => &self.host,
            Field::Port => &self.port,
            Field::User => &self.user,
            Field::Password => &self.password,
            Field::Database => &self.database,
            Field::Kind | Field::Tls | Field::Production => return None,
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
            Field::Kind | Field::Tls | Field::Production => return None,
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
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + step).rem_euclid(fields.len() as isize) as usize];
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
            // The type row: space and the arrows pick the next or previous type.
            KeyCode::Char(' ') | KeyCode::Right | KeyCode::Left if combo.mods.is_empty() && self.focus == Field::Kind => {
                let step = if combo.code == KeyCode::Left { -1 } else { 1 };
                return if self.cycle_kind(step) { FormKey::Edited } else { FormKey::Ignored };
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
            // A click on the type row picks the next type.
            None if field == Field::Kind => {
                if self.cycle_kind(1) {
                    FormKey::Edited
                } else {
                    FormKey::Moved
                }
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
        if self.file {
            // A file database: the path is the host, and it is all there is.
            // Whether the file is there is the Core's to say.
            if host.is_empty() {
                self.errors.push((Field::Host, "File is required: give the path of a SQLite file.".into()));
            } else if host.chars().any(char::is_control) {
                self.errors.push((Field::Host, "The path has a control character in it.".into()));
            }
            if let Some((field, _)) = self.errors.first() {
                self.focus = *field;
                return None;
            }
            return Some(ConnectionDraft {
                id: self.editing.clone(),
                name,
                kind: self.kind.clone(),
                host,
                port: None,
                database: None,
                username: String::new(),
                password: None,
                tls: false,
                production: self.production,
            });
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
