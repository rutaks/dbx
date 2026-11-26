//! The confirmation gate on MySQL (slice 23b): the statements MySQL has that
//! Postgres does not, and the places where this client's reading of a statement
//! and the server's can differ. Each must either ask or be proven safe. No
//! database is needed: these judge text with dbx's classifier, as `run` does.
//! `tests/mysql.rs` then runs the dangerous ones against the server.
//!
//! Two connections: `listed`, which names `edge_other` as a production
//! database while the tab is on `edge` (not production), and `production`,
//! which is marked production as a whole.

use super::*;

fn mysql(production: bool, marked: &[&str]) -> ConnectionConfig {
    let value = serde_json::json!({
        "id": "c", "name": "my", "db_type": "mysql", "host": "h", "port": 3306,
        "username": "u", "password": "", "database": "edge", "ssl": false, "driver_profile": null,
        "is_production": production, "production_databases": marked, "read_only": false,
    });
    serde_json::from_value(value).expect("a config")
}

fn listed() -> ConnectionConfig {
    mysql(false, &["edge_other"])
}

fn production() -> ConnectionConfig {
    mysql(true, &[])
}

/// What asking about `sql` on `edge` comes to, with the danger check on: `None`
/// runs it, `Some(true)` asks as production, `Some(false)` asks only because
/// dbx flags it dangerous.
fn asks(config: &ConnectionConfig, sql: &str) -> Option<bool> {
    assess(config, "edge", sql, true, false).map(|v| v.production)
}

/// Asks as production on both connections.
fn must_ask(sql: &str) {
    assert_eq!(asks(&listed(), sql), Some(true), "on the connection that lists edge_other, from edge: {sql}");
    assert_eq!(asks(&production(), sql), Some(true), "on the production connection: {sql}");
}

/// Runs on both connections: proven read-only, or not aimed at production.
fn must_run(sql: &str) {
    assert_eq!(asks(&listed(), sql), None, "on the connection that lists edge_other, from edge: {sql}");
    assert_eq!(asks(&production(), sql), None, "on the production connection: {sql}");
}

#[test]
fn writes_to_another_database_are_confirmed_however_the_name_is_spelled() {
    for sql in [
        "delete from edge_other.t where id = 1",
        "delete from `edge_other`.t where id = 1",
        "delete from `edge_other`.`t` where id = 1",
        "delete from edge_other . t where id = 1",
        "delete from edge_other.`t` where id = 1",
        "DELETE FROM `Edge_Other`.t WHERE id = 1",
        "update edge_other.t set v = 'x' where id = 1",
        "insert into edge_other.t values (9, 'x')",
        "replace into edge_other.t values (9, 'x')",
        "truncate edge_other.t",
        "drop table edge_other.t",
        "alter table edge_other.t add column x int",
        "rename table edge.t to edge_other.t2",
        "create table edge_other.n (id int)",
        "insert into edge.t select * from edge_other.t",
        "create view edge.v as select * from edge_other.t",
        "grant all on edge_other.* to x",
        "drop database edge_other",
    ] {
        assert_eq!(asks(&listed(), sql), Some(true), "{sql}");
    }
    // The same on the tab's own database is not aimed at production.
    assert_eq!(asks(&listed(), "delete from edge.t where id = 1"), None);
    assert_eq!(asks(&listed(), "delete from `edge`.`t` where id = 1"), None);
}

#[test]
fn a_use_is_confirmed_in_every_spelling_including_inside_an_executable_comment() {
    for sql in [
        "use edge_other",
        "USE `edge_other`",
        "use edge_other;",
        "/*!40101 use edge_other */",
        "/*! use edge_other */",
        "/*!50000 use*/ edge_other",
        "/*!50000 */ use edge_other",
        "use /* c */ edge_other",
        "-- c\nuse edge_other",
        "# c\nuse edge_other",
        "select 1; use edge_other",
        "select 1;\nUse `edge_other`",
    ] {
        let verdict = assess(&listed(), "edge", sql, true, false).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(verdict.production && verdict.reasons.contains(&Reason::SwitchesDatabase), "{sql}: {verdict:?}");
        assert!(switches_database(sql), "the session is marked: {sql}");
    }
}

