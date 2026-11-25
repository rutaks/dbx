//! Layer 1, slice 08 (ground truth): every cell of every table and view in
//! every user schema of the edge database, read through `DbxCore::run` and
//! through `psql -At`, compared after the normalisation documented per type
//! family in `normalise`.
//!
//! Needs the fixtures (`tests/fixtures/up.sh`) and the `dbxtui-fixtures-
//! postgres-1` container (the harness runs `psql` inside it with
//! `docker exec`). Run:
//! `cargo test -p dbx-tui --test ground_truth -- --test-threads=1`
//!
//! Every difference is listed in `tests/fixtures/KNOWN_DIFFERENCES.md`, the one
//! source of truth: the harness fails on a difference that is not listed, on a
//! listed one that no longer occurs, and on a note that is `UNREVIEWED`.
//! `GT_WRITE=1` rewrites the file from what the harness finds (a new or
//! changed row gets the note `UNREVIEWED`) and the run that wrote it FAILS: read
//! the diff, write the reasons, re-run without `GT_WRITE`. It is refused when
//! `CI` is set.
//!
//! The session is pinned and checked: the settings that change how a value
//! prints are read through both sides and must be equal (and are recorded at
//! the top of the file). The `gt_temporal` table is run a second time under
//! `Asia/Kolkata`, on both sides.
//!
//! Scratch store, key and HOME as in `live.rs` (see `common/mod.rs`).

mod common;
#[path = "ground_truth/known.rs"]
mod known;
#[path = "ground_truth/normalise.rs"]
mod normalise;
#[path = "ground_truth/psql.rs"]
mod psql;

use common::*;
use dbx_tui::app::probe::GridProbe;
use dbx_tui::core::{Cell, ConnectionDraft, ConnectionId, Core, Cursor, DatabasePath, DbxCore, ResultSet, RunId, RunRequest, SessionId, TabId};
use known::{esc, show, Entry};
use normalise::Verdict;

const FIXTURE_PASSWORD: &str = "dbxtui";

fn fixture(name: &str) -> ConnectionDraft {
    ConnectionDraft { password: Some(FIXTURE_PASSWORD.into()), production: false, ..draft(name) }
}

async fn require_fixtures() {
    let up = tokio::net::TcpStream::connect("127.0.0.1:55433").await.is_ok();
    assert!(up, "the fixture Postgres is not listening on 127.0.0.1:55433: run crates/dbx-tui/tests/fixtures/up.sh");
}

/// One run of `sql` on database `edge` in the harness's session number `serial`.
async fn run_core(core: &DbxCore, id: &ConnectionId, n: &mut u64, serial: u32, sql: &str, cap: usize) -> Result<dbx_tui::core::ResultSet, String> {
    *n += 1;
    let lines: Vec<&str> = sql.split('\n').collect();
    let request = RunRequest {
        connection: id.clone(),
        database: Some("edge".into()),
        session: SessionId { tab: TabId(1), serial },
        buffer: sql.into(),
        cursor: Cursor { row: lines.len() - 1, col: lines.last().unwrap().chars().count() },
        selection: None,
        row_cap: cap,
        confirm_dangerous: false, // The gate has its own tests (slice 17).
        confirmation: None,
    };
    core.run(RunId(*n), request).await.map_err(|e| e.message)
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// What the harness knows of one table or view: its columns (name and the
/// Postgres type name psql's catalog gives, `udt_name`) and the `order by`
/// that gives its rows a fixed order.
#[derive(Debug, Clone)]
struct Table {
    schema: String,
    name: String,
    columns: Vec<(String, String)>,
    order: String,
    /// SQL for each row's label.
    label_sql: String,
}

impl Table {
    fn qualified(&self) -> String {
        format!("{}.{}", quote(&self.schema), quote(&self.name))
    }
    fn display(&self) -> String {
        if self.schema == "public" { self.name.clone() } else { format!("{}.{}", self.schema, self.name) }
    }
}

/// The schemas the harness reads: every one that is not a system schema.
const USER_SCHEMAS: &str = "table_schema not in ('pg_catalog', 'information_schema') and left(table_schema, 3) <> 'pg_'";

/// The `order by` columns of a table: its primary key; else, for a view, its
/// `id` column; else every column.
fn order_columns(pk: &[String], columns: &[(String, String)], is_view: bool) -> Vec<String> {
    if !pk.is_empty() {
        pk.to_vec()
    } else if is_view && columns.iter().any(|(c, _)| c == "id") {
        vec!["id".to_string()]
    } else {
        columns.iter().map(|(c, _)| c.clone()).collect()
    }
}

fn discover() -> Vec<Table> {
    let listing = psql::query_all(&[
        ("tables".to_string(), format!("select table_schema || E'\\t' || table_name || E'\\t' || table_type from information_schema.tables where {USER_SCHEMAS} and table_type in ('BASE TABLE', 'VIEW') order by 1")),
        // What information_schema does not list: they would be skipped silently.
        ("others".to_string(), "select n.nspname || '.' || c.relname || ' (' || c.relkind::text || ')' from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.relkind in ('m', 'f', 'p') and n.nspname not in ('pg_catalog', 'information_schema') and left(n.nspname, 3) <> 'pg_'".to_string()),
    ]);
    let others: Vec<&String> = listing["others"].iter().flatten().collect();
    assert!(others.is_empty(), "relations the harness does not read (materialized views, foreign or partitioned tables): {others:?}");
    let mut tables = Vec::new();
    for line in listing["tables"].iter().flatten() {
        let mut parts = line.split('\t');
        let (schema, name, kind) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        tables.push((schema.to_string(), name.to_string(), kind == "VIEW"));
    }
    let mut queries = Vec::new();
    for (schema, name, _) in &tables {
        let lit = |s: &str| s.replace('\'', "''");
        queries.push((format!("cols {schema}.{name}"), format!(
            "select column_name || E'\\t' || udt_name from information_schema.columns where table_schema = '{}' and table_name = '{}' order by ordinal_position",
            lit(schema), lit(name))));
        queries.push((format!("pk {schema}.{name}"), format!(
            "select a.attname from pg_index i join pg_attribute a on a.attrelid = i.indrelid and a.attnum = any(i.indkey) where i.indisprimary and i.indrelid = '\"{}\".\"{}\"'::regclass order by array_position(i.indkey::int2[], a.attnum)",
            lit(&schema.replace('"', "\"\"")), lit(&name.replace('"', "\"\"")))));
    }
    let info = psql::query_all(&queries);
    tables
        .into_iter()
        .map(|(schema, name, is_view)| {
            let columns: Vec<(String, String)> = info[&format!("cols {schema}.{name}")]
                .iter()
                .flatten()
                .map(|l| {
                    let (c, t) = l.split_once('\t').unwrap();
                    (c.to_string(), t.to_string())
                })
                .collect();
            let pk: Vec<String> = info[&format!("pk {schema}.{name}")].iter().flatten().cloned().collect();
            let order = order_columns(&pk, &columns, is_view).iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
            let label_sql = if columns.iter().any(|(c, _)| c == "label") {
                "label".to_string()
            } else {
                format!("'row ' || row_number() over (order by {order})")
            };
            Table { schema, name, columns, order, label_sql }
        })
        .collect()
}

// ---- gathering -----------------------------------------------------------------

/// One column of one table: its Postgres type as psql's catalog names it,
/// what psql prints for it and what the Core gave for it alone and beside a
/// `timestamptz` column.
struct ColData {
    name: String,
    pg_type: String,
    truth: Vec<Option<String>>,
    alone: Result<ResultSet, String>,
    beside: Result<ResultSet, String>,
}

struct TableData {
    display: String,
    labels: Vec<String>,
    columns: Vec<ColData>,
}

/// The settings that change how a value prints, as each side reads them.
struct Settings {
    core: Vec<Option<String>>,
    psql: Vec<Option<String>>,
}

struct Gathered {
    tables: Vec<TableData>,
    /// Differences found, in the order of tables, columns, modes and rows.
    diffs: Vec<Entry>,
    /// Cells compared, alone and beside a timestamptz column.
    compared: (usize, usize),
    /// The million-row table: what was compared and what disagreed.
    million: Million,
    /// The session settings of the main run, and of the run under `ZONE`.
    settings: Settings,
    zone_settings: Settings,
    took: std::time::Duration,
}

struct Million {
    count: (Option<String>, Option<String>),
    cells: usize,
    disagreements: Vec<String>,
}

const ALONE: &str = "alone";
const BESIDE: &str = "beside tstz";
const TZ_COLUMN: &str = ", '2000-01-01 00:00:00+00'::timestamptz as \"_tz\"";
/// The table run a second time under a session time zone that is not UTC and
/// has a half-hour offset, on both sides.
const ZONE: &str = "Asia/Kolkata";
const ZONE_TABLE: &str = "gt_temporal";

fn cells_of(set: &ResultSet) -> Vec<Option<String>> {
    set.rows.iter().map(|r| match &r[0] { Cell::Null => None, Cell::Value(v) => Some(v.clone()) }).collect()
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Format => "FORMAT",
        Verdict::Loss => "LOSS",
        Verdict::Same => unreachable!("a cell that is the same is not a difference"),
    }
}

