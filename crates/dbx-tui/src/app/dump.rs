//! Dumping a database, or one schema, to a `.sql` file (slice 20).
//!
//! `D` on a database or schema in the sidebar opens the overlay: what goes in
//! (structure, data or both), the scope, drop tables, and the path. dbx does
//! the work; the shell checks the path first (a missing directory is refused,
//! a file that is there is replaced only after a red question naming it) and
//! reports what is at the path when the dump has ended. One dump runs at a
//! time; its progress is in the mode bar in every pane, and `D` (or a click on
//! that status) shows it with the key that cancels it.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::export::{rows_word, stem_of};
use super::grid::size_text;
use super::keys::KeyCombo;
use super::overlay::{Overlay, OverlayAction};
use super::requests::{RequestId, Target};
use super::text_field::{Edit, TextField};
use super::tree::NodeId;
use super::view::{duration_text, thousands};
use super::{App, Command, Effects};
use crate::core::{DatabasePath, Dialect, DumpContent, DumpFailure, DumpId, DumpProgress, DumpRequest, DumpStatus, Dumped, FileLeft};

/// How long quitting waits for a cancelled dump's final progress, as it waits
/// for a cancelled run (the shell's `RUN_WAIT`).
pub const DUMP_QUIT_WAIT: Duration = Duration::from_secs(2);

/// The dump in flight: one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dumping {
    pub id: DumpId,
    pub request: RequestId,
    pub job: DumpRequest,
    /// What the dump is called: the database, or the connection for one that
    /// has no database level (a SQLite file).
    pub base: String,
    pub started: Instant,
    /// The latest progress, `None` until dbx has said anything.
    pub progress: Option<DumpProgress>,
    /// When the user asked it to stop.
    pub cancelling: Option<Instant>,
    /// dbx's final progress (`Done` or `Cancelled`) has come; the end, with
    /// what is at the path, is still to come.
    pub ended: Option<DumpStatus>,
}

impl Dumping {
    /// `pagila`, or `pagila.public` for one schema.
    pub fn name(&self) -> String {
        if self.job.schema.is_empty() {
            self.base.clone()
        } else {
            format!("{}.{}", self.base, self.job.schema)
        }
    }

    /// Whether `c` can still ask it to stop.
    pub fn cancellable(&self) -> bool {
        self.cancelling.is_none() && self.ended.is_none()
    }
}

/// One row of the dump overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpField {
    Content,
    Scope,
    Drop,
    Path,
}

impl DumpField {
    pub const ALL: [DumpField; 4] = [DumpField::Content, DumpField::Scope, DumpField::Drop, DumpField::Path];

    pub fn label(self) -> &'static str {
        match self {
            DumpField::Content => "what",
            DumpField::Scope => "scope",
            DumpField::Drop => "tables",
            DumpField::Path => "path",
        }
    }
}

/// Where the overlay is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpStage {
    Editing,
    /// Sent; the shell is checking the path. The overlay closes when dbx's
    /// first progress comes.
    Sent(RequestId),
    /// Refused or failed before it started: the overlay says why.
    Failed(String),
    /// The file is there: replace it? (the path, as the shell named it)
    Replace(String),
}

#[derive(Debug, Clone)]
pub struct DumpOverlay {
    pub at: DatabasePath,
    /// The schema the overlay was opened on, if it was a schema.
    pub schema: Option<String>,
    /// The scope: the whole database, or `schema`.
    pub whole: bool,
    pub content: DumpContent,
    pub drop_tables: bool,
    pub path: TextField,
    pub focus: DumpField,
    pub stage: DumpStage,
    /// dbx's dump of this kind is known not to restore whole: the overlay
    /// says so, with the losses of this dialect.
    pub loses: Option<Dialect>,
    /// What the dump is called: the database, or the connection for one that
    /// has no database level (a SQLite file).
    pub base: String,
}

