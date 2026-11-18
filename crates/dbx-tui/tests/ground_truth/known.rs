//! `tests/fixtures/KNOWN_DIFFERENCES.md`: the one list of the ways the client's
//! text differs from `psql`'s, read by the harness (which fails on a
//! difference that is not listed, on a listed one that no longer occurs, and
//! on a listed one whose values changed, and on a note that is `UNREVIEWED`)
//! and, with `GT_WRITE=1`, rewritten from what the harness finds. A rewrite
//! keeps the note of a row only when the row is unchanged; a new or changed row
//! gets the note `UNREVIEWED`, which the normal run rejects, and the run that
//! wrote the file always fails: a person reads the diff and writes the reasons.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/KNOWN_DIFFERENCES.md")
}

/// One cell where the client's text differs from psql's. Every field is the
/// escaped text that appears in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub table: String,
    pub row: String,
    pub column: String,
    /// `alone` or `beside tstz`.
    pub mode: String,
    pub ours: String,
    pub theirs: String,
    /// `FORMAT`, `LOSS`, or `TYPE` (the Core reports another type than psql's).
    pub class: String,
    pub note: String,
}

pub type Key = (String, String, String, String);

impl Entry {
    pub fn key(&self) -> Key {
        (self.table.clone(), self.row.clone(), self.column.clone(), self.mode.clone())
    }

    fn describe(&self) -> String {
        format!("table {}, row {:?}, column {} ({}): we show {}, psql shows {} [{}]", self.table, self.row, self.column, self.mode, self.ours, self.theirs, self.class)
    }

    pub fn render(&self) -> String {
        format!("| {} | {} | {} | {} | `{}` | `{}` | {} | {} |", self.table, self.row, self.column, self.mode, self.ours, self.theirs, self.class, self.note)
    }
}

/// `s` as it is written in the file: no pipe, backtick, line break or control
/// character survives, so a value fits in one table cell.
pub fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '|' => out.push_str("\\x7c"),
            '`' => out.push_str("\\x60"),
            c if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A value as the file shows it: NULL bare, text in double quotes, cut at 70
/// characters with the full length said.
pub fn show(v: &Option<String>) -> String {
    match v {
        None => "NULL".to_string(),
        Some(s) => {
            let total = s.chars().count();
            if total > 70 {
                let head: String = s.chars().take(70).collect();
                format!("\"{}\"…({total} chars)", esc(&head))
            } else {
                format!("\"{}\"", esc(s))
            }
        }
    }
}

fn section<'a>(text: &'a str, name: &str) -> (usize, usize, &'a str) {
    let start_marker = format!("<!-- gt:{name}:start -->");
    let end_marker = format!("<!-- gt:{name}:end -->");
    let start = text.find(&start_marker).unwrap_or_else(|| panic!("KNOWN_DIFFERENCES.md has no `{start_marker}`")) + start_marker.len();
    let end = text.find(&end_marker).unwrap_or_else(|| panic!("KNOWN_DIFFERENCES.md has no `{end_marker}`"));
    (start, end, &text[start..end])
}

/// The cells of each data row of the table in section `name`.
pub fn rows(text: &str, name: &str) -> Vec<Vec<String>> {
    let (_, _, body) = section(text, name);
    body.lines()
        .filter(|l| l.trim_start().starts_with('|'))
        .skip(2)
        .map(|l| l.trim().trim_start_matches('|').trim_end_matches('|').split('|').map(|c| c.trim().trim_matches('`').to_string()).collect())
        .collect()
}

pub fn read() -> String {
    std::fs::read_to_string(file_path()).unwrap_or_else(|e| panic!("{}: {e}", file_path().display()))
}

pub fn listed_entries(text: &str) -> Vec<Entry> {
    rows(text, "differences")
        .into_iter()
        .map(|c| {
            assert_eq!(c.len(), 8, "a row of KNOWN_DIFFERENCES.md has eight cells (an unescaped `|` in a value?): {c:?}");
            Entry { table: c[0].clone(), row: c[1].clone(), column: c[2].clone(), mode: c[3].clone(), ours: c[4].clone(), theirs: c[5].clone(), class: c[6].clone(), note: c[7].clone() }
        })
        .collect()
}

