//! Layer 1, slice 23a (ground truth for SQLite): every cell of every table and
//! view of `edge.db` and `sakila.db`, read through `DbxCore::run` and through
//! `sqlite3 -quote` (which keeps NULL, `''` and `'NULL'` apart and prints
//! blobs as `X'..'`), compared value by value. No Docker.
//!
//! Run: `cargo test -p dbx-tui --test ground_truth_sqlite -- --test-threads=1`
//!
//! Every difference is in `tests/fixtures/KNOWN_DIFFERENCES-sqlite.md`, the one
//! source of truth: the run fails on a difference that is not listed, on a
//! listed one that no longer occurs, on a listed one whose count or example
//! changed, and on a reason that is still `UNREVIEWED`. `GT_WRITE=1` rewrites
//! the table from what the run finds (a new or changed row gets the reason
//! `UNREVIEWED`) and the run that wrote it FAILS: read the diff, write the
//! reasons, run again without `GT_WRITE`. It is refused when `CI` is set.
//!
//! A difference is one group: the table, the column, the class and the shape of
//! the two texts (digits and letters folded), with how many cells have it and
//! the first example. `FORMAT` is the same value in another spelling, `LOSS` is
//! a value the client shows wrongly or cannot show.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::*;
use dbx_tui::core::{Cell, ConnectionId, Core, Cursor, DbxCore, RunId, RunRequest, SessionId, TabId};

const SQLITE3: &str = "/usr/bin/sqlite3";

/// What `sqlite3 -quote` says one value is.
#[derive(Debug, Clone, PartialEq)]
enum Theirs {
    Null,
    Integer(String),
    Real(f64, String),
    Text(String),
    Blob(String),
}

impl Theirs {
    fn show(&self) -> String {
        match self {
            Theirs::Null => "NULL".into(),
            Theirs::Integer(s) => s.clone(),
            Theirs::Real(_, s) => s.clone(),
            Theirs::Text(s) => format!("'{s}'"),
            Theirs::Blob(h) => format!("X'{h}'"),
        }
    }
}

/// Parses the output of `sqlite3 -quote`: values separated by commas, rows by
/// line breaks outside quotes.
fn parse_quote(out: &str) -> Vec<Vec<Theirs>> {
    let chars: Vec<char> = out.chars().collect();
    let (mut rows, mut row, mut i) = (Vec::new(), Vec::new(), 0);
    while i < chars.len() {
        // Text with control characters comes as `unistr('a\u000db')`.
        let unistr = chars[i..].starts_with(&['u', 'n', 'i', 's', 't', 'r', '(']);
        let value = if unistr || chars[i] == '\'' {
            let mut text = String::new();
            i += if unistr { 8 } else { 1 };
            loop {
                match chars[i] {
                    '\\' if unistr => {
                        let (len, skip) = match chars[i + 1] {
                            '\\' => {
                                text.push('\\');
                                i += 2;
                                continue;
                            }
                            'u' => (4, 2),
                            'U' => (8, 2),
                            '+' => (6, 2),
                            _ => (4, 1),
                        };
                        let hex: String = chars[i + skip..i + skip + len].iter().collect();
                        text.push(char::from_u32(u32::from_str_radix(&hex, 16).expect("hex")).expect("a character"));
                        i += skip + len;
                    }
                    '\'' if chars.get(i + 1) == Some(&'\'') => {
                        text.push('\'');
                        i += 2;
                    }
                    '\'' => {
                        i += if unistr { 2 } else { 1 };
                        break;
                    }
                    c => {
                        text.push(c);
                        i += 1;
                    }
                }
            }
            // A real is asked for as its eight bytes (`ieee754_to_blob`, in
            // hex) and, to show, as `printf('%.17g')`: sqlite3's own quote()
            // prints some doubles one step off (1e300 as 9.99..e299).
            match text.strip_prefix("\u{1}R") {
                Some(real) => {
                    let (bits, shown) = real.split_once(' ').unwrap_or_else(|| panic!("bits and a spelling: {real:?}"));
                    let bits = u64::from_str_radix(bits, 16).unwrap_or_else(|_| panic!("the bits of a real: {real:?}"));
                    Theirs::Real(f64::from_bits(bits), shown.to_string())
                }
                None => Theirs::Text(text),
            }
        } else if chars[i] == 'X' && chars.get(i + 1) == Some(&'\'') {
            let end = chars[i + 2..].iter().position(|&c| c == '\'').expect("a closed blob") + i + 2;
            let hex: String = chars[i + 2..end].iter().collect();
            i = end + 1;
            Theirs::Blob(hex)
        } else {
            let end = chars[i..].iter().position(|&c| c == ',' || c == '\n').map_or(chars.len(), |p| p + i);
            let word: String = chars[i..end].iter().collect();
            i = end;
            match word.as_str() {
                "NULL" => Theirs::Null,
                w if w.parse::<i64>().is_ok() => Theirs::Integer(word),
                // Every real is asked for by its bits (above), never printed bare.
                w => panic!("a bare value that is not an integer: {w:?}"),
            }
        };
        row.push(value);
        match chars.get(i) {
            Some(',') => i += 1,
            Some('\n') => {
                rows.push(std::mem::take(&mut row));
                i += 1;
            }
            None => rows.push(std::mem::take(&mut row)),
            Some(c) => panic!("unexpected {c:?} after a value"),
        }
    }
    rows
}