impl DumpOverlay {
    fn new(at: DatabasePath, schema: Option<String>, dir: &std::path::Path, stamp: &str, loses: Option<Dialect>, base: String) -> Self {
        let path = dir.join(format!("{}-{stamp}.sql", stem_of(&base))).to_string_lossy().into_owned();
        DumpOverlay {
            whole: schema.is_none(),
            base,
            at,
            schema,
            content: DumpContent::StructureAndData,
            drop_tables: false,
            path: TextField::with_text(&path),
            focus: DumpField::Content,
            stage: DumpStage::Editing,
            loses,
        }
    }

    /// The title: `Dump pagila`.
    pub fn title(&self) -> String {
        format!("Dump {}", self.base)
    }

    /// Sent, or asking about the file: the fields take no edits.
    pub fn locked(&self) -> bool {
        matches!(self.stage, DumpStage::Sent(_) | DumpStage::Replace(_))
    }

    fn edited(&mut self) {
        if matches!(self.stage, DumpStage::Failed(_)) {
            self.stage = DumpStage::Editing;
        }
    }

    fn step_focus(&mut self, step: isize) -> bool {
        let at = DumpField::ALL.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = DumpField::ALL[(at + step).rem_euclid(DumpField::ALL.len() as isize) as usize];
        true
    }

    pub fn pick_content(&mut self, index: usize) -> bool {
        let Some(content) = DumpContent::ALL.get(index).copied() else { return false };
        if self.locked() || content == self.content {
            return false;
        }
        self.content = content;
        self.edited();
        true
    }

    /// The scope: only a schema node has a choice.
    pub fn pick_scope(&mut self, whole: bool) -> bool {
        if self.locked() || self.whole == whole || (!whole && self.schema.is_none()) {
            return false;
        }
        self.whole = whole;
        self.edited();
        true
    }

    pub fn pick_drop(&mut self, on: bool) -> bool {
        if self.locked() || self.drop_tables == on {
            return false;
        }
        self.drop_tables = on;
        self.edited();
        true
    }

    /// A key that is not a button. Returns whether anything changed on screen.
    pub fn on_key(&mut self, key: &KeyEvent) -> bool {
        if self.locked() {
            return false;
        }
        let combo = KeyCombo::from_event(key);
        let plain = combo.mods.is_empty();
        match combo.code {
            KeyCode::BackTab => return self.step_focus(-1),
            KeyCode::Tab if combo.mods.contains(KeyModifiers::SHIFT) => return self.step_focus(-1),
            KeyCode::Tab | KeyCode::Down if plain => return self.step_focus(1),
            KeyCode::Up if plain => return self.step_focus(-1),
            _ => {}
        }
        let left = matches!(combo.code, KeyCode::Left | KeyCode::Char('h')) && plain;
        let right = matches!(combo.code, KeyCode::Right | KeyCode::Char('l')) && plain;
        match self.focus {
            DumpField::Content => {
                let at = DumpContent::ALL.iter().position(|c| *c == self.content).unwrap_or(0);
                let to = match combo.code {
                    _ if left => at.saturating_sub(1),
                    _ if right => (at + 1).min(DumpContent::ALL.len() - 1),
                    KeyCode::Char(d @ '1'..='3') if plain => d as usize - '1' as usize,
                    _ => return false,
                };
                self.pick_content(to)
            }
            DumpField::Scope => match combo.code {
                _ if left => self.pick_scope(true),
                _ if right => self.pick_scope(false),
                KeyCode::Char(' ') if plain => self.pick_scope(!self.whole),
                _ => false,
            },
            DumpField::Drop => match combo.code {
                _ if left => self.pick_drop(false),
                _ if right => self.pick_drop(true),
                KeyCode::Char(' ') if plain => self.pick_drop(!self.drop_tables),
                _ => false,
            },
            DumpField::Path => match self.path.handle_key(key) {
                Edit::Ignored => false,
                Edit::Moved => true,
                Edit::Changed => {
                    self.edited();
                    true
                }
            },
        }
    }

    pub fn paste(&mut self, text: &str) -> bool {
        if self.locked() || self.focus != DumpField::Path {
            return false;
        }
        self.path.insert_str(text);
        self.edited();
        true
    }