#[test]
fn after_a_use_every_statement_not_proven_read_only_is_aimed_at_production_until_the_session_ends() {
    let c = listed();
    for sql in ["delete from t where id = 1", "update t set v = 1", "insert into t values (1)", "select f(1)", "commit", "set @x = 1"] {
        let v = assess(&c, "edge", sql, true, true).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(v.production && v.reasons.contains(&Reason::SessionSwitched), "{sql}: {v:?}");
    }
    for sql in ["select 1", "select * from t", "show tables", "explain select * from t"] {
        assert_eq!(assess(&c, "edge", sql, true, true).map(|v| v.production), None, "{sql}");
    }
}

#[test]
fn what_the_server_reads_as_a_second_statement_is_confirmed_even_where_this_client_sees_a_comment() {
    // MySQL's `--` needs a space (or a control character) after it. Without
    // one it is two minus signs, and the rest of the line is SQL. A scanner
    // that reads it as a comment sees only the harmless SELECT.
    must_ask("select 5 --1; delete from edge_other.t where id = 1");
    must_ask("select 5 --1; update edge_other.t set v = 'x' where id = 1");
    must_ask("select 5 --; delete from edge_other.t");
    // dbx reads a comment where the server reads two statements: which ones
    // is not taken from dbx, so the connection that lists a production
    // database asks as for production.
    assert_eq!(asks(&listed(), "select 5 --1; update edge.t set v = 'x'"), Some(true));
    assert_eq!(asks(&mysql(false, &[]), "select 5 --1; update edge.t set v = 'x'"), Some(false), "elsewhere the danger check asks");
    assert_eq!(asks(&production(), "select 5 --1; update edge.t set v = 'x'"), Some(true));
    // A block comment does not nest in MySQL: the first `*/` ends it.
    must_ask("select 1 /* /* */; delete from edge_other.t where id = 1; /* */");
    must_ask("select 1 /* a */ ; /* b */ delete /* c */ from /* d */ edge_other.t where id = 1");
    // Executable comments run: the version number only says from which server.
    must_ask("select 1 /*!50000 ; delete from edge_other.t where id = 1 */");
    must_ask("/*!50000 delete from edge_other.t where id = 1 */");
    must_ask("/*!99999 delete from edge_other.t where id = 1 */");
    must_ask("/*M!100000 delete from edge_other.t where id = 1 */");
    // A hash starts a comment; a space-less double dash does not.
    must_ask("select 1 # x\n; delete from edge_other.t where id = 1");
    must_ask("select 1 -- x\n; delete from edge_other.t where id = 1");
}

#[test]
fn comments_that_really_are_comments_hide_nothing_and_ask_nothing() {
    must_run("select 1 -- ; delete from edge_other.t");
    must_run("select 1 # ; delete from edge_other.t");
    must_run("select 1 /* delete from edge_other.t */");
    must_run("select 1 /*+ SET_VAR(sort_buffer_size = 16M) */");
    must_run("-- delete from edge_other.t\nselect 1");
    must_run("select 5 - -1");
    must_run("select '--1; delete from edge_other.t'");
    must_run("select 'a;b' as x, \"c;d\" as y, `e;f` as z from edge.t");
}

#[test]
fn a_quote_the_servers_sql_mode_can_end_early_is_confirmed_when_it_could_hide_a_statement() {
    // With NO_BACKSLASH_ESCAPES (a global or a session setting) `\'` does not
    // escape: the literal ends at it, and the rest is SQL. dbx reads the
    // backslash as an escape, so it sees one SELECT.
    for sql in [
        r"select 'a\'; delete from edge_other.t where id = 1; --'",
        r"select 'a\'; update edge.t set v = 'x'; select '",
        "select \"a\\\"; delete from edge_other.t where id = 1; --\"",
    ] {
        let v = assess(&listed(), "edge", sql, true, false).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(v.production && v.reasons.contains(&Reason::Ambiguous), "{sql}: {v:?}");
        let v = assess(&production(), "edge", sql, true, false).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(v.production && v.reasons.contains(&Reason::Ambiguous), "{sql}: {v:?}");
    }
    // With the danger check on, a connection that lists nothing asks too.
    let v = assess(&mysql(false, &[]), "edge", r"select 'a\'; delete from t; --'", true, false).expect("asked");
    assert!(!v.production && v.reasons[0] == Reason::Ambiguous, "{v:?}");
    assert!(v.reasons.contains(&Reason::Dangerous), "the reading without escapes is a delete with no WHERE: {v:?}");
    assert_eq!(assess(&mysql(false, &[]), "edge", r"select 'a\'; delete from t; --'", false, false).map(|v| v.production), None, "the setting is the user's");
    // A backslash that cannot change where a quote ends is not asked about.
    must_run(r"select 'C:\temp\file', 'a\\' as x");
    must_run("select 'no backslash; here'");
    // One that can, with no `;` outside quotes in any reading, is one
    // statement however the server reads it: the reading without escapes
    // never closes its quote (a syntax error), so it is not asked about.
    for sql in [r"select 'C:\temp\file', 'it\'s', 'a\\' as x", r"select 'a\'b' as x from edge.t"] {
        must_run(sql);
    }
}