/// What is wrong between what the harness found (`actual`) and what the file
/// lists (`listed`): empty when they agree. Each problem names the table, row
/// label and column, and says what to add or remove.
pub fn problems(actual: &[Entry], listed: &[Entry]) -> Vec<String> {
    let mut out = vec![];
    let mut by_key: BTreeMap<Key, &Entry> = BTreeMap::new();
    for e in listed {
        if by_key.insert(e.key(), e).is_some() {
            out.push(format!("listed twice: {}", e.describe()));
        }
        if e.note.trim().is_empty() || e.note.trim() == "?" {
            out.push(format!("listed without a reason: {}", e.describe()));
        }
        if e.note.trim() == UNREVIEWED {
            out.push(format!("{UNREVIEWED}: write the reason for {}", e.describe()));
        }
        if !matches!(e.class.as_str(), "FORMAT" | "LOSS" | "TYPE") {
            out.push(format!("class must be FORMAT, LOSS or TYPE: {}", e.describe()));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in actual {
        seen.insert(a.key());
        match by_key.get(&a.key()) {
            None => out.push(format!("NOT LISTED: {}", a.describe())),
            Some(l) if (l.ours.as_str(), l.theirs.as_str(), l.class.as_str()) != (a.ours.as_str(), a.theirs.as_str(), a.class.as_str()) => {
                out.push(format!("LISTED BUT CHANGED: now {} (the file says we show {}, psql shows {} [{}])", a.describe(), l.ours, l.theirs, l.class))
            }
            Some(_) => {}
        }
    }
    for l in listed {
        if !seen.contains(&l.key()) {
            out.push(format!("LISTED BUT NO LONGER OCCURS: {}", l.describe()));
        }
    }
    out
}

/// The file with the rows of section `name` replaced by `rows` (the header
/// lines of the table stay).
pub fn replace_rows(text: &str, name: &str, rows: &[String]) -> String {
    let (start, end, body) = section(text, name);
    let header: Vec<&str> = body.lines().filter(|l| l.trim_start().starts_with('|')).take(2).collect();
    let mut new = String::from("\n");
    for h in header {
        new.push_str(h);
        new.push('\n');
    }
    for r in rows {
        new.push_str(r);
        new.push('\n');
    }
    format!("{}{}{}", &text[..start], new, &text[end..])
}

// ---- writing the file -----------------------------------------------------------

/// The note of a row the harness wrote and nobody has read. The normal run
/// rejects it by name.
pub const UNREVIEWED: &str = "UNREVIEWED";

/// What a `GT_WRITE=1` run reports after it wrote the file: it always fails,
/// so a loss cannot be blessed and pass in the same run.
pub fn written_message() -> String {
    format!(
        "GT_WRITE=1 rewrote KNOWN_DIFFERENCES.md and this run fails on purpose. Review the diff (git diff tests/fixtures/KNOWN_DIFFERENCES.md): every new or changed row has the note {UNREVIEWED}; replace each with the real reason, then re-run without GT_WRITE."
    )
}

/// `Err` when a rewrite is asked for where it must not happen (CI).
pub fn write_guard(write_requested: bool, ci: bool) -> Result<(), String> {
    if write_requested && ci {
        Err("GT_WRITE=1 is refused when CI is set: the known-differences file is reviewed by a person, never rewritten by a build".into())
    } else {
        Ok(())
    }
}

pub fn write_requested() -> bool {
    std::env::var_os("GT_WRITE").is_some()
}

/// Called by a test that is about to write the file.
pub fn guard_from_env() {
    if let Err(e) = write_guard(write_requested(), std::env::var_os("CI").is_some()) {
        panic!("{e}");
    }
}

/// The rows to write for what the harness `found`: each keeps the note the
/// file has for it only when the file lists it with the same texts and class;
/// a new row, or one whose texts or class changed, is `UNREVIEWED`.
pub fn merge_notes(found: &[Entry], listed: &[Entry]) -> Vec<Entry> {
    found
        .iter()
        .map(|d| {
            let note = listed
                .iter()
                .find(|l| l.key() == d.key() && (l.ours.as_str(), l.theirs.as_str(), l.class.as_str()) == (d.ours.as_str(), d.theirs.as_str(), d.class.as_str()))
                .map(|l| l.note.clone())
                .unwrap_or_else(|| UNREVIEWED.to_string());
            Entry { note, ..d.clone() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(row: &str, ours: &str, class: &str, note: &str) -> Entry {
        Entry { table: "t".into(), row: row.into(), column: "c".into(), mode: "alone".into(), ours: ours.into(), theirs: "\"x\"".into(), class: class.into(), note: note.into() }
    }

    #[test]
    fn a_rewrite_keeps_the_note_of_an_unchanged_row_and_marks_a_new_or_changed_one_unreviewed() {
        let listed = vec![entry("kept", "\"a\"", "FORMAT", "a real reason"), entry("class", "\"b\"", "FORMAT", "a reason for a format"), entry("text", "\"c\"", "LOSS", "a reason for c")];
        let found = vec![
            entry("kept", "\"a\"", "FORMAT", ""),
            entry("class", "\"b\"", "LOSS", ""),
            entry("text", "\"d\"", "LOSS", ""),
            entry("brand new", "\"e\"", "LOSS", ""),
        ];
        let merged = merge_notes(&found, &listed);
        let notes: Vec<&str> = merged.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(notes, ["a real reason", UNREVIEWED, UNREVIEWED, UNREVIEWED]);
    }

    #[test]
    fn the_normal_run_rejects_an_unreviewed_note_naming_table_row_and_column() {
        let e = entry("a row", "\"a\"", "LOSS", UNREVIEWED);
        let found = problems(std::slice::from_ref(&e), std::slice::from_ref(&e));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with(UNREVIEWED), "{}", found[0]);
        for part in ["t", "a row", "c"] {
            assert!(found[0].contains(part), "{}", found[0]);
        }
    }

    #[test]
    fn a_write_is_refused_in_ci_and_always_ends_in_a_failure_that_says_what_to_do() {
        assert!(write_guard(true, true).is_err());
        assert!(write_guard(true, false).is_ok());
        assert!(write_guard(false, true).is_ok());
        let m = written_message();
        assert!(m.contains("fails") && m.contains("Review the diff") && m.contains("without GT_WRITE") && m.contains(UNREVIEWED), "{m}");
    }
}
