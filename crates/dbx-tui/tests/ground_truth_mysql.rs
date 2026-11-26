//! Layer 1, slice 23b (ground truth for MySQL): every cell of every table and
//! view of the databases `edge` and `sakila`, read through `DbxCore::run` and
//! through `mysql --xml` in the fixture container (which keeps NULL, `''` and
//! the text `NULL` apart), compared value by value.
//!
//! Needs `tests/fixtures/up-mysql.sh` first. Run:
//! `cargo test -p dbx-tui --test ground_truth_mysql -- --test-threads=1`
//!
//! Every difference is in `tests/fixtures/KNOWN_DIFFERENCES-mysql.md`, the one
//! source of truth: the run fails on a difference that is not listed, on a
//! listed one that no longer occurs, on a listed one whose count or example
//! changed, and on a reason that is still `UNREVIEWED`. `GT_WRITE=1` rewrites
//! the table from what the run finds (a new or changed row gets the reason
//! `UNREVIEWED`) and the run that wrote it FAILS: read the diff, write the
//! reasons, run again without `GT_WRITE`. It is refused when `CI` is set.
//!
//! A difference is one group: the table, the column, the class and the shape
//! of the two texts (digits and letters folded), with how many cells have it
//! and the first example. `FORMAT` is the same value in another spelling,
//! `LOSS` is a value the client shows wrongly or cannot show.
//!
//! What is asked of the server for each kind of column:
//! - `bit`, `binary`, `varbinary` and the blobs: `hex(column)`, since XML
//!   cannot carry the bytes;
//! - `float`: also `cast(column as double)`, which holds the single-precision
//!   value exactly (the `mysql` client prints a FLOAT with six significant
//!   digits, as dbx's driver does);
//! - `double`: its text, which MySQL prints as the shortest spelling that reads
//!   back as the same double, so its bits are known;
//! - everything else: the text, as is.

mod common;

use std::collections::BTreeMap;

use common::*;
use dbx_tui::core::{Cell, ConnectionDraft, ConnectionId, Core, Cursor, DatabasePath, DbKind, DbxCore, RunId, RunRequest, SessionId, TabId};

const CONTAINER: &str = "dbxtui-fixtures-mysql-mysql-1";

