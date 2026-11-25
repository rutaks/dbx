//! Export and copy-as (slice 19): `e` in the results pane writes the rows the
//! client holds to a file, `C` copies them to the clipboard as text.
//!
//! The overlay says before writing what the format does to values (its
//! "losses"), and how many rows go in. Nothing is said to have been written
//! until the shell reports it did (`CoreReply::Exported`), and a file that is
//! there is replaced only after a question that names it. dbx's formats do the
//! work (`Syntax::export_bytes`); this module is the questions and the words.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::grid::{shown, size_text, CLIPBOARD_LIMIT};
use super::keys::{Action, KeyCombo};
use super::overlay::Overlay;
use super::requests::{RequestId, Target};
use super::text_field::{Edit, TextField};
use super::view::thousands;
use super::{App, Command, Effects};
use crate::core::{Cell, ExportError, ExportFormat, ExportJob, ExportRequest, Exported, ResultSet, TabId};

/// Excel's cell limit, in UTF-16 units (dbx's `EXCEL_CELL_CHARACTER_LIMIT`).
const EXCEL_CELL_LIMIT: usize = 32_767;

/// Where an export goes by default, and the time it is stamped with.
#[derive(Clone)]
pub struct ExportEnv {
    pub dir: PathBuf,
    pub stamp: Arc<dyn Fn() -> String + Send + Sync>,
}

impl std::fmt::Debug for ExportEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportEnv").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl Default for ExportEnv {
    fn default() -> Self {
        ExportEnv::fixed(".", "00000000-000000")
    }
}

impl ExportEnv {
    /// A directory and a stamp that never change: what tests use.
    pub fn fixed(dir: &str, stamp: &str) -> Self {
        let stamp = stamp.to_string();
        ExportEnv { dir: PathBuf::from(dir), stamp: Arc::new(move || stamp.clone()) }
    }
}

/// One row of the export overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportField {
    Format,
    Path,
    Table,
    Schema,
    Null,
}

impl ExportField {
    pub fn label(self) -> &'static str {
        match self {
            ExportField::Format => "format",
            ExportField::Path => "path",
            ExportField::Table => "table",
            ExportField::Schema => "schema",
            ExportField::Null => "null",
        }
    }
}

/// Where the overlay is: being filled in, waiting for the shell, told that the
/// write failed, or asking whether to replace the file that is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Editing,
    Writing(RequestId),
    Failed(String),
    /// The path the job named.
    Replace(String),
}

/// What the overlay described, to tell that the result is still the one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    ran: String,
    rows: usize,
    columns: usize,
    truncated: bool,
    elapsed: Duration,
}

impl Held {
    fn of(set: &ResultSet) -> Self {
        Held { ran: set.ran.clone(), rows: set.rows.len(), columns: set.columns.len(), truncated: set.truncated, elapsed: set.elapsed }
    }
}

#[derive(Debug, Clone)]
pub struct ExportOverlay {
    /// The tab whose result it is, and the serial that ties its answer to it.
    pub tab: TabId,
    pub serial: u64,
    pub format: ExportFormat,
    pub path: TextField,
    pub table: TextField,
    pub schema: TextField,
    /// CSV: NULL is an empty field.
    pub null_empty: bool,
    pub focus: ExportField,
    pub stage: Stage,
    /// The rows held, whether the cap cut the result, and how many values
    /// are over Excel's cell limit.
    pub rows: usize,
    pub truncated: bool,
    pub long_cells: usize,
    /// SQL INSERT: the columns dbx leaves out of the statements.
    pub skipped: Vec<String>,
    held: Held,
    /// The default file name's stem and stamp, and its directory.
    stem: String,
    stamp: String,
    dir: PathBuf,
    /// The path was typed in: the format no longer changes it.
    path_edited: bool,
}

/// A file name's stem made of a table or `result`: nothing that would put the
/// file in another directory.
fn stem_of(name: &str) -> String {
    let stem: String = name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':') { '_' } else { c }).collect();
    if stem.trim().is_empty() {
        "result".into()
    } else {
        stem
    }
}

