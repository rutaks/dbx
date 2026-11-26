//! Writing an export to a file (slice 19). It runs off the event loop, from
//! `Command::Export`.
//!
//! The bytes go to a temporary file in the target's own directory first, then
//! a rename puts them in place, so a file is either the old one or the whole
//! new one. A file that is there already is replaced only when the job says so
//! (the app asked first). A directory that is not there is refused, not made.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::core::{ExportError, ExportJob, Exported, Syntax};

/// Builds the job's bytes with dbx's format and writes them.
pub fn run<S: Syntax + ?Sized>(core: &S, job: &ExportJob) -> Result<Exported, ExportError> {
    run_with_home(core, job, std::env::var_os("HOME").as_deref())
}

fn run_with_home<S: Syntax + ?Sized>(core: &S, job: &ExportJob, home: Option<&std::ffi::OsStr>) -> Result<Exported, ExportError> {
    let path = expand_home(&job.path, home);
    let path = path.as_path();
    // Refused before the bytes are built: a big export is not worth the wait
    // when the directory is not there.
    check_target(path, job.replace)?;
    let bytes = core.export_bytes(&job.request).map_err(|e| failed(path, &e))?;
    write(path, &bytes, job.replace)?;
    Ok(Exported { path: path.to_string_lossy().into_owned(), rows: job.request.rows.len(), bytes: bytes.len() as u64 })
}

fn failed(path: &Path, why: &str) -> ExportError {
    ExportError { exists: false, message: format!("Not written to {}: {why}", path.display()) }
}

fn exists(path: &Path) -> ExportError {
    ExportError { exists: true, message: format!("{} already exists.", path.display()) }
}

/// The directory a file goes in.
fn directory_of(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// What can be told before writing: no name, a directory that is not there, a
/// target that is a directory, a file that must not be replaced.
pub(crate) fn check_target(path: &Path, replace: bool) -> Result<(), ExportError> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(failed(path, "give a file name"));
    }
    let dir = directory_of(path);
    match fs::metadata(dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(failed(path, &format!("{} is not a directory", dir.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(failed(path, &format!("the directory {} does not exist", dir.display())))
        }
        Err(e) => return Err(failed(path, &format!("cannot reach {}: {e}", dir.display()))),
    }
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => Err(failed(path, "that is a directory")),
        Ok(_) if !replace => Err(exists(path)),
        _ => Ok(()),
    }
}

/// The temporary file's path: hidden, beside the target, and its own for this
/// export (process, a counter and the clock), so two exports never share one.
pub(crate) fn temporary_beside(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    directory_of(path).join(format!(".{name}.{}.{}.{nanos}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)))
}

/// `~/...` as the user's home (`HOME`, as given); anything else as typed, and
/// so is `~/...` when there is no home to put it in.
pub(crate) fn expand_home(typed: &str, home: Option<&std::ffi::OsStr>) -> PathBuf {
    match (typed.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) if !home.is_empty() => Path::new(home).join(rest),
        _ => PathBuf::from(typed),
    }
}

type Link<'a> = &'a dyn Fn(&Path, &Path) -> std::io::Result<()>;

fn write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), ExportError> {
    write_in(path, bytes, replace, &temporary_beside(path), &|a, b| fs::hard_link(a, b))
}

/// A file opened for writing that must not be there already: private to its
/// owner on Unix.
pub(crate) fn create_new(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes through `temporary` (a name this export alone uses) and `link` (the
/// no-replace step: a hard link in real use).
fn write_in(path: &Path, bytes: &[u8], replace: bool, temporary: &Path, link: Link) -> Result<(), ExportError> {
    check_target(path, replace)?;
    // Not created: nothing to remove, whatever the name's owner left there.
    let mut file = create_new(temporary).map_err(|e| failed(path, &e.to_string()))?;
    let written = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = fs::remove_file(temporary);
        return Err(failed(path, &e.to_string()));
    }
    if replace {
        // The file replaced keeps its mode.
        if let Ok(old) = fs::metadata(path) {
            let _ = fs::set_permissions(temporary, old.permissions());
        }
        return fs::rename(temporary, path).map_err(|e| {
            let _ = fs::remove_file(temporary);
            failed(path, &e.to_string())
        });
    }
    // A new name only: the link fails when the name is taken (even if it was
    // taken since the check), and the temporary name goes either way.
    let linked = link(temporary, path);
    let _ = fs::remove_file(temporary);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists(path)),
        // No hard links here (some filesystems): write the final name itself.
        Err(_) => write_directly(path, bytes),
    }
}

