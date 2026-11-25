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