    pub fn click(&mut self, field: DumpField, column: u16) -> bool {
        if self.locked() {
            return false;
        }
        self.focus = field;
        if field == DumpField::Path {
            self.path.click(column);
        }
        true
    }

    fn request(&self, prevent_overwrite: bool) -> DumpRequest {
        DumpRequest {
            at: self.at.clone(),
            schema: if self.whole { String::new() } else { self.schema.clone().unwrap_or_default() },
            content: self.content,
            drop_tables: self.drop_tables,
            path: self.path.text().to_string(),
            prevent_overwrite,
        }
    }
}

/// What dbx's dump is known to lose on Postgres, GaussDB and openGauss
/// (README "Dump", "What dbx's dump loses"; each is pinned by a live test).
pub const KNOWN_LOSSES: &str = "domains, partitions, composite arrays, NaN and Infinity (written as NULL), geometric and tsquery values (written as bytes), tsvector columns (dropped) and tables with several parents";

/// What dbx's dump is known to lose on SQLite (README "Dump", "What dbx's dump
/// loses on SQLite"; each is pinned by a live test).
pub const KNOWN_LOSSES_SQLITE: &str = "blobs (written as text starting 0x), Infinity (written as NULL), a time zone in a date or time (dropped) and integers past 2^53 in an untyped column (written as text)";

/// The losses of the dump of a connection speaking `dialect`.
pub fn known_losses(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => KNOWN_LOSSES_SQLITE,
        _ => KNOWN_LOSSES,
    }
}

/// The line the setup overlay shows, and a finished dump with errors repeats.
pub fn known_losses_note(dialect: Dialect) -> String {
    format!("dbx's dump is known not to restore whole: {}. See README \"Dump\".", known_losses(dialect))
}

/// `1 schema`, `22 objects`.
fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// What is at the path, as a sentence.
fn left_text(path: &str, file: FileLeft) -> String {
    match file {
        FileLeft::Nothing => format!("Nothing was written to {path}."),
        FileLeft::Unchanged => format!("The earlier file at {path} is untouched."),
        FileLeft::Written(n) => format!("A partial file is left at {path} ({}): it is not a whole dump.", size_text(n as usize)),
    }
}

impl App {
    /// The database (and schema) a dump of the row under the cursor would
    /// cover, when it is a database or schema node of a connection the
    /// client can query and browse.
    fn dump_target(&self) -> Option<(DatabasePath, Option<String>)> {
        let row = self.cursor_row()?;
        if !row.is_node() {
            return None;
        }
        let (at, schema) = match &row.node {
            NodeId::Database(d) => (d.clone(), None),
            NodeId::Schema(s) => (s.database.clone(), Some(s.schema.clone())),
            // A connection with no database level (a SQLite file) is itself
            // the database: dumped whole, under the empty database name.
            NodeId::Connection(id) if self.connection(id).is_some_and(|c| !c.capabilities.databases) => (DatabasePath { connection: id.clone(), database: String::new() }, None),
            _ => return None,
        };
        let c = self.connection(&at.connection)?;
        (c.capabilities.query && c.capabilities.browse).then_some((at, schema))
    }

    /// Whether `D` does something where the cursor is: dumps the node, or
    /// shows the dump that runs.
    pub(super) fn cursor_can_dump(&self) -> bool {
        self.dump_target().is_some() || self.model.dump.is_some()
    }

    /// `D` in the sidebar.
    pub(super) fn open_dump(&mut self, fx: &mut Effects) {
        let target = self.dump_target();
        if let Some(d) = &self.model.dump {
            // On another database or schema it is a second dump, refused; on
            // the one being dumped (or anywhere else) `D` shows its progress.
            let same = target.as_ref().is_some_and(|(at, schema)| *at == d.job.at && schema.as_deref().unwrap_or("") == d.job.schema);
            if target.is_some() && !same {
                let text = format!("One dump at a time: {} is being dumped to {}.", d.name(), d.job.path);
                self.notify(text, true, fx);
            }
            return self.open_dump_progress(fx);
        }
        let Some((at, schema)) = target else { return };
        let stamp = (self.ui.export_env.stamp)();
        let connection = self.connection(&at.connection);
        let loses = connection.filter(|c| c.capabilities.dump_loses).map(|c| c.capabilities.dialect);
        // A database is called by its name; a file by its connection's.
        let base = if at.database.is_empty() { connection.map(|c| c.name.clone()).unwrap_or_default() } else { at.database.clone() };
        let overlay = DumpOverlay::new(at, schema, &self.ui.export_env.dir, &stamp, loses, base);
        self.ui.overlay = Some(Overlay::Dump(Box::new(overlay)));
        fx.repaint = true;
    }