/// The differences between the Core's answer in `mode` for a column and psql's
/// `truth`. The rule that judges a cell is chosen by `pg_type`, the type psql's
/// catalog names for the column, never by what the Core says it is; a Core
/// that reports another type is listed as a difference of its own (`TYPE`).
fn compare_column(table: &str, labels: &[String], column: &str, pg_type: &str, truth: &[Option<String>], mode: &str, got: &Result<ResultSet, String>) -> (Vec<Entry>, usize) {
    let entry = |row: &str, ours: String, theirs: String, class: &str| Entry {
        table: esc(table),
        row: esc(row),
        column: esc(column),
        mode: mode.into(),
        ours,
        theirs,
        class: class.into(),
        note: String::new(),
    };
    match got {
        Err(message) => (vec![entry("*", format!("error: {}", esc(message.lines().next().unwrap_or(""))), "a value".into(), "LOSS")], 0),
        Ok(set) => {
            let ours = cells_of(set);
            if ours.len() != truth.len() {
                return (vec![entry("*", format!("{} rows", ours.len()), format!("{} rows", truth.len()), "LOSS")], 0);
            }
            let mut out = vec![];
            let kind = set.columns[0].kind;
            if normalise::type_mismatch(pg_type, &set.columns[0].db_type).is_some() {
                out.push(entry("*", esc(&set.columns[0].db_type), esc(pg_type), "TYPE"));
            }
            for (i, (o, t)) in ours.iter().zip(truth).enumerate() {
                match normalise::classify(kind, pg_type, o, t) {
                    Verdict::Same => {}
                    v => out.push(entry(&labels[i], show(o), show(t), verdict_name(v))),
                }
            }
            (out, ours.len())
        }
    }
}

/// The settings through the Core, one `name=value` per setting.
async fn core_settings(core: &DbxCore, id: &ConnectionId, n: &mut u64, serial: u32) -> Vec<Option<String>> {
    cells_of(&run_core(core, id, n, serial, &psql::settings_sql(), 100).await.expect("the Core reads the session settings"))
}

fn psql_settings(preamble: &str) -> Vec<Option<String>> {
    psql::query_all_in(preamble, &[("s".to_string(), psql::settings_sql())]).remove("s").unwrap()
}

