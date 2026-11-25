//! Running a dump (slice 20), from `Command::Dump`.
//!
//! The path is checked here before dbx is called, with the export's rules
//! (slice 19): a directory that is not there is refused by name rather than
//! made (dbx would make it), and a file that is there is refused unless the
//! user said to replace it.
//!
//! dbx is never given the target. It writes into a temporary file the shell
//! made beside it (`create_new`, mode 0600), and only when dbx has reported
//! done is that file put in place: linked without replacing for a new name,
//! renamed over the target after the user's yes. A cancel, a failure or a
//! return with no final report removes the temporary file, so the earlier file
//! is exactly as it was, and a symlink at the target is never written through.
//!
//! Afterwards the file at the path is looked at, so the app can say what is
//! there: nothing, the earlier file untouched, or the dump and its size.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::export::{check_target, create_new, expand_home, temporary_beside};
use crate::app::requests::RequestId;
use crate::app::CoreReply;
use crate::core::{Core, CoreErrorKind, DumpFailure, DumpId, DumpRequest, DumpStatus, Dumped, FileLeft};

/// What identifies the file that was at the path before a dump.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
}

fn seen(path: &Path) -> Option<Seen> {
    let m = std::fs::symlink_metadata(path).ok()?;
    Some(Seen {
        len: m.len(),
        modified: m.modified().ok(),
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        },
    })
}

/// What is at `path` now, against what was there before.
fn left(path: &Path, before: Option<&Seen>) -> FileLeft {
    match seen(path) {
        None => FileLeft::Nothing,
        Some(now) if Some(&now) == before => FileLeft::Unchanged,
        Some(now) => FileLeft::Written(now.len),
    }
}

/// Checks the path, runs the dump with its progress sent back for `id`, and
/// sends the end.
pub async fn run(core: Arc<dyn Core>, id: RequestId, dump: DumpId, mut request: DumpRequest, tx: mpsc::UnboundedSender<(RequestId, CoreReply)>) {
    let path: PathBuf = expand_home(&request.path, std::env::var_os("HOME").as_deref());
    let shown = path.to_string_lossy().into_owned();
    if let Err(e) = check_target(&path, !request.prevent_overwrite) {
        let file = if e.exists { FileLeft::Unchanged } else { FileLeft::Nothing };
        // The export's words name the file first; here the path is on screen
        // already, and the reason is what matters.
        let why = e.message.strip_prefix(&format!("Not written to {shown}: ")).unwrap_or(&e.message);
        let message = format!("Not dumped: {}.", why.trim_end_matches('.'));
        let failure = DumpFailure { exists: e.exists, refused: true, message, path: shown, file };
        let _ = tx.send((id, CoreReply::Dumped(Err(failure))));
        return;
    }
    let before = seen(&path);
    // dbx writes into a file of ours beside the target (made now, private to
    // its owner), never into the target: it truncates what it is given, and
    // follows a symlink. The target is touched only when dbx has said done.
    let temp = temporary_beside(&path);
    if let Err(e) = create_new(&temp) {
        let file = left(&path, before.as_ref());
        let message = format!("Not dumped: a temporary file could not be made beside {shown}: {e}.");
        let failure = DumpFailure { exists: false, refused: true, message, path: shown, file };
        let _ = tx.send((id, CoreReply::Dumped(Err(failure))));
        return;
    }
    let replace = !request.prevent_overwrite;
    request.path = temp.to_string_lossy().into_owned();
    // The temporary file is there, and is ours to write over.
    request.prevent_overwrite = false;
    let progress_tx = tx.clone();
    let sink = Arc::new(move |p| {
        let _ = progress_tx.send((id, CoreReply::DumpProgress(p)));
    });
    let result = core.dump(dump, request, sink).await;
    let reply = match result {
        Ok(end) if end.last.as_ref().is_some_and(|p| p.status == DumpStatus::Done) => match place(&path, &temp, replace) {
            Ok(()) => Ok(Dumped { path: shown, end, file: left(&path, before.as_ref()) }),
            Err(why) => {
                let file = left(&path, before.as_ref());
                let kept = match file {
                    FileLeft::Unchanged => " The earlier file is untouched.",
                    _ => "",
                };
                let message = format!("dbx finished, but the dump could not be put at {shown} ({why}), so it was not kept.{kept}");
                Err(DumpFailure { exists: false, refused: false, message, path: shown, file })
            }
        },
        // Cancelled, or returned with no final report: what dbx wrote is not
        // put anywhere.
        Ok(end) => {
            let _ = std::fs::remove_file(&temp);
            Ok(Dumped { path: shown, end, file: left(&path, before.as_ref()) })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            let refused = e.kind == CoreErrorKind::Refused;
            Err(DumpFailure { exists: false, refused, message: e.message, path: shown, file: left(&path, before.as_ref()) })
        }
    };
    let _ = tx.send((id, CoreReply::Dumped(reply)));
}