    /// The progress overlay (a click on the mode bar's status, or `D`).
    pub(super) fn open_dump_progress(&mut self, fx: &mut Effects) {
        let cancellable = self.model.dump.as_ref().is_some_and(Dumping::cancellable);
        self.ui.overlay = Some(Overlay::DumpProgress(cancellable));
        fx.repaint = true;
    }

    fn dump_overlay(&mut self) -> Option<&mut DumpOverlay> {
        match &mut self.ui.overlay {
            Some(Overlay::Dump(o)) => Some(o),
            _ => None,
        }
    }

    pub(super) fn on_dump_key(&mut self, key: &KeyEvent, fx: &mut Effects) {
        if let Some(o) = self.dump_overlay() {
            fx.repaint |= o.on_key(key);
        }
    }

    pub(super) fn on_dump_paste(&mut self, text: &str, fx: &mut Effects) {
        if let Some(o) = self.dump_overlay() {
            fx.repaint |= o.paste(text);
        }
    }

    pub(super) fn on_dump_click(&mut self, field: DumpField, column: u16, fx: &mut Effects) {
        if let Some(o) = self.dump_overlay() {
            fx.repaint |= o.click(field, column);
        }
    }

    /// What a button, or a key that is one, does in the dump overlays.
    pub(super) fn dump_action(&mut self, action: OverlayAction, fx: &mut Effects) {
        use OverlayAction as A;
        match action {
            A::DumpRun => self.begin_dump(false, fx),
            A::DumpReplace => self.begin_dump(true, fx),
            A::DumpKeep => {
                if let Some(o) = self.dump_overlay() {
                    o.stage = DumpStage::Editing;
                }
                fx.repaint = true;
            }
            A::DumpContent(i) => {
                if let Some(o) = self.dump_overlay() {
                    fx.repaint |= o.pick_content(i);
                }
            }
            A::DumpScope(whole) => {
                if let Some(o) = self.dump_overlay() {
                    fx.repaint |= o.pick_scope(whole);
                }
            }
            A::DumpDrop(on) => {
                if let Some(o) = self.dump_overlay() {
                    fx.repaint |= o.pick_drop(on);
                }
            }
            A::DumpFocus(field) => self.on_dump_click(field, 0, fx),
            A::DumpCancel => self.cancel_dump(fx),
            A::QuitDumpYes => self.quit_cancelling_dump(fx),
            _ => {}
        }
    }

    /// Sends the dump. `replace`: the question about the file that is there
    /// was answered yes, so dbx may replace it.
    fn begin_dump(&mut self, replace: bool, fx: &mut Effects) {
        fx.repaint = true;
        if let Some(d) = &self.model.dump {
            let text = format!("One dump at a time: {} is being dumped to {}.", d.name(), d.job.path);
            return self.notify(text, true, fx);
        }
        let Some(o) = self.dump_overlay() else { return };
        if matches!(o.stage, DumpStage::Sent(_)) || (matches!(o.stage, DumpStage::Replace(_)) && !replace) {
            return;
        }
        if o.path.text().trim().is_empty() {
            o.stage = DumpStage::Failed("Give the file a name: the path is empty.".into());
            o.focus = DumpField::Path;
            return;
        }
        let job = o.request(!replace);
        let base = o.base.clone();
        let id = self.model.requests.begin(Target::Dump);
        let dump = DumpId(self.model.next_dump);
        self.model.next_dump += 1;
        if let Some(o) = self.dump_overlay() {
            o.stage = DumpStage::Sent(id);
        }
        let started = self.ui.now.unwrap_or_else(Instant::now);
        self.model.dump = Some(Dumping { id: dump, request: id, job: job.clone(), base, started, progress: None, cancelling: None, ended: None });
        fx.commands.push(Command::Dump(id, dump, job));
    }