/// One table: every column through the Core in session `serial`, against what
/// psql prints (after `preamble`).
async fn gather_table(core: &DbxCore, id: &ConnectionId, n: &mut u64, serial: u32, t: &Table, preamble: &str, display: String) -> (TableData, Vec<Entry>, (usize, usize)) {
    let mut qs = vec![("labels".to_string(), format!("select {} from {} order by {}", t.label_sql, t.qualified(), t.order))];
    for (c, _) in &t.columns {
        qs.push((c.clone(), format!("select {} from {} order by {}", quote(c), t.qualified(), t.order)));
    }
    let mut truth = psql::query_all_in(preamble, &qs);
    let labels: Vec<String> = truth.remove("labels").unwrap().into_iter().map(|l| l.expect("a label")).collect();
    let mut seen = std::collections::BTreeSet::new();
    for l in &labels {
        assert!(seen.insert(l), "table {display}: two rows labelled {l:?}");
    }
    let mut columns = vec![];
    for (c, ty) in &t.columns {
        let sql = |extra: &str| format!("select {}{extra} from {} order by {}", quote(c), t.qualified(), t.order);
        let alone = run_core(core, id, n, serial, &sql(""), 2_000).await;
        let beside = run_core(core, id, n, serial, &sql(TZ_COLUMN), 2_000).await;
        columns.push(ColData { name: c.clone(), pg_type: ty.clone(), truth: truth.remove(c).unwrap(), alone, beside });
    }
    let (mut diffs, mut compared) = (vec![], (0, 0));
    for col in &columns {
        if let Ok(set) = &col.beside {
            assert_eq!(set.columns.len(), 2, "{display}.{}: the tz column came back too", col.name);
            assert!(set.rows.iter().all(|r| matches!(&r[1], Cell::Value(v) if v.starts_with("2000-01-01"))), "{display}.{}: the tz column's values", col.name);
        }
        for (mode, got) in [(ALONE, &col.alone), (BESIDE, &col.beside)] {
            let (d, count) = compare_column(&display, &labels, &col.name, &col.pg_type, &col.truth, mode, got);
            diffs.extend(d);
            if mode == ALONE { compared.0 += count } else { compared.1 += count }
        }
    }
    (TableData { display, labels, columns }, diffs, compared)
}

/// Queries whose columns have no table to take a type from: one column named
/// `c`, rows in a fixed order. Each is read through both sides like a table,
/// as `query: <name>`; the type that judges it is the one psql's catalog gives
/// the value (`pg_type.typname` of `pg_typeof(c)`).
const COMPUTED: [(&str, &str); 9] = [
    ("sum of numeric(40,20)", "select sum(n4020) as c from gt_numerics where n4020 <> 'NaN'"),
    ("sum of bigint", "select sum(i8) as c from gt_integers where i8 > 0"),
    ("avg of integer", "select avg(i4) as c from gt_integers"),
    ("count(*)", "select count(*) as c from gt_integers"),
    ("row(1, 'a')", "select row(1, 'a') as c"),
    ("a row of mixed types", "select row(1, 'a', null::int, 2.50::numeric, '2024-03-05 13:45:12+02'::timestamptz, array[1,2]) as c"),
    ("array_agg of bigint", "select array_agg(i8 order by id) as c from gt_integers where i8 is not null"),
    ("json_agg", "select json_agg(label order by id) as c from gt_integers"),
    ("jsonb_agg", "select jsonb_agg(label order by id) as c from gt_integers"),
];

/// One computed query, alone and beside a `timestamptz` column.
async fn gather_query(core: &DbxCore, id: &ConnectionId, n: &mut u64, name: &str, sql: &str) -> (TableData, Vec<Entry>, (usize, usize)) {
    let display = format!("query: {name}");
    let beside_sql = format!("select q.c{TZ_COLUMN} from ({sql}) q");
    let qs = vec![
        ("c".to_string(), sql.to_string()),
        ("type".to_string(), format!("select t.typname from pg_type t where t.oid = (select pg_typeof(q.c)::oid from ({sql}) q limit 1)")),
    ];
    let mut truth = psql::query_all(&qs);
    let pg_type = truth.remove("type").unwrap().remove(0).expect("a type name");
    let truth = truth.remove("c").unwrap();
    let labels: Vec<String> = (1..=truth.len()).map(|i| format!("row {i}")).collect();
    let alone = run_core(core, id, n, 1, sql, 2_000).await;
    let beside = run_core(core, id, n, 1, &beside_sql, 2_000).await;
    let (mut diffs, mut compared) = (vec![], (0, 0));
    for (mode, got) in [(ALONE, &alone), (BESIDE, &beside)] {
        let (d, count) = compare_column(&display, &labels, "c", &pg_type, &truth, mode, got);
        diffs.extend(d);
        if mode == ALONE { compared.0 += count } else { compared.1 += count }
    }
    (TableData { display, labels, columns: vec![ColData { name: "c".into(), pg_type, truth, alone, beside }] }, diffs, compared)
}

async fn gather() -> Gathered {
    let started = std::time::Instant::now();
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let id = core.save_connection(&fixture("ground truth")).await.expect("save").id;
    let mut n = 0u64;
    let settings = Settings { core: core_settings(&core, &id, &mut n, 1).await, psql: psql_settings("") };
    let all = discover();
    let mut tables = vec![];
    let mut diffs = vec![];
    let mut compared = (0, 0);
    for t in all.iter().filter(|t| t.name != "million_rows") {
        let (data, d, c) = gather_table(&core, &id, &mut n, 1, t, "", t.display()).await;
        tables.push(data);
        diffs.extend(d);
        compared = (compared.0 + c.0, compared.1 + c.1);
    }
    for (name, sql) in COMPUTED {
        let (data, d, c) = gather_query(&core, &id, &mut n, name, sql).await;
        tables.push(data);
        diffs.extend(d);
        compared = (compared.0 + c.0, compared.1 + c.1);
    }
    // The temporal table again, in a session whose zone is not UTC: a second
    // Core session, so the first stays as it was.
    let set_zone = format!("set time zone '{ZONE}'");
    run_core(&core, &id, &mut n, 2, &set_zone, 10).await.expect("set time zone through the Core");
    let zone_settings = Settings { core: core_settings(&core, &id, &mut n, 2).await, psql: psql_settings(&format!("{set_zone};")) };
    let zoned = all.iter().find(|t| t.name == ZONE_TABLE && t.schema == "public").expect("the temporal table");
    let (data, d, c) = gather_table(&core, &id, &mut n, 2, zoned, &format!("{set_zone};"), format!("{} @ {ZONE}", zoned.display())).await;
    tables.push(data);
    diffs.extend(d);
    compared = (compared.0 + c.0, compared.1 + c.1);
    let million = million(&core, &id, &mut n, all.iter().find(|t| t.name == "million_rows").expect("the million-row table")).await;
    for serial in [1, 2] {
        let _ = core.close_session(SessionId { tab: TabId(1), serial }, &DatabasePath { connection: id.clone(), database: "edge".into() }).await;
    }
    Gathered { tables, diffs, compared, million, settings, zone_settings, took: started.elapsed() }
}

