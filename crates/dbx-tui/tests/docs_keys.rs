//! The shipped docs tell the truth about keys (slice 24b).
//!
//! `docs/README.md` and `docs/getting-started.md` (the canonical copies; the
//! staged `rutaks/dbpane` repo holds identical ones) mark their key tables:
//!
//! ```text
//! <!-- keys: sidebar -->
//! | Key | Does |
//! |---|---|
//! | `o` `f4` | open the table <!-- OpenTable --> |
//! <!-- keys: end -->
//! ```
//!
//! Every backticked key in a marked table's first column must be bound, in the
//! pane the marker names, to the action the row's trailing comment names, in
//! the default `Keybinds` (the table the help overlay and the mode bar are
//! built from). Panes: `prefix` (keys written `ctrl+g t`), `direct`,
//! `sidebar`, `results`, `editor`, `resize`.
//!
//! Not checked: keys that only exist inside an overlay (`esc`, `enter`, `y`,
//! `tab` in the export or dump overlay) and keys named in prose. The docs keep
//! the keys that matter in marked tables, and the overlays print their own.

use std::path::{Path, PathBuf};

use dbx_tui::app::keys::{KeyCombo, Keybinds};

fn lookup(kb: &Keybinds, pane: &str, key: &str) -> Result<String, String> {
    let text = if pane == "prefix" {
        let rest = key.strip_prefix(&format!("{} ", kb.prefix)).ok_or_else(|| format!("`{key}` does not start with the prefix `{}`", kb.prefix))?;
        rest
    } else {
        key
    };
    let combo = KeyCombo::parse(text).ok_or_else(|| format!("`{key}` is not a key this client can parse"))?;
    let found = match pane {
        "prefix" => kb.lookup(&combo),
        "direct" => kb.lookup_direct(&combo),
        "sidebar" => kb.lookup_sidebar(&combo),
        "results" => kb.lookup_results(&combo),
        "editor" => kb.lookup_editor(&combo),
        "resize" => kb.lookup_resize(&combo),
        other => return Err(format!("unknown pane `{other}` in a keys marker")),
    };
    found.map(|a| format!("{a:?}")).ok_or_else(|| format!("`{key}` is not bound in the {pane} pane"))
}