/// The fallback: the final path opened with `create_new` (it still never
/// replaces), written, and removed again if the write fails.
fn write_directly(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    let mut file = match create_new(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(exists(path)),
        Err(e) => return Err(failed(path, &e.to_string())),
    };
    let written = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    written.map_err(|e| {
        let _ = fs::remove_file(path);
        failed(path, &e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cell, CellKind, Column, ExportFormat, ExportRequest, FakeCore};

    fn job(path: &Path, replace: bool) -> ExportJob {
        ExportJob {
            request: ExportRequest {
                format: ExportFormat::Csv,
                columns: vec![Column { name: "v".into(), db_type: "text".into(), kind: CellKind::Text, numbers: Vec::new() }],
                rows: vec![vec![Cell::Value("a".into())], vec![Cell::Null]],
                kind: None,
                table: "t".into(),
                schema: String::new(),
                null_empty: false,
            },
            path: path.to_string_lossy().into_owned(),
            replace,
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    const CSV: &str = "\"v\"\n\"a\"\n\\N";

    #[test]
    fn a_new_file_is_written_whole_and_reported_with_its_path_rows_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        let done = run(&FakeCore::empty(), &job(&path, false)).expect("written");
        assert_eq!(fs::read_to_string(&path).unwrap(), CSV);
        assert_eq!(done, Exported { path: path.to_string_lossy().into_owned(), rows: 2, bytes: CSV.len() as u64 });
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()], "no temporary file is left");
    }

    #[test]
    fn a_file_that_is_there_is_not_replaced_unless_the_job_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        fs::write(&path, "mine").unwrap();
        let err = run(&FakeCore::empty(), &job(&path, false)).expect_err("refused");
        assert!(err.exists, "{err:?}");
        assert!(err.message.contains("out.csv"), "{}", err.message);
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine", "the file is as it was");
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()], "and nothing is left beside it");
        run(&FakeCore::empty(), &job(&path, true)).expect("replaced");
        assert_eq!(fs::read_to_string(&path).unwrap(), CSV);
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()]);
    }

    #[test]
    fn a_directory_that_is_not_there_is_refused_by_name_and_not_made() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("out.csv");
        let err = run(&FakeCore::empty(), &job(&path, false)).expect_err("refused");
        assert!(!err.exists);
        assert!(err.message.contains(&dir.path().join("missing").to_string_lossy().into_owned()), "{}", err.message);
        assert!(err.message.contains("does not exist"), "{}", err.message);
        assert_eq!(names(dir.path()), Vec::<String>::new(), "nothing was made");
    }

    #[test]
    fn a_target_that_is_a_directory_is_refused_even_when_replacing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        fs::create_dir(&path).unwrap();
        for replace in [false, true] {
            let err = run(&FakeCore::empty(), &job(&path, replace)).expect_err("refused");
            assert!(err.message.contains("directory") && err.message.contains("out.csv"), "{}", err.message);
        }
        assert!(path.is_dir());
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()]);
    }

    #[test]
    fn an_empty_name_and_a_failing_format_are_refused_with_the_reason_and_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(&FakeCore::empty(), &job(Path::new(""), false)).expect_err("no name");
        assert!(err.message.contains("file name"), "{}", err.message);
        let path = dir.path().join("out.sql");
        let mut sql = job(&path, false);
        sql.request.format = ExportFormat::SqlInsert;
        sql.request.rows.clear();
        let err = run(&FakeCore::empty(), &sql).expect_err("nothing to insert");
        assert!(err.message.contains("out.sql") && err.message.contains("no rows"), "{}", err.message);
        assert_eq!(names(dir.path()), Vec::<String>::new());
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_is_private_and_a_replaced_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new.csv");
        run(&FakeCore::empty(), &job(&new, false)).expect("written");
        assert_eq!(mode_of(&new), 0o600, "a new export is for its owner only");
        let old = dir.path().join("old.csv");
        fs::write(&old, "mine").unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o640)).unwrap();
        run(&FakeCore::empty(), &job(&old, true)).expect("replaced");
        assert_eq!(fs::read_to_string(&old).unwrap(), CSV);
        assert_eq!(mode_of(&old), 0o640, "the replaced file's mode is kept");
    }

    #[test]
    fn without_hard_links_the_file_is_written_in_place_and_nothing_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        let no_links = |_: &Path, _: &Path| Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no hard links here"));
        write_in(&path, b"whole", false, &temporary_beside(&path), &no_links).expect("written anyway");
        assert_eq!(fs::read_to_string(&path).unwrap(), "whole");
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()]);
        // The name taken meanwhile is still refused, and the file is not touched.
        let err = write_in(&path, b"other", false, &temporary_beside(&path), &no_links).expect_err("taken");
        assert!(err.exists, "{err:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "whole");
        assert_eq!(names(dir.path()), vec!["out.csv".to_string()]);
    }

    #[test]
    fn another_exports_temporary_file_is_never_removed_and_each_export_has_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        assert_ne!(temporary_beside(&path), temporary_beside(&path), "a name per export");
        let theirs = dir.path().join(".out.csv.theirs.tmp");
        fs::write(&theirs, "half written").unwrap();
        let err = write_in(&path, b"mine", false, &theirs, &|a, b| fs::hard_link(a, b)).expect_err("name in use");
        assert!(!err.exists, "{err:?}");
        assert_eq!(fs::read_to_string(&theirs).unwrap(), "half written", "not ours to remove");
        assert!(!path.exists());
    }

    #[test]
    fn a_home_path_is_expanded_from_the_home_given_and_any_other_path_is_as_typed() {
        let home = std::ffi::OsString::from("/home/me");
        assert_eq!(expand_home("~/out.csv", Some(&home)), PathBuf::from("/home/me/out.csv"));
        assert_eq!(expand_home("~/a/b.csv", Some(&home)), PathBuf::from("/home/me/a/b.csv"));
        assert_eq!(expand_home("/x/~/b.csv", Some(&home)), PathBuf::from("/x/~/b.csv"));
        assert_eq!(expand_home("~other/b.csv", Some(&home)), PathBuf::from("~other/b.csv"));
        assert_eq!(expand_home("~/out.csv", None), PathBuf::from("~/out.csv"), "no home: as typed");
        let dir = tempfile::tempdir().unwrap();
        let done = run_with_home(&FakeCore::empty(), &job(Path::new("~/out.csv"), false), Some(dir.path().as_os_str())).expect("written");
        assert_eq!(fs::read_to_string(dir.path().join("out.csv")).unwrap(), CSV);
        assert_eq!(done.path, dir.path().join("out.csv").to_string_lossy());
        let err = run_with_home(&FakeCore::empty(), &job(Path::new("~/nope/out.csv"), false), None).expect_err("refused");
        assert!(err.message.contains("~/nope/out.csv") || err.message.contains("~/nope"), "{}", err.message);
    }
}