/// The million-row table is compared by its count and by a sample of rows.
async fn million(core: &DbxCore, id: &ConnectionId, n: &mut u64, table: &Table) -> Million {
    let one = |set: ResultSet| cells_of(&set).remove(0);
    let ours_count = one(run_core(core, id, n, 1, "select count(*) from million_rows", 10).await.expect("count"));
    let sample = "id in (1, 2, 3, 100, 1000, 12345, 499999, 500000, 500001, 999998, 999999, 1000000) or id % 99991 = 0";
    let mut qs = vec![("count".to_string(), "select count(*) from million_rows".to_string())];
    for c in ["id", "txt", "num"] {
        qs.push((c.to_string(), format!("select {c} from million_rows where {sample} order by id")));
    }
    let truth = psql::query_all(&qs);
    let mut disagreements = vec![];
    let mut cells = 0;
    for c in ["id", "txt", "num"] {
        let set = run_core(core, id, n, 1, &format!("select {c} from million_rows where {sample} order by id"), 2_000).await.expect("sample");
        let pg_type = &table.columns.iter().find(|(name, _)| name == c).expect("a column of the million-row table").1;
        if let Some(why) = normalise::type_mismatch(pg_type, &set.columns[0].db_type) {
            disagreements.push(format!("million_rows.{c}: {why}"));
        }
        let (ours, theirs) = (cells_of(&set), &truth[c]);
        if ours.len() != theirs.len() {
            disagreements.push(format!("{c}: {} sampled rows against psql's {}", ours.len(), theirs.len()));
            continue;
        }
        for (i, (o, t)) in ours.iter().zip(theirs).enumerate() {
            cells += 1;
            if normalise::classify(set.columns[0].kind, pg_type, o, t) != Verdict::Same {
                disagreements.push(format!("million_rows, sampled row {i}, column {c}: we show {}, psql shows {}", show(o), show(t)));
            }
        }
    }
    Million { count: (ours_count, truth["count"][0].clone()), cells, disagreements }
}

static GATHERED: tokio::sync::OnceCell<Gathered> = tokio::sync::OnceCell::const_new();

async fn gathered() -> &'static Gathered {
    GATHERED.get_or_init(gather).await
}

// ---- the known differences -------------------------------------------------------

/// The session settings as the file records them: one row per setting for the
/// main run, and the zone of the second run.
fn settings_rows(g: &Gathered) -> Vec<String> {
    let mut rows: Vec<String> = g.settings.psql.iter().flatten().map(|s| {
        let (name, value) = s.split_once('=').expect("name=value");
        format!("| {name} | {} |", esc(value))
    }).collect();
    let zone = g.zone_settings.psql.iter().flatten().find(|s| s.starts_with("TimeZone=")).expect("a TimeZone setting").trim_start_matches("TimeZone=").to_string();
    rows.push(format!("| TimeZone, run of {ZONE_TABLE} @ {ZONE} | {} |", esc(&zone)));
    rows
}

