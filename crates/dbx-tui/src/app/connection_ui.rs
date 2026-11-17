//! Adding, editing and removing connections: what the keys and the Core's
//! replies do to the form, the removal question and the sidebar's list.

use crossterm::event::KeyEvent;

use super::form::{ConnectionForm, FormKey, FormStatus};
use super::overlay::{ConfirmRemove, FailureOverlay, FieldHit, Overlay};
use super::requests::Target;
use super::{App, Command, CoreCall, Effects, Loadable, Mode};
use crate::core::{sort_connections, Connection, ConnectionId, CoreError, DbKind};

impl App {
    fn next_serial(&mut self) -> u64 {
        let serial = self.ui.next_form_serial;
        self.ui.next_form_serial += 1;
        serial
    }

    pub(super) fn open_new_form(&mut self) {
        let serial = self.next_serial();
        self.ui.overlay = Some(Overlay::Form(Box::new(ConnectionForm::new_connection(serial))));
        self.ui.mode = Mode::Normal;
    }

    pub(super) fn open_edit_form(&mut self, fx: &mut Effects) {
        let Some(connection) = self.selected_connection().cloned() else { return };
        // The form has Postgres's fields and nothing else's: another kind of
        // connection would be saved with the wrong shape.
        if connection.kind != DbKind::Postgres {
            self.say(format!("Edit it in the dbx app ({} is not handled by this form).", connection.kind.label()), false);
            fx.repaint = true;
            return;
        }
        let serial = self.next_serial();
        self.ui.overlay = Some(Overlay::Form(Box::new(ConnectionForm::edit(serial, &connection))));
        fx.repaint = true;
    }

    pub(super) fn open_remove_confirm(&mut self, fx: &mut Effects) {
        let Some(connection) = self.selected_connection() else { return };
        let ask = ConfirmRemove { id: connection.id.clone(), name: connection.name.clone() };
        self.ui.overlay = Some(Overlay::ConfirmRemove(ask));
        fx.repaint = true;
    }

    fn form(&mut self) -> Option<&mut ConnectionForm> {
        match &mut self.ui.overlay {
            Some(Overlay::Form(form)) => Some(form),
            _ => None,
        }
    }

    pub(super) fn on_form_key(&mut self, key: &KeyEvent, fx: &mut Effects) {
        let Some(form) = self.form() else { return };
        match form.on_key(key) {
            FormKey::Ignored => {}
            FormKey::Moved => fx.repaint = true,
            FormKey::Edited => self.form_edited(fx),
        }
    }

    pub(super) fn on_form_click(&mut self, hit: FieldHit, column: u16, fx: &mut Effects) {
        let Some(form) = self.form() else { return };
        if form.confirm_discard {
            return;
        }
        match form.click(hit.field, column) {
            FormKey::Edited => self.form_edited(fx),
            _ => fx.repaint = true,
        }
    }

    /// A value changed. A result shown for the old values is no longer true:
    /// drop it, and stop waiting for a test of values that are gone.
    pub(super) fn form_edited(&mut self, fx: &mut Effects) {
        fx.repaint = true;
        let Some(form) = self.form() else { return };
        let serial = form.serial;
        if form.status != FormStatus::Saving {
            form.status = FormStatus::Idle;
        }
        self.model.requests.supersede(Target::TestConnection(serial));
    }

    pub(super) fn begin_test(&mut self, fx: &mut Effects) {
        let Some(form) = self.form() else { return };
        fx.repaint = true;
        if form.status == FormStatus::Saving {
            return;
        }
        let Some(draft) = form.draft(false) else { return };
        form.status = FormStatus::Testing;
        let serial = form.serial;
        let id = self.model.requests.begin(Target::TestConnection(serial));
        fx.commands.push(Command::Core(id, CoreCall::TestConnection(draft)));
    }

    pub(super) fn begin_save(&mut self, fx: &mut Effects) {
        let Some(form) = self.form() else { return };
        fx.repaint = true;
        if form.status == FormStatus::Saving {
            return;
        }
        let Some(draft) = form.draft(true) else { return };
        form.status = FormStatus::Saving;
        let serial = form.serial;
        self.model.requests.supersede(Target::TestConnection(serial));
        let id = self.model.requests.begin(Target::SaveConnection(serial));
        fx.commands.push(Command::Core(id, CoreCall::SaveConnection(draft)));
    }