    /// `c` in the progress overlay.
    fn cancel_dump(&mut self, fx: &mut Effects) {
        let now = self.ui.now.unwrap_or_else(Instant::now);
        let Some(d) = &mut self.model.dump else { return };
        if !d.cancellable() {
            return;
        }
        d.cancelling = Some(now);
        fx.commands.push(Command::CancelDump(d.id));
        fx.repaint = true;
    }

    /// One report of dbx's progress, if it is for the dump in flight.
    pub(super) fn on_dump_progress(&mut self, id: RequestId, progress: DumpProgress, fx: &mut Effects) {
        if !self.model.requests.is_current(id) {
            return;
        }
        let Some(d) = &mut self.model.dump else { return };
        if d.request != id || d.ended.is_some() {
            return;
        }
        if matches!(progress.status, DumpStatus::Done | DumpStatus::Cancelled) {
            d.ended = Some(progress.status);
        }
        d.progress = Some(progress);
        // dbx has started: the setup overlay has done its work.
        if matches!(&self.ui.overlay, Some(Overlay::Dump(o)) if o.stage == DumpStage::Sent(id)) {
            self.ui.overlay = None;
        }
        fx.repaint = true;
        if self.model.dump.as_ref().is_some_and(|d| d.ended.is_some()) && self.model.quit_after_dump.is_some() {
            self.model.quit_after_dump = None;
            self.quit(fx);
        }
    }

    /// The dump has ended, or the shell refused it.
    pub(super) fn on_dumped(&mut self, id: RequestId, result: Result<Dumped, DumpFailure>, fx: &mut Effects) {
        if self.model.dump.as_ref().is_none_or(|d| d.request != id) {
            return;
        }
        let Some(d) = self.model.dump.take() else { return };
        fx.repaint = true;
        let now = self.ui.now.unwrap_or(d.started);
        let elapsed = now.saturating_duration_since(d.started);
        let name = d.name();
        let sent = matches!(&self.ui.overlay, Some(Overlay::Dump(o)) if o.stage == DumpStage::Sent(id));
        let (text, failure) = match result {
            Err(f) if f.refused || f.exists => {
                // Refused before dbx did anything: back to the overlay as typed.
                if sent {
                    if let Some(o) = self.dump_overlay() {
                        o.stage = if f.exists { DumpStage::Replace(f.path) } else { DumpStage::Failed(f.message) };
                        if !f.exists {
                            o.focus = DumpField::Path;
                        }
                    }
                    return self.after_dump(fx);
                }
                let text = if f.exists { format!("Not dumped: {} is there already, and nothing was replaced.", f.path) } else { f.message };
                (text, true)
            }
            Err(f) => {
                let mut text = format!("The dump of {name} failed: {}", f.message.trim_end_matches('.'));
                text.push_str(". ");
                text.push_str(&left_text(&f.path, f.file));
                (text, true)
            }
            Ok(done) if done.end.cancelled() => (format!("The dump of {name} was cancelled after {}. {}", duration_text(elapsed), left_text(&done.path, done.file)), false),
            // "Dumped" needs dbx's own final `Done` and a file that is new.
            Ok(done) => match (done.end.last.clone().filter(|p| p.status == DumpStatus::Done), done.file) {
                (Some(last), FileLeft::Written(n)) => {
                    let size = format!(" ({})", size_text(n as usize));
                    let objects = if d.job.schema.is_empty() { counted(last.total, "schema", "schemas") } else { counted(last.total, "object", "objects") };
                    let rows = rows_word(last.rows as usize);
                    let loses = self.connection(&d.job.at.connection).filter(|c| c.capabilities.dump_loses).map(|c| c.capabilities.dialect);
                    if last.errors > 0 {
                        let first = last.first_error.as_deref().map(|e| format!(" The first: {}.", e.trim_end_matches('.'))).unwrap_or_default();
                        let mut text = format!(
                            "The dump of {name} finished with {}.{first} The file {}{size} has -- ERROR comments where they happened.",
                            counted(last.errors as usize, "error", "errors"),
                            done.path
                        );
                        if let Some(dialect) = loses {
                            text.push_str(&format!(" {}", known_losses_note(dialect)));
                        }
                        (text, true)
                    } else if let Some(dialect) = loses {
                        // Not a bare "Dumped": dbx's dump of this kind is
                        // known not to restore whole (README "Dump").
                        let text = format!(
                            "Wrote the dump of {name} to {}{size}: {objects}, {rows} in {}. dbx reported no errors; its dump is known not to restore whole: {}. See README \"Dump\".",
                            done.path,
                            duration_text(elapsed),
                            known_losses(dialect)
                        );
                        (text, false)
                    } else {
                        (format!("Dumped {name} to {}{size}: {objects}, {rows} in {}.", done.path, duration_text(elapsed)), false)
                    }
                }
                (Some(_), file) => (format!("The dump of {name}: dbx reported done, but no new file is at the path. {}", left_text(&done.path, file)), true),
                (None, file) => (format!("The dump of {name}: dbx ended without reporting that it was done, so nothing says the file is whole. {}", left_text(&done.path, file)), true),
            },
        };
        if sent {
            self.ui.overlay = None;
        }
        self.ui.dump_ended = Some((text.clone(), failure));
        self.notify(text, failure, fx);
        self.after_dump(fx);
    }