fn rows_word(n: usize) -> String {
    format!("{} {}", thousands(n), if n == 1 { "row" } else { "rows" })
}

impl ExportOverlay {
    /// The overlay for `set`, in tab `tab`. `name` is the table of a browse
    /// tab (`None` for any other result), `schema` its schema.
    pub fn new(tab: TabId, serial: u64, set: &ResultSet, name: Option<&str>, schema: &str, env: &ExportEnv) -> Self {
        let long_cells = set
            .rows
            .iter()
            .flatten()
            .filter(|c| matches!(c, Cell::Value(v) if v.len() > EXCEL_CELL_LIMIT && v.encode_utf16().count() > EXCEL_CELL_LIMIT))
            .count();
        let mut overlay = ExportOverlay {
            tab,
            serial,
            format: ExportFormat::Csv,
            path: TextField::new(),
            table: TextField::with_text(name.unwrap_or("result")),
            schema: TextField::with_text(schema),
            null_empty: false,
            focus: ExportField::Format,
            stage: Stage::Editing,
            rows: set.rows.len(),
            truncated: set.truncated,
            long_cells,
            skipped: Vec::new(),
            held: Held::of(set),
            stem: stem_of(name.unwrap_or("result")),
            stamp: (env.stamp)(),
            dir: env.dir.clone(),
            path_edited: false,
        };
        overlay.path.set_text(&overlay.default_path());
        overlay
    }

    /// `<dir>/<name>-<stamp>.<extension>`.
    fn default_path(&self) -> String {
        self.dir.join(format!("{}-{}.{}", self.stem, self.stamp, self.format.extension())).to_string_lossy().into_owned()
    }

    /// The rows that apply to the format, in order.
    pub fn fields(&self) -> Vec<ExportField> {
        let mut fields = vec![ExportField::Format, ExportField::Path];
        match self.format {
            ExportFormat::SqlInsert => fields.extend([ExportField::Table, ExportField::Schema]),
            ExportFormat::Csv => fields.push(ExportField::Null),
            _ => {}
        }
        fields
    }

    /// Writing, or asking about the file: the fields take no edits.
    pub fn locked(&self) -> bool {
        matches!(self.stage, Stage::Writing(_) | Stage::Replace(_))
    }

    pub fn set_format(&mut self, format: ExportFormat) {
        self.format = format;
        if !self.path_edited {
            let path = self.default_path();
            self.path.set_text(&path);
        }
        if !self.fields().contains(&self.focus) {
            self.focus = ExportField::Format;
        }
    }

    /// A value changed: an error shown for the earlier values is gone.
    fn edited(&mut self) {
        if matches!(self.stage, Stage::Failed(_)) {
            self.stage = Stage::Editing;
        }
    }

