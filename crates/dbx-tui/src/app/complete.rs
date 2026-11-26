//! Completion in the editor: the popup that opens while typing an identifier
//! (and on a key), what it lists, and how a suggestion gets into the text.
//!
//! Three pure pieces do the thinking, each tested alone: `complete_context`
//! (what is being completed, from the tokens), `complete_match` (which names
//! match and in what order) and `complete_popup` (the list's selection and
//! window). This module joins them to the editor and to the Core:
//!
//! - **Sources, in order.** The keywords the position allows; then names the
//!   sidebar's tree already holds (it has them loaded when the user opened that
//!   node); then names the app has asked the Core for and kept
//!   (`CompletionCache`, per connection, database and schema, dropped when
//!   that part of the tree is refreshed); and for what is in neither, a Core
//!   call that is made now and filled in when it answers.
//! - **Typing never waits.** The popup shows what it has: a reply that comes
//!   later refreshes it only when the caret is still on the word it was asked
//!   about, and otherwise lands in the cache and changes nothing on screen.
//! - **By kind.** Postgres has the full set; any other kind gets keywords.

use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::complete_context::{analyse, Context, Keywords, TableRef};
use super::complete_insert::{keyword_text, quote_identifier};
use super::complete_match::{match_name, rank_key};
use super::complete_popup::{Item, ItemKind, Popup};
use super::complete_scope::{generate_alias, join_conditions, NamedColumn, Side, Source, SourceKind, Star, TableName};
use super::highlight::KEYWORDS;
use super::keys::{Action, KeyCombo};
use super::requests::{RequestId, Target};
use super::theme::Role;
use super::tree::{Listing, NodeId};
use super::{App, Command, CoreCall, CoreReply, Effects, PaneId};
use crate::core::TabId;
use crate::core::{
    display_safe, Candidate, CandidateKind, ColumnInfo, CompletionRequest, CoreErrorKind, Cursor, DatabasePath, ForeignKey, References, SchemaPath,
    SearchKind, TableKind, TablePath,
};

/// One thing the popup needs from the Core.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Need {
    CurrentSchema(DatabasePath),
    Schemas(DatabasePath),
    /// Tables and views of a schema.
    Relations(SchemaPath),
    Functions(SchemaPath),
    Columns(TablePath),
    /// The foreign keys of a table, for the conditions after `join x on`.
    ForeignKeys(TablePath),
    /// Tables and views of every schema of the database, to find a name that
    /// exists in more than one.
    AllRelations(DatabasePath),
    /// What dbx's analysis says the statement with this index in the buffer
    /// refers to (the text asked about is the mask): asked for by the semantic
    /// colours once the text has been still, and read by completion.
    Parse(DatabasePath, usize),
}

impl Need {
    fn database(&self) -> &DatabasePath {
        match self {
            Need::CurrentSchema(d) | Need::Schemas(d) | Need::AllRelations(d) | Need::Parse(d, _) => d,
            Need::Relations(s) | Need::Functions(s) => &s.database,
            Need::Columns(t) | Need::ForeignKeys(t) => &t.schema.database,
        }
    }

    /// What this is a need for, for a message.
    fn describe(&self) -> String {
        match self {
            Need::CurrentSchema(d) | Need::Schemas(d) | Need::AllRelations(d) | Need::Parse(d, _) => format!("database {}", d.database),
            Need::Relations(s) | Need::Functions(s) => format!("schema {}", s.schema),
            Need::Columns(t) | Need::ForeignKeys(t) => format!("table {}.{}", t.schema.schema, t.table),
        }
    }

    /// Whether the refresh of `node` (a part of the tree) makes this stale.
    fn is_under(&self, node: &NodeId) -> bool {
        match node {
            NodeId::Connection(c) => &self.database().connection == c,
            NodeId::Database(d) => self.database() == d,
            NodeId::Schema(s) => match self {
                Need::Relations(x) | Need::Functions(x) => x == s,
                Need::Columns(t) | Need::ForeignKeys(t) => &t.schema == s,
                Need::AllRelations(d) => d == &s.database,
                _ => false,
            },
            NodeId::Table(t) => matches!(self, Need::Columns(x) | Need::ForeignKeys(x) if x == t),
        }
    }
}

/// What a prefix search found, and what it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Searched {
    /// The prefix searched: everything is here that starts with it.
    pub prefix: String,
    pub found: Vec<Candidate>,
    /// The search stopped at its limit: there may be more.
    pub incomplete: bool,
}

impl Searched {
    /// Whether what a search for `word` wants starts with what was searched.
    fn covers(&self, word: &str) -> bool {
        word.to_lowercase().starts_with(&self.prefix.to_lowercase())
    }

    /// Whether this is everything there is for `word`.
    fn is_all_for(&self, word: &str) -> bool {
        self.covers(word) && !self.incomplete
    }
}

/// What the app has asked the Core for completion and kept. Per connection,
/// database and schema, so it is dropped with that part of the tree.
#[derive(Debug, Default)]
pub struct CompletionCache {
    pub(super) current_schema: HashMap<DatabasePath, String>,
    schemas: HashMap<DatabasePath, Searched>,
    pub(super) relations: HashMap<SchemaPath, Searched>,
    functions: HashMap<SchemaPath, Searched>,
    pub(super) columns: HashMap<TablePath, Vec<ColumnInfo>>,
    pub(super) foreign_keys: HashMap<TablePath, Vec<ForeignKey>>,
    all_relations: HashMap<DatabasePath, Searched>,
    /// What dbx's analysis made of each statement text asked about, for the
    /// database it was asked on: `None` inside is "does not parse".
    pub(super) parsed: HashMap<(DatabasePath, String), Option<References>>,
    /// Changes whenever something is stored, dropped or fails: what the
    /// semantic colours key by.
    pub(super) version: u64,
    /// Changes whenever what the semantic colours read of the schema does: the
    /// current schema, the relations and the columns (stored, failed or
    /// dropped). What a statement's marks are kept by.
    pub(super) schema_version: u64,
    /// Asked, and the Core said no: not asked again until that part of the
    /// tree is refreshed.
    pub(super) failed: HashSet<Need>,
    /// In flight, by the tab that asked, with the prefix asked.
    pub(super) inflight: HashMap<(TabId, Need), (RequestId, String)>,
}

impl CompletionCache {
    /// The schema unqualified names resolve in, once known.
    pub fn current_schema(&self, at: &DatabasePath) -> Option<&str> {
        self.current_schema.get(at).map(String::as_str)
    }

    /// How many things are kept (for tests).
    pub fn len(&self) -> usize {
        self.current_schema.len()
            + self.schemas.len()
            + self.relations.len()
            + self.functions.len()
            + self.columns.len()
            + self.foreign_keys.len()
            + self.all_relations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops what the refresh of `node` makes stale and returns the requests
    /// that were in flight for it.
    fn invalidate(&mut self, node: &NodeId) -> Vec<RequestId> {
        self.version += 1;
        self.schema_version += 1;
        let under = |d: &DatabasePath| match node {
            NodeId::Connection(c) => &d.connection == c,
            NodeId::Database(x) => d == x,
            _ => false,
        };
        self.current_schema.retain(|d, _| !under(d));
        self.schemas.retain(|d, _| !under(d));
        match node {
            NodeId::Schema(s) => {
                self.relations.remove(s);
                self.functions.remove(s);
                self.columns.retain(|t, _| &t.schema != s);
                self.foreign_keys.retain(|t, _| &t.schema != s);
                self.all_relations.remove(&s.database);
            }
            NodeId::Table(t) => {
                self.columns.remove(t);
                self.foreign_keys.remove(t);
            }
            _ => {
                self.relations.retain(|s, _| !under(&s.database));
                self.functions.retain(|s, _| !under(&s.database));
                self.columns.retain(|t, _| !under(&t.schema.database));
                self.foreign_keys.retain(|t, _| !under(&t.schema.database));
                self.all_relations.retain(|d, _| !under(d));
            }
        }
        self.failed.retain(|n| !n.is_under(node));
        let gone: Vec<(TabId, Need)> = self.inflight.keys().filter(|(_, n)| n.is_under(node)).cloned().collect();
        gone.iter().filter_map(|n| self.inflight.remove(n)).map(|(id, _)| id).collect()
    }
}

/// A completion that is open, or waiting for the Core to give it something to
/// show: where the word it was opened on starts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    anchor: Cursor,
    /// The word as it was when the list was last made.
    word: String,
    /// Opened by the key: it stays open with nothing typed yet.
    manual: bool,
}