    /// A dump is over: a quit that waited for it goes ahead.
    fn after_dump(&mut self, fx: &mut Effects) {
        if self.model.quit_after_dump.take().is_some() {
            self.quit(fx);
        }
    }

    /// Quit was asked for. While a dump runs it asks first, naming the file.
    pub(super) fn request_quit(&mut self, fx: &mut Effects) {
        if self.model.dump.is_some() && self.model.quit_after_dump.is_none() {
            self.ui.overlay = Some(Overlay::ConfirmQuitDump);
            fx.repaint = true;
            return;
        }
        self.quit(fx);
    }

    /// Yes to the quit question: cancel the dump, and quit once its final
    /// progress has come, or `DUMP_QUIT_WAIT` has passed.
    fn quit_cancelling_dump(&mut self, fx: &mut Effects) {
        self.ui.overlay = None;
        fx.repaint = true;
        if self.model.dump.as_ref().is_none_or(|d| d.ended.is_some()) {
            return self.quit(fx);
        }
        self.cancel_dump(fx);
        let now = self.ui.now.unwrap_or_else(Instant::now);
        self.model.quit_after_dump = Some(now + DUMP_QUIT_WAIT);
    }

    /// Quitting cancels a dump still running (a signal, or after the wait).
    pub(super) fn cancel_dump_for_quit(&mut self, fx: &mut Effects) {
        self.model.quit_after_dump = None;
        if let Some(d) = &mut self.model.dump {
            if d.cancelling.is_none() && d.ended.is_none() {
                d.cancelling = Some(d.started);
                fx.commands.push(Command::CancelDump(d.id));
            }
        }
    }

    /// When the dump's clock or the quit's wait next needs a wake.
    pub(super) fn dump_deadline(&self) -> Option<Instant> {
        let tick = self.model.dump.as_ref().map(|d| {
            let secs = self.ui.now.map_or(0, |now| now.saturating_duration_since(d.started).as_secs());
            d.started + Duration::from_secs(secs + 1)
        });
        [tick, self.model.quit_after_dump].into_iter().flatten().min()
    }

    /// Time passed: the dump's clock moved on; a quit waited long enough.
    pub(super) fn dump_on_time(&mut self, last: Option<Instant>, now: Instant, fx: &mut Effects) {
        if let (Some(d), Some(last)) = (&self.model.dump, last) {
            let shown = |t: Instant| t.saturating_duration_since(d.started).as_secs();
            if shown(now) != shown(last) {
                fx.repaint = true;
            }
        }
        if self.model.quit_after_dump.is_some_and(|at| now >= at) {
            self.model.quit_after_dump = None;
            self.quit(fx);
        }
    }

