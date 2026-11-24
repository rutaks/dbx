//! What the editor needs from dbx to draw: the type names of the active
//! connection's kind of database (asked once per kind, and shared by every
//! tab: `SyntaxState`) and, per tab (`TabSyntax`), the range of the statement
//! that would run (asked once per text and caret), with the highlighter that
//! keeps the colours between frames. `prepare` is called by the view on every
//! frame and does work only when something changed.

use std::sync::Arc;

use super::editor::Editor;
use super::highlight::{Highlighted, Highlighter, TypeNames};
use crate::core::{Capabilities, Cursor, DbKind, Dialect, Syntax};

/// Answers nothing: what an `App` has until the shell gives it a Core.
pub struct NoSyntax;

impl Syntax for NoSyntax {
    fn capabilities(&self, _kind: &DbKind) -> Capabilities {
        Capabilities::default()
    }

    fn type_names(&self, _kind: &DbKind) -> Vec<String> {
        Vec::new()
    }

    fn statement_range(&self, _kind: &DbKind, _buffer: &str, _cursor: Cursor) -> Option<(Cursor, Cursor)> {
        None
    }
}

/// What every tab's editor shares: the Core's syntax answers, and the type
/// names of each kind of database (they depend only on the kind).
pub struct SyntaxState {
    handle: Arc<dyn Syntax>,
    types: Vec<(DbKind, TypeNames)>,
}

impl std::fmt::Debug for SyntaxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxState").field("kinds", &self.types.len()).finish_non_exhaustive()
    }
}

impl Default for SyntaxState {
    fn default() -> Self {
        SyntaxState { handle: Arc::new(NoSyntax), types: Vec::new() }
    }
}

impl SyntaxState {
    pub fn set_handle(&mut self, handle: Arc<dyn Syntax>) {
        *self = SyntaxState { handle, types: Vec::new() };
    }

    /// What a kind of database can do (the Core's answer, no I/O).
    pub fn capabilities(&self, kind: &DbKind) -> Capabilities {
        self.handle.capabilities(kind)
    }

    /// Where the statement `cursor` is in lies in `buffer`, asked of the Core
    /// now (not cached: the editing actions ask when a key is pressed).
    pub fn statement_at(&self, kind: &DbKind, buffer: &str, cursor: Cursor) -> Option<(Cursor, Cursor)> {
        self.handle.statement_range(kind, buffer, cursor)
    }

    /// The type names of `kind`, asked of the Core the first time.
    fn types(&mut self, kind: Option<&DbKind>) -> &TypeNames {
        static NONE: std::sync::OnceLock<TypeNames> = std::sync::OnceLock::new();
        let Some(kind) = kind else { return NONE.get_or_init(TypeNames::default) };
        if !self.types.iter().any(|(k, _)| k == kind) {
            self.types.push((kind.clone(), TypeNames::new(self.handle.type_names(kind))));
        }
        self.types.iter().find(|(t, _)| t == kind).map(|(_, t)| t).expect("added above")
    }
}

struct RangeEntry {
    revision: u64,
    cursor: Cursor,
    kind: DbKind,
    range: Option<(Cursor, Cursor)>,
}

/// One tab's highlighter, which keeps the colours of its text between
/// frames, and the range of its statement that would run.
#[derive(Default)]
pub struct TabSyntax {
    highlighter: Highlighter,
    range: Option<RangeEntry>,
    /// Graphemes the editor walked to paint its last frame (work, for tests).
    pub graphemes_painted: usize,
    /// Lines of text the editor walked to paint its last frame (work, for
    /// tests).
    pub lines_painted: usize,
}

impl std::fmt::Debug for TabSyntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabSyntax").field("highlighter", &self.highlighter).finish_non_exhaustive()
    }
}

impl TabSyntax {
    pub fn stats(&self) -> super::highlight::Stats {
        self.highlighter.stats
    }

    /// The colours as of the last `prepare`.
    pub fn colours(&self) -> &Highlighted {
        self.highlighter.current()
    }

    /// Brings the colours up to date, and nothing else: what completion needs
    /// to know which characters are a string or a comment. The same cache as
    /// `prepare`, so the next frame tokenizes nothing more.
    pub fn refresh(&mut self, shared: &mut SyntaxState, editor: &Editor, kind: Option<&DbKind>, dialect: Option<Dialect>) {
        let types = shared.types(kind);
        self.highlighter.colours(editor.revision(), editor.lines(), dialect, types);
    }

    /// Brings the colours up to date and returns the
    /// range of the statement a run would send, if one can be placed. With a
    /// selection the run sends the selection, which is outlined as a
    /// selection already: no statement range then.
    pub fn prepare(&mut self, shared: &mut SyntaxState, editor: &Editor, kind: Option<&DbKind>, dialect: Option<Dialect>) -> Option<(Cursor, Cursor)> {
        self.refresh(shared, editor, kind, dialect);

        let kind = kind?;
        if editor.selection().is_some() {
            return None;
        }
        let (revision, cursor) = (editor.revision(), editor.cursor());
        if let Some(e) = &self.range {
            if e.revision == revision && e.cursor == cursor && &e.kind == kind {
                return e.range;
            }
        }
        let range = shared.statement_at(kind, &editor.text(), cursor);
        self.range = Some(RangeEntry { revision, cursor, kind: kind.clone(), range });
        range
    }
}