#[test]
fn the_statements_only_mysql_has_are_confirmed() {
    for sql in [
        "load data local infile '/etc/hosts' into table edge_other.t",
        "load data local infile '/etc/hosts' into table t",
        "load data infile '/etc/hosts' into table t",
        "load xml local infile '/x.xml' into table t",
        "handler edge_other.t open",
        "handler t open",
        "handler t read first",
        "handler t close",
        "lock tables edge_other.t write",
        "lock tables t write",
        "lock tables t read",
        "unlock tables",
        "set global max_connections = 10",
        "set persist max_connections = 10",
        "set persist_only max_connections = 10",
        "set @@global.max_connections = 10",
        "set global read_only = on",
        "call edge_other.p()",
        "call p()",
        "prepare s from 'delete from edge_other.t'",
        "execute s",
        "deallocate prepare s",
        "kill 5",
        "kill query 5",
        "flush tables",
        "flush privileges",
        "reset master",
        "optimize table edge.t",
        "analyze table edge.t",
        "alter user root identified by 'x'",
        "create user x identified by 'y'",
        "install plugin x soname 'x.so'",
        "xa start 'a'",
        "do sleep(1)",
        "create procedure p() delete from edge_other.t",
        "create trigger tr after insert on edge.t for each row delete from edge_other.t",
        "create event ev on schedule every 1 hour do delete from edge_other.t",
        "change master to master_host = 'x'",
        "start replica",
        "import table from '/x.sdi'",
    ] {
        let a = asks(&listed(), sql);
        let b = asks(&production(), sql);
        assert!(a.is_some() && b == Some(true), "{sql}: listed {a:?}, production {b:?}");
    }
    // Aimed at the connection as a whole, whatever they are written against.
    for sql in ["set global max_connections = 10", "handler t open", "lock tables t write", "kill 5", "flush tables", "prepare s from 'select 1'", "execute s", "call p()"] {
        assert_eq!(asks(&listed(), sql), Some(true), "{sql}");
    }
}

#[test]
fn a_statement_that_writes_a_file_on_the_server_is_confirmed_wherever_the_connection_lists_production() {
    for sql in [
        "select * from edge.t into outfile '/tmp/x'",
        "select * from edge.t into dumpfile '/tmp/x'",
        "SELECT 1 INTO /* c */ OUTFILE '/tmp/x'",
        "select 1 /*!50000 into outfile '/tmp/x' */",
        "select * from edge.t where id = 1 into outfile '/tmp/x' fields terminated by ','",
    ] {
        let v = assess(&listed(), "edge", sql, true, false).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(v.production && v.reasons.contains(&Reason::WritesFile), "{sql}: {v:?}");
        // With the danger check off the file is still written to the host
        // the production databases are on.
        let off = assess(&listed(), "edge", sql, false, false).unwrap_or_else(|| panic!("not asked with the check off: {sql}"));
        assert!(off.production && off.reasons.contains(&Reason::WritesFile), "{sql}: {off:?}");
    }
    // A connection that lists no production database leaves it to the danger check.
    let plain = mysql(false, &[]);
    assert!(assess(&plain, "edge", "select * from edge.t into outfile '/tmp/x'", true, false).is_some());
    assert_eq!(assess(&plain, "edge", "select * from edge.t into outfile '/tmp/x'", false, false).map(|v| v.production), None);
    // `into @x` and `into @a, @b` fill variables: nothing leaves the session.
    must_run_unproven_variable_only("select 1 into @x");
    // A word that merely contains the words is not it.
    must_run("select 'into outfile' as x");
    must_run("select 1 as outfile");
}

/// `select ... into @x` is not proven read-only by dbx (`Unproven`): it asks on a
/// production connection like any select calling a function, and not on one
/// that only lists a database.
fn must_run_unproven_variable_only(sql: &str) {
    assert_eq!(asks(&listed(), sql), None, "{sql}");
    assert!(assess(&production(), "edge", sql, true, false).is_some_and(|v| v.reasons == [Reason::Unproven]), "{sql}");
}