    /// Keeps the progress overlay's `c` offered only while it works.
    pub(super) fn sync_dump_overlay(&mut self) {
        let cancellable = self.model.dump.as_ref().is_some_and(Dumping::cancellable);
        if let Some(Overlay::DumpProgress(c)) = &mut self.ui.overlay {
            *c = cancellable;
        }
    }

    /// The mode bar's status for the dump: `dumping pagila: film (7/22)
    /// 12,340 rows 14s`.
    pub(super) fn dump_status(&self) -> Option<(String, bool)> {
        let d = self.model.dump.as_ref()?;
        let secs = self.ui.now.map_or(0, |now| now.saturating_duration_since(d.started).as_secs());
        let name = d.name();
        if d.ended == Some(DumpStatus::Cancelled) {
            return Some((format!("dump of {name} cancelled"), false));
        }
        if d.cancelling.is_some() {
            return Some((format!("cancelling dump of {name}… {secs}s"), false));
        }
        let what = match &d.progress {
            None => "starting".to_string(),
            Some(p) if p.status == DumpStatus::Preparing => "preparing".to_string(),
            Some(p) => {
                let mut parts = Vec::new();
                if !p.object.is_empty() {
                    parts.push(if p.total > 0 { format!("{} ({}/{})", p.object, p.index, p.total) } else { p.object.clone() });
                }
                parts.push(rows_word(p.rows as usize));
                parts.join(" ")
            }
        };
        Some((format!("dumping {name}: {what} {secs}s"), true))
    }

    /// The progress overlay's lines: what, where, how far; or how the last
    /// dump ended.
    pub(super) fn dump_progress_lines(&self) -> (String, Vec<String>, bool) {
        let Some(d) = &self.model.dump else {
            return match &self.ui.dump_ended {
                Some((text, failure)) => ("Dump".to_string(), vec![text.clone()], *failure),
                None => ("Dump".to_string(), vec!["No dump is running.".to_string()], false),
            };
        };
        let secs = self.ui.now.map_or(0, |now| now.saturating_duration_since(d.started).as_secs());
        let mut lines = vec![format!("to {}", d.job.path), format!("{}, {}", d.job.content.label(), if d.job.schema.is_empty() { "whole database" } else { "one schema" })];
        match &d.progress {
            None => lines.push("starting…".into()),
            Some(p) if p.status == DumpStatus::Preparing => lines.push("preparing: listing what there is".into()),
            Some(p) => {
                if !p.object.is_empty() {
                    lines.push(if p.total > 0 { format!("{} ({}/{})", p.object, p.index, p.total) } else { p.object.clone() });
                }
                lines.push(format!("{} so far", rows_word(p.rows as usize)));
                if p.errors > 0 {
                    lines.push(format!("{} so far, written as -- ERROR comments", counted(p.errors as usize, "error", "errors")));
                }
            }
        }
        lines.push(format!("{secs}s"));
        match (d.ended, d.cancelling) {
            (Some(DumpStatus::Cancelled), _) => lines.push("cancelled: looking at what is left at the path".into()),
            (_, Some(_)) => lines.push("cancelling…".into()),
            _ => {}
        }
        (format!("Dump {}", d.name()), lines, false)
    }

    /// The quit question's words.
    pub(super) fn quit_dump_message(&self) -> String {
        let Some(d) = &self.model.dump else { return "The dump has ended.".to_string() };
        // dbx writes a whole database schema by schema into copies in its own
        // temporary folder (`tempfile::tempdir()`), removed when it ends; its
        // folder name is not ours to know, only where it is made.
        let copy = if d.job.schema.is_empty() {
            format!(" dbx's temporary copy may be left in {} (a folder with schema-N.sql files).", self.ui.export_env.temp_dir.display())
        } else {
            String::new()
        };
        format!(
            "A dump of {} to {} is running. Quitting cancels it. Nothing is put at that path until dbx is done, but a hidden temporary file beside it (named .<file>.….tmp) may be left.{copy}",
            d.name(),
            d.job.path
        )
    }
}
