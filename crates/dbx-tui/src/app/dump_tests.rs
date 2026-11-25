//! Layers 2 and 3: the acceptance tests of slice 20 (database dump), run on
//! `App` with a `FakeCore` behind the shell's own command runner, so the path
//! checks are the shell's (on a temporary directory) and the progress comes
//! as it does in the client. What dbx's dump writes is tested in
//! `tests/live.rs`.

use std::time::Duration;

use super::export::ExportEnv;
use super::requests::RequestId;
use super::run_tests::{click_hint, dbp, mode_bar, notice_text, opened};
use super::*;
use crate::core::{Call, DumpContent, DumpFailure, DumpId, DumpProgress, DumpRequest, DumpStatus, Dumped, FileLeft, ScriptedDump};
use crate::harness::Harness;

const STAMP: &str = "20261125-101500";

/// The sidebar's cursor on `pagila`, dumps going to `dir`.
async fn on_pagila_row(dir: &std::path::Path) -> Harness {
    let mut h = opened().await;
    h.app.ui.export_env = ExportEnv::fixed(&dir.to_string_lossy(), STAMP);
    h.press("j j");
    h
}

fn default_path(dir: &std::path::Path) -> String {
    dir.join(format!("pagila-{STAMP}.sql")).to_string_lossy().into_owned()
}

/// The screen as one line of single-spaced words, box borders left out.
fn words(h: &mut Harness) -> String {
    let text: String = super::grid_tests::screen(h).join(" ").chars().filter(|c| !"│┌┐└┘─".contains(*c)).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dumps(commands: &[Command]) -> Vec<(RequestId, DumpId, DumpRequest)> {
    commands.iter().filter_map(|c| if let Command::Dump(id, d, r) = c { Some((*id, *d, r.clone())) } else { None }).collect()
}

fn cancels(commands: &[Command]) -> Vec<DumpId> {
    commands.iter().filter_map(|c| if let Command::CancelDump(d) = c { Some(*d) } else { None }).collect()
}

fn progress(status: DumpStatus, object: &str, index: usize, total: usize, rows: u64) -> DumpProgress {
    DumpProgress { object: object.into(), index, total, rows, ..DumpProgress::new(status) }
}

fn running(object: &str, index: usize, total: usize, rows: u64) -> DumpProgress {
    progress(DumpStatus::Running, object, index, total, rows)
}

/// A dump that reports `film (7/22) 12,340 rows` and then works for ten minutes.
fn long_dump() -> ScriptedDump {
    ScriptedDump::steps(vec![(Duration::ZERO, running("film", 7, 22, 12_340)), (Duration::from_secs(600), running("rental", 8, 22, 20_000))])
}

/// `D` then `enter`: the one dump command it sends.
fn start(h: &mut Harness) -> (RequestId, DumpId, DumpRequest) {
    h.press("D");
    let mut sent = dumps(&h.press_fx("enter"));
    assert_eq!(sent.len(), 1, "exactly one dump command");
    sent.remove(0)
}

fn feed(h: &mut Harness, id: RequestId, reply: CoreReply) -> bool {
    let now = h.now();
    let fx = h.app.update(Event::Core(id, reply), now);
    let repaint = fx.repaint;
    h.run(fx);
    repaint
}

// ---- layer 2 -----------------------------------------------------------------------

#[tokio::test]
async fn d_on_pagila_opens_the_overlay_with_the_defaults_and_enter_sends_one_dump() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    assert!(mode_bar(&mut h).contains("D dump"), "{}", mode_bar(&mut h));
    h.fake.script_dump(long_dump());
    h.press("D");
    let text = words(&mut h);
    // (A long temporary path scrolls in its field: its end is what shows.)
    for want in ["Dump pagila", "● structure and data", "○ structure only", "○ data only", "● whole database", "○ drop tables", &format!("pagila-{STAMP}.sql"), "↵ export", "esc cancel"] {
        assert!(text.contains(want), "{want:?} in {text}");
    }
    assert!(matches!(&h.app.ui.overlay, Some(Overlay::Dump(o)) if o.path.text() == default_path(dir.path())));
    let sent = dumps(&h.press_fx("enter"));
    assert_eq!(sent.len(), 1, "{sent:?}");
    let request = &sent[0].2;
    assert_eq!(request.at, dbp("local pg", "pagila"));
    assert_eq!(request.schema, "", "the whole database");
    assert_eq!(request.content, DumpContent::StructureAndData);
    assert!(!request.drop_tables);
    assert!(request.prevent_overwrite, "never replace without asking");
    assert_eq!(request.path, default_path(dir.path()));
    h.settle().await;
    // dbx is called with the same request but a temporary file beside the
    // target, which it may write over (the shell's doing, see shell/dump.rs).
    assert!(
        h.fake.calls().iter().any(|c| matches!(c, Call::Dump(_, r) if r.at == request.at && r.path != request.path && !r.prevent_overwrite && r.path.starts_with(&*dir.path().to_string_lossy()))),
        "dbx was called with a temporary file beside the path: {:?}",
        h.fake.calls()
    );
}