#[test]
fn something_not_proven_read_only_that_names_a_production_database_is_aimed_at_it() {
    // dbx looks for the target of writes only, and finds one object after
    // FROM, JOIN, UPDATE or INTO. These name the production database where it
    // does not look.
    for sql in [
        "select edge_other.f(1)",
        "select `edge_other`.f(1)",
        "select * from edge.t where id = edge_other.f(1)",
        "select * from edge_other.t lock in share mode",
        "delete b from edge.a a, edge_other.b b where a.id = b.id",
        "delete a, b from edge.a a, edge_other.b b where a.id = b.id",
        "update edge.a a, edge_other.b b set b.v = 1 where a.id = b.id",
        "update edge.a a, `edge_other`.`b` b set a.v = 1, b.v = 2 where a.id = b.id",
        "load data local infile '/etc/hosts' into table edge_other.t",
        "load data local infile '/etc/hosts' into table `edge_other`.`t`",
        "select 1 into outfile '/tmp/edge_other.txt'",
        "select * into @x from edge_other.t where id = f(1)",
    ] {
        assert_eq!(asks(&listed(), sql), Some(true), "{sql}");
    }
    // Reads of it that dbx proves read-only run.
    for sql in ["select * from edge_other.t", "select count(*) from `edge_other`.t", "show tables from edge_other", "show create table edge_other.t", "explain select * from edge_other.t", "desc edge_other.t"] {
        assert_eq!(asks(&listed(), sql), None, "{sql}");
    }
}

#[test]
fn explain_analyze_and_the_statements_around_it_are_judged_on_mysql_too() {
    for sql in ["explain analyze delete from edge_other.t where id = 1", "explain analyze format=tree delete from edge_other.t where id = 1"] {
        let v = assess(&listed(), "edge", sql, true, false).unwrap_or_else(|| panic!("{sql}"));
        assert!(v.production && v.reasons.contains(&Reason::ExplainRuns), "{sql}: {v:?}");
    }
    must_run("explain select * from edge.t");
    must_run("explain analyze select * from edge.t");
}

#[test]
fn what_is_proven_read_only_runs_on_a_production_connection() {
    for sql in [
        "select 1",
        "select * from edge.t where id = 1",
        "select count(*), max(id) from edge.t group by v",
        "show tables",
        "show create table t",
        "show variables like 'sql_mode'",
        "describe t",
        "explain select 1",
        "with x as (select 1 as a) select * from x",
        "select * from edge.t order by id limit 10 offset 20",
        "select `select`, `Order` from `Mixed Case`",
        "SELECT * FROM `a``b`",
        "select * from ```x``; select sleep(0) #```",
    ] {
        assert_eq!(asks(&production(), sql), None, "{sql}");
    }
}

#[test]
fn a_view_of_the_text_the_server_reads_removes_comments_and_nothing_else() {
    let view = |s: &str| mysql_view(s, true);
    assert_eq!(view("select 1 -- x\n, 2"), "select 1  \n, 2");
    assert_eq!(view("select 1 # x"), "select 1  ");
    assert_eq!(view("select 5 --1; select 2"), "select 5 - -1; select 2", "no space: two minus signs, written so");
    assert_eq!(view("select 5---1"), "select 5- - -1", "every dash of a run that is no comment stands alone");
    assert_eq!(view("select 5--- 1"), "select 5-  ", "the last two of a run are a comment when a space follows");
    assert_eq!(view("select 5 --\t1"), "select 5  ", "a tab after it: a comment");
    assert_eq!(view("select 5 --"), "select 5  ", "at the end: a comment");
    assert_eq!(view("a /* x */ b"), "a   b");
    assert_eq!(view("a /*+ hint */ b"), "a   b");
    assert_eq!(view("a /* /* */ b */"), "a   b */", "block comments do not nest");
    assert_eq!(view("a /*!50000 b */ c"), "a   b   c", "an executable comment is SQL");
    assert_eq!(view("a /*M!100000 b */ c"), "a   c", "MariaDB's: a comment to MySQL");
    let mariadb = mysql_read::Mode { mariadb: true, ..mysql_read::Mode::DEFAULT };
    assert_eq!(mysql_read::read("a /*M!100000 b */ c", mariadb).plain, "a   b   c", "and SQL to MariaDB");
    let old = mysql_read::Mode { version: 50000, ..mysql_read::Mode::DEFAULT };
    assert_eq!(mysql_read::read("a /*!50001 b */ c", old).plain, "a   c", "a version above the server's: a comment");
    assert_eq!(mysql_read::read("a /*!50001 b /* x */ c */ d", old).plain, "a   d", "in which one comment may nest");
    let ansi = mysql_read::Mode { ansi_quotes: true, ..mysql_read::Mode::DEFAULT };
    assert_eq!(mysql_read::read("select \"a`b\" # c", ansi).plain, "select `a``b`  ", "a name in double quotes, written in backticks");
    assert_eq!(mysql_read::read("select 'a;b--c' as \"x;y\"", ansi).neutral, "select 'a b  c' as `x y`", "rewritten so no lexer misreads it");
    assert_eq!(view("select '-- x', \"# y\", `/* z */`"), "select '-- x', \"# y\", `/* z */`");
    assert_eq!(view(r"select 'a\'-- b'"), r"select 'a\'-- b'", "a backslash escapes the quote in the default mode");
    assert_eq!(mysql_view(r"select 'a\'-- b'", false), "select 'a\\' ", "and does not in NO_BACKSLASH_ESCAPES");
    assert_eq!(view("select 'it''s -- x'"), "select 'it''s -- x'");
    assert_eq!(view("/* never closed"), " ");
    assert_eq!(view("select 'never closed -- x"), "select 'never closed -- x");
}