#[tokio::test]
async fn every_cell_of_every_table_matches_psql_or_is_listed_in_the_known_differences() {
    let g = gathered().await;
    if known::write_requested() {
        known::guard_from_env();
        let text = known::read();
        let rows: Vec<String> = known::merge_notes(&g.diffs, &known::listed_entries(&text)).iter().map(Entry::render).collect();
        let text = known::replace_rows(&text, "differences", &rows);
        let text = known::replace_rows(&text, "settings", &settings_rows(g));
        std::fs::write(known::file_path(), text).expect("write KNOWN_DIFFERENCES.md");
        panic!("{}", known::written_message());
    }
    let text = known::read();
    let listed = known::listed_entries(&text);
    let class = |c: &str| g.diffs.iter().filter(|d| d.class == c).count();
    eprintln!(
        "GROUND TRUTH: {} tables, {} cells compared alone and {} beside a timestamptz column; {} FORMAT, {} LOSS and {} TYPE differences; the harness took {:?}",
        g.tables.len(),
        g.compared.0,
        g.compared.1,
        class("FORMAT"),
        class("LOSS"),
        class("TYPE"),
        g.took
    );
    let mut problems = known::problems(&g.diffs, &listed);
    let recorded: Vec<String> = known::rows(&text, "settings").iter().map(|c| format!("| {} | {} |", c[0], c[1])).collect();
    for row in settings_rows(g) {
        if !recorded.contains(&row) {
            problems.push(format!("SETTING NOT RECORDED: {row} (the file records: {recorded:?})"));
        }
    }
    assert!(
        problems.is_empty(),
        "{} problem(s) with KNOWN_DIFFERENCES.md (GT_WRITE=1 rewrites it from what the harness finds, marks new rows UNREVIEWED and fails):\n{}",
        problems.len(),
        problems.iter().take(30).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[tokio::test]
async fn the_session_settings_that_change_how_a_value_prints_are_the_same_through_the_core_and_psql() {
    let g = gathered().await;
    for (what, s) in [("the main run", &g.settings), ("the run under the second zone", &g.zone_settings)] {
        assert_eq!(s.psql.len(), psql::SESSION_SETTINGS.len(), "{what}: every setting is read");
        for (i, name) in psql::SESSION_SETTINGS.iter().enumerate() {
            let prefix = format!("{name}=");
            assert!(s.psql[i].as_deref().is_some_and(|v| v.starts_with(&prefix)), "{what}: psql's row {i} is {name}");
            assert_eq!(s.core[i], s.psql[i], "{what}: the Core and psql disagree on {name}");
        }
    }
    let zone = g.zone_settings.psql[0].as_deref().unwrap();
    assert_eq!(zone, format!("TimeZone={ZONE}"), "the second run really ran under {ZONE}");
    assert_ne!(g.settings.psql[0], g.zone_settings.psql[0]);
}

#[tokio::test]
async fn removing_or_changing_a_listed_entry_makes_the_check_fail_naming_table_row_and_column() {
    let g = gathered().await;
    // A list that agrees with what the harness found, so the file's own state does not matter here.
    let listed: Vec<Entry> = g.diffs.iter().map(|d| Entry { note: "a reason".into(), ..d.clone() }).collect();
    assert!(known::problems(&g.diffs, &listed).is_empty());
    let target = listed.iter().find(|e| e.class == "LOSS").expect("a LOSS is found");
    let without: Vec<Entry> = listed.iter().filter(|e| e.key() != target.key()).cloned().collect();
    let found = known::problems(&g.diffs, &without);
    assert_eq!(found.len(), 1, "{found:?}");
    for part in [&target.table, &target.row, &target.column] {
        assert!(found[0].contains(part.as_str()), "the message names {part:?}: {}", found[0]);
    }
    assert!(found[0].starts_with("NOT LISTED"), "{}", found[0]);

    let mut changed = listed.clone();
    let i = changed.iter().position(|e| e.key() == target.key()).unwrap();
    changed[i].ours = "\"something else\"".into();
    let found = known::problems(&g.diffs, &changed);
    assert!(found.len() == 1 && found[0].starts_with("LISTED BUT CHANGED"), "{found:?}");

    let mut stale = listed.clone();
    stale.push(Entry { table: "numbers".into(), row: "2^53 + 1".into(), column: "big".into(), mode: ALONE.into(), ours: "\"9007199254740992\"".into(), theirs: "\"9007199254740993\"".into(), class: "LOSS".into(), note: "made up".into() });
    let found = known::problems(&g.diffs, &stale);
    assert!(found.len() == 1 && found[0].starts_with("LISTED BUT NO LONGER OCCURS") && found[0].contains("2^53 + 1"), "{found:?}");

    // A row nobody has reviewed is named too.
    let mut unreviewed = listed.clone();
    unreviewed[i].note = known::UNREVIEWED.into();
    let found = known::problems(&g.diffs, &unreviewed);
    assert!(found.len() == 1 && found[0].starts_with(known::UNREVIEWED), "{found:?}");
    for part in [&target.table, &target.row, &target.column] {
        assert!(found[0].contains(part.as_str()), "the message names {part:?}: {}", found[0]);
    }
}

// ---- the rules of this harness ---------------------------------------------------

fn core_set(db_type: &str, kind: dbx_tui::core::CellKind, values: &[Option<&str>]) -> ResultSet {
    ResultSet {
        columns: vec![dbx_tui::core::Column { name: "c".into(), db_type: db_type.into(), kind }],
        rows: values.iter().map(|v| vec![v.map_or(Cell::Null, |v| Cell::Value(v.to_string()))]).collect(),
        truncated: false,
        elapsed: std::time::Duration::ZERO,
        messages: vec![],
        affected: 0,
        ran: String::new(),
        rerun: dbx_tui::core::Rerun::Proven,
    }
}

#[test]
fn the_rule_that_judges_a_cell_comes_from_psqls_type_and_a_core_that_misreports_it_is_listed() {
    use dbx_tui::core::CellKind;
    // psql says numeric; the Core says float8 and lost digits that a float8 cannot hold anyway.
    let got = Ok(core_set("float8", CellKind::Float, &[Some("0.1")]));
    let truth = vec![Some("0.10000000000000000001".to_string())];
    let (found, _) = compare_column("t", &["r".into()], "c", "numeric", &truth, ALONE, &got);
    let classes: Vec<&str> = found.iter().map(|e| e.class.as_str()).collect();
    assert_eq!(classes, ["TYPE", "LOSS"], "{found:?}");
    assert_eq!((found[0].ours.as_str(), found[0].theirs.as_str(), found[0].row.as_str()), ("float8", "numeric", "*"));
    // The same text from a Core that reports the type right is also a loss, with no TYPE row.
    let got = Ok(core_set("numeric", CellKind::Decimal, &[Some("0.1")]));
    let (found, _) = compare_column("t", &["r".into()], "c", "numeric", &truth, ALONE, &got);
    assert_eq!(found.iter().map(|e| e.class.as_str()).collect::<Vec<_>>(), ["LOSS"]);
    // Where the Core is right about the type, nothing extra is listed.
    let got = Ok(core_set("float8", CellKind::Float, &[Some("1.0")]));
    let (found, _) = compare_column("t", &["r".into()], "c", "float8", &[Some("1".into())], ALONE, &got);
    assert_eq!(found.iter().map(|e| e.class.as_str()).collect::<Vec<_>>(), ["FORMAT"]);
}

#[test]
fn a_view_is_ordered_by_its_id_and_a_table_by_its_key() {
    let cols: Vec<(String, String)> = [("id", "int4"), ("label", "text"), ("j", "json")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(order_columns(&[], &cols, true), ["id"], "json has no ordering: a view orders by id only");
    assert_eq!(order_columns(&["k".to_string()], &cols, true), ["k"]);
    assert_eq!(order_columns(&[], &cols, false), ["id", "label", "j"]);
}

#[tokio::test]
async fn every_schema_that_holds_a_table_or_view_is_read() {
    let g = gathered().await;
    let schemas = psql::query_all(&[("s".to_string(), format!("select distinct table_schema from information_schema.tables where {USER_SCHEMAS} and table_type in ('BASE TABLE', 'VIEW') order by 1"))]);
    let want: Vec<String> = schemas["s"].iter().flatten().cloned().collect();
    assert!(want.len() >= 2, "the fixture has more than one schema: {want:?}");
    for schema in &want {
        let prefix = if schema == "public" { String::new() } else { format!("{schema}.") };
        assert!(g.tables.iter().any(|t| (prefix.is_empty() && !t.display.contains('.')) || t.display.starts_with(&prefix)), "schema {schema} is read");
    }
    assert!(g.tables.iter().any(|t| t.display == "other.things"));
    assert!(g.tables.iter().any(|t| t.display == "gt_computed"), "a view is read");
}

// ---- the families, character for character --------------------------------------

fn col<'a>(g: &'a Gathered, table: &str, column: &str) -> &'a ColData {
    g.tables.iter().find(|t| t.display == table).unwrap_or_else(|| panic!("no table {table}")).columns.iter().find(|c| c.name == column).unwrap_or_else(|| panic!("no column {table}.{column}"))
}

fn label_index(g: &Gathered, table: &str, label: &str) -> usize {
    g.tables.iter().find(|t| t.display == table).unwrap().labels.iter().position(|l| l == label).unwrap_or_else(|| panic!("{table} has no row {label:?}"))
}

#[tokio::test]
async fn integers_beyond_2_53_and_30_digit_numerics_match_psql_character_for_character() {
    let g = gathered().await;
    for (table, column, label, want) in [
        ("gt_integers", "i8", "2^53 + 1", "9007199254740993"),
        ("gt_integers", "i8", "-(2^53) - 1", "-9007199254740993"),
        ("gt_integers", "i8", "maximums", "9223372036854775807"),
        ("gt_integers", "i8", "minimums", "-9223372036854775808"),
        ("gt_numerics", "n", "30 digits, no scale", "123456789012345678901234567890"),
        ("gt_numerics", "n", "negative 30 digits", "-123456789012345678901234567890.123456789"),
        ("gt_numerics", "n4020", "20.20 digits", "12345678901234567890.12345678901234567890"),
        ("gt_numerics", "n300", "30 digits, no scale", "999999999999999999999999999999"),
        ("numbers", "big", "2^53 + 1", "9007199254740993"),
        ("numbers", "exact", "2^53 + 1", "12345678901234567890.123456789"),
    ] {
        let c = col(g, table, column);
        let i = label_index(g, table, label);
        assert_eq!(c.truth[i].as_deref(), Some(want), "psql: {table}.{column} row {label:?}");
        for (mode, got) in [(ALONE, &c.alone), (BESIDE, &c.beside)] {
            let ours = cells_of(got.as_ref().unwrap_or_else(|e| panic!("{table}.{column} {mode}: {e}")));
            assert_eq!(ours[i].as_deref(), Some(want), "the Core: {table}.{column} row {label:?} ({mode})");
        }
    }
    // Every integer and numeric column, in every row and both modes: identical text.
    for (table, column) in [("gt_integers", "i2"), ("gt_integers", "i4"), ("gt_integers", "i8"), ("gt_integers", "o"), ("gt_numerics", "n"), ("gt_numerics", "n102"), ("gt_numerics", "n4020"), ("gt_numerics", "n300"), ("numbers", "big"), ("numbers", "exact")] {
        let c = col(g, table, column);
        for got in [&c.alone, &c.beside] {
            assert_eq!(cells_of(got.as_ref().unwrap()), c.truth, "{table}.{column}");
        }
    }
}

#[tokio::test]
async fn nan_and_infinity_floats_are_listed_as_a_known_loss_where_dbx_returns_null() {
    let g = gathered().await;
    let listed = known::listed_entries(&known::read());
    for (table, column) in [("gt_floats", "f4"), ("gt_floats", "f8")] {
        for label in ["NaN", "Infinity", "-Infinity"] {
            let e = listed
                .iter()
                .find(|e| e.table == table && e.column == column && e.row == label && e.mode == ALONE)
                .unwrap_or_else(|| panic!("{table}.{column} row {label}: not listed"));
            assert_eq!((e.class.as_str(), e.ours.as_str()), ("LOSS", "NULL"), "{e:?}");
            assert!(e.note.contains("UPSTREAM-PROPOSAL.md"), "points at the upstream write-up: {e:?}");
            // And beside a timestamptz column dbx decodes it as text and gets it right.
            assert!(!listed.iter().any(|e| e.table == table && e.column == column && e.row == label && e.mode == BESIDE), "{table}.{column} {label} is exact beside a timestamptz");
            let c = col(g, table, column);
            assert_eq!(cells_of(c.beside.as_ref().unwrap())[label_index(g, table, label)].as_deref(), Some(label));
        }
    }
    assert!(known::read().contains("Upstream issue:"), "the file says where the upstream issue is");
}

#[tokio::test]
async fn the_million_row_table_agrees_by_its_count_and_by_a_sample_of_rows() {
    let m = &gathered().await.million;
    assert_eq!(m.count.0.as_deref(), Some("1000000"), "the Core's count");
    assert_eq!(m.count.0, m.count.1, "psql's count");
    assert!(m.cells >= 60, "{} sampled cells", m.cells);
    assert!(m.disagreements.is_empty(), "{:#?}", m.disagreements);
}

// ---- through the rendered grid ---------------------------------------------------

const W: u16 = 120;
const H: u16 = 60;
/// The most rows of a column drawn at once (the frame shows about 55).
const GRID_ROWS: usize = 40;
/// The widest a column is drawn, in cells (design section 17, "Column widths").
const MAX_COLUMN: usize = 40;

/// `col`'s first rows as a one-column result, as the Core gave them.
fn grid_set(set: &ResultSet) -> ResultSet {
    ResultSet { rows: set.rows.iter().take(GRID_ROWS).cloned().collect(), truncated: false, ..set.clone() }
}

/// Whether the header line of the grid names the column (a name wider than
/// the column is cut with `…`, so the head of it must show).
fn header_shows(line: &str, name: &str) -> bool {
    line.contains(name) && !name.is_empty()
}

/// What the grid draws for each row of a one-column result: the cell region
/// of its line, without the row number, trimmed.
fn drawn(set: &ResultSet) -> Vec<String> {
    let mut probe = GridProbe::new(set.clone());
    let pane = probe.pane(W, H);
    assert!(pane.len() >= 2 + set.rows.len(), "the frame has room for {} rows", set.rows.len());
    assert!(header_shows(&pane[1], &set.columns[0].name), "the header row: {:?}", pane[1]);
    (0..set.rows.len())
        .map(|i| {
            let line = pane[2 + i].trim_start();
            let (number, rest) = line.split_once(' ').unwrap_or((line, ""));
            assert_eq!(number, (i + 1).to_string(), "the row number of row {i}: {line:?}");
            rest.trim().to_string()
        })
        .collect()
}

/// What the grid is documented to draw for a cell, as far as the column
/// shows it: NULL as `∅`, line breaks as `⏎` and tabs as `⇥`, control
/// characters as `�`, bidirectional overrides dropped (they would reorder what
/// is drawn around them).
fn expected(cell: &Cell) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let Cell::Value(v) = cell else { return "∅".into() };
    v.graphemes(true)
        .map(|g| match g {
            "\r\n" | "\n" | "\r" => "⏎".to_string(),
            "\t" => "⇥".to_string(),
            g => g.chars().filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).map(|c| if c.is_control() { '\u{fffd}' } else { c }).collect(),
        })
        .collect()
}