/// The backticked tokens of a table cell.
fn ticks(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

/// Checks one document: returns the problems and how many keys were checked.
fn check(doc: &str, kb: &Keybinds) -> (Vec<String>, usize) {
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut pane: Option<String> = None;
    for (n, line) in doc.lines().enumerate() {
        let at = n + 1;
        let t = line.trim();
        if let Some(m) = t.strip_prefix("<!-- keys:").and_then(|r| r.strip_suffix("-->")) {
            let m = m.trim();
            pane = (m != "end").then(|| m.to_string());
            continue;
        }
        let Some(pane) = pane.as_deref() else { continue };
        if !t.starts_with('|') {
            continue;
        }
        let (body, expected) = match t.split_once("<!--") {
            Some((b, c)) => (b, c.trim_end().trim_end_matches('|').trim_end().strip_suffix("-->").map(str::trim)),
            None => (t, None),
        };
        let first = body.trim_start_matches('|').split('|').next().unwrap_or("");
        let keys = ticks(first);
        if keys.is_empty() {
            let f = first.trim();
            if f.eq_ignore_ascii_case("key") || f.eq_ignore_ascii_case("keys") || f.starts_with('-') {
                continue;
            }
            problems.push(format!("line {at}: a row in a {pane} keys table has no backticked key"));
            continue;
        }
        let Some(expected) = expected else {
            problems.push(format!("line {at}: the row has no `<!-- ActionName -->` comment"));
            continue;
        };
        for key in keys {
            checked += 1;
            match lookup(kb, pane, key) {
                Ok(action) if action == expected => {}
                Ok(action) => problems.push(format!("line {at}: `{key}` does {action} in the {pane} pane, not {expected}")),
                Err(e) => problems.push(format!("line {at}: {e}")),
            }
        }
    }
    (problems, checked)
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(name: &str) -> String {
    std::fs::read_to_string(crate_dir().join("docs").join(name)).unwrap_or_else(|e| panic!("docs/{name}: {e}"))
}

#[test]
fn the_readme_keys_are_bound() {
    let (problems, checked) = check(&read("README.md"), &Keybinds::default());
    assert!(problems.is_empty(), "README.md:\n{}", problems.join("\n"));
    assert!(checked >= 20, "README.md checks only {checked} keys: its key tables lost their markers?");
}

#[test]
fn the_getting_started_keys_are_bound() {
    let (problems, checked) = check(&read("getting-started.md"), &Keybinds::default());
    assert!(problems.is_empty(), "getting-started.md:\n{}", problems.join("\n"));
    assert!(checked >= 10, "getting-started.md checks only {checked} keys: its key tables lost their markers?");
}

/// The checker can fail: each kind of lie is caught.
#[test]
fn the_checker_catches_a_wrong_doc() {
    let kb = Keybinds::default();
    let doc = |pane: &str, row: &str| format!("<!-- keys: {pane} -->\n| Key | Does |\n|---|---|\n{row}\n<!-- keys: end -->\n");
    // A key that is bound, in the right pane, to the right action: clean.
    assert_eq!(check(&doc("sidebar", "| `o` | open <!-- OpenTable -->"), &kb), (vec![], 1));
    assert_eq!(check(&doc("prefix", "| `ctrl+g t` | tab <!-- NewTab -->"), &kb), (vec![], 1));
    // Not bound at all.
    assert_eq!(check(&doc("sidebar", "| `ctrl+shift+q` | x <!-- Quit -->"), &kb).0.len(), 1);
    // Bound, but in another pane (`e` exports in the results, edits in the sidebar; `o` is only the sidebar's).
    assert_eq!(check(&doc("results", "| `o` | open <!-- OpenTable -->"), &kb).0.len(), 1);
    // Bound in the pane, to another action.
    assert_eq!(check(&doc("sidebar", "| `e` | export <!-- Export -->"), &kb).0.len(), 1);
    // No action named, a prefix key without the prefix, an unknown pane.
    assert_eq!(check(&doc("sidebar", "| `o` | open |"), &kb).0.len(), 1);
    assert_eq!(check(&doc("prefix", "| `t` | tab <!-- NewTab -->"), &kb).0.len(), 1);
    assert_eq!(check(&doc("sidebarr", "| `o` | open <!-- OpenTable -->"), &kb).0.len(), 1);
    // A row with no key in a keys table.
    assert_eq!(check(&doc("sidebar", "| open | x <!-- OpenTable -->"), &kb).0.len(), 1);
}

// ---- the config examples ------------------------------------------------------------------
//
// The key tables above are checked against the *default* `Keybinds`, so a
// remapped key never reaches them: the docs say they show the defaults, and the
// config file is how they change. What this half checks is the config section
// itself: every ```toml block must be a file dbpane reads without a problem,
// unless the line before it is `<!-- config: problems -->`, in which case the
// next ```text block must be exactly the problems dbpane reports for it.

/// Fenced blocks of a document: (the marker comment just above, the language,
/// the body).
fn fences(doc: &str) -> Vec<(Option<String>, String, String)> {
    let mut out = Vec::new();
    let mut previous = String::new();
    let mut open: Option<(Option<String>, String, String)> = None;
    for line in doc.lines() {
        let t = line.trim_end();
        match (&mut open, t.strip_prefix("```")) {
            (None, Some(lang)) => {
                let marker = previous.strip_prefix("<!-- config:").and_then(|m| m.strip_suffix("-->")).map(|m| m.trim().to_string());
                open = Some((marker, lang.trim().to_string(), String::new()));
            }
            (Some(_), Some("")) => out.push(open.take().expect("open")),
            (Some((_, _, body)), _) => {
                body.push_str(t);
                body.push('\n');
            }
            (None, None) => {}
        }
        if !t.is_empty() {
            previous = t.to_string();
        }
    }
    out
}

/// Checks one document's config examples: returns the problems and how many
/// toml blocks were checked.
fn check_config(doc: &str) -> (Vec<String>, usize) {
    let blocks = fences(doc);
    let (mut problems, mut checked) = (Vec::new(), 0);
    for (i, (marker, lang, body)) in blocks.iter().enumerate() {
        if lang != "toml" {
            continue;
        }
        checked += 1;
        let found: Vec<String> = dbx_tui::app::config::parse(body).problems.iter().map(|p| p.text()).collect();
        if marker.as_deref() == Some("problems") {
            let shown = blocks[i + 1..].iter().find(|(_, l, _)| l == "text").map(|(_, _, b)| b.lines().map(str::to_string).collect::<Vec<_>>());
            match shown {
                Some(shown) if shown == found => {}
                Some(shown) => problems.push(format!("block {}: the docs say the help lists {shown:?}, dbpane reports {found:?}", i + 1)),
                None => problems.push(format!("block {}: marked `config: problems` with no text block after it", i + 1)),
            }
        } else if !found.is_empty() {
            problems.push(format!("block {}: the example has problems: {found:?}", i + 1));
        }
    }
    (problems, checked)
}

#[test]
fn the_readme_config_examples_are_files_dbpane_reads_as_the_docs_say() {
    let (problems, checked) = check_config(&read("README.md"));
    assert!(problems.is_empty(), "README.md:\n{}", problems.join("\n"));
    assert!(checked >= 2, "README.md has {checked} toml blocks: the config section lost its examples?");
}

#[test]
fn the_getting_started_config_example_is_a_file_dbpane_reads_without_a_problem() {
    let (problems, checked) = check_config(&read("getting-started.md"));
    assert!(problems.is_empty(), "getting-started.md:\n{}", problems.join("\n"));
    assert!(checked >= 1, "getting-started.md has {checked} toml blocks");
}

#[test]
fn the_config_checker_catches_a_wrong_doc() {
    let doc = |marker: &str, toml: &str, text: &str| format!("{marker}\n```toml\n{toml}```\n\n```text\n{text}```\n");
    // A clean example, and a marked one whose quoted problem is the real one.
    assert_eq!(check_config(&doc("", "[run]\nrow_cap = 5\n", "")), (vec![], 1));
    let real = "line 2, column 11: [run] row_cap: expected a whole number from 1 to 100000\n";
    assert_eq!(check_config(&doc("<!-- config: problems -->", "[run]\nrow_cap = 0\n", real)), (vec![], 1));
    // An example with a mistake the docs do not mark.
    assert_eq!(check_config(&doc("", "[run]\nrow_cap = 0\n", "")).0.len(), 1);
    // A quoted problem that is not what dbpane says (wrong line).
    let wrong = real.replace("line 2", "line 3");
    assert_eq!(check_config(&doc("<!-- config: problems -->", "[run]\nrow_cap = 0\n", &wrong)).0.len(), 1);
    // A marked example that has no problem.
    assert_eq!(check_config(&doc("<!-- config: problems -->", "[run]\nrow_cap = 5\n", real)).0.len(), 1);
}

/// The docs name every place the file can be, the command that prints it and
/// the variables, as `dbpane --help` does.
#[test]
fn the_docs_name_where_the_file_is_and_the_commands_and_variables_for_it() {
    let readme = read("README.md");
    let started = read("getting-started.md");
    for needle in ["DBPANE_CONFIG", "DBX_TUI_CONFIG", "XDG_CONFIG_HOME/dbpane/config.toml", "~/.config/dbpane/config.toml", "dbpane config --path", "dbpane config >", "DBPANE_THEME"] {
        assert!(readme.contains(needle), "README.md does not mention {needle}");
    }
    for needle in ["dbpane config --path", "~/.config/dbpane/config.toml", "DBPANE_CONFIG"] {
        assert!(started.contains(needle), "getting-started.md does not mention {needle}");
    }
    // What the docs say is where the binary looks.
    let at = dbx_tui::shell::config::locate(&|k| match k {
        "HOME" => Some("/h".to_string()),
        _ => None,
    })
    .expect("a path");
    assert_eq!(at.path, Path::new("/h/.config/dbpane/config.toml"));
}

/// When the staged `rutaks/dbpane` repo is next to the fork (`.dbpane-repo`),
/// its copies of the docs are the canonical ones. Skipped where it is absent
/// (a fresh checkout of the fork).
#[test]
fn the_staged_repo_holds_the_same_docs() {
    let staged = std::env::var_os("DBPANE_REPO_DIR").map(PathBuf::from).unwrap_or_else(|| crate_dir().join("../../.dbpane-repo"));
    if !staged.is_dir() {
        eprintln!("no staged repo at {}: skipped", staged.display());
        return;
    }
    let same = |canonical: &str, staged_path: &Path| {
        let a = read(canonical);
        let b = std::fs::read_to_string(staged.join(staged_path)).unwrap_or_else(|e| panic!("{}: {e}", staged_path.display()));
        assert!(a == b, "{} differs from crates/dbx-tui/docs/{canonical}: run crates/dbx-tui/scripts/stage-docs.sh", staged_path.display());
    };
    same("README.md", Path::new("README.md"));
    same("getting-started.md", Path::new("docs/getting-started.md"));
    // The legal files the repo root carries are the crate's `legal/` ones.
    for name in ["LICENSE", "NOTICE"] {
        let a = std::fs::read_to_string(crate_dir().join("legal").join(name)).unwrap_or_else(|e| panic!("legal/{name}: {e}"));
        let b = std::fs::read_to_string(staged.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(a == b, "{name} differs from crates/dbx-tui/legal/{name}: run crates/dbx-tui/scripts/stage-docs.sh");
    }
}