/// The popup, as the UI holds it.
#[derive(Debug, Default)]
pub struct CompleteUi {
    pub popup: Popup,
    session: Option<Session>,
    /// An expansion of `*` waiting for the columns it needs: the text's
    /// revision and the caret it was asked at.
    pending_star: Option<(u64, Cursor)>,
}

impl CompleteUi {
    /// Whether the popup is on screen.
    pub fn visible(&self) -> bool {
        self.session.is_some() && !self.popup.is_empty()
    }

    /// Where the word the completion is on starts.
    pub fn anchor(&self) -> Option<Cursor> {
        self.session.as_ref().map(|s| s.anchor)
    }

    /// Forgets the completion and its list, without saying anything to the
    /// screen (the view uses it mid-frame).
    pub(super) fn reset(&mut self) {
        self.session = None;
        self.popup.set_items(Vec::new(), false);
    }

    /// Forgets a `*` expansion waiting for columns (its tab was left).
    pub(super) fn forget_pending(&mut self) {
        self.pending_star = None;
    }

    /// Whether a completion is open or on its way (it may have nothing to
    /// show yet).
    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }
}

/// How a refresh may start a completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// A key moved the caret or changed the text: keep an open one up to date.
    Keep,
    /// The Core answered: the same, and the selection stays where it was.
    Fill,
    /// A typed character: opens if a word of at least one character is being
    /// typed, or a name was just followed by a dot.
    Typed,
    /// The key: opens whatever the word.
    Manual,
}

/// The longest word that is completed, in bytes.
const MAX_WORD: usize = 128;

/// The most suggestions kept.
const MAX_ITEMS: usize = 500;

/// The most names a search for a whole schema's tables asks for, and for
/// functions (which are many, and only asked for by what is typed).
const RELATION_LIMIT: usize = 1000;
const FUNCTION_LIMIT: usize = 300;
/// The fewest characters a function search is made for.
const MIN_FUNCTION_WORD: usize = 2;

/// Functions Postgres has that dbx's function search does not return, or that
/// are syntax and not in the catalogue at all: the aggregates and window
/// functions (`pg_proc.prokind` is `a` or `w`, and the search asks for `f` and
/// `p`) and the conditional expressions. They are what people type most.
const BUILTIN_FUNCTIONS: &[&str] = &[
    "array_agg", "avg", "bool_and", "bool_or", "coalesce", "count", "cume_dist", "dense_rank", "every", "first_value", "greatest",
    "json_agg", "jsonb_agg", "lag", "last_value", "lead", "least", "max", "min", "nth_value", "nullif", "ntile", "percent_rank",
    "rank", "row_number", "string_agg", "sum",
];

/// A suggestion before it is ranked.
struct Cand {
    name: String,
    kind: ItemKind,
    detail: Option<String>,
    /// The first letter's case matters (names; not keywords).
    strict: bool,
    /// Which source it came from, for the order of equals and of the list
    /// before anything is typed: columns, outer columns, tables, schemas,
    /// functions, keywords.
    group: u8,
    /// What accepting it writes, when that is not the name (a column that
    /// two tables have is written with its alias); already quoted.
    insert: Option<String>,
    /// Shown beside the name: the alias of the table a column is from.
    owner: Option<String>,
    /// The schema of a table or view: what a name in two schemas is told by.
    schema: Option<String>,
    /// Listed before everything else that matches (a whole join condition,
    /// the select list's own expressions).
    top: bool,
}

impl Cand {
    fn new(name: String, kind: ItemKind, detail: Option<String>, strict: bool, group: u8) -> Self {
        Cand { name, kind, detail, strict, group, insert: None, owner: None, schema: None, top: false }
    }
}

fn table_item_kind(kind: TableKind) -> ItemKind {
    match kind {
        TableKind::Table => ItemKind::Table,
        TableKind::View => ItemKind::View,
        TableKind::MaterializedView => ItemKind::MatView,
        TableKind::Foreign => ItemKind::Foreign,
    }
}

fn candidate_item_kind(kind: CandidateKind) -> ItemKind {
    match kind {
        CandidateKind::Schema => ItemKind::Schema,
        CandidateKind::Table => ItemKind::Table,
        CandidateKind::View => ItemKind::View,
        CandidateKind::MaterializedView => ItemKind::MatView,
        CandidateKind::Function => ItemKind::Function,
        CandidateKind::Procedure => ItemKind::Procedure,
    }
}

/// A character that can be part of a name being typed.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// What an acceptance replaces: the word from its start to the caret, or to
/// the end of the word when `replace` is set, and for a quoted name that has
/// its closing quote, through that quote.
fn accept_range(ctx: &Context, caret: Cursor, replace: bool) -> (Cursor, Cursor) {
    let through = replace || (ctx.quoted && ctx.quote_closed);
    (ctx.word_start, if through { ctx.word_end } else { caret })
}

impl App {
    /// Whether the popup is on screen.
    pub(super) fn completion_visible(&self) -> bool {
        self.ui.tab.complete.visible() && self.ui.focus == PaneId::Editor && self.ui.overlay.is_none()
    }

    /// Closes the popup, and forgets what it was waiting for.
    pub(super) fn close_completion(&mut self, fx: &mut Effects) {
        let was = self.ui.tab.complete.session.is_some() || !self.ui.tab.complete.popup.is_empty();
        self.ui.tab.complete.reset();
        fx.repaint |= was;
    }

    /// A key given to the focused editor: the popup's keys first, when it is
    /// open; then the editor's own, after which the list follows the text.
    pub(super) fn on_editor_key(&mut self, key: KeyEvent, combo: KeyCombo, fx: &mut Effects) {
        if self.ui.tab.template.active() && self.template_key(&combo, fx) {
            return;
        }
        // The template expansion key (`tab`, by default; `enter` is only the
        // popup's, when a template is highlighted) where a postfix template or
        // an abbreviation stands.
        if combo.code != KeyCode::Enter
            && self.ui.keys.lookup_complete(&combo, true) == Some(Action::ExpandTemplate)
            && self.tab_expands(fx)
        {
            return;
        }
        if self.completion_visible() {
            let template = self.ui.tab.complete.popup.selected_item().is_some_and(|i| i.kind == ItemKind::Template);
            if let Some(action) = self.ui.keys.lookup_complete(&combo, template) {
                // `enter` after a word that is already typed in full is the
                // editor's: a line break, not text the user did not type.
                if action != Action::CompleteInsert || !self.enter_is_a_line_break(fx) {
                    return self.perform_complete(action, fx);
                }
            }
        }
        // `esc` with a completion on its way but nothing to show yet ends it,
        // and is still the editor's key: it is not swallowed.
        if combo.code == KeyCode::Esc && self.ui.tab.complete.is_open() {
            self.close_completion(fx);
        }
        // While the find or go-to-line bar is open the keys that are text are its.
        if self.bar_open() {
            return self.bar_key(&key, &combo, fx);
        }
        // The editor's own actions: IntelliJ's keys, and the fallbacks.
        if let Some(action) = self.ui.keys.lookup_editor(&combo) {
            let now = self.ui.now.unwrap_or_else(std::time::Instant::now);
            return self.perform_edit(action, now, fx);
        }
        let before = self.ui.tab.editor.revision();
        let template_before = self.template_before();
        // Typed text goes through the smart keys first (pairs, indentation).
        if !self.smart_key(&key, fx) {
            fx.repaint |= self.ui.tab.editor.key(key);
        }
        self.track_template(template_before, fx);
        let edited = self.ui.tab.editor.revision() != before;
        let typed = match key.code {
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => Some(c),
            _ => None,
        };
        if self.ui.tab.complete.is_open() {
            self.refresh_completion(Open::Keep, fx);
        }
        if !self.ui.tab.complete.is_open() && edited && typed.is_some_and(|c| is_word_char(c) || c == '.') {
            self.refresh_completion(Open::Typed, fx);
        }
    }