fn width(s: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s.graphemes(true).map(unicode_width::UnicodeWidthStr::width).sum()
}

/// What the grid draws for text wider than a column (`put` in `view/mod.rs`):
/// the graphemes that fit in the column's width less one cell, then `…`.
fn cut_to_column(text: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let (mut out, mut used) = (String::new(), 0);
    for g in text.graphemes(true) {
        let w = unicode_width::UnicodeWidthStr::width(g);
        if w == 0 {
            continue;
        }
        if used + w > MAX_COLUMN - 1 {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

/// `Ok` when `drawn` is `expected` (both trimmed: the grid pads and aligns),
/// or, for a cell wider than a column, exactly the value's prefix that fills
/// the column's width, then `…`.
fn matches_drawn(cell: &Cell, drawn: &str) -> Result<(), String> {
    let full = expected(cell);
    if width(&full) <= MAX_COLUMN {
        let want = full.trim();
        return if drawn == want { Ok(()) } else { Err(format!("drew {drawn:?}, expected {want:?}")) };
    }
    let want = cut_to_column(&full);
    let want = want.trim();
    if drawn == want {
        Ok(())
    } else {
        Err(format!("drew {drawn:?} for a cell wider than {MAX_COLUMN}: expected the prefix that fills the column, {want:?}"))
    }
}

#[test]
fn a_cut_cell_must_be_the_values_prefix_of_the_full_column_width() {
    let long = Cell::Value("x".repeat(100));
    let full = format!("{}…", "x".repeat(MAX_COLUMN - 1));
    assert!(matches_drawn(&long, &full).is_ok());
    assert!(matches_drawn(&long, "x…").is_err(), "a one-character prefix is not the column's width");
    assert!(matches_drawn(&long, &"x".repeat(MAX_COLUMN)).is_err(), "no ellipsis");
    assert!(matches_drawn(&long, &format!("{}…", "y".repeat(MAX_COLUMN - 1))).is_err(), "not the value's prefix");
    // A wide character is never drawn half: 19 of them fill 38 cells, the 20th would reach 40.
    let wide = Cell::Value("あ".repeat(30));
    assert!(matches_drawn(&wide, &format!("{}…", "あ".repeat(19))).is_ok());
    assert!(matches_drawn(&wide, &format!("{}…", "あ".repeat(18))).is_err());
    // A cell that fits is drawn whole.
    assert!(matches_drawn(&Cell::Value("abc".into()), "abc").is_ok());
    assert!(matches_drawn(&Cell::Value("abc".into()), "ab…").is_err());
}

#[test]
fn the_header_check_needs_the_whole_name() {
    assert!(header_shows("  #  abc  def", "abc"));
    assert!(!header_shows("  #  xyz", "abc"), "a name longer than one character is not shown by other text");
    assert!(!header_shows("  #  ab", "abc"));
}

#[tokio::test]
async fn the_rendered_cell_of_every_family_equals_the_cores_text_or_its_documented_truncation() {
    let g = gathered().await;
    let (mut cells, mut truncated) = (0, 0);
    let mut wrong = vec![];
    for t in &g.tables {
        for c in &t.columns {
            for (mode, got) in [(ALONE, &c.alone), (BESIDE, &c.beside)] {
                let Ok(set) = got else { continue };
                let mut one = grid_set(set);
                one.columns.truncate(1);
                one.rows.iter_mut().for_each(|r| r.truncate(1));
                if one.rows.is_empty() {
                    continue;
                }
                for (i, (cell, d)) in one.rows.iter().zip(drawn(&one)).enumerate() {
                    cells += 1;
                    if width(expected(&cell[0]).trim()) > MAX_COLUMN {
                        truncated += 1;
                    }
                    if let Err(why) = matches_drawn(&cell[0], &d) {
                        wrong.push(format!("table {}, row {:?}, column {} ({mode}): {why}", t.display, t.labels[i], c.name));
                    }
                }
            }
        }
    }
    eprintln!("GRID: {cells} cells drawn and compared with the Core's text, {truncated} of them cut at {MAX_COLUMN} cells");
    assert!(wrong.is_empty(), "{} cell(s) drawn other than documented:\n{}", wrong.len(), wrong.join("\n"));
    assert!(truncated >= 4, "the 100 kB and 5000-character cells were drawn cut");
}

/// The text area of the cell viewer for row `row` of a one-column result:
/// its body lines, without the border and the title (which names the row).
fn viewer_body(set: &ResultSet, row: usize) -> String {
    let mut probe = GridProbe::new(set.clone());
    probe.press(&"j ".repeat(row));
    probe.press("enter");
    assert!(probe.overlay_open(), "the viewer opened on row {row}");
    let screen = probe.screen(W, H);
    let top = screen.iter().position(|l| l.contains('┌')).expect("the viewer's top border");
    let bottom = screen.iter().position(|l| l.contains('└')).expect("the viewer's bottom border");
    let mut lines: Vec<String> = screen[top + 1..bottom - 2]
        .iter()
        .map(|l| {
            let (a, b) = (l.find('│').expect("left border") + '│'.len_utf8(), l.rfind('│').expect("right border"));
            l[a..b].strip_prefix(' ').unwrap_or(&l[a..b]).trim_end().to_string()
        })
        .collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Cells of one column that draw alike but hold different values, as the rows
/// of the look-alikes table: table, column, what the grid draws, the values
/// (first row of each, by label), what the cell viewer shows for each, and
/// whether the viewer tells them apart.
fn lookalikes(g: &Gathered) -> Vec<Vec<String>> {
    let mut out = vec![];
    for t in &g.tables {
        for c in &t.columns {
            let Ok(set) = &c.alone else { continue };
            let mut one = grid_set(set);
            one.columns.truncate(1);
            one.rows.iter_mut().for_each(|r| r.truncate(1));
            if one.rows.len() < 2 {
                continue;
            }
            let draws = drawn(&one);
            let mut seen: Vec<&String> = vec![];
            for d in &draws {
                if seen.contains(&d) {
                    continue;
                }
                seen.push(d);
                // The first row of each distinct value that draws as `d`.
                let mut members: Vec<usize> = vec![];
                for (i, (cell, di)) in one.rows.iter().zip(&draws).enumerate() {
                    if di == d && !members.iter().any(|m| one.rows[*m][0] == cell[0]) {
                        members.push(i);
                    }
                }
                if members.len() < 2 {
                    continue;
                }
                let bodies: Vec<String> = members.iter().map(|m| viewer_body(&one, *m)).collect();
                let apart = (0..bodies.len()).all(|a| (a + 1..bodies.len()).all(|b| bodies[a] != bodies[b]));
                let value = |m: &usize| match &one.rows[*m][0] {
                    Cell::Null => None,
                    Cell::Value(v) => Some(v.clone()),
                };
                out.push(vec![
                    esc(&t.display),
                    esc(&c.name),
                    if d.is_empty() { "(blank)".to_string() } else { format!("`{}`", esc(d)) },
                    members.iter().map(|m| format!("{}: `{}`", esc(&t.labels[*m]), show(&value(m)))).collect::<Vec<_>>().join("; "),
                    bodies.iter().map(|b| format!("`{}`", short(&esc(b)))).collect::<Vec<_>>().join(" / "),
                    if apart { "yes" } else { "NO" }.to_string(),
                ]);
            }
        }
    }
    out
}

fn short(s: &str) -> String {
    if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s.to_string() }
}

fn render_lookalike(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

#[tokio::test]
async fn what_looks_alike_in_the_grid_is_listed_and_the_cell_viewer_tells_each_apart() {
    let g = gathered().await;
    let text = known::read();
    let found = lookalikes(g);
    if known::write_requested() {
        known::guard_from_env();
        let rows: Vec<String> = found.iter().map(|c| render_lookalike(c)).collect();
        std::fs::write(known::file_path(), known::replace_rows(&text, "lookalikes", &rows)).expect("write");
        panic!("{}", known::written_message());
    }
    // The file keeps the backticks that make a cell readable; the cells are compared without them.
    let listed: Vec<String> = known::rows(&known::read(), "lookalikes").iter().map(|c| render_lookalike(c).replace('`', "")).collect();
    let actual: Vec<String> = found.iter().map(|c| render_lookalike(c).replace('`', "")).collect();
    let mut problems = vec![];
    for a in &actual {
        if !listed.contains(a) {
            problems.push(format!("NOT LISTED (look-alikes): {a}"));
        }
    }
    for l in &listed {
        if !actual.contains(l) {
            problems.push(format!("LISTED BUT NO LONGER OCCURS (look-alikes): {l}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(!listed.is_empty(), "the empty string and a single space look alike in the grid");
    // The look-alikes a person can meet in text must be told apart by the viewer.
    for c in &found {
        if (c[0] == "text_values" && c[1] == "v") || (c[0] == "gt_text") {
            assert_eq!(c[5], "yes", "the viewer tells these text values apart: {c:?}");
        }
    }
    let alike_in_both: Vec<&Vec<String>> = found.iter().filter(|c| c[5] == "NO").collect();
    eprintln!("LOOK-ALIKES: {} groups alike in the grid, {} of them alike in the cell viewer too", found.len(), alike_in_both.len());
}

#[tokio::test]
async fn null_the_empty_string_the_text_null_a_space_and_a_missing_cell_are_told_apart() {
    let g = gathered().await;
    let c = col(g, "text_values", "v");
    let set = c.alone.as_ref().unwrap();
    let cell = |label: &str| set.rows[label_index(g, "text_values", label)][0].clone();
    // The Core: four different results.
    let four = [cell("null"), cell("empty"), cell("text NULL"), cell("one space")];
    assert_eq!(four, [Cell::Null, Cell::Value("".into()), Cell::Value("NULL".into()), Cell::Value(" ".into())]);
    // A result whose last row is shorter than its header has a fifth: no cell.
    let mut two = ResultSet { columns: vec![set.columns[0].clone(), set.columns[0].clone()], rows: vec![], ..set.clone() };
    for c in &four {
        two.rows.push(vec![c.clone(), Cell::Value("x".into())]);
    }
    two.rows.push(vec![Cell::Value("short row".into())]);
    two.columns[1].name = "w".into();
    // In the grid.
    let mut probe = GridProbe::new(two.clone());
    let pane = probe.pane(W, H);
    let first_column: Vec<String> = (0..5)
        .map(|i| {
            let line = pane[2 + i].trim_start();
            let rest = line.split_once(' ').map_or("", |(_, r)| r);
            // The second column starts after the first one's width plus a gap: cut at the `x`.
            let cut = rest.find(" x").map_or(rest.len(), |at| at);
            rest[..cut].trim().to_string()
        })
        .collect();
    let second_of_last = pane[2 + 4].trim_start().split_once(' ').unwrap().1.trim().to_string();
    assert_eq!(&first_column[..4], ["∅", "", "NULL", ""], "the grid: NULL, empty and the text NULL differ; empty and one space are blank both");
    assert!(second_of_last.starts_with("short row") && second_of_last.ends_with('⊘'), "the missing cell is drawn as ⊘: {second_of_last:?}");
    assert_ne!(first_column[0], first_column[1]);
    assert_ne!(first_column[1], first_column[2]);
    assert_ne!(first_column[0], first_column[2]);
    // In the cell viewer, all five.
    let mut bodies: Vec<String> = (0..4).map(|row| viewer_body(&grid_set(&one_column(&two, 0)), row)).collect();
    let mut last = GridProbe::new(two.clone());
    last.press("j j j j l enter");
    let screen = last.screen(W, H);
    assert!(screen.iter().any(|l| l.contains("no cell")), "the viewer says a missing cell is not there: {screen:#?}");
    bodies.push("no cell".into());
    for a in 0..bodies.len() {
        for b in a + 1..bodies.len() {
            assert_ne!(bodies[a], bodies[b], "the viewer tells result {a} from result {b}: {bodies:?}");
        }
    }
    assert!(bodies[0].contains("NULL") && bodies[1].contains("empty string"), "{bodies:?}");
}

fn one_column(set: &ResultSet, i: usize) -> ResultSet {
    ResultSet { columns: vec![set.columns[i].clone()], rows: set.rows.iter().map(|r| vec![r[i].clone()]).collect(), ..set.clone() }
}