fn sqlite3_quote(db: &Path, sql: &str) -> Vec<Vec<Theirs>> {
    let out = std::process::Command::new(SQLITE3).args(["-batch", "-readonly", "-quote"]).arg(db).arg(sql).output().expect("sqlite3");
    assert!(out.status.success(), "sqlite3 {sql}: {}", String::from_utf8_lossy(&out.stderr));
    parse_quote(&String::from_utf8(out.stdout).expect("UTF-8"))
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
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

/// The text of a cell as the file shows it: NULL bare, anything else quoted.
fn ours_text(c: &Cell) -> String {
    match c {
        Cell::Null => "NULL".into(),
        Cell::Value(v) => format!("'{v}'"),
    }
}

/// Whether the client's text `ours` says the value `theirs` does.
/// `None` is equal; `Some(class)` is a difference.
fn judge(ours: &Cell, theirs: &Theirs) -> Option<&'static str> {
    match (ours, theirs) {
        (Cell::Null, Theirs::Null) => None,
        (Cell::Null, _) => Some("LOSS"),
        (Cell::Value(_), Theirs::Null) => Some("LOSS"),
        (Cell::Value(v), Theirs::Text(t)) => {
            if v == t {
                None
            } else {
                Some("LOSS")
            }
        }
        (Cell::Value(v), Theirs::Integer(t)) => {
            if v == t {
                None
            } else if v.parse::<i64>().map(|n| n.to_string() == *t).unwrap_or(false) {
                Some("FORMAT")
            } else {
                Some("LOSS")
            }
        }
        (Cell::Value(v), Theirs::Real(f, t)) => match v.parse::<f64>() {
            // The same double, bit for bit, as the eight bytes the file
            // holds: any other is a LOSS, however close.
            Ok(n) if n.to_bits() == f.to_bits() => {
                if v == t {
                    None
                } else {
                    Some("FORMAT")
                }
            }
            _ => Some("LOSS"),
        },
        (Cell::Value(v), Theirs::Blob(hex)) => {
            if v.eq_ignore_ascii_case(&format!("0x{hex}")) {
                Some("FORMAT")
            } else {
                Some("LOSS")
            }
        }
    }
}

struct Table {
    name: String,
    columns: Vec<String>,
    order: String,
}