    fn move_focus(&mut self, step: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + step).rem_euclid(fields.len() as isize) as usize];
    }

    pub fn text_mut(&mut self, field: ExportField) -> Option<&mut TextField> {
        match field {
            ExportField::Path => Some(&mut self.path),
            ExportField::Table => Some(&mut self.table),
            ExportField::Schema => Some(&mut self.schema),
            _ => None,
        }
    }

    pub fn text(&self, field: ExportField) -> Option<&TextField> {
        match field {
            ExportField::Path => Some(&self.path),
            ExportField::Table => Some(&self.table),
            ExportField::Schema => Some(&self.schema),
            _ => None,
        }
    }

    /// A key that is not a button: moving between rows, choosing, typing.
    /// Returns whether anything changed on screen.
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
        match self.focus {
            ExportField::Format => {
                let at = ExportFormat::ALL.iter().position(|f| *f == self.format).unwrap_or(0);
                let to = match combo.code {
                    KeyCode::Left | KeyCode::Char('h') if plain => at.saturating_sub(1),
                    KeyCode::Right | KeyCode::Char('l') if plain => (at + 1).min(ExportFormat::ALL.len() - 1),
                    KeyCode::Char(d @ '1'..='5') if plain => d as usize - '1' as usize,
                    _ => return false,
                };
                self.pick_format(to)
            }
            ExportField::Null => {
                let to = match combo.code {
                    KeyCode::Left | KeyCode::Char('h') if plain => false,
                    KeyCode::Right | KeyCode::Char('l') if plain => true,
                    KeyCode::Char(' ') if plain => !self.null_empty,
                    _ => return false,
                };
                self.pick_null(to)
            }
            field => {
                let Some(text) = self.text_mut(field) else { return false };
                match text.handle_key(key) {
                    Edit::Ignored => false,
                    Edit::Moved => true,
                    Edit::Changed => {
                        if field == ExportField::Path {
                            self.path_edited = true;
                        }
                        self.edited();
                        true
                    }
                }
            }
        }
    }

    fn step_focus(&mut self, step: isize) -> bool {
        self.move_focus(step);
        true
    }

    pub fn pick_format(&mut self, index: usize) -> bool {
        let Some(format) = ExportFormat::ALL.get(index).copied() else { return false };
        if self.locked() || format == self.format {
            return false;
        }
        self.set_format(format);
        self.edited();
        true
    }

    pub fn pick_null(&mut self, empty: bool) -> bool {
        if self.locked() || self.null_empty == empty {
            return false;
        }
        self.null_empty = empty;
        self.edited();
        true
    }

    /// Text pasted into the focused row.
    pub fn paste(&mut self, text: &str) -> bool {
        if self.locked() {
            return false;
        }
        let focus = self.focus;
        let Some(field) = self.text_mut(focus) else { return false };
        field.insert_str(text);
        if focus == ExportField::Path {
            self.path_edited = true;
        }
        self.edited();
        true
    }

    /// A click on a row: focus it and put the cursor at `column`.
    pub fn click(&mut self, field: ExportField, column: u16) -> bool {
        if self.locked() || !self.fields().contains(&field) {
            return false;
        }
        self.focus = field;
        if let Some(text) = self.text_mut(field) {
            text.click(column);
        }
        true
    }

    /// Whether the result is still the one the overlay described.
    fn describes(&self, set: &ResultSet) -> bool {
        self.held == Held::of(set)
    }

    /// How many rows go in, and what the cap did to them.
    pub fn rows_line(&self, load_more: Option<&str>) -> String {
        if self.truncated {
            let more = load_more.map(|k| format!(" ({k})")).unwrap_or_default();
            format!(
                "Writes the first {}: the result was cut at the row cap, so load more{more} or run a narrower query to get the rest.",
                rows_word(self.rows)
            )
        } else {
            format!("Writes the {} held.", rows_word(self.rows))
        }
    }

    /// What the format does to values, one sentence each.
    pub fn losses(&self) -> Vec<String> {
        match self.format {
            ExportFormat::Csv => vec![
                if self.null_empty {
                    "NULL is an empty field, and a reader may take it for ''. An empty string is written as \"\".".to_string()
                } else {
                    "NULL is written as \\N, an empty string as \"\".".to_string()
                },
                "a value starting with = + - or @ gets a ' in front, so a spreadsheet will not run it; the file then holds the value with that ' (numbers are left alone).".to_string(),
            ],
            ExportFormat::Json => vec![
                "Loses nothing: NULL is null, integers that fit 64 bits are numbers, everything else is the exact text.".to_string(),
            ],
            ExportFormat::Excel => {
                let mut lines = vec![
                    "NULL and '' both show as an empty cell (the file tells them apart: NULL has no value, '' is empty text).".to_string(),
                    "dates and times become Excel dates, not the text held; numbers up to 15 digits become numbers, longer ones stay text.".to_string(),
                    "no formula guard: a value starting with = is stored as text, not run.".to_string(),
                ];
                lines.push(match self.long_cells {
                    0 => format!("a value over {}, Excel's cell limit, is written whole, and Excel may cut it.", thousands(EXCEL_CELL_LIMIT) + " characters"),
                    n => format!(
                        "{n} {} here {} over {} characters, Excel's cell limit: {} written whole, and Excel may cut {}.",
                        if n == 1 { "value" } else { "values" },
                        if n == 1 { "is" } else { "are" },
                        thousands(EXCEL_CELL_LIMIT),
                        if n == 1 { "it is" } else { "they are" },
                        if n == 1 { "it" } else { "them" },
                    ),
                });
                lines
            }
            ExportFormat::Markdown => vec![
                "NULL looks like the text NULL; line breaks become <br>; | becomes \\|; trailing spaces are padding.".to_string(),
            ],
            ExportFormat::SqlInsert => {
                let mut lines = vec![
                    "only INSERT statements are written: the table is not created, so it must exist with columns of these names.".to_string(),
                    "Each value is written as the text held, and the server converts it.".to_string(),
                ];
                if !self.skipped.is_empty() {
                    lines.push(format!(
                        "{} left out of the INSERT: {} (dbx does not write {}), so the file holds no value for {}.",
                        if self.skipped.len() == 1 { "1 column is" } else { "columns are" },
                        self.skipped.join(", "),
                        if self.skipped.len() == 1 { "it" } else { "them" },
                        if self.skipped.len() == 1 { "it" } else { "them" },
                    ));
                }
                lines
            }
        }
    }
}

