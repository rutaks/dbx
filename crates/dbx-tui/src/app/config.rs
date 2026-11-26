//! The config file (slice 22): theme, keys and settings in one TOML file.
//!
//! Pure: `parse` takes the file's text and returns what it sets, with the
//! default for everything it does not or gets wrong, and a `Problem` (line,
//! column, message) for each mistake. The shell finds and reads the file
//! (`shell/config.rs`); `App::apply_config` puts the result to use.
//!
//! The file is read once at startup (design section 8: no hot reload). A
//! mistake never stops the client: an entry with a problem keeps its default,
//! and the rest of the file still applies. Invalid TOML is the exception, as
//! the parser cannot say what the rest means: everything is default, with the
//! one problem the parser found.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use toml_edit::{Document, Item, TableLike, Value};

use super::format::{CommaPosition, KeywordCase};
use super::keys::{Action, Chord, KeyCombo, Keybinds, Refused, Scope, Taken};
use super::query::Notice;
use super::settings::{RunAtCaret, Settings};
use super::theme::Palette;
use crate::core::{DEFAULT_ROW_CAP, MAX_ROW_CAP};

/// One thing wrong with the file: where (1-based; 0 when it has no place), and
/// what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl Problem {
    /// `line 3, column 1: unknown key "foo" ...`, or the message alone for a
    /// problem with no place (the file could not be read).
    pub fn text(&self) -> String {
        if self.line == 0 {
            self.message.clone()
        } else {
            format!("line {}, column {}: {}", self.line, self.column, self.message)
        }
    }
}

/// What the file sets, with the defaults for what it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub keys: Keybinds,
    pub settings: Settings,
    pub row_cap: usize,
    /// The theme the file names (always one that exists).
    pub theme: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config { keys: Keybinds::default(), settings: Settings::default(), row_cap: DEFAULT_ROW_CAP, theme: None }
    }
}

/// A file's settings and its problems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub problems: Vec<Problem>,
}

/// What the client keeps of the file for the help overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The file that was looked for, if the client knows where it would be.
    pub path: Option<PathBuf>,
    /// Whether there was a file there.
    pub found: bool,
    pub problems: Vec<Problem>,
}

/// How long the startup notice stays: longer than other notices, which are
/// short news; this one sends the user to the help to read the problems.
const NOTICE_FOR: Duration = Duration::from_secs(10);

/// The values `[editor] run_at_caret` takes.
const RUN_AT_CARET: [(&str, RunAtCaret); 3] = [("ask", RunAtCaret::Ask), ("smallest", RunAtCaret::Smallest), ("statement", RunAtCaret::Statement)];
const KEYWORD_CASE: [(&str, KeywordCase); 3] = [("leave", KeywordCase::Leave), ("upper", KeywordCase::Upper), ("lower", KeywordCase::Lower)];
const COMMA: [(&str, CommaPosition); 2] = [("trailing", CommaPosition::Trailing), ("leading", CommaPosition::Leading)];

/// The widest an indent step or a tab may be.
const MAX_INDENT: i64 = 16;

const SECTIONS: &str = "theme, keys, editor, run";
const EDITOR_KEYS: [&str; 10] = [
    "alias_on_table",
    "auto_pair",
    "smart_enter",
    "smart_home",
    "run_at_caret",
    "hard_tabs",
    "tab_width",
    "format_keyword_case",
    "format_indent",
    "format_comma",
];
const RUN_KEYS: [&str; 2] = ["row_cap", "confirm_dangerous"];

/// Reads the file's text.
pub fn parse(text: &str) -> Loaded {
    let mut cx = Cx::new(text);
    match Document::parse(text) {
        Ok(doc) => {
            if let Some(root) = doc.as_item().as_table_like() {
                cx.root(root);
            }
        }
        // The parser stops at its first error and cannot say what the rest of
        // the file means: nothing from it is used.
        Err(e) => {
            cx.config = Config::default();
            cx.problems.clear();
            cx.problem(e.span(), (1, 1), e.message().to_string());
        }
    }
    cx.finish()
}

/// A key entry waiting to be applied, in the order the file has them.
struct Entry {
    line: usize,
    column: usize,
    action: Action,
    chords: Vec<Chord>,
}