#[tokio::test]
async fn a_schema_node_offers_this_schema_and_the_choices_reach_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_schemas(crate::core::Scripted::ok(vec!["public".into()]));
    h.press("enter");
    h.settle().await;
    h.press("j");
    h.fake.script_dump(long_dump());
    h.press("D");
    assert!(words(&mut h).contains("● schema public"), "{}", words(&mut h));
    // what: data only; scope: the whole database; drop tables: on.
    h.press("right right tab left tab space");
    let sent = dumps(&h.press_fx("enter"));
    let request = &sent[0].2;
    assert_eq!((request.content, request.schema.as_str(), request.drop_tables), (DumpContent::DataOnly, "", true));
    // Back on the schema: it is the default scope.
    let dir2 = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir2.path()).await;
    h.fake.script_schemas(crate::core::Scripted::ok(vec!["public".into()]));
    h.press("enter");
    h.settle().await;
    h.press("j");
    h.fake.script_dump(long_dump());
    let (_, _, request) = start(&mut h);
    assert_eq!(request.schema, "public");
}

#[tokio::test]
async fn a_path_that_is_there_asks_first_naming_it_and_yes_sends_prevent_overwrite_false() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let path = default_path(dir.path());
    std::fs::write(&path, "mine").unwrap();
    let (_, _, first) = start(&mut h);
    assert!(first.prevent_overwrite);
    h.settle().await;
    let text = words(&mut h);
    assert!(text.contains("Replace the file?") && text.contains(&format!("pagila-{STAMP}.sql is there already")), "{text}");
    assert!(matches!(&h.app.ui.overlay, Some(Overlay::Dump(o)) if o.stage == super::dump::DumpStage::Replace(path.clone())), "names the file");
    assert!(!h.fake.calls().iter().any(|c| matches!(c, Call::Dump(..))), "dbx was not called");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
    // `enter` is not a yes.
    assert!(dumps(&h.press_fx("enter")).is_empty());
    h.fake.script_dump(long_dump());
    let sent = dumps(&h.press_fx("y"));
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].2.prevent_overwrite, "replacing was confirmed");
    assert_eq!(sent[0].2.path, path);
}

#[tokio::test]
async fn a_directory_that_is_not_there_is_refused_naming_it_and_the_overlay_keeps_its_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let missing = dir.path().join("missing");
    h.press("D");
    // data only, then a path in a directory that is not there.
    h.press("right right tab tab tab ctrl+u");
    h.type_text(&missing.join("p.sql").to_string_lossy());
    let sent = dumps(&h.press_fx("enter"));
    assert_eq!(sent.len(), 1);
    h.settle().await;
    let text = words(&mut h);
    assert!(text.contains("Dump pagila"), "the overlay is still open: {text}");
    assert!(text.contains("does not exist"), "{text}");
    assert!(text.contains("● data only"), "{text}");
    let Some(Overlay::Dump(o)) = &h.app.ui.overlay else { panic!("the overlay") };
    assert!(matches!(&o.stage, super::dump::DumpStage::Failed(m) if m.contains(&format!("the directory {} does not exist", missing.to_string_lossy()))), "{:?}", o.stage);
    assert_eq!(o.path.text(), missing.join("p.sql").to_string_lossy(), "the path as typed");
    assert!(!h.fake.calls().iter().any(|c| matches!(c, Call::Dump(..))), "dbx was not called");
    assert!(!missing.exists(), "and the directory was not made");
    assert!(h.app.model.dump.is_none(), "no dump is running");
}

#[tokio::test(start_paused = true)]
async fn progress_updates_the_mode_bar_in_every_pane_and_the_elapsed_time_is_woken_for() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(long_dump());
    start(&mut h);
    h.settle().await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("dumping pagila: film (7/22) 12,340 rows 0s"), "{bar}");
    assert!(h.app.next_deadline().is_some(), "woken for the clock");
    h.advance(Duration::from_secs(14));
    assert!(mode_bar(&mut h).contains("12,340 rows 14s"), "{}", mode_bar(&mut h));
    h.press("ctrl+g l");
    assert!(mode_bar(&mut h).contains("dumping pagila: film (7/22)"), "in the editor too: {}", mode_bar(&mut h));
}