/// The copy-as list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyAsOverlay {
    pub selected: usize,
    pub rows: usize,
    pub truncated: bool,
}

impl App {
    /// The table a browse tab shows, as the default name of a file and of the
    /// SQL table, and its schema.
    fn export_names(&self) -> (Option<String>, String) {
        match &self.tab().browse {
            Some(b) => (Some(b.table.table.clone()), b.table.schema.schema.clone()),
            None => (None, String::new()),
        }
    }

    /// `e` in the results pane.
    pub(super) fn open_export(&mut self, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (name, schema) = self.export_names();
        let serial = self.ui.next_export;
        let mut overlay = ExportOverlay::new(self.ui.tab.id, serial, set, name.as_deref(), &schema, &self.ui.export_env);
        let probe = ExportRequest {
            format: ExportFormat::SqlInsert,
            columns: set.columns.clone(),
            rows: set.rows.iter().take(1).cloned().collect(),
            kind: self.active_kind(),
            table: String::new(),
            schema: String::new(),
            null_empty: false,
        };
        overlay.skipped = self.ui.syntax.export_skipped_columns(&probe);
        self.ui.next_export += 1;
        self.ui.overlay = Some(Overlay::Export(Box::new(overlay)));
        fx.repaint = true;
    }

    /// `C` in the results pane.
    pub(super) fn open_copy_as(&mut self, fx: &mut Effects) {
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (rows, truncated) = (set.rows.len(), set.truncated);
        self.ui.overlay = Some(Overlay::CopyAs(CopyAsOverlay { selected: 0, rows, truncated }));
        fx.repaint = true;
    }

    fn export_overlay(&mut self) -> Option<&mut ExportOverlay> {
        match &mut self.ui.overlay {
            Some(Overlay::Export(o)) => Some(o),
            _ => None,
        }
    }

    /// A key in the export overlay that is not a button.
    pub(super) fn on_export_key(&mut self, key: &KeyEvent, fx: &mut Effects) {
        if let Some(o) = self.export_overlay() {
            fx.repaint |= o.on_key(key);
        }
    }

    pub(super) fn on_export_paste(&mut self, text: &str, fx: &mut Effects) {
        if let Some(o) = self.export_overlay() {
            fx.repaint |= o.paste(text);
        }
    }

    /// A click on a row of the export overlay.
    pub(super) fn on_export_click(&mut self, field: ExportField, column: u16, fx: &mut Effects) {
        if let Some(o) = self.export_overlay() {
            fx.repaint |= o.click(field, column);
        }
    }