fn mysql_args(extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "-i", CONTAINER, "mysql", "-uroot", "-pdbxtui", "--default-character-set=utf8mb4"].map(String::from).to_vec();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// Rows of tab-separated values, as `mysql -N -B` prints them (no NULLs here:
/// only for the catalogue).
fn mysql_rows(sql: &str) -> Vec<Vec<String>> {
    let out = std::process::Command::new("docker").args(mysql_args(&["-N", "-B", "-e", sql])).output().expect("docker exec mysql");
    assert!(out.status.success(), "mysql {sql}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("UTF-8").lines().map(|l| l.split('\t').map(str::to_string).collect()).collect()
}

/// `mysql --xml`'s rows: each field NULL (`xsi:nil`) or its text.
fn mysql_xml(db: &str, sql: &str) -> Vec<Vec<Option<String>>> {
    let out = std::process::Command::new("docker").args(mysql_args(&["--xml", "-e", sql, db])).output().expect("docker exec mysql");
    assert!(out.status.success(), "mysql --xml {sql}: {}", String::from_utf8_lossy(&out.stderr));
    parse_xml(&String::from_utf8(out.stdout).unwrap_or_else(|e| panic!("UTF-8: {e}")))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').unwrap_or_else(|| panic!("an entity never ends: {rest:?}"));
        let entity = &rest[1..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            n if n.starts_with("#x") => out.push(char::from_u32(u32::from_str_radix(&n[2..], 16).expect("hex entity")).expect("a character")),
            n if n.starts_with('#') => out.push(char::from_u32(n[1..].parse().expect("entity")).expect("a character")),
            other => panic!("unknown entity &{other};"),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The rows of `<resultset>` as `mysql --xml` writes them: `<row>` with a
/// `<field name="..">text</field>` or `<field name=".." xsi:nil="true" />`
/// for each column, in order.
fn parse_xml(xml: &str) -> Vec<Vec<Option<String>>> {
    let mut rows = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<row>") {
        let after = &rest[start + 5..];
        let end = after.find("</row>").expect("a row ends");
        let mut body = &after[..end];
        let mut row = Vec::new();
        while let Some(f) = body.find("<field ") {
            body = &body[f..];
            let tag_end = body.find('>').expect("a tag ends");
            let tag = &body[..tag_end];
            if tag.ends_with('/') {
                assert!(tag.contains("xsi:nil=\"true\""), "an empty element is NULL: {tag}");
                row.push(None);
                body = &body[tag_end + 1..];
            } else {
                let close = body.find("</field>").expect("a field ends");
                row.push(Some(unescape(&body[tag_end + 1..close])));
                body = &body[close + 8..];
            }
        }
        rows.push(row);
        rest = &after[end + 6..];
    }
    rows
}

fn quote(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    table: String,
    column: String,
    class: &'static str,
    /// The two texts with digits and letters folded.
    shape: String,
    count: usize,
    example: String,
    note: String,
}

fn fold(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().take(40) {
        let f = if c.is_ascii_digit() { '9' } else if c.is_alphabetic() { 'a' } else { c };
        if !(out.ends_with(f) && (f == '9' || f == 'a')) {
            out.push(f);
        }
    }
    out
}

fn shown(c: &Cell) -> String {
    match c {
        Cell::Null => "NULL".into(),
        Cell::Value(v) => format!("'{v}'"),
    }
}

fn shown_theirs(t: &Option<String>) -> String {
    match t {
        None => "NULL".into(),
        Some(v) => format!("'{v}'"),
    }
}

/// What kind of value a column holds, from `information_schema.columns.data_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bit,
    Bytes,
    Float,
    Double,
    Temporal,
    Other,
}

fn kind_of(data_type: &str) -> Kind {
    match data_type {
        "time" | "datetime" | "timestamp" => Kind::Temporal,
        "bit" => Kind::Bit,
        "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => Kind::Bytes,
        "float" => Kind::Float,
        "double" => Kind::Double,
        _ => Kind::Other,
    }
}

/// What the server holds for one cell, as asked above.
struct Theirs {
    text: Option<String>,
    /// For a FLOAT: its exact value as a double's text.
    exact: Option<String>,
}

/// Whether the client's text `ours` says what the server holds. `None` is
/// equal; `Some(class)` is a difference.
fn judge(kind: Kind, ours: &Cell, theirs: &Theirs) -> Option<&'static str> {
    let (Cell::Value(v), Some(t)) = (ours, &theirs.text) else {
        return if matches!((ours, &theirs.text), (Cell::Null, None)) { None } else { Some("LOSS") };
    };
    match kind {
        // BIT(n) is shown as its binary digits; the server's `hex()` is the same number.
        Kind::Bit => match (u64::from_str_radix(v, 2), u64::from_str_radix(t, 16)) {
            (Ok(a), Ok(b)) if a == b && v.chars().all(|c| c == '0' || c == '1') => Some("FORMAT"),
            _ => Some("LOSS"),
        },
        // Bytes are shown as `0x` and their hex.
        Kind::Bytes => {
            if v.eq_ignore_ascii_case(&format!("0x{t}")) {
                Some("FORMAT")
            } else {
                Some("LOSS")
            }
        }
        // A FLOAT is compared as the single-precision number it is: the exact
        // value the server holds (as a double) against the number the client's
        // text reads as in single precision. Anything else is a LOSS, however
        // close.
        Kind::Float => {
            let exact = theirs.exact.as_deref().and_then(|e| e.parse::<f64>().ok());
            match (v.parse::<f32>(), exact) {
                (Ok(a), Some(b)) if f64::from(a).to_bits() == b.to_bits() => {
                    if v == t {
                        None
                    } else {
                        Some("FORMAT")
                    }
                }
                _ => Some("LOSS"),
            }
        }
        // A DOUBLE: the server prints the shortest text that reads back as the
        // same double, so the same bits is the same value.
        Kind::Double => match (v.parse::<f64>(), t.parse::<f64>()) {
            (Ok(a), Ok(b)) if a.to_bits() == b.to_bits() => {
                if v == t {
                    None
                } else {
                    Some("FORMAT")
                }
            }
            _ => Some("LOSS"),
        },
        // A date or time with fractional seconds: the same instant is the same
        // value, whatever digits of zeros follow the point.
        Kind::Temporal => {
            if v == t {
                None
            } else if strip_fraction_zeros(v) == strip_fraction_zeros(t) {
                Some("FORMAT")
            } else {
                Some("LOSS")
            }
        }
        Kind::Other => {
            if v == t {
                None
            } else {
                Some("LOSS")
            }
        }
    }
}

/// `00:00:01.500000` is `00:00:01.5`, and `2000-02-29 00:00:00.000000` is
/// `2000-02-29 00:00:00`.
fn strip_fraction_zeros(s: &str) -> String {
    match s.split_once('.') {
        Some((whole, fraction)) => {
            let kept = fraction.trim_end_matches('0');
            if kept.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{kept}")
            }
        }
        None => s.to_string(),
    }
}

struct TableInfo {
    name: String,
    columns: Vec<(String, Kind)>,
    key: Vec<String>,
}