#[tokio::test]
async fn a_dump_that_finished_with_errors_says_how_many_shows_the_first_and_mentions_the_error_comments() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let done = DumpProgress { errors: 3, first_error: Some("table film: permission denied".into()), ..progress(DumpStatus::Done, "pagila", 22, 22, 65_000) };
    h.fake.script_dump(ScriptedDump::steps(vec![(Duration::ZERO, running("film", 7, 22, 100)), (Duration::ZERO, done)]));
    start(&mut h);
    h.settle().await;
    let notice = notice_text(&h);
    assert!(notice.contains("finished with 3 errors"), "{notice}");
    assert!(notice.contains("table film: permission denied"), "{notice}");
    assert!(notice.contains("-- ERROR"), "{notice}");
    assert!(h.app.model.dump.is_none(), "over");
}

#[tokio::test]
async fn a_finished_dump_gives_the_path_size_objects_rows_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (id, _, request) = start(&mut h);
    // dbx counts a whole database's objects per schema; its last report
    // counts the schemas.
    let done = progress(DumpStatus::Done, "pagila", 1, 1, 65_685);
    feed(&mut h, id, CoreReply::DumpProgress(done.clone()));
    let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: Some(done) }, file: FileLeft::Written(5_723_018) };
    feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
    let notice = notice_text(&h);
    for want in [request.path.as_str(), "5.7 MB", "1 schema", "65,685 rows"] {
        assert!(notice.contains(want), "{want:?} in {notice}");
    }
}

#[tokio::test]
async fn a_cancelled_progress_shows_the_cancelled_state_and_then_what_is_left_at_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (id, _, request) = start(&mut h);
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    h.press("D");
    feed(&mut h, id, CoreReply::DumpProgress(progress(DumpStatus::Cancelled, "film", 7, 22, 100)));
    assert!(words(&mut h).contains("cancelled"), "{}", words(&mut h));
    let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: Some(progress(DumpStatus::Cancelled, "", 0, 0, 0)) }, file: FileLeft::Written(141_342) };
    feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
    let text = words(&mut h);
    assert!(text.contains("cancelled") && text.contains("partial file") && text.contains("141.3 kB"), "{text}");
    // Nothing there: it says so.
    let (id, _, request) = {
        h.press("esc");
        start(&mut h)
    };
    feed(&mut h, id, CoreReply::DumpProgress(progress(DumpStatus::Cancelled, "", 0, 0, 0)));
    let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: Some(progress(DumpStatus::Cancelled, "", 0, 0, 0)) }, file: FileLeft::Nothing };
    feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
    assert!(notice_text(&h).contains("Nothing was written"), "{}", notice_text(&h));
}

#[tokio::test]
async fn a_failed_dump_shows_dbxs_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(ScriptedDump::failing(vec![(Duration::ZERO, running("film", 7, 22, 100))], "connection refused"));
    start(&mut h);
    h.settle().await;
    let notice = notice_text(&h);
    assert!(notice.contains("failed") && notice.contains("connection refused"), "{notice}");
    assert!(h.app.model.dump.is_none());
}

#[tokio::test]
async fn quit_during_a_dump_asks_naming_the_file_and_yes_cancels_and_quits_after_the_final_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(long_dump());
    let (_, dump, request) = start(&mut h);
    h.settle().await;
    let commands = h.press_fx("ctrl+g q");
    assert!(!commands.contains(&Command::Quit), "asks first");
    let text = words(&mut h);
    assert!(text.contains(&format!("pagila-{STAMP}.sql is running")), "names the file: {text}");
    assert!(h.app.quit_dump_message().contains(&request.path));
    // `esc` keeps it running.
    h.press("esc");
    assert!(h.app.model.dump.is_some() && !h.quit);
    h.press("ctrl+g q");
    let commands = h.press_fx("y");
    assert_eq!(cancels(&commands), vec![dump], "the dump is cancelled");
    assert!(!commands.contains(&Command::Quit), "not before its final progress");
    assert!(!h.quit);
    h.settle().await;
    assert!(h.quit, "quit once the final progress came");
}