    /// What a button, or a key that is one, does in the export overlay.
    pub(super) fn export_action(&mut self, action: super::overlay::OverlayAction, fx: &mut Effects) {
        use super::overlay::OverlayAction as A;
        match action {
            A::ExportRun => self.begin_export(false, fx),
            A::ExportReplace => self.begin_export(true, fx),
            A::ExportKeep => {
                if let Some(o) = self.export_overlay() {
                    o.stage = Stage::Editing;
                }
                fx.repaint = true;
            }
            A::ExportFormat(i) => {
                if let Some(o) = self.export_overlay() {
                    fx.repaint |= o.pick_format(i);
                }
            }
            A::ExportNull(empty) => {
                if let Some(o) = self.export_overlay() {
                    fx.repaint |= o.pick_null(empty);
                }
            }
            A::ExportFocus(field) => self.on_export_click(field, 0, fx),
            _ => {}
        }
    }

    /// Sends the export, unless something is wrong with it, which the overlay
    /// then says. `replace`: the question about the file that is there was
    /// answered yes.
    fn begin_export(&mut self, replace: bool, fx: &mut Effects) {
        fx.repaint = true;
        let key = self.ui.keys.key_for(Action::Export).unwrap_or_else(|| "e".into());
        let Some(o) = self.export_overlay() else { return };
        if matches!(o.stage, Stage::Writing(_)) || (matches!(o.stage, Stage::Replace(_)) && !replace) {
            return;
        }
        let (tab, serial, format, null_empty) = (o.tab, o.serial, o.format, o.null_empty);
        let path = o.path.text().to_string();
        let (table, schema) = (o.table.text().trim().to_string(), o.schema.text().trim().to_string());
        let described = o.clone();
        let refuse = |o: &mut ExportOverlay, why: &str, focus: ExportField| {
            o.stage = Stage::Failed(why.to_string());
            o.focus = focus;
        };
        if path.trim().is_empty() {
            if let Some(o) = self.export_overlay() {
                refuse(o, "Give the file a name: the path is empty.", ExportField::Path);
            }
            return;
        }
        if format == ExportFormat::SqlInsert && table.is_empty() {
            if let Some(o) = self.export_overlay() {
                refuse(o, "Give the table a name: the INSERT statements need one.", ExportField::Table);
            }
            return;
        }
        let kind = self.active_kind();
        let request = match self.model.tab(tab).and_then(|t| shown(&t.outcome)) {
            Some(set) if described.describes(set) => {
                ExportRequest { format, columns: set.columns.clone(), rows: set.rows.clone(), kind, table, schema, null_empty }
            }
            _ => {
                self.ui.overlay = None;
                self.notify(format!("The result changed while the export was open: press {key} again to export what is shown now."), true, fx);
                return;
            }
        };
        let id = self.model.requests.begin(Target::Export(serial));
        if let Some(o) = self.export_overlay() {
            o.stage = Stage::Writing(id);
        }
        fx.commands.push(Command::Export(id, ExportJob { request, path, replace }));
    }

    /// The shell's answer to an export.
    pub(super) fn on_exported(&mut self, serial: u64, result: Result<Exported, ExportError>, fx: &mut Effects) {
        self.model.requests.forget(&Target::Export(serial));
        let open = matches!(&self.ui.overlay, Some(Overlay::Export(o)) if o.serial == serial);
        match result {
            Ok(done) => {
                if open {
                    self.ui.overlay = None;
                }
                let text = format!("Exported {} to {} ({})", rows_word(done.rows), done.path, size_text(done.bytes as usize));
                self.notify(text, false, fx);
            }
            Err(e) if e.exists => {
                if let (true, Some(o)) = (open, self.export_overlay()) {
                    let path = o.path.text().to_string();
                    o.stage = Stage::Replace(path);
                    fx.repaint = true;
                } else {
                    self.notify(format!("Not written: {} Nothing was replaced.", e.message), true, fx);
                }
            }
            Err(e) => {
                if let (true, Some(o)) = (open, self.export_overlay()) {
                    o.stage = Stage::Failed(e.message);
                    fx.repaint = true;
                } else {
                    self.notify(e.message, true, fx);
                }
            }
        }
    }