fn discover(db: &str) -> Vec<TableInfo> {
    let tables = mysql_rows(&format!("select table_name from information_schema.tables where table_schema = '{db}' and table_type in ('BASE TABLE', 'VIEW') order by table_name"));
    tables
        .into_iter()
        .map(|t| {
            let name = t[0].clone();
            let columns = mysql_rows(&format!("select column_name, data_type from information_schema.columns where table_schema = '{db}' and table_name = '{}' order by ordinal_position", name.replace('\'', "''")))
                .into_iter()
                .map(|c| (c[0].clone(), kind_of(&c[1])))
                .collect();
            let key = mysql_rows(&format!("select column_name from information_schema.statistics where table_schema = '{db}' and table_name = '{}' and index_name = 'PRIMARY' order by seq_in_index", name.replace('\'', "''")))
                .into_iter()
                .map(|c| c[0].clone())
                .collect();
            TableInfo { name, columns, key }
        })
        .collect()
}

async fn compare(core: &DbxCore, id: &ConnectionId, db: &str, groups: &mut BTreeMap<(String, String, String, String), Group>, cells: &mut usize) {
    for (n, t) in discover(db).iter().enumerate() {
        // The same order on both sides: the key, or every column.
        let order = if t.key.is_empty() { (1..=t.columns.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(", ") } else { t.key.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ") };
        let from = format!("{}.{}", quote(db), quote(&t.name));
        let ours_sql = format!("select * from {from} order by {order}");
        let mut list = Vec::new();
        let mut exact_at = Vec::new();
        for (c, (name, kind)) in t.columns.iter().enumerate() {
            let q = quote(name);
            list.push(match kind {
                Kind::Bit | Kind::Bytes => format!("hex({q}) as c{c}"),
                _ => format!("{q} as c{c}"),
            });
        }
        for (c, (name, kind)) in t.columns.iter().enumerate() {
            if *kind == Kind::Float {
                exact_at.push(c);
                list.push(format!("cast({} as double) as x{c}", quote(name)));
            }
        }
        let theirs = mysql_xml(db, &format!("select {} from {from} order by {order}", list.join(", ")));
        let session = SessionId { tab: TabId(1), serial: n as u32 + 1 };
        let request = RunRequest {
            connection: id.clone(),
            database: Some(db.to_string()),
            session,
            buffer: ours_sql.clone(),
            cursor: Cursor { row: 0, col: ours_sql.chars().count() },
            selection: None,
            row_cap: 1_000_000,
            confirm_dangerous: false,
            confirmation: None,
        };
        let ours = core.run(RunId(n as u64 + 1), request).await.unwrap_or_else(|e| panic!("{}: {}", t.name, e.message));
        // One session per table: closed at once, so nothing is left open on the server.
        core.close_session(session, &DatabasePath { connection: id.clone(), database: db.to_string() }).await.expect("the session closes");
        assert!(!ours.truncated);
        assert_eq!(ours.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), t.columns.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), "{}: the columns", t.name);
        assert_eq!(ours.rows.len(), theirs.len(), "{}: the number of rows", t.name);
        for (r, (o_row, t_row)) in ours.rows.iter().zip(&theirs).enumerate() {
            for (c, o) in o_row.iter().enumerate() {
                *cells += 1;
                let exact = exact_at.iter().position(|&e| e == c).map(|i| t_row[t.columns.len() + i].clone()).unwrap_or(None);
                let th = Theirs { text: t_row[c].clone(), exact };
                let Some(class) = judge(t.columns[c].1, o, &th) else { continue };
                let (os, ts) = (shown(o), shown_theirs(&th.text));
                let table = format!("{db}.{}", t.name);
                let shape = match t.columns[c].1 {
                    Kind::Bit => "'<bits>' -> hex".to_string(),
                    Kind::Bytes => "'0x<hex>' -> hex".to_string(),
                    _ => format!("{} -> {}", fold(&os), fold(&ts)),
                };
                let key = (table.clone(), t.columns[c].0.clone(), class.to_string(), shape);
                let g = groups.entry(key.clone()).or_insert_with(|| Group {
                    table,
                    column: t.columns[c].0.clone(),
                    class,
                    shape: key.3.clone(),
                    count: 0,
                    example: format!("row {}: ours `{}`, mysql `{}`{}", r + 1, shorten(&os), shorten(&ts), th.exact.as_ref().map(|e| format!(" (exact {e})")).unwrap_or_default()),
                    note: String::new(),
                });
                g.count += 1;
            }
        }
    }
}