#[test]
fn a_use_the_server_reads_is_noted_for_the_session_even_where_this_client_sees_a_comment() {
    let (a, b) = (SessionId { tab: TabId(1), serial: 1 }, SessionId { tab: TabId(1), serial: 2 });
    let marks = SwitchedSessions::default();
    marks.note_for(a, "select 1 --1; use edge_other", DatabaseType::Mysql);
    assert!(marks.contains(a));
    marks.note_for(b, "select 1 -- ; use edge_other", DatabaseType::Mysql);
    assert!(!marks.contains(b), "a real comment is not a USE");
    marks.note_for(b, r"select 'a\'; use edge_other; --'", DatabaseType::Mysql);
    assert!(marks.contains(b), "nor is a quote the server may end early one");
}

#[test]
fn a_statement_load_more_would_repeat_is_judged_as_the_server_reads_it() {
    let rerun = |sql: &str| rerun_on(sql, DatabaseType::Mysql, None);
    assert_eq!(rerun("select * from t order by id limit 10"), Rerun::Proven);
    assert_eq!(rerun("select * from t -- the first ten\n order by id"), Rerun::Proven);
    assert_eq!(rerun("select 5 --1; delete from edge_other.t where id = 1"), Rerun::Write, "the second statement is not a comment");
    assert_eq!(rerun("select 1 /*!50000 ; delete from t */"), Rerun::Write);
    assert_eq!(rerun(r"select 'a\'; delete from t; --'"), Rerun::Write, "it may be two statements");
    assert_eq!(rerun("select f(1)"), Rerun::ReadOnlyUnproven);
    let c = production();
    assert!(proven_read_only("select 1 -- x", &c));
    assert!(!proven_read_only("select 5 --1; delete from edge_other.t", &c));
    assert!(!proven_read_only(r"select 'a\'; delete from t; --'", &c));
    assert!(!proven_read_only("/*!50000 use edge_other */", &c));
}

#[test]
fn the_current_schema_is_the_current_database_on_mysql_and_not_current_schema() {
    assert_eq!(current_schema_sql(DatabaseType::Mysql), Some("select database()"));
    assert_eq!(current_schema_sql(DatabaseType::Postgres), Some("select current_schema()"));
    assert_eq!(current_schema_sql(DatabaseType::Sqlite), None, "a file's is fixed");
}

// ---- review of slice 23b: readings the gate must not miss ---------------------------

include!("../../../tests/fixtures/mysql/gate_corpus.rs");

#[test]
fn a_double_dash_and_then_whitespace_that_is_not_ascii_is_two_minus_signs_and_hides_nothing() {
    // MySQL ends `--` as a comment only at ASCII whitespace or a control
    // character. dbx's scanners take any `--` for a comment, so the gate must
    // not lean on their verdict here.
    for ws in ['\u{a0}', '\u{85}', '\u{3000}'] {
        let sql = format!("select 1 --{ws} from (select 2 as {ws}) x; delete from edge_other.t where id = 1");
        must_ask(&sql);
        let v = assess(&listed(), "edge", &sql, true, false).expect("asked");
        assert!(v.reasons.contains(&Reason::Ambiguous), "dbx reads a comment where the server does not: {v:?}");
        assert!(!proven_read_only(&sql, &production()), "{sql:?}");
        assert_eq!(rerun_on(&sql, DatabaseType::Mysql, None), Rerun::Write, "{sql:?}");
        // A USE behind it switches the session.
        let s = SessionId { tab: TabId(9), serial: 1 };
        let marks = SwitchedSessions::default();
        marks.note_for(s, &format!("select 1 --{ws} from (select 2 as {ws}) x; use edge_other"), DatabaseType::Mysql);
        assert!(marks.contains(s), "{ws:?}");
    }
    assert_eq!(mysql_view("select 1 --\u{a0}x", true), "select 1 - -\u{a0}x");
}