fn discover(db: &Path) -> Vec<Table> {
    let listing = String::from_utf8(
        std::process::Command::new(SQLITE3)
            .args(["-batch", "-readonly", "-separator", "\t"])
            .arg(db)
            .arg("select name, type from sqlite_master where type in ('table', 'view') and name not like 'sqlite_%' order by name")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let mut tables = Vec::new();
    for line in listing.lines() {
        let (name, kind) = line.split_once('\t').expect("name and type");
        let info = sqlite3_quote(db, &format!("select name from pragma_table_info({})", quote_str(name)));
        let columns: Vec<String> = info.iter().map(|r| if let Theirs::Text(t) = &r[0] { t.clone() } else { panic!("a name") }).collect();
        let order = if kind == "view" { columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ") } else { "rowid".to_string() };
        tables.push(Table { name: name.into(), columns, order });
    }
    tables
}

fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

async fn compare(core: &DbxCore, id: &ConnectionId, db: &Path, label: &str, groups: &mut BTreeMap<(String, String, String, String), Group>, cells: &mut usize) {
    for (n, t) in discover(db).iter().enumerate() {
        let sql = format!("select * from {} order by {}", quote(&t.name), t.order);
        let list: Vec<String> = t
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!("case when typeof({q}) = 'real' then char(1) || 'R' || hex(ieee754_to_blob({q})) || ' ' || printf('%.17g', {q}) else {q} end as c{i}", q = quote(c))
            })
            .collect();
        let theirs = sqlite3_quote(db, &format!("select {} from {} order by {}", list.join(", "), quote(&t.name), t.order));
        let request = RunRequest {
            connection: id.clone(),
            database: Some(String::new()),
            session: SessionId { tab: TabId(1), serial: n as u32 + 1 },
            buffer: sql.clone(),
            cursor: Cursor { row: 0, col: sql.chars().count() },
            selection: None,
            row_cap: 1_000_000,
            confirm_dangerous: false,
            confirmation: None,
        };
        let ours = core.run(RunId(n as u64 + 1), request).await.unwrap_or_else(|e| panic!("{}: {}", t.name, e.message));
        assert!(!ours.truncated);
        assert_eq!(ours.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), t.columns.iter().map(String::as_str).collect::<Vec<_>>(), "{}: the columns", t.name);
        assert_eq!(ours.rows.len(), theirs.len(), "{}: the number of rows", t.name);
        for (r, (o_row, t_row)) in ours.rows.iter().zip(&theirs).enumerate() {
            for (c, (o, th)) in o_row.iter().zip(t_row).enumerate() {
                *cells += 1;
                let Some(class) = judge(o, th) else { continue };
                let (os, ts) = (ours_text(o), th.show());
                let table = format!("{label}.{}", t.name);
                let shape = if matches!(th, Theirs::Blob(_)) { "'0x<hex>' -> X'<hex>'".to_string() } else { format!("{} -> {}", fold(&os), fold(&ts)) };
                let key = (table.clone(), t.columns[c].clone(), class.to_string(), shape);
                let g = groups.entry(key.clone()).or_insert_with(|| Group {
                    table,
                    column: t.columns[c].clone(),
                    class,
                    shape: key.3.clone(),
                    count: 0,
                    example: format!("row {}: ours `{}`, sqlite3 `{}`", r + 1, shorten(&os), shorten(&ts)),
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/KNOWN_DIFFERENCES-sqlite.md")
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

#[tokio::test]
async fn every_cell_of_the_edge_and_sakila_files_is_compared_with_sqlite3_and_every_difference_is_listed_and_reviewed() {
    let s = Scratch::new();
    let core = s.core();
    let mut groups = BTreeMap::new();
    let mut cells = 0;
    let mut version = String::new();
    for name in ["edge", "sakila"] {
        let (_dir, file) = sqlite_copy(name);
        let id = core.save_connection(&sqlite_draft(name, &file)).await.expect("save").id;
        compare(&core, &id, &file, name, &mut groups, &mut cells).await;
        version = std::fs::read_to_string(sqlite_fixture_dir().join("sqlite3-version.txt")).unwrap_or_default();
    }
    eprintln!("compared {cells} cells; {} groups of differences; sqlite3 {}", groups.len(), version.trim());
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
        let table = format!("| table | column | class | shape (ours -> sqlite3) | cells | first example | why |\n|---|---|---|---|---|---|---|\n{}", rows.join("\n"));
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