fn shorten(s: &str) -> String {
    let esc: String = s
        .chars()
        .flat_map(|c| match c {
            '|' => vec!['\\', 'x', '7', 'c'],
            '`' => vec!['\\', 'x', '6', '0'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c if c.is_control() => format!("\\u{{{:x}}}", c as u32).chars().collect(),
            c => vec![c],
        })
        .collect();
    if esc.chars().count() > 60 {
        format!("{}…({} chars)", esc.chars().take(60).collect::<String>(), s.chars().count())
    } else {
        esc
    }
}

fn file_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/KNOWN_DIFFERENCES-mysql.md")
}

const START: &str = "<!-- gt:differences:start -->";
const END: &str = "<!-- gt:differences:end -->";

fn render(g: &Group) -> String {
    format!("| {} | {} | {} | `{}` | {} | {} | {} |", g.table, g.column, g.class, g.shape.replace('|', "\\x7c"), g.count, g.example, g.note)
}

/// The groups the file lists, in file order, from its table.
fn listed(text: &str) -> Vec<Group> {
    let body = text.split(START).nth(1).and_then(|t| t.split(END).next()).unwrap_or("");
    body.lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| table ") && !l.starts_with("|---"))
        .map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').splitn(7, " | ").map(str::trim).collect();
            assert_eq!(cells.len(), 7, "a row of seven cells: {l}");
            Group {
                table: cells[0].into(),
                column: cells[1].into(),
                class: if cells[2] == "FORMAT" { "FORMAT" } else { "LOSS" },
                shape: cells[3].trim_matches('`').replace("\\x7c", "|"),
                count: cells[4].parse().expect("a count"),
                example: cells[5].into(),
                note: cells[6].trim_end_matches('|').trim().into(),
            }
        })
        .collect()
}

fn draft() -> ConnectionDraft {
    ConnectionDraft {
        id: None,
        name: "my".into(),
        kind: DbKind::MySql,
        host: "127.0.0.1".into(),
        port: Some(53307),
        database: Some("edge".into()),
        username: "root".into(),
        password: Some("dbxtui".into()),
        tls: false,
        production: false,
    }
}

#[tokio::test]
async fn every_cell_of_the_edge_and_sakila_databases_is_compared_with_mysql_and_every_difference_is_listed_and_reviewed() {
    let up = tokio::net::TcpStream::connect("127.0.0.1:53307").await.is_ok();
    assert!(up, "the fixture MySQL is not listening on 127.0.0.1:53307: run crates/dbx-tui/tests/fixtures/up-mysql.sh");
    let s = Scratch::new();
    let core = s.core();
    let id = core.save_connection(&draft()).await.expect("save").id;
    let mut groups = BTreeMap::new();
    let mut cells = 0;
    for db in ["edge", "sakila"] {
        compare(&core, &id, db, &mut groups, &mut cells).await;
    }
    let version = mysql_rows("select version()")[0][0].clone();
    eprintln!("compared {cells} cells; {} groups of differences; MySQL {version}", groups.len());
    assert!(cells > 300_000, "{cells} cells compared");
    let found: Vec<Group> = groups.into_values().collect();
    let path = file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let known = listed(&text);
    if std::env::var_os("GT_WRITE").is_some() {
        assert!(std::env::var_os("CI").is_none(), "GT_WRITE is refused in CI");
        let mut rows = Vec::new();
        for g in &found {
            let mut g = g.clone();
            let same = known.iter().find(|k| k.table == g.table && k.column == g.column && k.class == g.class && k.shape == g.shape);
            g.note = match same {
                Some(k) if k.count == g.count && k.example == g.example => k.note.clone(),
                _ => "UNREVIEWED".into(),
            };
            rows.push(render(&g));
        }
        let head = text.split(START).next().unwrap_or("").to_string();
        let tail = text.split(END).nth(1).unwrap_or("").to_string();
        let table = format!("| table | column | class | shape (ours -> mysql) | cells | first example | why |\n|---|---|---|---|---|---|---|\n{}", rows.join("\n"));
        std::fs::write(&path, format!("{head}{START}\n{table}\n{END}{tail}")).expect("write the file");
        panic!("GT_WRITE: {} groups written to {}; review the diff, write the reasons and run again", found.len(), path.display());
    }
    let mut problems = Vec::new();
    for g in &found {
        match known.iter().find(|k| k.table == g.table && k.column == g.column && k.class == g.class && k.shape == g.shape) {
            None => problems.push(format!("not listed: {}", render(g))),
            Some(k) if k.count != g.count || k.example != g.example => problems.push(format!("changed: listed `{}`; found `{}`", render(k), render(g))),
            Some(k) if k.note.contains("UNREVIEWED") || k.note.is_empty() => problems.push(format!("not reviewed: {}", render(k))),
            Some(_) => {}
        }
    }
    for k in &known {
        if !found.iter().any(|g| g.table == k.table && g.column == k.column && g.class == k.class && g.shape == k.shape) {
            problems.push(format!("listed but no longer found: {}", render(k)));
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}