    // ---- copy as -------------------------------------------------------------------------

    /// A key in the copy-as list. Returns whether it took the key.
    pub(super) fn on_copy_as_key(&mut self, combo: &KeyCombo, fx: &mut Effects) -> bool {
        let plain = combo.mods.is_empty();
        let name = combo.to_string();
        let key = |names: &[&str]| names.contains(&name.as_str());
        if let KeyCode::Char(d @ '1'..='4') = combo.code {
            if plain {
                self.copy_as(d as usize - '1' as usize, fx);
                return true;
            }
        }
        let Some(Overlay::CopyAs(c)) = &mut self.ui.overlay else { return false };
        let last = ExportFormat::COPYABLE.len() - 1;
        if key(&["down", "j", "ctrl+n"]) {
            c.selected = (c.selected + 1).min(last);
        } else if key(&["up", "k", "ctrl+p"]) {
            c.selected = c.selected.saturating_sub(1);
        } else {
            return false;
        }
        fx.repaint = true;
        true
    }

    /// A button of the copy-as list.
    pub(super) fn copy_as_action(&mut self, action: super::overlay::OverlayAction, fx: &mut Effects) {
        use super::overlay::OverlayAction as A;
        match (action, &self.ui.overlay) {
            (A::CopyAsRun, Some(Overlay::CopyAs(c))) => {
                let at = c.selected;
                self.copy_as(at, fx)
            }
            (A::CopyAsPick(i), _) => self.copy_as(i, fx),
            _ => {}
        }
    }

    /// Copies the result as the `index`th text format: refused over the
    /// clipboard's limit, which is told before the rows are formatted when
    /// the values alone are over it.
    fn copy_as(&mut self, index: usize, fx: &mut Effects) {
        let Some(format) = ExportFormat::COPYABLE.get(index).copied() else { return };
        self.ui.overlay = None;
        fx.repaint = true;
        let Some(set) = shown(&self.model.active(self.ui.tab.id).outcome) else { return };
        let (rows, truncated, cells) = (set.rows.len(), set.truncated, set.rows.len() * set.columns.len());
        // Every format writes at least the text of a value and a byte for the
        // cell: below that count nothing can fit, so nothing is built.
        let text_bytes: usize = set
            .rows
            .iter()
            .flat_map(|r| r.iter().take(set.columns.len()))
            .map(|c| if let Cell::Value(v) = c { v.len() } else { 0 })
            .sum();
        let export_key = self.ui.keys.key_for(Action::Export).unwrap_or_else(|| "e".into());
        let refuse = |app: &mut App, size: usize, at_least: &str, fx: &mut Effects| {
            let text = format!(
                "Not copied: the {} is {at_least}{}, too large for the terminal clipboard (limit {}): export it to a file ({export_key}).",
                format.label(),
                size_text(size),
                size_text(CLIPBOARD_LIMIT)
            );
            app.notify(text, true, fx);
        };
        if text_bytes + cells > CLIPBOARD_LIMIT {
            return refuse(self, text_bytes + cells, "at least ", fx);
        }
        let (name, schema) = self.export_names();
        let request = ExportRequest {
            format,
            columns: set.columns.clone(),
            rows: set.rows.clone(),
            kind: self.active_kind(),
            table: name.unwrap_or_else(|| "result".into()),
            schema,
            null_empty: false,
        };
        let text = match self.ui.syntax.export_bytes(&request) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => return self.notify(format!("Not copied: {e}"), true, fx),
        };
        if text.len() > CLIPBOARD_LIMIT {
            return refuse(self, text.len(), "", fx);
        }
        let held = if truncated { format!("the first {}, the result was cut at the row cap", rows_word(rows)) } else { rows_word(rows) };
        self.copy_text(text, &format!("{} ({held})", format.label()), fx);
    }
}