    /// `esc` on the form: close it, unless that would lose what was typed.
    pub(super) fn request_close_form(&mut self, fx: &mut Effects) {
        let Some(form) = self.form() else { return };
        // A form that is saving is inert: closing it now could discard a save
        // that then fails, with nothing left to show why.
        if form.locked() {
            return;
        }
        if !form.dirty() {
            return self.close_form(fx);
        }
        form.confirm_discard = true;
        fx.repaint = true;
    }

    pub(super) fn close_form(&mut self, fx: &mut Effects) {
        if let Some(Overlay::Form(form)) = self.ui.overlay.take() {
            // Its test is no longer wanted. A save is: what it saved is real.
            self.model.requests.supersede(Target::TestConnection(form.serial));
        }
        fx.repaint = true;
    }

    pub(super) fn confirm_remove(&mut self, fx: &mut Effects) {
        let Some(Overlay::ConfirmRemove(ask)) = self.ui.overlay.take() else { return };
        let id = self.model.requests.begin(Target::RemoveConnection(ask.id.clone()));
        fx.commands.push(Command::Core(id, CoreCall::RemoveConnection(ask.id)));
        fx.repaint = true;
    }

    pub(super) fn on_tested(&mut self, serial: u64, result: Result<(), CoreError>) {
        if let Some(form) = self.form().filter(|f| f.serial == serial) {
            form.status = match result {
                Ok(()) => FormStatus::TestOk,
                Err(e) => FormStatus::TestFailed(e.message),
            };
        }
    }

    pub(super) fn on_saved(&mut self, serial: u64, result: Result<Connection, CoreError>, fx: &mut Effects) {
        let open = self.form().is_some_and(|f| f.serial == serial);
        match result {
            Ok(connection) => {
                // Saved is saved, even if the form was closed meanwhile: the
                // sidebar lists it. Only the form itself is the reply's to close.
                self.upsert_connection(connection, fx);
                if open {
                    self.ui.overlay = None;
                }
            }
            Err(e) => {
                if let Some(form) = self.form().filter(|f| f.serial == serial) {
                    form.status = FormStatus::SaveFailed(e.message);
                }
            }
        }
    }

    pub(super) fn on_removed(&mut self, id: ConnectionId, result: Result<(), CoreError>, fx: &mut Effects) {
        // The request is done either way: it need not be remembered.
        self.model.requests.forget(&super::requests::Target::RemoveConnection(id.clone()));
        match result {
            Ok(()) => {
                if let Loadable::Ready(list) = &mut self.model.connections {
                    list.retain(|c| c.id != id);
                }
                // Its tree, and any listing still on its way, go with it.
                self.forget_connection(&id, fx);
            }
            Err(e) if self.ui.overlay.is_some() => {
                // Whatever is open (a form with typed text, say) is not ours
                // to replace: say it at the foot of the sidebar instead.
                self.say(format!("Could not remove: {}", e.message), true);
            }
            Err(e) => {
                self.ui.overlay = Some(Overlay::Failure(FailureOverlay {
                    title: "Could not remove connection".into(),
                    message: e.message,
                }));
                self.ui.mode = Mode::Normal;
            }
        }
    }

    /// Puts a saved connection in the list (replacing its old self if it was
    /// an edit), in the order the Core lists them, with the cursor on it.
    pub(super) fn upsert_connection(&mut self, connection: Connection, fx: &mut Effects) {
        let id = connection.id.clone();
        let mut list = match std::mem::replace(&mut self.model.connections, Loadable::Loading) {
            Loadable::Ready(list) => list,
            _ => Vec::new(),
        };
        match list.iter_mut().find(|c| c.id == id) {
            Some(existing) => {
                // What was loaded came from the old settings: if they changed
                // where the connection points, it may no longer be true.
                let moved = (&existing.kind, &existing.host, existing.port, &existing.database, &existing.username, existing.tls)
                    != (&connection.kind, &connection.host, connection.port, &connection.database, &connection.username, connection.tls);
                *existing = connection;
                if moved {
                    self.forget_connection(&id, fx);
                }
            }
            None => list.push(connection),
        }
        sort_connections(&mut list);
        self.model.connections = Loadable::Ready(list);
        self.ui.sidebar.cursor_key = Some(super::tree::RowKey { node: super::tree::NodeId::Connection(id), slot: super::tree::Slot::Node });
        self.touch_rows();
    }
}