#[test]
fn a_double_quote_ansi_quotes_makes_a_name_cannot_hide_a_second_statement() {
    // Under ANSI_QUOTES `"\"` is a name (no escapes in it): the `;` after it
    // ends the statement, and the `#` comments out the closing quote.
    let sql = "select '\\'' as \"\\\"; delete from edge_other.t where id = 1 #\"";
    must_ask(sql);
    let v = assess(&listed(), "edge", sql, true, false).expect("asked");
    assert!(v.reasons.contains(&Reason::Ambiguous), "{v:?}");
    // ANSI_QUOTES and NO_BACKSLASH_ESCAPES together: `'a\'` ends at the
    // backslash, `"x"` is a name.
    must_ask("select 'a\\' as \"x\"; delete from edge_other.t where id = 1; -- '");
    // Readings that differ in what lies outside the quotes but are each one
    // statement, the one without escapes never closing its quote, are not.
    must_run("select 'it\\'s' as x from edge.t");
    // What a `"` is (a string or a name) is not a different statement.
    must_run("select \"a\" from edge.t");
    must_run("select 'C:\\temp\\file', 'a\\\\' as x");
}

#[test]
fn every_text_of_the_corpus_is_asked_about_unless_its_second_statement_is_hidden_in_every_reading() {
    for (text, hidden) in GATE_CORPUS {
        let sql = with_statement(text, "delete from edge_other.t where id = 1");
        if *hidden {
            assert_eq!(asks(&listed(), &sql), None, "in a comment or a quote however the server reads it: {sql:?}");
        } else {
            assert_eq!(asks(&listed(), &sql), Some(true), "on the connection that lists edge_other: {sql:?}");
            assert_eq!(asks(&production(), &sql), Some(true), "on the production connection: {sql:?}");
        }
    }
}

#[test]
fn into_outfile_is_found_in_every_reading_of_the_quotes() {
    for sql in [
        "select 'a\\' into outfile '/tmp/x'",
        "select 'a\\', 'b' into outfile '/tmp/x' -- '",
        "select \"a\\\" into dumpfile '/tmp/x'",
        "select \"a\" into outfile '/tmp/x'",
    ] {
        let v = assess(&listed(), "edge", sql, false, false).unwrap_or_else(|| panic!("not asked: {sql}"));
        assert!(v.production && v.reasons.contains(&Reason::WritesFile), "{sql}: {v:?}");
    }
}

#[test]
fn a_version_in_an_executable_comment_is_five_digits_or_six_as_mysql_reads_it() {
    // Read as a server new enough for every version (the open reading).
    let view = |s: &str| mysql_view(s, true);
    assert_eq!(view("a /*!50000 b */ c"), "a   b   c");
    assert_eq!(view("a /*!123456 b */ c"), "a   b   c", "six digits");
    assert_eq!(view("a /*!1234567 b */ c"), "a  7 b   c", "six at most: the seventh is SQL");
    assert_eq!(view("a /*!1234 b */ c"), "a  1234 b   c", "fewer than five: no version, the digits are SQL");
    // A version above the server's makes it a comment: a `--` in it hides nothing.
    must_ask("select 1 /*!99999 -- */ --1; delete from edge_other.t where id = 1");
    must_ask("select 1 /*!99999 -- */; delete from edge_other.t where id = 1");
}

#[test]
fn the_verdicts_of_every_reading_are_merged_reasons_and_named_databases_alike() {
    // The default reading writes to edge_other, the one without backslash
    // escapes to edge_x: both are named.
    let c = mysql(false, &["edge_other", "edge_x"]);
    let sql = "select 'a\\' ; delete from edge_x.t; select ' ; delete from edge_other.t; -- '";
    let v = assess(&c, "edge", sql, true, false).expect("asked");
    assert!(v.production, "{v:?}");
    assert!(v.named.contains(&"edge_other".to_string()) && v.named.contains(&"edge_x".to_string()), "{v:?}");
    assert!(v.reasons.contains(&Reason::Ambiguous) && v.reasons.contains(&Reason::Writes), "{v:?}");
}

