//! What the editor needs from dbx to draw: the type names of the active
//! connection's kind of database (asked once per kind) and the range of the
//! statement that would run (asked once per text and caret), with the
//! highlighter that keeps the colours between frames. `prepare` is called by
//! the view on every frame and does work only when something changed.

use std::sync::Arc;

use super::editor::Editor;
use super::highlight::{Highlighted, Highlighter, TypeNames};
use crate::core::{Cursor, DbKind, Syntax};

/// Answers nothing: what an `App` has until the shell gives it a Core.
pub struct NoSyntax;

impl Syntax for NoSyntax {
    fn type_names(&self, _kind: &DbKind) -> Vec<String> {
        Vec::new()
    }

    fn statement_range(&self, _kind: &DbKind, _buffer: &str, _cursor: Cursor) -> Option<(Cursor, Cursor)> {
        None
    }
}

struct RangeEntry {
    revision: u64,
    cursor: Cursor,
    kind: DbKind,
    range: Option<(Cursor, Cursor)>,
}

pub struct SyntaxState {
    handle: Arc<dyn Syntax>,
    highlighter: Highlighter,
    types: Vec<(DbKind, TypeNames)>,
    range: Option<RangeEntry>,
    /// Graphemes the editor walked to paint its last frame (work, for tests).
    pub graphemes_painted: usize,
}

impl std::fmt::Debug for SyntaxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxState").field("highlighter", &self.highlighter).finish_non_exhaustive()
    }
}

impl Default for SyntaxState {
    fn default() -> Self {
        SyntaxState { handle: Arc::new(NoSyntax), highlighter: Highlighter::default(), types: Vec::new(), range: None, graphemes_painted: 0 }
    }
}

impl SyntaxState {
    pub fn set_handle(&mut self, handle: Arc<dyn Syntax>) {
        *self = SyntaxState { handle, ..SyntaxState::default() };
    }

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
    pub fn refresh(&mut self, editor: &Editor, kind: Option<&DbKind>) {
        if let Some(kind) = kind {
            if !self.types.iter().any(|(k, _)| k == kind) {
                self.types.push((kind.clone(), TypeNames::new(self.handle.type_names(kind))));
            }
        }
        let none = TypeNames::default();
        let types = kind.and_then(|k| self.types.iter().find(|(t, _)| t == k)).map_or(&none, |(_, t)| t);
        self.highlighter.colours(editor.revision(), editor.lines(), kind, types);
    }

    /// Brings the colours up to date and returns the
    /// range of the statement a run would send, if one can be placed. With a
    /// selection the run sends the selection, which is outlined as a
    /// selection already: no statement range then.
    pub fn prepare(&mut self, editor: &Editor, kind: Option<&DbKind>) -> Option<(Cursor, Cursor)> {
        self.refresh(editor, kind);

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
        let range = self.handle.statement_range(kind, &editor.text(), cursor);
        self.range = Some(RangeEntry { revision, cursor, kind: kind.clone(), range });
        range
    }
}