struct Cx<'a> {
    text: &'a str,
    config: Config,
    problems: Vec<Problem>,
    prefix: Option<(usize, usize, KeyCombo)>,
    entries: Vec<Entry>,
    /// Every action a key can be set for, by scope and name.
    actions: HashMap<(Scope, String), Action>,
    /// The entry being read, for the problems with its value.
    ctx: String,
}

impl<'a> Cx<'a> {
    fn new(text: &'a str) -> Self {
        let actions = Keybinds::default().bound_actions().into_iter().map(|a| ((a.scope(), a.config_name()), a)).collect();
        Cx { text, config: Config::default(), problems: Vec::new(), prefix: None, entries: Vec::new(), actions, ctx: String::new() }
    }

    /// Line and column (1-based, in characters) of a byte offset.
    fn position(&self, offset: usize) -> (usize, usize) {
        let before = self.text.get(..offset).unwrap_or(self.text);
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, column)
    }

    /// Where `span` starts, or `fallback` when the parser gave none.
    fn at(&self, span: Option<Range<usize>>, fallback: (usize, usize)) -> (usize, usize) {
        span.map_or(fallback, |s| self.position(s.start))
    }

    fn problem(&mut self, span: Option<Range<usize>>, fallback: (usize, usize), message: String) {
        let (line, column) = self.at(span, fallback);
        self.problems.push(Problem { line, column, message });
    }

    /// A problem with the value of the entry being read, named as `[run] row_cap`.
    fn value_problem(&mut self, span: Option<Range<usize>>, fallback: (usize, usize), message: String) {
        let message = format!("{}: {message}", self.ctx);
        self.problem(span, fallback, message);
    }

    fn root(&mut self, root: &dyn TableLike) {
        for (name, item) in root.iter() {
            let here = root.get_key_value(name).and_then(|(k, _)| k.span());
            let at = self.at(here.clone(), (1, 1));
            let Some(table) = item.as_table_like() else {
                self.problem(here, at, format!("unknown key \"{name}\": keys go inside a section. The sections are {SECTIONS}"));
                continue;
            };
            match name {
                "theme" => self.theme(table, at),
                "keys" => self.keys(table, at),
                "editor" => self.editor(table, at),
                "run" => self.run(table, at),
                other => self.problem(here, at, format!("unknown section [{other}]. The sections are {SECTIONS}")),
            }
        }
    }

    fn theme(&mut self, table: &dyn TableLike, at: (usize, usize)) {
        for (name, item) in table.iter() {
            let here = table.get_key_value(name).and_then(|(k, _)| k.span());
            self.ctx = format!("[theme] {name}");
            match name {
                "name" => {
                    let Some(value) = self.string(item, at) else { continue };
                    match Palette::by_name(&value) {
                        Some(_) => self.config.theme = Some(value),
                        None => self.value_problem(item.span(), at, format!("unknown theme \"{value}\". The themes are {}", Palette::NAMES.join(", "))),
                    }
                }
                other => self.unknown_key("theme", other, &["name"], here, at),
            }
        }
    }

    fn editor(&mut self, table: &dyn TableLike, at: (usize, usize)) {
        for (name, item) in table.iter() {
            let here = table.get_key_value(name).and_then(|(k, _)| k.span());
            self.ctx = format!("[editor] {name}");
            match name {
                "alias_on_table" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.alias_on_table, v);
                }
                "auto_pair" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.auto_pair, v);
                }
                "smart_enter" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.smart_enter, v);
                }
                "smart_home" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.smart_home, v);
                }
                "hard_tabs" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.hard_tabs, v);
                }
                "tab_width" => {
                    let v = self.number_in(item, 1, MAX_INDENT, at);
                    set(&mut self.config.settings.tab_width, v.map(|n| n as usize));
                }
                "format_indent" => {
                    let v = self.number_in(item, 1, MAX_INDENT, at);
                    set(&mut self.config.settings.format_indent, v.map(|n| n as usize));
                }
                "run_at_caret" => {
                    let v = self.choice_in(item, &RUN_AT_CARET, at);
                    set(&mut self.config.settings.run_at_caret, v);
                }
                "format_keyword_case" => {
                    let v = self.choice_in(item, &KEYWORD_CASE, at);
                    set(&mut self.config.settings.format_keyword_case, v);
                }
                "format_comma" => {
                    let v = self.choice_in(item, &COMMA, at);
                    set(&mut self.config.settings.format_comma, v);
                }
                other => self.unknown_key("editor", other, &EDITOR_KEYS, here, at),
            }
        }
    }

    fn run(&mut self, table: &dyn TableLike, at: (usize, usize)) {
        for (name, item) in table.iter() {
            let here = table.get_key_value(name).and_then(|(k, _)| k.span());
            self.ctx = format!("[run] {name}");
            match name {
                "row_cap" => {
                    let v = self.number_in(item, 1, MAX_ROW_CAP as i64, at);
                    set(&mut self.config.row_cap, v.map(|n| n as usize));
                }
                "confirm_dangerous" => {
                    let v = self.flag_in(item, at);
                    set(&mut self.config.settings.confirm_dangerous, v);
                }
                "confirm_production" => self.problem(
                    here,
                    at,
                    "unknown key \"confirm_production\" in [run]: a write to production is always confirmed, and no setting turns that off. \
                     The keys are row_cap, confirm_dangerous"
                        .to_string(),
                ),
                other => self.unknown_key("run", other, &RUN_KEYS, here, at),
            }
        }
    }

    fn keys(&mut self, table: &dyn TableLike, at: (usize, usize)) {
        for (name, item) in table.iter() {
            let here = table.get_key_value(name).and_then(|(k, _)| k.span());
            let at = self.at(here.clone(), at);
            if name == "prefix" {
                self.ctx = "[keys] prefix".to_string();
                let Some(text) = self.string(item, at) else { continue };
                match KeyCombo::parse(&text) {
                    Some(key) => self.prefix = Some((at.0, at.1, key)),
                    None => self.value_problem(item.span(), at, not_a_key(&text)),
                }
                continue;
            }
            let Some(scope) = Scope::from_name(name) else {
                let scopes: Vec<&str> = Scope::ALL.iter().map(|s| s.name()).collect();
                self.problem(here, at, format!("unknown key group [keys.{name}]. The groups are prefix (the prefix key itself), {}", scopes.join(", ")));
                continue;
            };
            let Some(actions) = item.as_table_like() else {
                self.problem(item.span(), at, format!("[keys.{name}] must be a table of action = \"key\" lines"));
                continue;
            };
            for (action_name, value) in actions.iter() {
                let here = actions.get_key_value(action_name).and_then(|(k, _)| k.span());
                let place = self.at(here.clone(), at);
                let Some(&action) = self.actions.get(&(scope, action_name.to_string())) else {
                    self.problem(here, at, format!("unknown action \"{action_name}\" in [keys.{name}]. The help overlay lists every name"));
                    continue;
                };
                self.ctx = format!("[keys.{name}] {action_name}");
                if let Some(chords) = self.chords(value, place) {
                    self.entries.push(Entry { line: place.0, column: place.1, action, chords });
                }
            }
        }
    }

    /// The keys of one entry: a key, or a list of keys (an empty one unbinds).
    /// None, with the problems said, when any is wrong.
    fn chords(&mut self, item: &Item, at: (usize, usize)) -> Option<Vec<Chord>> {
        let value = item.as_value();
        let texts: Vec<(String, Option<Range<usize>>)> = match value {
            Some(Value::String(s)) => vec![(s.value().clone(), value.and_then(Value::span))],
            Some(Value::Array(a)) => {
                let mut out = Vec::new();
                for v in a.iter() {
                    match v.as_str() {
                        Some(s) => out.push((s.to_string(), v.span())),
                        None => {
                            self.value_problem(v.span(), at, "a key in the list must be text such as \"ctrl+b\"".to_string());
                            return None;
                        }
                    }
                }
                out
            }
            _ => {
                self.value_problem(item.span(), at, "expected a key such as \"ctrl+b\", or a list of keys (a list with none unbinds)".to_string());
                return None;
            }
        };
        let mut chords = Vec::new();
        let mut ok = true;
        for (text, span) in texts {
            match Chord::parse(&text) {
                Some(c) => chords.push(c),
                None => {
                    ok = false;
                    self.value_problem(span, at, not_a_key(&text));
                }
            }
        }
        ok.then_some(chords)
    }

    fn unknown_key(&mut self, section: &str, name: &str, known: &[&str], span: Option<Range<usize>>, at: (usize, usize)) {
        self.problem(span, at, format!("unknown key \"{name}\" in [{section}]. The keys are {}", known.join(", ")));
    }

    fn string(&mut self, item: &Item, at: (usize, usize)) -> Option<String> {
        match item.as_str() {
            Some(s) => Some(s.trim().to_string()),
            None => {
                self.value_problem(item.span(), at, "expected text in quotes".to_string());
                None
            }
        }
    }

    fn flag_in(&mut self, item: &Item, at: (usize, usize)) -> Option<bool> {
        let v = item.as_bool();
        if v.is_none() {
            self.value_problem(item.span(), at, "expected true or false".to_string());
        }
        v
    }

    fn number_in(&mut self, item: &Item, lo: i64, hi: i64, at: (usize, usize)) -> Option<i64> {
        match item.as_integer() {
            Some(n) if (lo..=hi).contains(&n) => Some(n),
            _ => {
                self.value_problem(item.span(), at, format!("expected a whole number from {lo} to {hi}"));
                None
            }
        }
    }

    fn choice_in<T: Copy>(&mut self, item: &Item, choices: &[(&str, T)], at: (usize, usize)) -> Option<T> {
        let found = item.as_str().and_then(|s| choices.iter().find(|(name, _)| *name == s.trim()).map(|(_, v)| *v));
        if found.is_none() {
            let names: Vec<String> = choices.iter().map(|(n, _)| format!("\"{n}\"")).collect();
            self.value_problem(item.span(), at, format!("expected one of {}", names.join(", ")));
        }
        found
    }

    /// Applies the keys, the prefix first, then each entry in file order, and
    /// puts the problems in file order.
    fn finish(mut self) -> Loaded {
        let mut keys = Keybinds::default();
        let mut claimed: Vec<Action> = Vec::new();
        let mut lines: HashMap<Action, usize> = HashMap::new();
        if let Some((line, column, key)) = self.prefix {
            match keys.set_prefix(key) {
                Ok(taken) => {
                    if !taken.is_empty() {
                        let message = format!("{key} is now the prefix key, so it is taken from {}", describe_taken(&keys, &taken));
                        self.problems.push(Problem { line, column, message });
                    }
                }
                Err(why) => self.problems.push(Problem { line, column, message: refused(why, "the prefix key", &lines) }),
            }
        }
        self.entries.sort_by_key(|e| (e.line, e.column));
        for e in &self.entries {
            // The ways out cannot be left without a key.
            if e.chords.is_empty() && matches!(e.action, Action::Quit | Action::Help | Action::ResizeDone) {
                let message = format!("{} cannot be unbound: you would be left without a way to {}; it keeps its keys", e.action.qualified_name(), match e.action {
                    Action::Quit => "quit",
                    Action::Help => "open the help",
                    _ => "leave the resize mode",
                });
                self.problems.push(Problem { line: e.line, column: e.column, message });
                continue;
            }
            match keys.set_keys(e.action, &e.chords, &claimed) {
                Ok(taken) => {
                    if !taken.is_empty() {
                        let keys_text = e.chords.iter().map(chord_text).collect::<Vec<_>>().join(", ");
                        let message = format!(
                            "{keys_text} {} also bound to {}; your binding wins",
                            if e.chords.len() == 1 { "was" } else { "were" },
                            describe_taken(&keys, &taken)
                        );
                        self.problems.push(Problem { line: e.line, column: e.column, message });
                    }
                    claimed.push(e.action);
                    lines.insert(e.action, e.line);
                }
                Err(why) => self.problems.push(Problem { line: e.line, column: e.column, message: refused(why, &e.action.qualified_name(), &lines) }),
            }
        }
        self.config.keys = keys;
        self.problems.sort_by_key(|p| (p.line, p.column));
        Loaded { config: self.config, problems: self.problems }
    }
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *slot = v;
    }
}