    /// One of the popup's own actions, or the key that opens it.
    pub(super) fn perform_complete(&mut self, action: Action, fx: &mut Effects) {
        match action {
            Action::CompleteOpen => self.refresh_completion(Open::Manual, fx),
            Action::CompleteInsert | Action::ExpandTemplate => self.accept_completion(false, fx),
            Action::CompleteReplace => self.accept_completion(true, fx),
            Action::CompleteClose => self.close_completion(fx),
            Action::CompleteUp => fx.repaint |= self.ui.tab.complete.popup.up(),
            Action::CompleteDown => fx.repaint |= self.ui.tab.complete.popup.down(),
            Action::CompletePageUp => fx.repaint |= self.ui.tab.complete.popup.page_up(),
            Action::CompletePageDown => fx.repaint |= self.ui.tab.complete.popup.page_down(),
            _ => {}
        }
    }

    /// A press at (`x`, `y`) while the popup is open: on an item it accepts it
    /// (as `enter` does); anywhere else it closes the popup, and the press
    /// goes on to what is under it. Returns whether the press was the popup's.
    pub(super) fn press_completion(&mut self, x: u16, y: u16, fx: &mut Effects) -> bool {
        let rect = self.ui.hits.popup;
        if !self.completion_visible() {
            return false;
        }
        if rect.contains((x, y).into()) {
            let index = self.ui.tab.complete.popup.top() + (y - rect.y) as usize;
            if index < self.ui.tab.complete.popup.len() {
                self.ui.tab.complete.popup.select(index);
                self.accept_completion(false, fx);
            }
            return true;
        }
        self.close_completion(fx);
        false
    }

    /// The wheel over the popup scrolls its list. Returns whether it was over it.
    pub(super) fn wheel_completion(&mut self, x: u16, y: u16, rows: isize, fx: &mut Effects) -> bool {
        if !self.completion_visible() || !self.ui.hits.popup.contains((x, y).into()) {
            return false;
        }
        fx.repaint |= self.ui.tab.complete.popup.scroll(rows);
        true
    }

    /// The completion has nothing to do with what is on screen any more: the
    /// editor lost focus, an overlay or the prefix mode took over.
    pub(super) fn completion_housekeeping(&mut self, fx: &mut Effects) {
        if self.ui.tab.complete.is_open() && (self.ui.focus != PaneId::Editor || self.ui.overlay.is_some() || self.ui.mode != super::Mode::Normal) {
            self.close_completion(fx);
        }
    }

    // ---- what is listed --------------------------------------------------------

    /// Whether the characters just before the caret are drawn as a string or
    /// a comment (the highlighter's roles, from the whole buffer's tokens).
    /// The whole buffer is tokenized (once per revision: the frame's own pass
    /// is the same cache), which typing a word must not do on every key; so
    /// this is asked where it matters: when the popup is drawn, and before a
    /// key puts text into the buffer. The cheap read of the statement round
    /// the caret (`analyse`) is what typing relies on.
    pub(super) fn caret_in_literal(&mut self) -> bool {
        if self.ui.tab.complete.session.as_ref().is_none_or(|s| s.word.is_empty()) {
            return false;
        }
        let (kind, dialect) = (self.active_kind(), self.active_dialect());
        self.ui.tab.syntax.refresh(&mut self.ui.syntax, &self.ui.tab.editor, kind.as_ref(), dialect);
        let caret = self.ui.tab.editor.cursor();
        let Some(before) = caret.col.checked_sub(1) else { return false };
        self.ui
            .tab
            .syntax
            .colours()
            .line(caret.row)
            .and_then(|spans| spans.iter().find(|s| s.start <= before && before < s.end))
            .is_some_and(|s| matches!(s.role, Role::SynString | Role::SynComment))
    }

    /// Whether `enter` is to be the editor's: the suggestion it would insert
    /// is what the word already is (any case for a keyword), and the user has
    /// not moved the selection to it. The popup closes then. Also so when the
    /// caret turns out to be inside a string or a comment.
    fn enter_is_a_line_break(&mut self, fx: &mut Effects) -> bool {
        let in_literal = self.caret_in_literal();
        let typed = !self.ui.tab.complete.popup.touched()
            && self.ui.tab.complete.popup.selected_item().is_some_and(|item| {
                let caret = self.ui.tab.editor.cursor();
                self.typed_in_full(item, &analyse(self.ui.tab.editor.lines(), caret), caret)
            });
        if in_literal || typed {
            self.close_completion(fx);
        }
        in_literal || typed
    }

    /// The text `item` would replace, and what it replaces it with, are the
    /// same: a keyword in any case, a name exactly.
    fn typed_in_full(&self, item: &Item, ctx: &Context, caret: Cursor) -> bool {
        // A template is never typed in full: what is highlighted is expanded.
        if item.kind == ItemKind::Template {
            return false;
        }
        let (from, to) = accept_range(ctx, caret, false);
        if from.row != to.row || from.col > to.col {
            return false;
        }
        let Some(line) = self.ui.tab.editor.lines().get(from.row) else { return false };
        let typed: String = line.chars().skip(from.col).take(to.col - from.col).collect();
        if item.kind == ItemKind::Keyword {
            typed.to_lowercase() == item.insert.to_lowercase()
        } else {
            typed == item.insert
        }
    }

    /// Recomputes the list for the caret as it is now.
    fn refresh_completion(&mut self, open: Open, fx: &mut Effects) {
        let session = self.ui.tab.complete.session.clone();
        if session.is_none() && matches!(open, Open::Keep | Open::Fill) {
            return;
        }
        // The key asks again for what the Core failed to give.
        if open == Open::Manual {
            if let Some(db) = self.tab().target.clone() {
                self.model.completions.failed.retain(|n| n.database() != &db);
            }
        }
        let caret = self.ui.tab.editor.cursor();
        let mut ctx = analyse(self.ui.tab.editor.lines(), caret);
        // No name is that long (Postgres stops at 63 bytes): what is being
        // typed is not one, and matching it against everything would be work
        // for nothing.
        if !ctx.offers() || ctx.word.len() > MAX_WORD {
            return self.close_completion(fx);
        }
        // The caret left the word the completion was on.
        if session.as_ref().is_some_and(|s| s.anchor != ctx.word_start) {
            return self.close_completion(fx);
        }
        let manual = open == Open::Manual || session.as_ref().is_some_and(|s| s.manual);
        // Typed text opens it only once a name is being typed, or a name was
        // just followed by a dot.
        if !manual && ctx.word.is_empty() && ctx.wants.qualified.is_none() {
            return self.close_completion(fx);
        }
        // Right after a table's name the word is its alias, as often as not,
        // and `enter` must not turn it into a keyword: only the key lists
        // what can follow a table there.
        if open == Open::Typed && ctx.alias_position && !manual {
            return self.close_completion(fx);
        }
        self.use_analysis(&mut ctx);
        let (mut items, needs) = self.completion_items(&ctx);
        // A name typed in full that is all there is to offer is not worth a
        // popup: it waits for something else to be listed.
        if !manual && items.len() == 1 && self.typed_in_full(&items[0], &ctx, caret) {
            items.clear();
        }
        let waiting = self.request_completion(needs, fx);
        if items.is_empty() && !waiting {
            return self.close_completion(fx);
        }
        let keep = open == Open::Fill || session.as_ref().is_some_and(|s| s.word == ctx.word);
        let before = (self.ui.tab.complete.session.clone(), self.ui.tab.complete.popup.clone());
        self.ui.tab.complete.session = Some(Session { anchor: ctx.word_start, word: ctx.word.clone(), manual });
        self.ui.tab.complete.popup.set_items(items, keep);
        if before != (self.ui.tab.complete.session.clone(), self.ui.tab.complete.popup.clone()) {
            fx.repaint = true;
        }
    }