#[test]
fn the_statements_the_gate_reads_are_none_when_the_server_may_read_them_otherwise() {
    let read = |sql: &str| mysql_statements_read(sql, false);
    assert_eq!(read("select 1; select 'MARK'"), Some(vec!["select 1".to_string(), "select 'MARK'".to_string()]));
    assert_eq!(read("select 1 -- x\n; select 'MARK'"), Some(vec!["select 1".to_string(), "select 'MARK'".to_string()]));
    assert_eq!(read("select 'a\\'; select 'MARK'; -- '"), None);
    assert_eq!(read("select 1 --\u{a0}; select 'MARK'"), None);
    assert_eq!(read("select '\\'' as \"\\\"; select 'MARK' #\""), None);
    // One statement in every reading: one statement.
    assert_eq!(read("select 'O\\'Brien' select 'MARK' '").map(|s| s.len()), Some(1));
    // Under a client charset that can swallow a backslash, what is not ASCII is not read.
    assert_eq!(read("select '\u{4e01}\\\\', '; select 'MARK'; -- '").map(|s| s.len()), Some(1));
    assert_eq!(mysql_statements_read("select '\u{4e01}\\\\', '; select 'MARK'; -- '", true), None);
    assert_eq!(mysql_statements_read("select 1; select 'MARK'", true).map(|s| s.len()), Some(2), "ASCII reads alike in every charset");
}

// ---- review of slice 23b, round 3: client charsets and quotes the mode ends early ----

/// The client charsets of MySQL 8.4 with a two-byte character whose second
/// byte is `\` or `` ` `` (0x5C, 0x60); none has one ending in `'` or `"`. Found
/// on the fixture by `char_length(convert(unhex(...) using cs)) = 1` over every
/// lead byte: big5 89 leads, cp932 and sjis 60, gb18030 and gbk 126. eucjpms,
/// ujis, euckr and gb2312 have none; ucs2, utf16, utf16le and utf32 cannot be
/// a client charset.
const RISKY: [&str; 5] = ["big5", "cp932", "gb18030", "gbk", "sjis"];

fn with_params(mut c: ConnectionConfig, params: &str) -> ConnectionConfig {
    c.url_params = Some(params.to_string());
    c
}

/// `assess` in a session marked as having run a statement that may change the
/// client charset (`CharsetSessions`).
fn asks_in_marked_session(config: &ConnectionConfig, sql: &str) -> Option<bool> {
    assess_in(config, "edge", sql, true, SessionMarks { switched: false, charset: true }).map(|v| v.production)
}

#[test]
fn under_a_client_charset_that_can_swallow_a_backslash_or_a_backtick_text_that_is_not_ascii_is_asked_about() {
    for cs in RISKY {
        for params in [format!("charset={cs}"), format!("charset={}", cs.to_uppercase()), format!("sessionVariables=character_set_client%3D{cs}"), format!("ssl-mode=disabled&charset={cs}")] {
            for text in CHARSET_CORPUS {
                let sql = with_statement(text, "delete from edge_other.t where id = 1");
                assert_eq!(asks(&with_params(listed(), &params), &sql), Some(true), "{params}: {sql:?}");
                assert_eq!(asks(&with_params(production(), &params), &sql), Some(true), "{params}: {sql:?}");
                let v = assess(&with_params(mysql(false, &[]), &params), "edge", &sql, true, false).unwrap_or_else(|| panic!("{params}: {sql:?}"));
                assert_eq!(v.reasons[0], Reason::Ambiguous, "{params}: {sql:?}");
            }
            // ASCII text is read alike in every charset: as before.
            assert_eq!(asks(&with_params(production(), &params), r"select * from edge.t where v = 'a\\'"), None, "{params}");
            assert_eq!(asks(&with_params(listed(), &params), "select 5 --1; delete from edge_other.t"), Some(true), "{params}");
        }
    }
    // Read as UTF-8, the default, they are one SELECT each, as the server reads them.
    for params in ["", "charset=utf8mb4", "charset=utf8", "charset=latin1", "charset=ujis", "dataCharset=GBK"] {
        for text in CHARSET_CORPUS {
            let sql = with_statement(text, "delete from edge_other.t where id = 1");
            assert_eq!(asks(&with_params(listed(), params), &sql), None, "{params}: {sql:?}");
            assert_eq!(asks(&with_params(production(), params), &sql), None, "{params}: {sql:?}");
        }
    }
}