fn chord_text(c: &Chord) -> String {
    match c {
        Chord::Direct(k) => k.to_string(),
        Chord::AfterPrefix(k) => format!("prefix+{k}"),
    }
}

fn not_a_key(text: &str) -> String {
    format!("\"{text}\" is not a key. Write modifiers and the key joined by +, such as \"ctrl+shift+d\", \"alt+enter\", \"f5\" or \"space\"; \"prefix+x\" is x after the prefix")
}

/// `direct.cancel (keeps ctrl+f2, cmd+f2)`, one per action that lost a key.
fn describe_taken(keys: &Keybinds, taken: &[Taken]) -> String {
    let mut seen: Vec<Action> = Vec::new();
    let mut parts = Vec::new();
    for t in taken {
        if seen.contains(&t.from) {
            continue;
        }
        seen.push(t.from);
        let left = keys.all_keys(t.from);
        let now = if left.is_empty() { "now unbound".to_string() } else { format!("keeps {}", left.join(", ")) };
        parts.push(format!("{} ({now})", t.from.qualified_name()));
    }
    parts.join(", ")
}

fn refused(why: Refused, what: &str, lines: &HashMap<Action, usize>) -> String {
    match why {
        Refused::NoPrefixKeys => format!("{what} has no key after the prefix: \"prefix+\" is for the after_prefix group and the editor's actions"),
        Refused::Typing(k) => format!("\"{k}\" is a key you type or edit text with, so {what} on it would stop you typing it. Add ctrl, alt or cmd, or use another key"),
        Refused::IsPrefix(k) => format!("\"{k}\" is the prefix key and cannot do anything else; the key stays as it was for {what}"),
        Refused::Claimed { key, by } => {
            let at = lines.get(&by).map_or(String::new(), |l| format!(" (line {l})"));
            format!("\"{key}\" is already given to {}{at}; {what} keeps the keys it had", by.qualified_name())
        }
    }
}