    /// Puts the selected suggestion into the text: `enter` at the caret,
    /// `tab` over the rest of the word to its right; a closed quoted name is
    /// replaced through its closing quote either way.
    fn accept_completion(&mut self, replace: bool, fx: &mut Effects) {
        let Some(item) = self.ui.tab.complete.popup.selected_item().cloned() else { return };
        // Not into a string or a comment, which a keystroke may have made of
        // the caret's place before a frame could close the popup.
        if self.caret_in_literal() {
            return self.close_completion(fx);
        }
        let caret = self.ui.tab.editor.cursor();
        let ctx = analyse(self.ui.tab.editor.lines(), caret);
        // A template is expanded: what is highlighted is what you get.
        if item.kind == ItemKind::Template {
            if let (Some(template), true) = (super::template::Template::named(&item.name), ctx.statement_start) {
                return self.expand_template(template, ctx.word_start, ctx.word_end, fx);
            }
            return self.close_completion(fx);
        }
        let (from, to) = accept_range(&ctx, caret, replace);
        if from.row == to.row && from.col <= to.col {
            // An alias goes after a name that ends here, not into the middle of a word.
            let in_template = self.in_table_variable();
            let text = if to == ctx.word_end && !in_template { item.text() } else { item.insert.clone() };
            if let Err(why) = self.ui.tab.editor.replace(from, to, &text) {
                self.notify(format!("Could not insert {}: {why}.", display_safe(&text)), true, fx);
            } else if self.ui.tab.template.active() {
                let is_table = matches!(item.kind, ItemKind::Table | ItemKind::View | ItemKind::MatView | ItemKind::Foreign);
                let table = is_table.then(|| (item.name.clone(), item.owner.clone()));
                self.template_accepted(from.col, to.col, text.chars().count(), table, fx);
            }
        }
        self.close_completion(fx);
        fx.repaint = true;
    }