#[test]
fn a_session_that_may_have_changed_its_client_charset_asks_about_text_that_is_not_ascii_until_it_closes() {
    for sql in [
        "set names gbk",
        "SET NAMES 'sjis'",
        "set character set cp932",
        "set charset big5",
        "set character_set_client = gb18030",
        "set @@session.character_set_client = 28",
        "/*!40101 set names gbk */",
        "select 1; set names gbk",
        // Variable names are compared as utf8mb3_general_ci: `à` is `a`, `ſ` is `s`.
        "set @@ch\u{e0}racter_\u{17f}et_client = 'gbk'",
        "set @@`ch\u{e0}racter_set_client` = 'gbk'",
        "prepare s from @q",
    ] {
        assert!(may_change_client_charset(sql), "{sql}");
    }
    for sql in ["select 1", "set sql_mode = ''", "update t set v = '\u{e9}'", "select '\u{540d}' from t", "select * from t limit 1 offset 2", "select first_names from t"] {
        assert!(!may_change_client_charset(sql), "{sql}");
    }
    let (a, b) = (SessionId { tab: TabId(1), serial: 1 }, SessionId { tab: TabId(2), serial: 1 });
    let marks = CharsetSessions::default();
    marks.note(a, "select 1", DatabaseType::Mysql);
    assert!(!marks.contains(a));
    marks.note(a, "set names gbk", DatabaseType::Mysql);
    marks.note(b, "set names gbk", DatabaseType::Postgres);
    assert!(marks.contains(a) && !marks.contains(b), "MySQL only");
    marks.note(a, "select 1", DatabaseType::Mysql);
    assert!(marks.contains(a), "it lasts");
    marks.forget(a);
    assert!(!marks.contains(a), "a closed session starts as the connection does");
    // In a marked session, text that is not ASCII is asked about; ASCII is not.
    for text in CHARSET_CORPUS {
        let sql = with_statement(text, "delete from edge_other.t where id = 1");
        assert_eq!(asks_in_marked_session(&listed(), &sql), Some(true), "{sql:?}");
        assert_eq!(asks_in_marked_session(&production(), &sql), Some(true), "{sql:?}");
    }
    assert_eq!(asks_in_marked_session(&production(), "select * from edge.t"), None);
    // A text that changes the charset and goes on: what follows is read in the new one.
    must_ask("set names gbk; select '\u{4e01}\\\\', '; delete from edge_other.t where id = 1; -- '");
    must_ask("set @@ch\u{e0}racter_set_client = 'gbk'; select 1 as `\u{4e01}`, '`; delete from edge_other.t where id = 1; -- '");
    // In a session nothing has marked, on a connection read as UTF-8, text
    // that is not ASCII is read as before.
    must_run("select '\u{4e01}' from edge.t");
    must_run("select '\u{4e01}\\\\', '; delete from edge_other.t where id = 1; -- '");
}

#[test]
fn a_use_in_text_the_gate_cannot_read_is_noted_for_the_session() {
    let s = SessionId { tab: TabId(3), serial: 1 };
    let marks = SwitchedSessions::default();
    marks.note_unread(s, "select '\u{4e01}', 'x'");
    assert!(!marks.contains(s));
    marks.note_unread(s, "select '\u{4e01}\\\\', '; use edge_other; -- '");
    assert!(marks.contains(s));
}

#[test]
fn a_quote_the_sql_mode_ends_early_is_not_asked_about_when_every_reading_is_one_statement_with_the_same_verdict() {
    for sql in [
        r"select * from users where name = 'O\'Brien'",
        r#"select * from users where name = "a\"b""#,
        r#"select 'O\'Brien', "a\"b" from edge.t"#,
        r"select * from edge.t where v like 'a\_b' or v = 'it\'s'",
    ] {
        must_run(sql);
        assert_eq!(asks(&mysql(false, &[]), sql), None, "with the danger check on: {sql}");
        assert!(mysql_statements_read(sql, false).is_some_and(|s| s.len() == 1), "{sql}");
    }
    // A reading with a `;` outside quotes, or readings whose verdicts differ,
    // still may be other statements.
    must_ask(r"select 'a\'; delete from edge_other.t where id = 1; --'");
    let v = assess(&production(), "edge", r"select 'a\', 'b' into outfile '/tmp/x' -- '", true, false).expect("asked");
    assert!(v.reasons.contains(&Reason::Ambiguous) && v.reasons.contains(&Reason::WritesFile), "{v:?}");
}