/// Puts the finished dump at `path`. A new name is linked without replacing
/// anything (it fails if the name has been taken since the check); a name the
/// user agreed to replace is taken by a rename, which replaces a symlink
/// itself and never writes through it, and keeps the replaced file's mode.
/// The temporary name is gone either way.
fn place(path: &Path, temp: &Path, replace: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // What dbx did to the file's mode does not matter: private, or the
        // replaced regular file's own.
        let mut mode = 0o600;
        if replace {
            if let Ok(old) = std::fs::symlink_metadata(path) {
                if old.is_file() {
                    mode = old.permissions().mode() & 0o7777;
                }
            }
        }
        let _ = std::fs::set_permissions(temp, std::fs::Permissions::from_mode(mode));
    }
    if replace {
        return std::fs::rename(temp, path).map_err(|e| {
            let _ = std::fs::remove_file(temp);
            e.to_string()
        });
    }
    let linked = std::fs::hard_link(temp, path);
    let result = match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err("that name was taken while the dump ran".to_string()),
        // No hard links here (some filesystems): copy into the final name,
        // still without replacing, and take it back if the copy fails.
        Err(_) => copy_new(temp, path),
    };
    let _ = std::fs::remove_file(temp);
    result
}

fn copy_new(from: &Path, to: &Path) -> Result<(), String> {
    let mut out = match create_new(to) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err("that name was taken while the dump ran".to_string()),
        Err(e) => return Err(e.to_string()),
    };
    let copied = std::fs::File::open(from).and_then(|mut input| std::io::copy(&mut input, &mut out)).and_then(|_| out.sync_all());
    drop(out);
    copied.map_err(|e| {
        let _ = std::fs::remove_file(to);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Call, ConnectionId, DumpContent, DumpProgress, DumpStatus, FakeCore, ScriptedDump};

    fn request(path: &Path, prevent_overwrite: bool) -> DumpRequest {
        DumpRequest {
            at: crate::core::DatabasePath { connection: ConnectionId("c".into()), database: "edge".into() },
            schema: String::new(),
            content: DumpContent::StructureAndData,
            drop_tables: false,
            path: path.to_string_lossy().into_owned(),
            prevent_overwrite,
        }
    }

    fn step(status: DumpStatus) -> (std::time::Duration, DumpProgress) {
        (std::time::Duration::ZERO, DumpProgress::new(status))
    }

    /// Runs one dump through the shell and returns its end, and what the fake
    /// was asked to write to.
    async fn dump(script: ScriptedDump, request: DumpRequest) -> (Result<Dumped, DumpFailure>, Vec<String>) {
        let fake = Arc::new(FakeCore::new());
        fake.script_dump(script);
        let core: Arc<dyn Core> = fake.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(core, RequestId(1), DumpId(1), request, tx).await;
        let mut end = None;
        while let Ok((_, reply)) = rx.try_recv() {
            if let CoreReply::Dumped(r) = reply {
                end = Some(r);
            }
        }
        let paths = fake.calls().into_iter().filter_map(|c| if let Call::Dump(_, r) = c { Some(r.path) } else { None }).collect();
        (end.expect("the shell sent an end"), paths)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn replacing_a_file_and_then_cancelling_leaves_the_original_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sql");
        let mine: Vec<u8> = (0..=255u8).cycle().take(5_000).collect();
        std::fs::write(&path, &mine).unwrap();
        let script = ScriptedDump::steps(vec![step(DumpStatus::Cancelled)]).writing(b"-- a partial dump");
        let (end, given) = dump(script, request(&path, false)).await;
        let dumped = end.expect("a cancel is an end, not a failure");
        assert!(dumped.end.cancelled());
        assert_eq!(dumped.file, FileLeft::Unchanged, "the earlier file is untouched");
        assert_eq!(std::fs::read(&path).unwrap(), mine);
        assert_ne!(given, vec![path.to_string_lossy().into_owned()], "dbx was never handed the real path");
        assert_eq!(names(dir.path()), vec!["p.sql".to_string()], "the temporary file is gone");
    }

    #[tokio::test]
    async fn replacing_a_file_and_then_failing_leaves_the_original_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sql");
        std::fs::write(&path, "mine").unwrap();
        let script = ScriptedDump::failing(Vec::new(), "connection lost").writing(b"-- half");
        let (end, _) = dump(script, request(&path, false)).await;
        let failure = end.expect_err("failed");
        assert_eq!(failure.file, FileLeft::Unchanged);
        assert!(!failure.refused);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(names(dir.path()), vec!["p.sql".to_string()]);
    }

    #[tokio::test]
    async fn a_failure_or_a_cancel_on_a_new_path_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sql");
        let (end, _) = dump(ScriptedDump::failing(Vec::new(), "boom").writing(b"-- half"), request(&path, true)).await;
        assert_eq!(end.expect_err("failed").file, FileLeft::Nothing);
        let (end, _) = dump(ScriptedDump::steps(vec![step(DumpStatus::Cancelled)]).writing(b"-- half"), request(&path, true)).await;
        assert_eq!(end.expect("ended").file, FileLeft::Nothing);
        assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_dump_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sql");
        let (end, _) = dump(ScriptedDump::steps(vec![step(DumpStatus::Done)]).writing(b"-- dump"), request(&path, true)).await;
        assert_eq!(end.expect("done").file, FileLeft::Written(7));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "-- dump");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(names(dir.path()), vec!["new.sql".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_replaced_file_keeps_its_mode_and_takes_the_dump() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sql");
        std::fs::write(&path, "mine").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let (end, _) = dump(ScriptedDump::steps(vec![step(DumpStatus::Done)]).writing(b"-- dump"), request(&path, false)).await;
        assert_eq!(end.expect("done").file, FileLeft::Written(7));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "-- dump");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(names(dir.path()), vec!["p.sql".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_at_the_target_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.sql");
        let link = dir.path().join("link.sql");
        std::fs::write(&real, "the real file").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Cancelled: the link and what it points at are as they were.
        let (end, _) = dump(ScriptedDump::steps(vec![step(DumpStatus::Cancelled)]).writing(b"-- half"), request(&link, false)).await;
        assert_eq!(end.expect("ended").file, FileLeft::Unchanged);
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "the real file");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        // Replaced after a yes: the link itself is replaced, its target is not written.
        let (end, given) = dump(ScriptedDump::steps(vec![step(DumpStatus::Done)]).writing(b"-- dump"), request(&link, false)).await;
        assert_eq!(end.expect("done").file, FileLeft::Written(7));
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "the real file", "the file behind the link is untouched");
        assert!(!std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "-- dump");
        assert!(given.iter().all(|p| !p.ends_with("link.sql")), "dbx never saw the link: {given:?}");
        // Not confirmed: refused, and a dangling link counts as there.
        let dangling = dir.path().join("dangling.sql");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), &dangling).unwrap();
        let (end, _) = dump(ScriptedDump::steps(vec![step(DumpStatus::Done)]).writing(b"-- dump"), request(&dangling, true)).await;
        assert!(end.expect_err("refused").exists);
        assert!(!dir.path().join("nowhere").exists());
    }

    #[tokio::test]
    async fn a_dump_that_returns_without_reporting_done_is_not_put_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sql");
        let (end, _) = dump(ScriptedDump::ending_without_a_report(Vec::new()).writing(b"-- ?"), request(&path, true)).await;
        let dumped = end.expect("ended");
        assert_eq!(dumped.end.last, None);
        assert_eq!(dumped.file, FileLeft::Nothing);
        assert!(names(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_refusal_from_the_core_comes_back_as_a_refusal_with_nothing_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sql");
        std::fs::write(&path, "mine").unwrap();
        let error = crate::core::CoreError::new(crate::core::CoreErrorKind::Refused, "Not dumped: a column is named \"\"a\"\".");
        let (end, _) = dump(ScriptedDump::failing_with(error), request(&path, false)).await;
        let failure = end.expect_err("refused");
        assert!(failure.refused && failure.message.contains("a column is named"), "{failure:?}");
        assert_eq!(failure.file, FileLeft::Unchanged);
        assert_eq!(names(dir.path()), vec!["p.sql".to_string()]);
    }

    #[test]
    fn what_is_left_tells_nothing_the_same_file_and_a_new_one_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sql");
        assert_eq!(left(&path, None), FileLeft::Nothing);
        std::fs::write(&path, "mine").unwrap();
        let before = seen(&path);
        assert_eq!(left(&path, before.as_ref()), FileLeft::Unchanged);
        assert_eq!(left(&path, None), FileLeft::Written(4), "not there before: written");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "other text").unwrap();
        assert_eq!(left(&path, before.as_ref()), FileLeft::Written(10), "replaced: another file");
    }
}