#[tokio::test]
async fn quit_during_a_dump_quits_at_the_time_limit_if_the_final_progress_never_comes() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    start(&mut h);
    // (The overlay waits for dbx's first word, which never comes here.)
    h.press("esc ctrl+g q");
    h.press("y");
    assert!(!h.quit);
    h.advance(Duration::from_millis(500));
    assert!(!h.quit, "still waiting");
    h.advance(Duration::from_secs(3));
    assert!(h.quit, "gave up waiting");
}

// ---- layer 3 -----------------------------------------------------------------------

#[tokio::test]
async fn a_cancel_shows_cancelling_through_late_progress_then_cancelled_and_never_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(long_dump());
    let (id, dump, _) = start(&mut h);
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    // A click on the status opens the progress overlay.
    click_hint(&mut h, "dumping pagila");
    let text = words(&mut h);
    assert!(text.contains("c cancel dump"), "{text}");
    let commands = h.press_fx("c");
    assert_eq!(cancels(&commands), vec![dump]);
    assert!(words(&mut h).contains("cancelling…"), "{}", words(&mut h));
    // dbx is still working: more Running progress comes before it notices.
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 600)));
    feed(&mut h, id, CoreReply::DumpProgress(running("rental", 8, 22, 900)));
    let text = words(&mut h);
    assert!(text.contains("cancelling…") && !text.contains("c cancel dump"), "{text}");
    assert!(mode_bar(&mut h).contains("cancelling"), "{}", mode_bar(&mut h));
    feed(&mut h, id, CoreReply::DumpProgress(progress(DumpStatus::Cancelled, "rental", 8, 22, 900)));
    let text = words(&mut h);
    assert!(text.contains("cancelled") && !text.contains("cancelling"), "{text}");
    assert!(!text.to_lowercase().contains("dumped pagila") && !text.contains("done"), "{text}");
    h.settle().await;
    let text = words(&mut h);
    assert!(text.contains("cancelled") && !text.contains("done"), "{text}");
    assert!(h.app.model.dump.is_none());
}

#[tokio::test]
async fn progress_of_an_earlier_cancelled_dump_changes_nothing_and_repaints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (old, _, request) = start(&mut h);
    let cancelled = progress(DumpStatus::Cancelled, "", 0, 0, 0);
    feed(&mut h, old, CoreReply::DumpProgress(cancelled.clone()));
    feed(&mut h, old, CoreReply::Dumped(Ok(Dumped { path: request.path, end: crate::core::DumpEnd { last: Some(cancelled) }, file: FileLeft::Nothing })));
    assert!(h.app.model.dump.is_none());
    h.press("esc");
    let (new, _, _) = start(&mut h);
    feed(&mut h, new, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    let before = mode_bar(&mut h);
    let state = format!("{:?}", h.app.model.dump);
    assert!(!feed(&mut h, old, CoreReply::DumpProgress(running("actor", 1, 22, 99_999))), "no repaint");
    assert!(!feed(&mut h, old, CoreReply::Dumped(Err(DumpFailure { exists: false, refused: false, message: "late".into(), path: String::new(), file: FileLeft::Nothing }))));
    assert_eq!(format!("{:?}", h.app.model.dump), state, "nothing changed");
    assert_eq!(mode_bar(&mut h), before);
}

#[tokio::test]
async fn a_second_dump_while_one_runs_is_refused_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (id, _, _) = start(&mut h);
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    h.press("k");
    let commands = h.press_fx("D");
    assert!(dumps(&commands).is_empty());
    assert!(notice_text(&h).contains("One dump at a time"), "{}", notice_text(&h));
    assert!(words(&mut h).contains("film (7/22)"), "the running one's progress is shown: {}", words(&mut h));
    let commands = h.press_fx("enter");
    assert!(dumps(&commands).is_empty(), "enter does not start one from the progress view");
    // On the database being dumped, `D` just shows its progress.
    h.press("esc j");
    h.app.ui.notice = None;
    h.press("D");
    assert!(notice_text(&h).is_empty(), "{}", notice_text(&h));
    assert!(words(&mut h).contains("c cancel dump"), "{}", words(&mut h));
}

// ---- what dbx's dump is known to lose, and what the end claims ---------------------

/// The words the setup overlay and a clean end must carry on a connection
/// whose dump is known not to restore whole.
const LOSSES: [&str; 7] = ["known not to restore whole", "domains", "partitions", "composite arrays", "NaN", "tsvector", "README"];

