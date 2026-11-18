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
use super::highlight::KEYWORDS;
use super::keys::{Action, KeyCombo};
use super::requests::{RequestId, Target};
use super::theme::Role;
use super::tree::{Listing, NodeId};
use super::{App, Command, CoreCall, CoreReply, Effects, PaneId};
use crate::core::{
    display_safe, Candidate, CandidateKind, ColumnInfo, CompletionRequest, CoreErrorKind, Cursor, DatabasePath, DbKind, SchemaPath, SearchKind, TableKind,
    TablePath,
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
}

impl Need {
    fn database(&self) -> &DatabasePath {
        match self {
            Need::CurrentSchema(d) | Need::Schemas(d) => d,
            Need::Relations(s) | Need::Functions(s) => &s.database,
            Need::Columns(t) => &t.schema.database,
        }
    }

    /// What this is a need for, for a message.
    fn describe(&self) -> String {
        match self {
            Need::CurrentSchema(d) | Need::Schemas(d) => format!("database {}", d.database),
            Need::Relations(s) | Need::Functions(s) => format!("schema {}", s.schema),
            Need::Columns(t) => format!("table {}.{}", t.schema.schema, t.table),
        }
    }

    /// Whether the refresh of `node` (a part of the tree) makes this stale.
    fn is_under(&self, node: &NodeId) -> bool {
        match node {
            NodeId::Connection(c) => &self.database().connection == c,
            NodeId::Database(d) => self.database() == d,
            NodeId::Schema(s) => match self {
                Need::Relations(x) | Need::Functions(x) => x == s,
                Need::Columns(t) => &t.schema == s,
                _ => false,
            },
            NodeId::Table(t) => matches!(self, Need::Columns(x) if x == t),
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
    current_schema: HashMap<DatabasePath, String>,
    schemas: HashMap<DatabasePath, Searched>,
    relations: HashMap<SchemaPath, Searched>,
    functions: HashMap<SchemaPath, Searched>,
    columns: HashMap<TablePath, Vec<ColumnInfo>>,
    /// Asked, and the Core said no: not asked again until that part of the
    /// tree is refreshed.
    failed: HashSet<Need>,
    /// In flight, with the prefix asked.
    inflight: HashMap<Need, (RequestId, String)>,
}

impl CompletionCache {
    /// The schema unqualified names resolve in, once known.
    pub fn current_schema(&self, at: &DatabasePath) -> Option<&str> {
        self.current_schema.get(at).map(String::as_str)
    }

    /// How many things are kept (for tests).
    pub fn len(&self) -> usize {
        self.current_schema.len() + self.schemas.len() + self.relations.len() + self.functions.len() + self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops what the refresh of `node` makes stale and returns the requests
    /// that were in flight for it.
    fn invalidate(&mut self, node: &NodeId) -> Vec<RequestId> {
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
            }
            NodeId::Table(t) => {
                self.columns.remove(t);
            }
            _ => {
                self.relations.retain(|s, _| !under(&s.database));
                self.functions.retain(|s, _| !under(&s.database));
                self.columns.retain(|t, _| !under(&t.schema.database));
            }
        }
        self.failed.retain(|n| !n.is_under(node));
        let gone: Vec<Need> = self.inflight.keys().filter(|n| n.is_under(node)).cloned().collect();
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
    /// before anything is typed: columns, tables, schemas, functions, keywords.
    group: u8,
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
        self.ui.complete.visible() && self.ui.focus == PaneId::Editor && self.ui.overlay.is_none()
    }

    /// Closes the popup, and forgets what it was waiting for.
    pub(super) fn close_completion(&mut self, fx: &mut Effects) {
        let was = self.ui.complete.session.is_some() || !self.ui.complete.popup.is_empty();
        self.ui.complete.reset();
        fx.repaint |= was;
    }

    /// A key given to the focused editor: the popup's keys first, when it is
    /// open; then the editor's own, after which the list follows the text.
    pub(super) fn on_editor_key(&mut self, key: KeyEvent, combo: KeyCombo, fx: &mut Effects) {
        if self.completion_visible() {
            if let Some(action) = self.ui.keys.lookup_complete(&combo) {
                // `enter` after a word that is already typed in full is the
                // editor's: a line break, not text the user did not type.
                if action != Action::CompleteInsert || !self.enter_is_a_line_break(fx) {
                    return self.perform_complete(action, fx);
                }
            }
        }
        // `esc` with a completion on its way but nothing to show yet ends it,
        // and is still the editor's key: it is not swallowed.
        if combo.code == KeyCode::Esc && self.ui.complete.is_open() {
            self.close_completion(fx);
        }
        let before = self.ui.editor.revision();
        fx.repaint |= self.ui.editor.key(key);
        let edited = self.ui.editor.revision() != before;
        let typed = match key.code {
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => Some(c),
            _ => None,
        };
        if self.ui.complete.is_open() {
            self.refresh_completion(Open::Keep, fx);
        }
        if !self.ui.complete.is_open() && edited && typed.is_some_and(|c| is_word_char(c) || c == '.') {
            self.refresh_completion(Open::Typed, fx);
        }
    }

    /// One of the popup's own actions, or the key that opens it.
    pub(super) fn perform_complete(&mut self, action: Action, fx: &mut Effects) {
        match action {
            Action::CompleteOpen => self.refresh_completion(Open::Manual, fx),
            Action::CompleteInsert => self.accept_completion(false, fx),
            Action::CompleteReplace => self.accept_completion(true, fx),
            Action::CompleteClose => self.close_completion(fx),
            Action::CompleteUp => fx.repaint |= self.ui.complete.popup.up(),
            Action::CompleteDown => fx.repaint |= self.ui.complete.popup.down(),
            Action::CompletePageUp => fx.repaint |= self.ui.complete.popup.page_up(),
            Action::CompletePageDown => fx.repaint |= self.ui.complete.popup.page_down(),
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
            let index = self.ui.complete.popup.top() + (y - rect.y) as usize;
            if index < self.ui.complete.popup.len() {
                self.ui.complete.popup.select(index);
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
        fx.repaint |= self.ui.complete.popup.scroll(rows);
        true
    }

    /// The completion has nothing to do with what is on screen any more: the
    /// editor lost focus, an overlay or the prefix mode took over.
    pub(super) fn completion_housekeeping(&mut self, fx: &mut Effects) {
        if self.ui.complete.is_open() && (self.ui.focus != PaneId::Editor || self.ui.overlay.is_some() || self.ui.mode != super::Mode::Normal) {
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
        if self.ui.complete.session.as_ref().is_none_or(|s| s.word.is_empty()) {
            return false;
        }
        let kind = self.active_kind();
        self.ui.syntax.refresh(&self.ui.editor, kind.as_ref());
        let caret = self.ui.editor.cursor();
        let Some(before) = caret.col.checked_sub(1) else { return false };
        self.ui
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
        let typed = !self.ui.complete.popup.touched()
            && self.ui.complete.popup.selected_item().is_some_and(|item| {
                let caret = self.ui.editor.cursor();
                self.typed_in_full(item, &analyse(self.ui.editor.lines(), caret), caret)
            });
        if in_literal || typed {
            self.close_completion(fx);
        }
        in_literal || typed
    }

    /// The text `item` would replace, and what it replaces it with, are the
    /// same: a keyword in any case, a name exactly.
    fn typed_in_full(&self, item: &Item, ctx: &Context, caret: Cursor) -> bool {
        let (from, to) = accept_range(ctx, caret, false);
        if from.row != to.row || from.col > to.col {
            return false;
        }
        let Some(line) = self.ui.editor.lines().get(from.row) else { return false };
        let typed: String = line.chars().skip(from.col).take(to.col - from.col).collect();
        if item.kind == ItemKind::Keyword {
            typed.to_lowercase() == item.insert.to_lowercase()
        } else {
            typed == item.insert
        }
    }

    /// Recomputes the list for the caret as it is now.
    fn refresh_completion(&mut self, open: Open, fx: &mut Effects) {
        let session = self.ui.complete.session.clone();
        if session.is_none() && matches!(open, Open::Keep | Open::Fill) {
            return;
        }
        // The key asks again for what the Core failed to give.
        if open == Open::Manual {
            if let Some(db) = self.tab().target.clone() {
                self.model.completions.failed.retain(|n| n.database() != &db);
            }
        }
        let caret = self.ui.editor.cursor();
        let ctx = analyse(self.ui.editor.lines(), caret);
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
        let before = (self.ui.complete.session.clone(), self.ui.complete.popup.clone());
        self.ui.complete.session = Some(Session { anchor: ctx.word_start, word: ctx.word.clone(), manual });
        self.ui.complete.popup.set_items(items, keep);
        if before != (self.ui.complete.session.clone(), self.ui.complete.popup.clone()) {
            fx.repaint = true;
        }
    }

    /// Puts the selected suggestion into the text: `enter` at the caret,
    /// `tab` over the rest of the word to its right; a closed quoted name is
    /// replaced through its closing quote either way.
    fn accept_completion(&mut self, replace: bool, fx: &mut Effects) {
        let Some(item) = self.ui.complete.popup.selected_item().cloned() else { return };
        // Not into a string or a comment, which a keystroke may have made of
        // the caret's place before a frame could close the popup.
        if self.caret_in_literal() {
            return self.close_completion(fx);
        }
        let caret = self.ui.editor.cursor();
        let ctx = analyse(self.ui.editor.lines(), caret);
        let (from, to) = accept_range(&ctx, caret, replace);
        if from.row == to.row && from.col <= to.col {
            if let Err(why) = self.ui.editor.replace(from, to, &item.insert) {
                self.notify(format!("Could not insert {}: {why}.", display_safe(&item.insert)), true, fx);
            }
        }
        self.close_completion(fx);
        fx.repaint = true;
    }

    /// The suggestions for `ctx`, ranked, and what is still missing to make
    /// them complete.
    fn completion_items(&self, ctx: &Context) -> (Vec<Item>, Vec<(Need, String)>) {
        let kind = self.active_kind();
        let word = ctx.word.as_str();
        let mut cands: Vec<Cand> = Vec::new();
        let mut needs: Vec<(Need, String)> = Vec::new();

        let keywords: &[&str] = match ctx.wants.keywords {
            Keywords::Any => KEYWORDS,
            Keywords::Only(only) => only,
            Keywords::None => &[],
        };
        cands.extend(keywords.iter().map(|k| Cand { name: (*k).to_string(), kind: ItemKind::Keyword, detail: None, strict: false, group: 4 }));

        // Names are the Core's for Postgres; other kinds get keywords for now.
        if kind == Some(DbKind::Postgres) {
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
        if word.is_empty() {
            // Nothing typed: the sources in order, each as it came (columns in
            // the table's order).
            ranked.sort_by_key(|(_, i, c)| (c.group, *i));
        } else {
            ranked.sort_by(|a, b| {
                rank_key(&a.0, &a.2.name).cmp(&rank_key(&b.0, &b.2.name)).then(a.2.group.cmp(&b.2.group)).then(a.1.cmp(&b.1))
            });
        }
        let mut seen = HashSet::new();
        let items = ranked
            .into_iter()
            .filter(|(_, _, c)| seen.insert((c.name.clone(), c.kind)))
            .take(MAX_ITEMS)
            .map(|(m, _, c)| {
                let insert =
                    if c.kind == ItemKind::Keyword { keyword_text(word, &c.name) } else { quote_identifier(&c.name, kind.as_ref()) };
                Item { name: c.name, insert, kind: c.kind, detail: c.detail, matched: m.positions }
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
        // Functions are searched for by what is typed, from two letters on: one
        // letter matches hundreds.
        let search_functions = wants.functions && word.chars().count() >= MIN_FUNCTION_WORD;
        // Unqualified names resolve in the current schema, asked once, and
        // only when something here needs it.
        let wants_schema = wants.qualified.is_some() || wants.tables || (wants.columns && !ctx.refs.is_empty()) || search_functions;
        let current = match cache.current_schema.get(db) {
            Some(schema) => Some(schema.clone()),
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
            out.extend(BUILTIN_FUNCTIONS.iter().map(|f| Cand { name: (*f).to_string(), kind: ItemKind::Function, detail: None, strict: true, group: 3 }));
        }
        if let Some(schema) = &current {
            if wants.tables {
                for (name, kind) in self.relations_in(db, schema, word, needs) {
                    out.push(Cand { name, kind, detail: None, strict: true, group: 1 });
                }
            }
            if wants.columns {
                let mut done: Vec<&TableRef> = Vec::new();
                for r in &ctx.refs {
                    if done.contains(&r) {
                        continue;
                    }
                    done.push(r);
                    self.push_columns(db, r, Some(schema), out, needs);
                }
            }
            if search_functions {
                let mut schemas = vec![schema.as_str()];
                if schema != "pg_catalog" {
                    schemas.push("pg_catalog");
                }
                for s in schemas {
                    for c in self.functions_in(db, s, word, needs) {
                        out.push(Cand { name: c.name, kind: candidate_item_kind(c.kind), detail: c.detail, strict: true, group: 3 });
                    }
                }
            }
        }
        if wants.schemas {
            for name in self.schemas_in(db, needs).unwrap_or_default() {
                out.push(Cand { name, kind: ItemKind::Schema, detail: None, strict: true, group: 2 });
            }
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
                        out.push(Cand { name, kind, detail: None, strict: true, group: 1 });
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

    fn push_columns(&self, db: &DatabasePath, r: &TableRef, current: Option<&str>, out: &mut Vec<Cand>, needs: &mut Vec<(Need, String)>) {
        let Some(schema) = r.schema.as_deref().or(current) else { return };
        let path = TablePath { schema: SchemaPath { database: db.clone(), schema: schema.to_string() }, table: r.name.clone() };
        for c in self.columns_of(&path, needs) {
            out.push(Cand { name: c.name, kind: ItemKind::Column, detail: Some(c.db_type), strict: true, group: 0 });
        }
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
    fn columns_of(&self, path: &TablePath, needs: &mut Vec<(Need, String)>) -> Vec<ColumnInfo> {
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

    // ---- asking the Core -------------------------------------------------------

    /// Asks the Core for what is not yet asked. Returns whether anything is
    /// still outstanding (asked now, or earlier and not answered).
    fn request_completion(&mut self, needs: Vec<(Need, String)>, fx: &mut Effects) -> bool {
        let mut outstanding = false;
        for (need, mask) in needs {
            let cache = &mut self.model.completions;
            if cache.failed.contains(&need) {
                continue;
            }
            outstanding = true;
            match cache.inflight.get(&need) {
                // Already asked this.
                Some((_, asked)) if *asked == mask => continue,
                // Asked for another prefix: that answer is no longer wanted.
                Some((old, _)) => fx.commands.push(Command::Abandon(*old)),
                None => {}
            }
            let id = self.model.requests.begin(Target::Complete(need.clone()));
            self.model.completions.inflight.insert(need.clone(), (id, mask.clone()));
            let call = match &need {
                Need::CurrentSchema(db) => CoreCall::CurrentSchema(db.clone()),
                Need::Schemas(db) => CoreCall::Complete(CompletionRequest {
                    at: db.clone(),
                    schema: None,
                    kind: SearchKind::Schema,
                    prefix: mask,
                    limit: RELATION_LIMIT,
                }),
                Need::Relations(s) => CoreCall::Complete(CompletionRequest {
                    at: s.database.clone(),
                    schema: Some(s.schema.clone()),
                    kind: SearchKind::Relation,
                    prefix: mask,
                    limit: RELATION_LIMIT,
                }),
                Need::Functions(s) => CoreCall::Complete(CompletionRequest {
                    at: s.database.clone(),
                    schema: Some(s.schema.clone()),
                    kind: SearchKind::Function,
                    prefix: mask,
                    limit: FUNCTION_LIMIT,
                }),
                Need::Columns(t) => CoreCall::TableColumns(t.clone()),
            };
            fx.commands.push(Command::Core(id, call));
        }
        outstanding
    }

    /// The reply to a completion call that is still current. Whatever it says
    /// is kept; the popup changes only when it is open on the same word.
    /// Returns whether anything on screen changed.
    pub(super) fn on_complete_reply(&mut self, need: Need, reply: CoreReply, fx: &mut Effects) -> bool {
        let prefix = self.model.completions.inflight.remove(&need).map(|(_, p)| p).unwrap_or_default();
        let cache = &mut self.model.completions;
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
            (Need::Columns(t), CoreReply::TableColumns(Ok(columns))) => {
                cache.columns.insert(t.clone(), columns);
            }
            (_, CoreReply::CurrentSchema(Err(e)) | CoreReply::Completed(Err(e)) | CoreReply::TableColumns(Err(e))) => {
                if failed(&e) {
                    cache.failed.insert(need.clone());
                    failure = Some(e.message);
                }
            }
            _ => return false,
        }
        if let Some(reason) = failure {
            // Once for each thing that failed: it is not asked again until
            // the key or a refresh in the sidebar asks.
            let reason = reason.trim().trim_end_matches('.');
            self.notify(format!("Completion for {} failed: {reason}. Refresh it in the sidebar to retry.", need.describe()), true, fx);
        }
        if !self.ui.complete.is_open() {
            return false;
        }
        let before = (self.ui.complete.session.clone(), self.ui.complete.popup.clone());
        self.refresh_completion(Open::Fill, fx);
        before != (self.ui.complete.session.clone(), self.ui.complete.popup.clone())
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
    pub(super) fn after_run_forget_current_schema(&mut self, fx: &mut Effects) {
        if !std::mem::take(&mut self.model.tabs[0].run_mentions_search_path) {
            return;
        }
        let Some(db) = self.tab().target.clone() else { return };
        let need = Need::CurrentSchema(db.clone());
        let cache = &mut self.model.completions;
        cache.current_schema.remove(&db);
        cache.failed.remove(&need);
        if let Some((id, _)) = cache.inflight.remove(&need) {
            fx.commands.push(Command::Abandon(id));
            self.model.requests.forget_where(|t| matches!(t, Target::Complete(n) if *n == need));
        }
    }

    /// A part of the tree is being refreshed or dropped: what completion
    /// kept for it is stale.
    pub(super) fn invalidate_completions(&mut self, node: &NodeId, fx: &mut Effects) {
        let inflight = self.model.completions.invalidate(node);
        self.model.requests.forget_where(|t| matches!(t, Target::Complete(n) if n.is_under(node)));
        fx.commands.extend(inflight.into_iter().map(Command::Abandon));
    }
}
