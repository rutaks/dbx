//! `psql -At` inside the fixture container, one process for many queries.
//!
//! Safety: the container is the fixture Postgres (never `dbxspike`), the
//! database is `edge`, and every statement is a `select` (or a `set time
//! zone` in the preamble of a run).
//!
//! Records end in NUL (`--record-separator-zero`) and fields are separated by
//! NUL (`--field-separator-zero`): Postgres text cannot contain NUL, so a value
//! with a newline, a tab, a control byte, the text `NULL` or an empty string
//! cannot be mistaken for a separator or another value. NULL is printed as a
//! marker with a per-run nonce, and each query's output is preceded by a
//! marker line with the same nonce; the parser asserts no value is the NULL
//! marker or holds a marker.
//!
//! The session is pinned: `PGCLIENTENCODING=UTF8` is set, `PGTZ` and
//! `PGDATESTYLE` are cleared, and the harness reads the settings that change
//! how values print (`SESSION_SETTINGS`) through both sides and compares them.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

/// The settings that change how a value is printed, read on both sides.
pub const SESSION_SETTINGS: [&str; 7] = ["TimeZone", "DateStyle", "IntervalStyle", "extra_float_digits", "bytea_output", "lc_monetary", "client_encoding"];

/// The query that reads them, one `name=value` row per setting.
pub fn settings_sql() -> String {
    let names = SESSION_SETTINGS.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ");
    format!("select name || '=' || current_setting(name) from unnest(array[{names}]) with ordinality as s(name, n) order by n")
}

/// A string no fixture value holds, different on every run.
fn nonce() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), t)
}

fn marker(nonce: &str, i: usize) -> String {
    format!("@@gt-{nonce}-{i}@@")
}

fn null_mark(nonce: &str) -> String {
    format!("@@gt-{nonce}-NULL@@")
}

/// Runs every `(key, sql)` (each a `select` of one column) in one psql
/// process and returns the column's values per key: `None` is NULL.
pub fn query_all(queries: &[(String, String)]) -> BTreeMap<String, Vec<Option<String>>> {
    query_all_in("", queries)
}

/// As `query_all`, after the statements of `preamble` (for example a `set time
/// zone`), run in the same psql session.
pub fn query_all_in(preamble: &str, queries: &[(String, String)]) -> BTreeMap<String, Vec<Option<String>>> {
    let nonce = nonce();
    let mut script = String::from(preamble);
    script.push('\n');
    for (i, (_, sql)) in queries.iter().enumerate() {
        script.push_str(&format!("\\qecho {}\n{sql};\n", marker(&nonce, i)));
    }
    let mut child = Command::new("docker")
        .args(["exec", "-i", "dbxtui-fixtures-postgres-1", "env", "-u", "PGTZ", "-u", "PGDATESTYLE", "PGCLIENTENCODING=UTF8"])
        .args(["psql", "-q", "-U", "postgres", "-d", "edge", "-At", "-X", "--record-separator-zero", "--field-separator-zero"])
        .arg("-P")
        .arg(format!("null={}", null_mark(&nonce)))
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("docker exec psql");
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let out = child.wait_with_output().expect("psql output");
    writer.join().unwrap().expect("script written");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && stderr.trim().is_empty(), "psql failed: {stderr}");
    let text = String::from_utf8(out.stdout).expect("psql output is UTF-8");
    let keys: Vec<&str> = queries.iter().map(|(k, _)| k.as_str()).collect();
    parse_output(&text, &keys, &nonce)
}

/// The output of a script of `\qecho marker` and query pairs, by key.
pub fn parse_output(text: &str, keys: &[&str], nonce: &str) -> BTreeMap<String, Vec<Option<String>>> {
    let mut result = BTreeMap::new();
    let mut rest = text;
    for (i, key) in keys.iter().enumerate() {
        let header = format!("{}\n", marker(nonce, i));
        let body = if i == 0 {
            rest.strip_prefix(header.as_str()).unwrap_or_else(|| panic!("psql output does not start with the marker of query 0: {:?}", rest.chars().take(60).collect::<String>()))
        } else {
            rest
        };
        let (body, next) = match keys.get(i + 1) {
            Some(_) => {
                let next_header = format!("{}\n", marker(nonce, i + 1));
                let at = body.find(&next_header).unwrap_or_else(|| panic!("no marker for query {}", i + 1));
                (&body[..at], &body[at + next_header.len()..])
            }
            None => (body, ""),
        };
        assert!(result.insert(key.to_string(), records(body, nonce)).is_none(), "query key {key:?} used twice");
        rest = next;
    }
    assert_eq!(result.len(), keys.len(), "every query answered");
    result
}

/// The records of one query's output: none when it is empty (no rows), else
/// the values, each ended by NUL.
fn records(body: &str, nonce: &str) -> Vec<Option<String>> {
    if body.is_empty() {
        return vec![];
    }
    let null = null_mark(nonce);
    let body = body.strip_suffix('\0').expect("every record ends with NUL");
    body.split('\0')
        .map(|r| {
            assert!(!r.contains(&null) || r == null, "a value holds the NULL marker: {r:?}");
            assert!(!r.contains("@@gt-") || r == null, "a value holds a marker: {r:?}");
            if r == null { None } else { Some(r.to_string()) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_bytes_1d_to_1f_newlines_empty_and_null_text_are_each_a_value_of_their_own() {
        let n = "abc";
        let null = null_mark(n);
        let text = format!(
            "{}\nx\u{1d}y\0\u{1e}\0\u{1f}\0\0{null}\0NULL\0a\nb\0{}\n{}\n1\0",
            marker(n, 0),
            marker(n, 1),
            marker(n, 2),
        );
        // Query 1 has no rows: its marker is followed at once by query 2's.
        let got = parse_output(&text, &["a", "none", "c"], n);
        let s = |v: &str| Some(v.to_string());
        assert_eq!(got["a"], vec![s("x\u{1d}y"), s("\u{1e}"), s("\u{1f}"), s(""), None, s("NULL"), s("a\nb")]);
        assert_eq!(got["none"], Vec::<Option<String>>::new());
        assert_eq!(got["c"], vec![s("1")]);
    }

    #[test]
    #[should_panic(expected = "marker")]
    fn a_value_that_holds_a_marker_is_refused() {
        let n = "abc";
        let text = format!("{}\nsay @@gt-abc-NULL@@ twice\0", marker(n, 0));
        parse_output(&text, &["a"], n);
    }

    #[test]
    fn the_settings_query_names_every_setting_in_order() {
        let sql = settings_sql();
        let mut last = 0;
        for name in SESSION_SETTINGS {
            let at = sql.find(&format!("'{name}'")).unwrap_or_else(|| panic!("{name} is read"));
            assert!(at > last);
            last = at;
        }
    }
}