#[tokio::test]
async fn the_setup_overlay_says_what_dbxs_dump_is_known_to_lose_on_postgres_and_only_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.press("D");
    let text = words(&mut h);
    for want in LOSSES.into_iter().chain(["geometric", "tsquery", "parents", "NULL"]) {
        assert!(text.contains(want), "{want:?} in {text}");
    }
    // A kind whose dump nobody has checked says nothing it does not know.
    h.press("esc");
    if let Loadable::Ready(list) = &mut h.app.model.connections {
        for c in list.iter_mut() {
            c.capabilities.dump_loses = false;
        }
    }
    h.press("D");
    assert!(!words(&mut h).contains("known not to restore"), "{}", words(&mut h));
}

#[tokio::test]
async fn a_clean_end_on_postgres_says_dbx_reported_no_errors_and_what_its_dump_loses_never_a_bare_dumped() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (id, _, request) = start(&mut h);
    let done = progress(DumpStatus::Done, "pagila", 1, 1, 65_685);
    feed(&mut h, id, CoreReply::DumpProgress(done.clone()));
    let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: Some(done) }, file: FileLeft::Written(5_723_018) };
    feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
    let notice = notice_text(&h);
    assert!(notice.contains("dbx reported no errors"), "{notice}");
    for want in LOSSES {
        assert!(notice.contains(want), "{want:?} in {notice}");
    }
    assert!(!notice.starts_with("Dumped"), "{notice}");
    // The progress overlay shows the same words.
    h.press("D");
    assert!(words(&mut h).contains("known not to restore whole"), "{}", words(&mut h));
}

#[tokio::test]
async fn a_return_without_a_final_done_is_not_called_dumped_and_says_what_is_at_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    let (id, _, request) = start(&mut h);
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: None }, file: FileLeft::Written(4_000) };
    feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
    let notice = notice_text(&h);
    assert!(notice.contains("ended without reporting that it was done"), "{notice}");
    assert!(notice.contains("4.0 kB") && notice.contains(&request.path), "what is at the path: {notice}");
    assert!(!notice.contains("Dumped") && !notice.contains("no errors"), "{notice}");
    assert!(h.app.ui.dump_ended.as_ref().is_some_and(|(_, failure)| *failure), "shown as a problem");
}

#[tokio::test]
async fn done_with_no_new_file_at_the_path_is_not_called_dumped() {
    let dir = tempfile::tempdir().unwrap();
    for (file, want) in [(FileLeft::Nothing, "Nothing was written"), (FileLeft::Unchanged, "earlier file")] {
        let mut h = on_pagila_row(dir.path()).await;
        let (id, _, request) = start(&mut h);
        let done = progress(DumpStatus::Done, "pagila", 1, 1, 10);
        feed(&mut h, id, CoreReply::DumpProgress(done.clone()));
        let dumped = Dumped { path: request.path.clone(), end: crate::core::DumpEnd { last: Some(done) }, file };
        feed(&mut h, id, CoreReply::Dumped(Ok(dumped)));
        let notice = notice_text(&h);
        assert!(notice.contains("reported done, but") && notice.contains(want), "{file:?}: {notice}");
        assert!(!notice.contains("Dumped") && !notice.contains("no errors"), "{notice}");
    }
}

#[tokio::test]
async fn a_failure_or_cancel_over_a_file_says_the_earlier_file_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(ScriptedDump::failing(vec![(Duration::ZERO, running("film", 7, 22, 100))], "connection refused"));
    let (id, _, request) = start(&mut h);
    feed(&mut h, id, CoreReply::DumpProgress(running("film", 7, 22, 100)));
    let failure = DumpFailure { exists: false, refused: false, message: "connection refused".into(), path: request.path.clone(), file: FileLeft::Unchanged };
    feed(&mut h, id, CoreReply::Dumped(Err(failure)));
    let notice = notice_text(&h);
    assert!(notice.contains("failed") && notice.contains("earlier file") && notice.contains("untouched"), "{notice}");
}

#[tokio::test]
async fn the_quit_question_says_where_temporary_copies_may_be_left() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = on_pagila_row(dir.path()).await;
    h.fake.script_dump(long_dump());
    start(&mut h);
    h.settle().await;
    let whole = h.app.quit_dump_message();
    let temp = std::env::temp_dir().to_string_lossy().into_owned();
    assert!(whole.contains("dbx's temporary copy may be left in") && whole.contains(&temp), "{whole}");
    assert!(whole.contains("beside"), "our own temporary file is named too: {whole}");
    // One schema is written straight to our file: no copy of dbx's.
    h.app.model.dump.as_mut().unwrap().job.schema = "public".into();
    let one = h.app.quit_dump_message();
    assert!(!one.contains("dbx's temporary copy") && one.contains("beside"), "{one}");
}