    /// The suggestions for `ctx`, ranked, and what is still missing to make
    /// them complete.
    fn completion_items(&self, ctx: &Context) -> (Vec<Item>, Vec<(Need, String)>) {
        let dialect = self.active_dialect();
        let word = ctx.word.as_str();
        let scope = &ctx.scope;
        let mut cands: Vec<Cand> = Vec::new();
        let mut needs: Vec<(Need, String)> = Vec::new();

        let keywords: &[&str] = match ctx.wants.keywords {
            Keywords::Any => KEYWORDS,
            Keywords::Only(only) => only,
            Keywords::None => &[],
        };
        cands.extend(keywords.iter().map(|k| Cand::new((*k).to_string(), ItemKind::Keyword, None, false, 5)));

        // Live templates, where a statement starts (nothing but whitespace and
        // comments since the last `;`) and an abbreviation is being typed. The
        // one the word is exactly comes first, highlighted: it is what `enter`
        // and `tab` expand.
        if !word.is_empty() && !ctx.quoted && ctx.wants.qualified.is_none() && ctx.statement_start {
            let lower = word.to_lowercase();
            for t in super::template::TEMPLATES.iter().filter(|t| t.abbr.starts_with(&lower)) {
                let mut cand = Cand::new(t.abbr.to_string(), ItemKind::Template, Some(t.preview()), false, 6);
                cand.insert = Some(t.abbr.to_string());
                cand.top = t.abbr == lower;
                cands.push(cand);
            }
        }

        // Names are the Core's where the connection type completes
        // (`Capabilities::completes`); other kinds get keywords for now.
        if self.core_names() {
            if let Some(db) = self.tab().target.clone() {
                self.metadata_candidates(&db, ctx, &mut cands, &mut needs);
            }
        }

        // A name with a control character in it (a line break) cannot be put
        // into the buffer as one name.
        cands.retain(|c| !c.name.chars().any(char::is_control));
        let mut ranked: Vec<(super::complete_match::Match, usize, Cand)> = cands
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| match_name(word, &c.name, c.strict).map(|m| (m, i, c)))
            .collect();
        // A name more than one schema has is written with its schema.
        let mut schemas_of: HashMap<&str, HashSet<&str>> = HashMap::new();
        for (_, _, c) in &ranked {
            if let (Some(schema), true) = (&c.schema, matches!(c.kind, ItemKind::Table | ItemKind::View | ItemKind::MatView | ItemKind::Foreign)) {
                schemas_of.entry(c.name.as_str()).or_default().insert(schema.as_str());
            }
        }
        let ambiguous: HashSet<String> = schemas_of.into_iter().filter(|(_, s)| s.len() > 1).map(|(n, _)| n.to_string()).collect();
        // The keywords a column of the same tier gives way to, or does not.
        let by_scope = ctx.wants.keywords == Keywords::Any && scope.clause().column_position() || scope.clause() == super::complete_scope::Clause::None;
        let keyword_bucket = |c: &Cand| -> u8 {
            if c.kind != ItemKind::Keyword || !by_scope {
                1
            } else if scope.keyword_is_valid(&c.name) {
                0
            } else {
                2
            }
        };
        if word.is_empty() {
            // Nothing typed: the sources in order, each as it came (columns in
            // the table's order).
            ranked.sort_by_key(|(_, i, c)| (!c.top, c.group, *i));
        } else {
            ranked.sort_by(|a, b| {
                (a.2.kind == ItemKind::Template && !a.2.top, !a.2.top, rank_key(&a.0, &a.2.name).0, keyword_bucket(&a.2))
                    .cmp(&(b.2.kind == ItemKind::Template && !b.2.top, !b.2.top, rank_key(&b.0, &b.2.name).0, keyword_bucket(&b.2)))
                    .then_with(|| rank_key(&a.0, &a.2.name).cmp(&rank_key(&b.0, &b.2.name)))
                    .then(a.2.group.cmp(&b.2.group))
                    .then(a.1.cmp(&b.1))
            });
        }
        let settings = &self.model.settings;
        let mut seen = HashSet::new();
        let items = ranked
            .into_iter()
            .map(|(m, _, mut c)| {
                if ambiguous.contains(&c.name) {
                    if let Some(schema) = &c.schema {
                        c.insert = Some(format!("{}.{}", quote_identifier(schema, dialect), quote_identifier(&c.name, dialect)));
                        c.owner = Some(schema.clone());
                    }
                }
                (m, c)
            })
            .filter(|(_, c)| seen.insert((c.name.clone(), c.kind, c.owner.clone())))
            .take(MAX_ITEMS)
            .map(|(m, c)| {
                let insert = match (&c.insert, c.kind) {
                    (Some(text), _) => text.clone(),
                    (None, ItemKind::Keyword) => keyword_text(word, &c.name),
                    (None, ItemKind::Condition | ItemKind::ColumnList | ItemKind::Select) => c.name.clone(),
                    (None, _) => quote_identifier(&c.name, dialect),
                };
                let is_table = matches!(c.kind, ItemKind::Table | ItemKind::View | ItemKind::MatView | ItemKind::Foreign);
                // The statement already calls the table by its own name before a
                // dot: an alias would make that invalid.
                let named_by_itself = ctx.qualifiers.iter().any(|q| super::complete_scope::same_name(q, &c.name));
                let alias = match (&ctx.alias_taken, is_table && settings.alias_on_table && !named_by_itself) {
                    (Some(taken), true) => Some(quote_identifier(&generate_alias(&c.name, taken), dialect)),
                    _ => None,
                };
                Item { name: c.name, insert, kind: c.kind, detail: c.detail, matched: m.positions, owner: c.owner, alias }
            })
            .collect();
        (items, needs)
    }

    /// The names of a database that fit `ctx`, from the sidebar's tree, the
    /// cache, or (as `needs`) what has to be asked.
    fn metadata_candidates(&self, db: &DatabasePath, ctx: &Context, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let cache = &self.model.completions;
        let word = ctx.word.as_str();
        let wants = &ctx.wants;
        let scope = &ctx.scope;
        // Functions are searched for by what is typed, from two letters on: one
        // letter matches hundreds.
        let search_functions = wants.functions && word.chars().count() >= MIN_FUNCTION_WORD;
        // Unqualified names resolve in the current schema, asked once, and
        // only when something here needs it.
        let wants_schema = wants.qualified.is_some()
            || wants.tables
            || (wants.columns && (!ctx.refs.is_empty() || scope.has_sources()))
            || search_functions;
        let current = match self.current_schema_of(db) {
            Some(schema) => Some(schema),
            None if !wants_schema => None,
            None if cache.failed.contains(&Need::CurrentSchema(db.clone())) => Some("public".to_string()),
            None => {
                needs.push((Need::CurrentSchema(db.clone()), String::new()));
                None
            }
        };
        if let Some(q) = &wants.qualified {
            return self.qualified_candidates(db, &q.parts, ctx, current.as_deref(), out, needs);
        }
        if wants.functions {
            // Aggregates and window functions: not in dbx's search, and known
            // without asking.
            out.extend(BUILTIN_FUNCTIONS.iter().map(|f| Cand::new((*f).to_string(), ItemKind::Function, None, true, 4)));
        }
        if let Some(schema) = &current {
            if wants.tables {
                self.table_candidates(db, schema, ctx, out, needs);
            }
            if wants.columns {
                self.column_candidates(db, schema, ctx, out, needs);
            }
            if search_functions {
                let mut schemas = vec![schema.as_str()];
                if schema != "pg_catalog" {
                    schemas.push("pg_catalog");
                }
                for s in schemas {
                    for c in self.functions_in(db, s, word, needs) {
                        out.push(Cand::new(c.name, candidate_item_kind(c.kind), c.detail, true, 4));
                    }
                }
            }
        }
        if wants.schemas {
            for name in self.schemas_in(db, needs).unwrap_or_default() {
                out.push(Cand::new(name, ItemKind::Schema, None, true, 3));
            }
        }
    }

    /// The tables and views a table position offers: those of the current
    /// schema, the same names in other schemas (which is how a name two
    /// schemas have is found), and the CTEs defined before the caret.
    fn table_candidates(&self, db: &DatabasePath, schema: &str, ctx: &Context, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let word = ctx.word.as_str();
        let here = self.relations_in(db, schema, word, needs);
        let names: HashSet<&str> = here.iter().map(|(n, _)| n.as_str()).collect();
        let others: Vec<Candidate> = self
            .all_relations_in(db, word, needs)
            .into_iter()
            .filter(|c| c.schema.as_deref().is_some_and(|s| s != schema) && names.contains(c.name.as_str()))
            .collect();
        for (name, kind) in &here {
            let mut cand = Cand::new(name.clone(), *kind, None, true, 2);
            cand.schema = Some(schema.to_string());
            out.push(cand);
        }
        for c in others {
            let mut cand = Cand::new(c.name, candidate_item_kind(c.kind), None, true, 2);
            cand.schema = c.schema;
            out.push(cand);
        }
        for (name, _) in &ctx.scope.ctes {
            out.push(Cand::new(name.clone(), ItemKind::Cte, None, true, 2));
        }
    }

    /// The columns a column position offers: those of every source in scope,
    /// each with the alias of its table, the innermost block's first. Where
    /// the position takes them, also the whole condition of a join, the whole
    /// column list of an `insert`, and the select list's own expressions.
    fn column_candidates(&self, db: &DatabasePath, current: &str, ctx: &Context, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let scope = &ctx.scope;
        let dialect = self.active_dialect();
        let q = |n: &str| quote_identifier(n, dialect);
        let table_path = |t: &TableName| TablePath {
            schema: SchemaPath { database: db.clone(), schema: t.schema.clone().unwrap_or_else(|| current.to_string()) },
            table: t.name.clone(),
        };
        let columns_of = |t: &TableName, needs: &mut Vec<(Need, String)>| -> Vec<NamedColumn> {
            self.columns_of(&table_path(t), needs).into_iter().map(|c| (c.name, Some(c.db_type))).collect()
        };

        // `insert into t (`: every column at once.
        if let Some(t) = &scope.insert_into {
            let cols = columns_of(t, needs);
            if !cols.is_empty() {
                let list = cols.iter().map(|c| q(&c.0)).collect::<Vec<_>>().join(", ");
                let mut cand = Cand::new(cols.iter().map(|c| c.0.clone()).collect::<Vec<_>>().join(", "), ItemKind::ColumnList, None, false, 0);
                cand.insert = Some(list);
                cand.top = true;
                out.push(cand);
            }
        }
        // `join x on`: whole conditions from foreign keys.
        if scope.after_on {
            self.condition_candidates(db, current, ctx, out, needs);
        }
        // The select list's expressions and aliases, for `group by` and `order by`.
        let mut select_texts: HashSet<String> = HashSet::new();
        if matches!(scope.clause(), super::complete_scope::Clause::GroupBy | super::complete_scope::Clause::OrderBy) {
            for item in &scope.select_items {
                for text in item.alias.iter().chain(std::iter::once(&item.expr)) {
                    let mut cand = Cand::new(text.clone(), ItemKind::Select, None, true, 0);
                    cand.top = true;
                    out.push(cand);
                    if item.plain || item.alias.as_ref() == Some(text) {
                        select_texts.insert(text.clone());
                    }
                }
            }
        }
        // Every column of every source, and how many sources have each name.
        // A source whose columns are not (all) known makes every column
        // ambiguous: a bare name might be that source's, which Postgres
        // resolves first when it is nearer.
        let mut found: Vec<(usize, &Source, NamedColumn)> = Vec::new();
        let mut incomplete = false;
        for (level, l) in scope.levels.iter().enumerate() {
            for source in &l.sources {
                if !source.enumerable() {
                    incomplete = true;
                }
                for c in source.columns(&mut |t| {
                    let cols = columns_of(t, needs);
                    incomplete |= cols.is_empty();
                    cols
                }) {
                    found.push((level, source, c));
                }
            }
        }
        let mut count: HashMap<&str, usize> = HashMap::new();
        for (_, _, c) in &found {
            *count.entry(c.0.as_str()).or_default() += 1;
        }
        for (level, source, (name, db_type)) in &found {
            let qualified = format!("{}.{}", q(&source.label), q(name));
            let shared = count.get(name.as_str()).copied().unwrap_or(0) > 1;
            if select_texts.contains(&qualified) || (!shared && select_texts.contains(&q(name))) {
                continue;
            }
            let mut cand = Cand::new(name.clone(), ItemKind::Column, db_type.clone(), true, if *level == 0 { 0 } else { 1 });
            cand.owner = Some(source.label.clone());
            // A column of an outer query is always qualified: a bare name
            // would be the nearer query's own column when it has one.
            if shared || *level > 0 || incomplete {
                cand.insert = Some(qualified);
            }
            out.push(cand);
        }
        // Tables the statement names that are not in the scope's blocks (the
        // flat read, for text the block reader could not place) still count.
        if !scope.has_sources() {
            for r in &ctx.refs {
                let t = TableName { schema: r.schema.clone(), name: r.name.clone() };
                for (name, db_type) in columns_of(&t, needs) {
                    let mut cand = Cand::new(name, ItemKind::Column, db_type, true, 0);
                    cand.owner = Some(r.alias.clone().unwrap_or_else(|| r.name.clone()));
                    out.push(cand);
                }
            }
        }
    }

    /// The conditions the foreign keys between the table just joined and the
    /// tables before it give.
    fn condition_candidates(&self, db: &DatabasePath, current: &str, ctx: &Context, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let dialect = self.active_dialect();
        let q = |n: &str| quote_identifier(n, dialect);
        let scope = &ctx.scope;
        let tables: Vec<(&Source, &TableName)> = scope
            .sources()
            .iter()
            .filter(|s| (s.at.row, s.at.col) < (ctx.word_start.row, ctx.word_start.col))
            .filter_map(|s| match &s.kind {
                SourceKind::Table(t) => Some((s, t)),
                SourceKind::Derived(_) => None,
            })
            .collect();
        let Some(((new, new_table), earlier)) = tables.split_last().map(|(l, e)| (*l, e)) else { return };
        let path = |t: &TableName| TablePath {
            schema: SchemaPath { database: db.clone(), schema: t.schema.clone().unwrap_or_else(|| current.to_string()) },
            table: t.name.clone(),
        };
        let keys_of = |t: &TableName, needs: &mut Vec<(Need, String)>| -> Vec<ForeignKey> {
            let path = path(t);
            let cache = &self.model.completions;
            if let Some(keys) = cache.foreign_keys.get(&path) {
                return keys.clone();
            }
            if !cache.failed.contains(&Need::ForeignKeys(path.clone())) {
                needs.push((Need::ForeignKeys(path), String::new()));
            }
            Vec::new()
        };
        let new_keys = keys_of(new_table, needs);
        let earlier_keys: Vec<Vec<ForeignKey>> = earlier.iter().map(|(_, t)| keys_of(t, needs)).collect();
        let schema_of = |t: &TableName| t.schema.clone().unwrap_or_else(|| current.to_string());
        let (new_schema, earlier_schemas): (String, Vec<String>) = (schema_of(new_table), earlier.iter().map(|(_, t)| schema_of(t)).collect());
        let new_side = Side { label: &new.label, table: &new_table.name, schema: &new_schema, keys: &new_keys };
        let earlier_sides: Vec<Side> = earlier
            .iter()
            .zip(&earlier_keys)
            .zip(&earlier_schemas)
            .map(|(((s, t), keys), schema)| Side { label: &s.label, table: &t.name, schema, keys })
            .collect();
        for cond in join_conditions(&new_side, &earlier_sides, &q) {
            let mut cand = Cand::new(cond, ItemKind::Condition, None, false, 0);
            cand.top = true;
            out.push(cand);
        }
    }

    /// After `name.`: what `name` is decides: an alias or a table the
    /// statement names gives columns; a schema gives its tables; otherwise it
    /// is tried as a table of the current schema.
    fn qualified_candidates(
        &self,
        db: &DatabasePath,
        parts: &[String],
        ctx: &Context,
        current: Option<&str>,
        out: &mut Vec<Cand>,
        needs: &mut Vec<(Need, String)>,
    ) {
        match parts {
            [q] => {
                // A source in scope: an alias, a subquery's alias, a CTE's
                // name, or a table the statement names.
                if let Some(source) = ctx.scope.find(q) {
                    return self.push_source_columns(db, source, current, out, needs);
                }
                // A CTE defined before the caret, named without being selected from.
                if let Some((_, d)) = ctx.scope.ctes.iter().rev().find(|(n, _)| super::complete_scope::same_name(n, q)) {
                    let source = Source { label: q.clone(), aliased: false, kind: SourceKind::Derived(d.clone()), at: Cursor::default() };
                    return self.push_source_columns(db, &source, current, out, needs);
                }
                let same = |a: &str| a == q || a.eq_ignore_ascii_case(q);
                if let Some(r) = ctx.refs.iter().find(|r| r.alias.as_deref().is_some_and(same)) {
                    return self.push_columns(db, r, current, out, needs);
                }
                if let Some(r) = ctx.refs.iter().find(|r| r.alias.is_none() && same(&r.name)) {
                    return self.push_columns(db, r, current, out, needs);
                }
                // Whether it is a schema is a matter for the schema list.
                let Some(schemas) = self.schemas_in(db, needs) else { return };
                let named = schemas.iter().find(|s| *s == q).or_else(|| schemas.iter().find(|s| s.eq_ignore_ascii_case(q)));
                if let Some(schema) = named {
                    for (name, kind) in self.relations_in(db, schema, &ctx.word, needs) {
                        out.push(Cand::new(name, kind, None, true, 2));
                    }
                } else if let Some(schema) = current {
                    let r = TableRef { schema: None, name: q.clone(), alias: None };
                    self.push_columns(db, &r, Some(schema), out, needs);
                }
            }
            [schema, table] => {
                let r = TableRef { schema: Some(schema.clone()), name: table.clone(), alias: None };
                self.push_columns(db, &r, current, out, needs);
            }
            _ => {}
        }
    }

    fn push_source_columns(&self, db: &DatabasePath, source: &Source, current: Option<&str>, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let Some(current) = current else { return };
        let mut asked: Vec<(Need, String)> = Vec::new();
        let cols = source.columns(&mut |t| {
            let path = TablePath {
                schema: SchemaPath { database: db.clone(), schema: t.schema.clone().unwrap_or_else(|| current.to_string()) },
                table: t.name.clone(),
            };
            self.columns_of(&path, &mut asked).into_iter().map(|c| (c.name, Some(c.db_type))).collect()
        });
        needs.extend(asked);
        for (name, db_type) in cols {
            out.push(Cand::new(name, ItemKind::Column, db_type, true, 0));
        }
    }

    fn push_columns(&self, db: &DatabasePath, r: &TableRef, current: Option<&str>, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let Some(schema) = r.schema.as_deref().or(current) else { return };
        let path = TablePath { schema: SchemaPath { database: db.clone(), schema: schema.to_string() }, table: r.name.clone() };
        for c in self.columns_of(&path, needs) {
            out.push(Cand::new(c.name, ItemKind::Column, Some(c.db_type), true, 0));
        }
    }

    /// Tables and views of every schema, by what is typed: the whole database
    /// once, by prefix only when that was cut.
    fn all_relations_in(&self, db: &DatabasePath, word: &str, needs: &mut Vec<(Need, String)>) -> Vec<Candidate> {
        let cache = &self.model.completions;
        let need = Need::AllRelations(db.clone());
        let held = cache.all_relations.get(db);
        if !cache.failed.contains(&need) {
            match held {
                None => needs.push((need, String::new())),
                Some(e) if e.is_all_for(word) => {}
                Some(e) if e.covers(word) && word.chars().count() <= e.prefix.chars().count() => {}
                Some(_) => needs.push((need, word.to_string())),
            }
        }
        held.filter(|e| e.covers(word)).map(|e| e.found.clone()).unwrap_or_default()
    }

    /// The tables and views of a schema: the tree's listing if it has one,
    /// else what a search gave; and a search for what is missing.
    fn relations_in(&self, db: &DatabasePath, schema: &str, word: &str, needs: &mut Vec<(Need, String)>) -> Vec<(String, ItemKind)> {
        let path = SchemaPath { database: db.clone(), schema: schema.to_string() };
        if let Some(Listing::Tables(tables)) = self.model.schema.state(&NodeId::Schema(path.clone())).listing() {
            return tables.iter().map(|t| (t.name.clone(), table_item_kind(t.kind))).collect();
        }
        let cache = &self.model.completions;
        let need = Need::Relations(path.clone());
        let held = cache.relations.get(&path);
        if !cache.failed.contains(&need) {
            match held {
                // The whole schema, once; by prefix only when it was too big.
                None => needs.push((need, String::new())),
                Some(e) if e.is_all_for(word) => {}
                Some(e) if e.covers(word) && word.chars().count() <= e.prefix.chars().count() => {}
                Some(_) => needs.push((need, word.to_string())),
            }
        }
        held.filter(|e| e.covers(word))
            .map(|e| e.found.iter().map(|c| (c.name.clone(), candidate_item_kind(c.kind))).collect())
            .unwrap_or_default()
    }

    /// Functions of a schema that start with `word`.
    fn functions_in(&self, db: &DatabasePath, schema: &str, word: &str, needs: &mut Vec<(Need, String)>) -> Vec<Candidate> {
        let path = SchemaPath { database: db.clone(), schema: schema.to_string() };
        let cache = &self.model.completions;
        let need = Need::Functions(path.clone());
        let held = cache.functions.get(&path);
        if !cache.failed.contains(&need) {
            match held {
                Some(e) if e.is_all_for(word) => {}
                Some(e) if e.covers(word) && word.chars().count() <= e.prefix.chars().count() => {}
                _ => needs.push((need, word.to_string())),
            }
        }
        held.filter(|e| e.covers(word)).map(|e| e.found.clone()).unwrap_or_default()
    }

    /// The schemas of the database, or `None` while they are being asked for.
    fn schemas_in(&self, db: &DatabasePath, needs: &mut Vec<(Need, String)>) -> Option<Vec<String>> {
        if let Some(Listing::Schemas(names)) = self.model.schema.state(&NodeId::Database(db.clone())).listing() {
            return Some(names.clone());
        }
        let cache = &self.model.completions;
        if let Some(e) = cache.schemas.get(db) {
            return Some(e.found.iter().map(|c| c.name.clone()).collect());
        }
        let need = Need::Schemas(db.clone());
        if cache.failed.contains(&need) {
            return Some(Vec::new());
        }
        needs.push((need, String::new()));
        None
    }

    /// The columns of a table: the tree's, the cache's, or asked for.
    pub(super) fn columns_of(&self, path: &TablePath, needs: &mut Vec<(Need, String)>) -> Vec<ColumnInfo> {
        if let Some(Listing::Columns(columns)) = self.model.schema.state(&NodeId::Table(path.clone())).listing() {
            return columns.clone();
        }
        let cache = &self.model.completions;
        if let Some(columns) = cache.columns.get(path) {
            return columns.clone();
        }
        let need = Need::Columns(path.clone());
        if !cache.failed.contains(&need) {
            needs.push((need, String::new()));
        }
        Vec::new()
    }

    /// Puts dbx's analysis of the statement into `ctx`'s scope when it has
    /// it. Completion never asks for one: the semantic colours do, once the
    /// text has been still, for the very same text of the statement (the
    /// whole of it, the word being typed included), and the reply refreshes
    /// the popup. The tokens' scope stands until then, and when the text does
    /// not parse.
    fn use_analysis(&self, ctx: &mut Context) {
        if !self.core_names() || !ctx.scope.has_sources() {
            return;
        }
        let Some(db) = self.tab().target.clone() else { return };
        let Some(sql) = ctx.scope.statement_text(self.ui.tab.editor.lines()) else { return };
        if let (Some(Some(refs)), Some((origin, _))) = (self.model.completions.parsed.get(&(db, sql)), ctx.scope.text) {
            ctx.scope.apply_analysis(refs, origin, ctx.word_start);
        }
    }

    /// The `*` of a select list the caret is on or beside, if any.
    pub(crate) fn star_site(&self) -> Option<Star> {
        let caret = self.ui.tab.editor.cursor();
        let line = self.ui.tab.editor.lines().get(caret.row)?;
        let near = |col: usize| line.chars().nth(col) == Some('*');
        if !(near(caret.col) || caret.col.checked_sub(1).is_some_and(near)) {
            return None;
        }
        analyse(self.ui.tab.editor.lines(), caret).scope.star
    }

    /// Replaces the `*` under the caret with the columns it stands for: those
    /// of every table in scope, qualified by alias when there is more than one
    /// table or the table has an alias. One undo step. When a table's columns
    /// are not known yet they are asked for, and the expansion is made when
    /// they have come, if the text has not changed meanwhile.
    pub(super) fn expand_star(&mut self, fx: &mut Effects) {
        self.ui.tab.complete.pending_star = None;
        let Some(star) = self.star_site() else { return };
        let caret = self.ui.tab.editor.cursor();
        let ctx = analyse(self.ui.tab.editor.lines(), caret);
        let Some(db) = self.tab().target.clone() else { return };
        if !self.core_names() {
            return self.notify("Expanding * needs a PostgreSQL or SQLite connection for now.".to_string(), true, fx);
        }
        let Some(current) = self.current_schema_of(&db) else {
            let waiting = self.request_completion(vec![(Need::CurrentSchema(db), String::new())], fx);
            self.ui.tab.complete.pending_star = waiting.then(|| (self.ui.tab.editor.revision(), caret));
            return;
        };
        let sources: Vec<&Source> = match &star.qualifier {
            Some(q) => ctx.scope.find(q).into_iter().collect(),
            None => ctx.scope.sources().iter().collect(),
        };
        // Only what the client knows is expanded: a `*` over a source it cannot
        // list, or over joins that merge columns, would drop or repeat some.
        let block = ctx.scope.levels.first();
        let refusal = if block.is_some_and(|l| l.opaque) {
            Some("a source of the select is not a table (a function, VALUES, LATERAL or a parenthesised join)")
        } else if block.is_some_and(|l| l.merges) {
            Some("its joins use USING or NATURAL, which merge columns")
        } else if sources.iter().any(|s| !s.enumerable()) {
            Some("a subquery or CTE has a column with no name the client can tell")
        } else {
            None
        };
        if let Some(why) = refusal {
            return self.notify(format!("Could not expand *: {why}."), true, fx);
        }
        let dialect = self.active_dialect();
        let mut needs = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        let mut empty: Vec<String> = Vec::new();
        let qualify = star.qualifier.is_some() || sources.len() > 1 || sources.iter().any(|s| s.aliased);
        for source in &sources {
            let cols = source.columns(&mut |t| {
                let path = TablePath {
                    schema: SchemaPath { database: db.clone(), schema: t.schema.clone().unwrap_or_else(|| current.clone()) },
                    table: t.name.clone(),
                };
                self.columns_of(&path, &mut needs).into_iter().map(|c| (c.name, Some(c.db_type))).collect()
            });
            if cols.is_empty() {
                empty.push(source.label.clone());
            }
            for (name, _) in cols {
                let name = quote_identifier(&name, dialect);
                parts.push(if qualify { format!("{}.{name}", quote_identifier(&source.label, dialect)) } else { name });
            }
        }
        if !needs.is_empty() && self.request_completion(needs, fx) {
            self.ui.tab.complete.pending_star = Some((self.ui.tab.editor.revision(), caret));
            return;
        }
        if sources.is_empty() || !empty.is_empty() {
            let what = if sources.is_empty() { "no table in scope".to_string() } else { format!("the columns of {} are not known", empty.join(", ")) };
            return self.notify(format!("Could not expand *: {what}."), true, fx);
        }
        let text = parts.join(", ");
        match self.ui.tab.editor.replace(star.start, star.end, &text) {
            Ok(_) => fx.repaint = true,
            Err(why) => self.notify(format!("Could not expand *: {why}."), true, fx),
        }
        self.close_completion(fx);
    }

    /// An expansion that was waiting for names has them now (or the text moved on).
    fn continue_pending_star(&mut self, fx: &mut Effects) {
        let Some((revision, caret)) = self.ui.tab.complete.pending_star else { return };
        if self.ui.tab.editor.revision() != revision || self.ui.tab.editor.cursor() != caret || self.ui.focus != PaneId::Editor {
            self.ui.tab.complete.pending_star = None;
            return;
        }
        self.expand_star(fx);
    }

    // ---- asking the Core -------------------------------------------------------

    /// Asks the Core for what is not yet asked. Returns whether anything is
    /// still outstanding (asked now, or earlier and not answered).
    pub(super) fn request_completion(&mut self, needs: Vec<(Need, String)>, fx: &mut Effects) -> bool {
        let mut outstanding = false;
        let tab = self.ui.tab.id;
        for (need, mask) in needs {
            let cache = &mut self.model.completions;
            if cache.failed.contains(&need) {
                continue;
            }
            outstanding = true;
            match cache.inflight.get(&(tab, need.clone())) {
                // Already asked this.
                Some((_, asked)) if *asked == mask => continue,
                // Asked for another prefix: that answer is no longer wanted.
                Some((old, _)) => fx.commands.push(Command::Abandon(*old)),
                None => {}
            }
            let id = self.model.requests.begin(Target::Complete(tab, need.clone()));
            self.model.completions.inflight.insert((tab, need.clone()), (id, mask.clone()));
            let call = match &need {
                Need::CurrentSchema(db) => CoreCall::CurrentSchema(db.clone()),
                Need::Schemas(db) => CoreCall::Complete(CompletionRequest {
                    at: db.clone(),
                    schema: None,
                    kind: SearchKind::Schema,
                    prefix: mask,
                    limit: RELATION_LIMIT,
                    global: false,
                }),
                Need::Relations(s) => CoreCall::Complete(CompletionRequest {
                    at: s.database.clone(),
                    schema: Some(s.schema.clone()),
                    kind: SearchKind::Relation,
                    prefix: mask,
                    limit: RELATION_LIMIT,
                    global: false,
                }),
                Need::Functions(s) => CoreCall::Complete(CompletionRequest {
                    at: s.database.clone(),
                    schema: Some(s.schema.clone()),
                    kind: SearchKind::Function,
                    prefix: mask,
                    limit: FUNCTION_LIMIT,
                    global: false,
                }),
                Need::Columns(t) => CoreCall::TableColumns(t.clone()),
                Need::ForeignKeys(t) => CoreCall::ForeignKeys(t.clone()),
                // Only asked for Postgres tabs, so the kind is always known.
                Need::Parse(..) => CoreCall::AnalyzeReferences(self.active_kind().unwrap_or_default(), mask),
                Need::AllRelations(d) => CoreCall::Complete(CompletionRequest {
                    at: d.clone(),
                    schema: None,
                    kind: SearchKind::Relation,
                    prefix: mask,
                    limit: RELATION_LIMIT,
                    global: true,
                }),
            };
            fx.commands.push(Command::Core(id, call));
        }
        outstanding
    }

    /// The reply to a completion call of tab `tab` that is still current.
    /// Whatever it says is kept, for every tab (it is what the Core said of
    /// the database); the popup changes only when it is open on the same
    /// word, in that tab, on screen. Returns whether anything on screen
    /// changed.
    pub(super) fn on_complete_reply(&mut self, tab: TabId, need: Need, reply: CoreReply, fx: &mut Effects) -> bool {
        let prefix = self.model.completions.inflight.remove(&(tab, need.clone())).map(|(_, p)| p).unwrap_or_default();
        let cache = &mut self.model.completions;
        cache.version += 1;
        if matches!(need, Need::CurrentSchema(_) | Need::Relations(_) | Need::Columns(_)) {
            cache.schema_version += 1;
        }
        let searched = |c: crate::core::Completion| Searched { prefix: prefix.clone(), found: c.candidates, incomplete: c.incomplete };
        let failed = |e: &crate::core::CoreError| e.kind != CoreErrorKind::Cancelled;
        let mut failure = None;
        match (&need, reply) {
            (Need::CurrentSchema(db), CoreReply::CurrentSchema(Ok(schema))) => {
                cache.current_schema.insert(db.clone(), schema.unwrap_or_else(|| "public".to_string()));
            }
            (Need::Schemas(db), CoreReply::Completed(Ok(c))) => {
                cache.schemas.insert(db.clone(), searched(c));
            }
            (Need::Relations(s), CoreReply::Completed(Ok(c))) => {
                cache.relations.insert(s.clone(), searched(c));
            }
            (Need::Functions(s), CoreReply::Completed(Ok(c))) => {
                cache.functions.insert(s.clone(), searched(c));
            }
            (Need::AllRelations(d), CoreReply::Completed(Ok(c))) => {
                cache.all_relations.insert(d.clone(), searched(c));
            }
            (Need::Columns(t), CoreReply::TableColumns(Ok(columns))) => {
                cache.columns.insert(t.clone(), columns);
            }
            (Need::Parse(db, _), CoreReply::References(Ok(references))) => {
                if cache.parsed.len() >= 64 {
                    cache.parsed.clear();
                }
                cache.parsed.insert((db.clone(), prefix.clone()), references);
            }
            (Need::ForeignKeys(t), CoreReply::ForeignKeys(Ok(keys))) => {
                cache.foreign_keys.insert(t.clone(), keys);
            }
            (
                _,
                CoreReply::CurrentSchema(Err(e))
                | CoreReply::Completed(Err(e))
                | CoreReply::TableColumns(Err(e))
                | CoreReply::ForeignKeys(Err(e))
                | CoreReply::References(Err(e)),
            ) => {
                if failed(&e) {
                    cache.failed.insert(need.clone());
                    failure = Some(e.message);
                }
            }
            _ => return false,
        }
        // A failed background analysis is not worth a notice: nothing is marked.
        if let (Some(reason), false) = (failure, matches!(need, Need::Parse(..))) {
            // Once for each thing that failed: it is not asked again until
            // the key or a refresh in the sidebar asks. A tab not on screen
            // keeps it to say when it is shown again.
            let reason = reason.trim().trim_end_matches('.');
            self.notify_tab(tab, format!("Completion for {} failed: {reason}. Refresh it in the sidebar to retry.", need.describe()), fx);
        }
        // A tab not on screen has no popup open, no `*` or template waiting
        // (leaving a tab closes them).
        if !self.is_active(tab) {
            return false;
        }
        let had_star = self.ui.tab.complete.pending_star.is_some();
        self.continue_pending_star(fx);
        self.continue_template(fx);
        if !self.ui.tab.complete.is_open() {
            return had_star;
        }
        let before = (self.ui.tab.complete.session.clone(), self.ui.tab.complete.popup.clone());
        self.refresh_completion(Open::Fill, fx);
        before != (self.ui.tab.complete.session.clone(), self.ui.tab.complete.popup.clone())
    }

    /// A run in the tab ended whose text had `search_path` in it (it may have
    /// been `SET search_path`, or changed the role's or database's): the
    /// schema names resolve in is asked again the next time it is needed.
    ///
    /// A limit: the Core asks the connection's pool, not the tab's session
    /// (it must not queue behind the tab's own statement), so a `SET` made in
    /// the session is not what is learned, only what a fresh connection
    /// resolves names in (an `ALTER ROLE ... SET search_path`, say). Matching
    /// the text is conservative: any run that mentions it counts.
    pub(super) fn after_run_forget_current_schema(&mut self, tab: TabId, fx: &mut Effects) {
        let Some(t) = self.model.tab_mut(tab) else { return };
        if !std::mem::take(&mut t.run_mentions_search_path) {
            return;
        }
        let Some(db) = t.target.clone() else { return };
        let need = Need::CurrentSchema(db.clone());
        let cache = &mut self.model.completions;
        cache.current_schema.remove(&db);
        cache.schema_version += 1;
        cache.failed.remove(&need);
        let asked: Vec<(TabId, Need)> = cache.inflight.keys().filter(|(_, n)| *n == need).cloned().collect();
        for key in asked {
            if let Some((id, _)) = cache.inflight.remove(&key) {
                fx.commands.push(Command::Abandon(id));
            }
        }
        self.model.requests.forget_where(|t| matches!(t, Target::Complete(_, n) if *n == need));
    }

    /// A part of the tree is being refreshed or dropped: what completion
    /// kept for it is stale.
    pub(super) fn invalidate_completions(&mut self, node: &NodeId, fx: &mut Effects) {
        let inflight = self.model.completions.invalidate(node);
        self.model.requests.forget_where(|t| matches!(t, Target::Complete(_, n) if n.is_under(node)));
        fx.commands.extend(inflight.into_iter().map(Command::Abandon));
    }
}
