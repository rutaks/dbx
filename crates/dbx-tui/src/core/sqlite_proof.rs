//! Whether SQLite itself calls a statement read-only, on the connection's own
//! file. dbx's `prove_read_only_for_database` proves nothing for SQLite (it
//! answers `Unproven` for every statement), so without this every read on a
//! production file would ask, and "load more" would always ask to run again.
//!
//! The proof opens the file read-only and immutable (`mode=ro&immutable=1`:
//! no lock, no journal, no `-wal` or `-shm` is opened or made, and a file that
//! is not there is not created), prepares each statement, never steps one,
//! and keeps it only if every statement:
//!
//! - starts with `SELECT`, `WITH` or `VALUES` (so `ATTACH`, `DETACH`,
//!   `VACUUM`, `PRAGMA`, `BEGIN`, `COMMIT` and `EXPLAIN`, which SQLite also
//!   calls read-only, are never proven);
//! - prepares on the file (a table that is not there, or a function only
//!   dbx's connection has, such as its `unistr`, or one an extension adds,
//!   fails here and stays unproven);
//! - is read-only by `sqlite3_stmt_readonly`;
//! - does not name `load_extension` or `fts3_tokenizer`.
//!
//! Immutable means SQLite reads only the main file: a table created since the
//! last checkpoint of a WAL file is not seen, so a statement on it is not
//! proven, and asks. The answer can only be "no" more often, never "yes" for
//! a write.

use std::path::Path;

/// Whether every statement of `sql` is a read SQLite proves on the file at
/// `path` (an absolute path to an existing regular file).
pub(crate) fn proves_read_only(path: &Path, sql: &str) -> bool {
    use rusqlite::{Batch, Connection, OpenFlags};
    if !path.is_absolute() || !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let lower = sql.to_ascii_lowercase();
    if lower.contains("load_extension") || lower.contains("fts3_tokenizer") {
        return false;
    }
    // No `SQLITE_OPEN_CREATE`: a file that went away is not made again.
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(immutable_uri(path), flags) else { return false };
    let mut batch = Batch::new(&conn, sql);
    let mut any = false;
    loop {
        match batch.next() {
            Ok(Some(statement)) => {
                let text = statement.expanded_sql().unwrap_or_default();
                if !starts_as_a_read(&text) || !statement.readonly() || statement.is_explain() != 0 {
                    return false;
                }
                any = true;
            }
            Ok(None) => return any,
            Err(_) => return false,
        }
    }
}

/// `file:` and the path, every byte but the unreserved ones escaped, so a
/// `?`, `#` or `%` in a file name stays part of it, then `mode=ro&immutable=1`.
fn immutable_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    uri
}

/// The statement's first word, past spaces, comments and opening brackets,
/// is `SELECT`, `WITH` or `VALUES`.
fn starts_as_a_read(text: &str) -> bool {
    let mut rest = text.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix('(') {
            rest = after.trim_start();
        } else {
            break;
        }
    }
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    matches!(word.as_str(), "select" | "with" | "values")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A file database in a new directory, made with SQLite read-write and
    /// closed again.
    fn made(wal: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b%?#é.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        if wal {
            conn.query_row("pragma journal_mode = wal", [], |_| Ok(())).unwrap();
        }
        conn.execute_batch("create table t (id integer primary key, v text); insert into t values (1, 'one');").unwrap();
        drop(conn);
        (dir, path)
    }

    /// Every file in `dir`, with its length, time of change and bytes.
    fn snapshot(dir: &Path) -> BTreeMap<String, (u64, std::time::SystemTime, Vec<u8>)> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                (e.file_name().to_string_lossy().into_owned(), (m.len(), m.modified().unwrap(), std::fs::read(e.path()).unwrap()))
            })
            .collect()
    }

    const READS: [&str; 6] = [
        "select * from t",
        "SELECT count(*) FROM t WHERE v = 'one'",
        "with c as (select id from t) select * from c",
        "values (1), (2)",
        "select 1; select v from t;",
        "-- a comment first\nselect id from t /* and after */",
    ];

    const NOT_PROVEN: [&str; 22] = [
        "insert into t values (2, 'two')",
        "update t set v = 'x'",
        "delete from t",
        "select 1; delete from t",
        "with c as (select 1) insert into t select 3, 'c' from c",
        "create table u (x)",
        "drop table t",
        "ATTACH 'other.db' AS y",
        "detach y",
        "VACUUM INTO 'copy.db'",
        "vacuum",
        "PRAGMA writable_schema = 1",
        "pragma table_info(t)",
        "begin",
        "commit",
        "savepoint s",
        "explain select * from t",
        "select load_extension('/x')",
        "select fts3_tokenizer('x')",
        "select * from no_such_table",
        "select unistr('a')",
        "-- only a comment",
    ];

    #[test]
    fn sqlite_proves_reads_on_the_file_and_nothing_else() {
        let (_dir, path) = made(false);
        for sql in READS {
            assert!(proves_read_only(&path, sql), "{sql:?} is proven");
        }
        for sql in NOT_PROVEN.iter().chain(&[""]) {
            assert!(!proves_read_only(&path, sql), "{sql:?} is not proven");
        }
    }

    #[test]
    fn the_proof_never_writes_or_makes_a_file_in_the_directory_of_the_database() {
        for wal in [false, true] {
            let (dir, path) = made(wal);
            let attach = format!("attach '{}' as y", dir.path().join("attached.db").display());
            let vacuum = format!("vacuum into '{}'", dir.path().join("vacuumed.db").display());
            let before = snapshot(dir.path());
            assert_eq!(before.len(), 1, "only the database, its -wal and -shm removed at close: {:?}", before.keys());
            for sql in READS.iter().chain(&NOT_PROVEN).map(|s| s.to_string()).chain([attach, vacuum]) {
                proves_read_only(&path, &sql);
            }
            assert_eq!(snapshot(dir.path()), before, "wal {wal}: nothing written, nothing made");
        }
    }

    #[test]
    fn with_a_writer_open_on_a_wal_file_the_proof_touches_none_of_its_files() {
        let (dir, path) = made(true);
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("insert into t values (2, 'two'); create table later (x);").unwrap();
        let before = snapshot(dir.path());
        assert!(before.keys().any(|k| k.ends_with("-wal")) && before.keys().any(|k| k.ends_with("-shm")), "{:?}", before.keys());
        assert!(proves_read_only(&path, "select * from t"));
        // A table made since the last checkpoint is in the -wal, which the
        // proof does not read: not proven, so it asks.
        assert!(!proves_read_only(&path, "select * from later"));
        assert_eq!(snapshot(dir.path()), before);
        drop(writer);
    }

    #[test]
    fn a_path_that_is_missing_relative_or_a_directory_proves_nothing_and_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.db");
        assert!(!proves_read_only(&missing, "select 1"));
        assert!(!missing.exists());
        assert!(!proves_read_only(dir.path(), "select 1"));
        assert!(!proves_read_only(Path::new("relative.db"), "select 1"));
        assert!(!Path::new("relative.db").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