// ---- the file `dbpane config` prints ---------------------------------------------------

/// A commented file of every default: reading it back gives the defaults.
pub fn default_file() -> String {
    let defaults = Keybinds::default();
    let settings = Settings::default();
    fn name_of<T: PartialEq + Copy>(choices: &[(&'static str, T)], want: T) -> &'static str {
        choices.iter().find(|(_, v)| *v == want).map_or("", |(n, _)| *n)
    }
    let mut out = String::new();
    out.push_str(
        "# dbpane's config file, with every default written out. Change what you want and\n\
         # delete the rest: anything left out keeps its default.\n\
         #\n\
         # It is read once, at startup: quit and start dbpane again after a change. A\n\
         # mistake never stops dbpane. It starts with the default for what is wrong, says\n\
         # how many problems there are, and the help (the prefix key, then ?) lists each\n\
         # with its line and column.\n\
         #\n\
         # `dbpane config --path` prints where this file belongs.\n\
         \n\
         [theme]\n\
         # catppuccin, terminal or intellij-dark. The DBPANE_THEME environment variable\n\
         # (DBX_TUI_THEME too) wins over this.\n\
         name = \"catppuccin\"\n\
         \n\
         [keys]\n\
         # App commands follow the prefix key (design: prefix keys, not modes).\n\
         prefix = \"ctrl+g\"\n",
    );
    out.push_str(
        "\n# Below, [keys.<group>] gives each action its key, or a list of keys; [] unbinds\n\
         # it. A list is every key the action has. \"prefix+x\" is x typed after the prefix.\n\
         # A key given to an action takes it from any other action that has it where both\n\
         # are read at the same time, and the help lists that as a problem. A plain\n\
         # character (no ctrl, alt or cmd) cannot be bound where you type.\n",
    );
    for scope in Scope::ALL {
        out.push_str(&format!("\n[keys.{}]\n# {}\n", scope.name(), scope_note(scope)));
        for (s, action, keys) in defaults.entries() {
            if s != scope {
                continue;
            }
            let quoted: Vec<String> = keys.iter().map(|k| toml_string(k)).collect();
            let value = if quoted.len() == 1 { quoted[0].clone() } else { format!("[{}]", quoted.join(", ")) };
            out.push_str(&format!("{} = {value}\n", action.config_name()));
        }
    }
    out.push_str(&format!(
        "\n[editor]\n\
         # Typing the alias after a table in from or join (film_actor fa).\n\
         alias_on_table = {}\n\
         # Typing ( [ ' or \" adds the pair; typing the closer moves over it.\n\
         auto_pair = {}\n\
         # enter keeps the indentation and opens a block between ( and ).\n\
         smart_enter = {}\n\
         # home toggles between the first character and column 0.\n\
         smart_home = {}\n\
         # What running does with the caret inside a subquery: \"ask\", \"smallest\" or \"statement\".\n\
         run_at_caret = \"{}\"\n\
         # tab inserts a tab character instead of spaces, and how wide an indent step is (1 to {MAX_INDENT}).\n\
         hard_tabs = {}\n\
         tab_width = {}\n\
         # Reformat: the case of keywords (\"leave\", \"upper\" or \"lower\"), the indent step in\n\
         # spaces, and where the comma of a broken list goes (\"trailing\" or \"leading\").\n\
         format_keyword_case = \"{}\"\n\
         format_indent = {}\n\
         format_comma = \"{}\"\n\
         \n\
         [run]\n\
         # The rows a run fetches (1 to {MAX_ROW_CAP}); more load on request.\n\
         row_cap = {DEFAULT_ROW_CAP}\n\
         # Ask before a statement dbx flags as dangerous. A write to production is always\n\
         # confirmed: no setting turns that off.\n\
         confirm_dangerous = {}\n",
        settings.alias_on_table,
        settings.auto_pair,
        settings.smart_enter,
        settings.smart_home,
        name_of(&RUN_AT_CARET, settings.run_at_caret),
        settings.hard_tabs,
        settings.tab_width,
        name_of(&KEYWORD_CASE, settings.format_keyword_case),
        settings.format_indent,
        name_of(&COMMA, settings.format_comma),
        settings.confirm_dangerous,
    ));
    out
}

fn scope_note(scope: Scope) -> &'static str {
    match scope {
        Scope::Prefix => "Typed after the prefix key (written prefix+x; a bare x means the same here).",
        Scope::Direct => "Read whichever pane has focus, before the pane sees the key.",
        Scope::Sidebar => "Read while the sidebar has focus.",
        Scope::Results => "Read while the results pane has focus, and in the cell viewer.",
        Scope::Resize => "Read in the resize mode.",
        Scope::Complete => "Read while the completion popup is open.",
        Scope::Template => "Read while a live template is being filled in.",
        Scope::Editor => "Read while the editor has focus; prefix+x keys work in the editor only.",
    }
}

/// `text` as a TOML basic string.
fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- using it ----------------------------------------------------------------------------

impl super::App {
    /// Puts a read config file to use: keys, settings, row cap and theme, the
    /// report the help overlay shows, and, if the file had problems, one
    /// notice saying how many and where to read them. `env_theme` is what the
    /// environment names, which wins over the file when it is a theme we have.
    pub fn apply_config(&mut self, loaded: Loaded, path: Option<PathBuf>, found: bool, env_theme: Option<String>) {
        let Loaded { config, problems } = loaded;
        self.ui.keys = config.keys;
        self.model.settings = config.settings;
        self.model.row_cap = config.row_cap;
        let theme = env_theme.filter(|t| Palette::by_name(t.trim()).is_some()).or(config.theme);
        self.ui.theme = Palette::from_setting(theme.as_deref());
        let count = problems.len();
        self.ui.config = Report { path, found, problems };
        if count > 0 {
            let help = match self.ui.keys.key_for(Action::Help) {
                Some(k) => format!("{} {k}", self.ui.keys.prefix),
                None => "the help overlay".to_string(),
            };
            let (noun, them) = if count == 1 { ("problem", "it") } else { ("problems", "them") };
            let text = format!("The config file has {count} {noun}; dbpane started with the defaults for what is wrong. {help} lists {them}.");
            let now = self.ui.now.unwrap_or_else(std::time::Instant::now);
            self.ui.notice = Some(Notice { text, failure: true, expires: now + NOTICE_FOR });
        }
    }
}
