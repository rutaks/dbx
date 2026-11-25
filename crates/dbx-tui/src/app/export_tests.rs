//! Layer 2: the acceptance tests of slice 19 (export and copy), run on `App`
//! with a `FakeCore` behind the shell's command runner. What dbx's formats
//! write is tested in `tests/live.rs`; here the fake uses the same formats.

use ratatui::layout::Rect;

use super::export::ExportEnv;
use super::grid_tests::{focused, screen, truncated_set, where_is};
use super::requests::RequestId;
use super::run_tests::{col, mode_bar, notice_text, rows_of};
use super::testing::click;
use super::*;
use crate::core::{Cell, CellKind, ExportError, ExportFormat, ExportJob, Exported, ResultSet};
use crate::harness::Harness;

// ---- helpers ------------------------------------------------------------------

const DIR: &str = "/work";
const STAMP: &str = "20261125-101500";

/// id, name: `1 a`, `2 NULL`, `3 x|y`.
fn three() -> ResultSet {
    let v = |s: &str| Cell::Value(s.into());
    ResultSet {
        columns: vec![col("id", "integer", CellKind::Integer), col("name", "text", CellKind::Text)],
        rows: vec![vec![v("1"), v("a")], vec![v("2"), Cell::Null], vec![v("3"), v("x|y")]],
        ran: "select id, name from t order by id".into(),
        ..rows_of(&[])
    }
}

/// A result shown and focused, with the export directory and clock fixed.
async fn shown(set: ResultSet) -> Harness {
    let mut h = focused(set).await;
    h.app.ui.export_env = ExportEnv::fixed(DIR, STAMP);
    h
}

/// The whole screen as one line of single-spaced words, box borders left out,
/// so a phrase that wrapped is still found.
fn words(h: &mut Harness) -> String {
    let text: String = screen(h).join(" ").chars().filter(|c| !"│┌┐└┘─".contains(*c)).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn exports(commands: &[Command]) -> Vec<(RequestId, ExportJob)> {
    commands.iter().filter_map(|c| if let Command::Export(id, job) = c { Some((*id, job.clone())) } else { None }).collect()
}

fn copied(commands: &[Command]) -> Vec<String> {
    commands.iter().filter_map(|c| if let Command::CopyToClipboard(t) = c { Some(t.clone()) } else { None }).collect()
}

/// Feeds the shell's answer for export request `id`.
fn answer(h: &mut Harness, id: RequestId, reply: Result<Exported, ExportError>) {
    let now = h.now();
    let fx = h.app.update(Event::Core(id, CoreReply::Exported(reply)), now);
    h.run(fx);
}

fn written(path: &str, rows: usize, bytes: u64) -> Result<Exported, ExportError> {
    Ok(Exported { path: path.into(), rows, bytes })
}

fn exists(path: &str) -> Result<Exported, ExportError> {
    Err(ExportError { exists: true, message: format!("{path} already exists.") })
}

/// `enter` in the export overlay: the one command it sends.
fn confirm(h: &mut Harness) -> (RequestId, ExportJob) {
    let mut sent = exports(&h.press_fx("enter"));
    assert_eq!(sent.len(), 1, "exactly one export command");
    sent.remove(0)
}

fn csv_path() -> String {
    format!("{DIR}/result-{STAMP}.csv")
}

// ---- the overlay -------------------------------------------------------------------

#[tokio::test]
async fn the_mode_bar_offers_export_and_copy_as_where_they_work_and_a_click_on_the_hint_opens_the_overlay() {
    let mut h = shown(three()).await;
    let bar = mode_bar(&mut h);
    assert!(bar.contains("e export") && bar.contains("C copy as"), "{bar:?}");
    let (x, y) = where_is(&mut h, "e export").expect("the hint");
    click(&mut h.app, Rect::new(x, y, 1, 1));
    assert!(words(&mut h).contains("Export result"), "{}", words(&mut h));
    // Not in the editor, and not with no result.
    let mut h = shown(three()).await;
    h.press("ctrl+g k");
    let bar = mode_bar(&mut h);
    assert!(!bar.contains("e export") && !bar.contains("copy as"), "{bar:?}");
    assert!(h.press_fx("e").is_empty() && !words(&mut h).contains("Export result"));
}

#[tokio::test]
async fn e_opens_the_overlay_with_five_formats_and_the_default_path() {
    let mut h = shown(three()).await;
    h.press("e");
    let text = words(&mut h);
    for label in ["● CSV", "○ JSON", "○ Excel", "○ Markdown", "○ SQL INSERT"] {
        assert!(text.contains(label), "{label:?} in {text}");
    }
    assert!(text.contains(&csv_path()), "{text}");
    assert!(text.contains("Writes the 3 rows held"), "{text}");
    // The extension follows the format while the path is the default.
    h.press("right");
    assert!(words(&mut h).contains(&format!("{DIR}/result-{STAMP}.json")), "{}", words(&mut h));
    h.press("right right right");
    assert!(words(&mut h).contains(&format!("{DIR}/result-{STAMP}.sql")), "{}", words(&mut h));
    h.press("left left");
    assert!(words(&mut h).contains(&format!("{DIR}/result-{STAMP}.xlsx")));
    h.press("right");
    assert!(words(&mut h).contains(&format!("{DIR}/result-{STAMP}.md")));
}

#[tokio::test]
async fn a_path_that_was_typed_keeps_its_name_when_the_format_changes() {
    let mut h = shown(three()).await;
    h.press("e tab");
    h.type_text("-mine");
    assert!(words(&mut h).contains(&format!("{}-mine", csv_path())));
    h.press("shift+tab right");
    assert!(words(&mut h).contains(&format!("{}-mine", csv_path())), "an edited path is the user's: {}", words(&mut h));
    let (_, job) = confirm(&mut h);
    assert_eq!(job.path, format!("{}-mine", csv_path()));
    assert_eq!(job.request.format, ExportFormat::Json);
}

#[tokio::test]
async fn a_browse_tab_names_the_file_and_the_sql_table_after_its_table() {
    let mut h = super::browse_tests::browsing(3).await;
    h.press("ctrl+g j");
    h.app.ui.export_env = ExportEnv::fixed(DIR, STAMP);
    h.press("e");
    assert!(words(&mut h).contains(&format!("{DIR}/film-{STAMP}.csv")), "{}", words(&mut h));
    h.press("right right right right");
    let text = words(&mut h);
    assert!(text.contains("table film") && text.contains("schema public"), "{text}");
    let (_, job) = confirm(&mut h);
    assert_eq!((job.request.table.as_str(), job.request.schema.as_str()), ("film", "public"));
    assert_eq!(job.path, format!("{DIR}/film-{STAMP}.sql"));
    assert_eq!(job.request.kind, Some(crate::core::DbKind::Postgres), "the SQL is for the database the rows came from");
}

#[tokio::test]
async fn the_sql_fields_and_the_csv_null_choice_show_only_for_their_formats() {
    let mut h = shown(three()).await;
    h.press("e");
    let text = words(&mut h);
    assert!(text.contains("null ● \\N ○ empty") && !text.contains("table result"), "{text}");
    h.press("right");
    let text = words(&mut h);
    assert!(!text.contains("null ●") && !text.contains("schema"), "{text}");
    h.press("right right right");
    let text = words(&mut h);
    assert!(text.contains("table result") && text.contains("schema") && !text.contains("null ●"), "{text}");
}

// ---- writing ---------------------------------------------------------------------------

#[tokio::test]
async fn confirming_sends_one_export_command_and_the_notice_waits_for_the_shell_to_say_it_wrote() {
    let set = three();
    let mut h = shown(set.clone()).await;
    h.press("e");
    let (id, job) = confirm(&mut h);
    assert_eq!(job.request.format, ExportFormat::Csv);
    assert_eq!(job.request.columns, set.columns);
    assert_eq!(job.request.rows, set.rows, "the rows held");
    assert_eq!(job.path, csv_path());
    assert!(!job.replace && !job.request.null_empty);
    assert!(h.app.ui.notice.is_none(), "nothing is said before the shell reports: {:?}", h.app.ui.notice);
    assert!(words(&mut h).contains("writing"), "{}", words(&mut h));
    // A second enter while the write is out sends nothing more.
    assert!(exports(&h.press_fx("enter")).is_empty());
    answer(&mut h, id, written(&csv_path(), 3, 41));
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(notice_text(&h), format!("Exported 3 rows to {} (41 B)", csv_path()));
    assert!(!h.app.ui.notice.as_ref().unwrap().failure);
}

#[tokio::test]
async fn a_failed_write_names_the_path_and_the_error_and_keeps_every_field() {
    let mut h = shown(three()).await;
    h.press("e right tab");
    h.type_text("-x");
    let (id, job) = confirm(&mut h);
    let err = ExportError { exists: false, message: format!("Not written to {}: the directory {DIR} does not exist", job.path) };
    answer(&mut h, id, Err(err));
    assert!(h.app.ui.notice.is_none(), "{:?}", h.app.ui.notice);
    // (The message wraps inside the box, so its parts are looked for apart.)
    let text = words(&mut h);
    assert!(text.contains("Not written to") && text.contains("does not exist"), "{text}");
    assert!(text.matches(&job.path).count() == 2, "the path is in the error and in the field: {text}");
    assert!(text.contains("● JSON"), "every field as typed: {text}");
    // The user fixes the path and tries again from where they were.
    h.press("backspace backspace");
    let (_, again) = confirm(&mut h);
    assert_eq!(again.path, format!("{DIR}/result-{STAMP}.json"));
    assert!(!words(&mut h).contains("does not exist"), "the old error is gone while the new write is out");
}

#[tokio::test]
async fn a_path_that_is_there_already_asks_first_naming_it_and_esc_returns_with_every_field_as_typed() {
    let mut h = shown(three()).await;
    h.press("e right tab");
    h.type_text("x");
    let (id, job) = confirm(&mut h);
    assert!(!job.replace);
    answer(&mut h, id, exists(&job.path));
    let text = words(&mut h);
    assert!(text.contains("Replace") && text.contains(&job.path), "the question names the file: {text}");
    assert!(h.app.ui.notice.is_none());
    // `enter` is not a yes: it is the key that sent the export.
    assert!(exports(&h.press_fx("enter")).is_empty());
    assert!(words(&mut h).contains("Replace"));
    // `esc` goes back to the overlay, as it was.
    h.press("esc");
    let text = words(&mut h);
    assert!(!text.contains("Replace") && text.contains("Export result") && text.contains(&job.path) && text.contains("● JSON"), "{text}");
    // Asking again and saying yes sends the same job with the replace allowed.
    let (id, _) = confirm(&mut h);
    answer(&mut h, id, exists(&job.path));
    let sent = exports(&h.press_fx("y"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, ExportJob { replace: true, ..job.clone() });
    answer(&mut h, sent[0].0, written(&job.path, 3, 99));
    assert_eq!(notice_text(&h), format!("Exported 3 rows to {} (99 B)", job.path));
}

#[tokio::test]
async fn esc_while_the_write_is_out_closes_the_overlay_and_the_answer_still_tells_the_truth() {
    let mut h = shown(three()).await;
    h.press("e");
    let (id, job) = confirm(&mut h);
    h.press("esc");
    assert!(h.app.ui.overlay.is_none(), "esc always backs out");
    answer(&mut h, id, written(&job.path, 3, 41));
    assert_eq!(notice_text(&h), format!("Exported 3 rows to {} (41 B)", job.path), "it was written, so it is said");
    // The same for an answer that a file is there: nothing was replaced.
    h.press("e");
    let (id, job) = confirm(&mut h);
    h.press("esc");
    answer(&mut h, id, exists(&job.path));
    let text = notice_text(&h);
    assert!(text.contains(&job.path) && text.contains("already exists") && text.contains("Nothing was replaced"), "{text}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    // And a failure.
    h.press("e");
    let (id, _) = confirm(&mut h);
    h.press("esc");
    answer(&mut h, id, Err(ExportError { exists: false, message: "Not written to /work/x: disk full".into() }));
    assert_eq!(notice_text(&h), "Not written to /work/x: disk full");
}

#[tokio::test]
async fn an_empty_path_sends_nothing_and_says_what_to_do() {
    let mut h = shown(three()).await;
    h.press("e tab");
    // However the field was emptied, an empty name is never sent.
    for _ in 0..(csv_path().len() + 2) {
        h.press("backspace");
    }
    assert!(exports(&h.press_fx("enter")).is_empty());
    assert!(words(&mut h).contains("Give the file a name"), "{}", words(&mut h));
}

#[tokio::test]
async fn a_result_that_changed_while_the_overlay_was_open_is_not_exported_under_the_old_description() {
    let mut h = shown(three()).await;
    h.press("e");
    // Another run lands in the tab.
    let id = h.app.ui.tab.id;
    h.app.model.active_mut(id).outcome = Some(super::query::Outcome::Rows(ResultSet { rows: vec![], ..three() }));
    assert!(exports(&h.press_fx("enter")).is_empty());
    assert!(h.app.ui.overlay.is_none());
    assert!(notice_text(&h).contains("changed") && notice_text(&h).contains("press e again"), "{}", notice_text(&h));
}

#[tokio::test]
async fn the_csv_null_choice_and_the_sql_names_reach_the_command() {
    let mut h = shown(three()).await;
    h.press("e down down");
    h.press("space");
    assert!(words(&mut h).contains("○ \\N ● empty"), "{}", words(&mut h));
    let (_, job) = confirm(&mut h);
    assert!(job.request.null_empty);
    let mut h = shown(three()).await;
    h.press("e right right right right down down");
    h.type_text("-t");
    h.press("tab");
    h.type_text("sch");
    let (_, job) = confirm(&mut h);
    assert_eq!((job.request.table.as_str(), job.request.schema.as_str()), ("result-t", "sch"));
}

#[tokio::test]
async fn the_mouse_picks_a_format_and_presses_the_export_button() {
    let mut h = shown(three()).await;
    h.press("e");
    screen(&mut h);
    let (x, y) = where_is(&mut h, "JSON").expect("the format");
    click(&mut h.app, Rect::new(x, y, 1, 1));
    assert!(words(&mut h).contains("● JSON"), "{}", words(&mut h));
    screen(&mut h);
    let (x, y) = where_is(&mut h, "export").expect("the button");
    let sent = exports(&click(&mut h.app, Rect::new(x, y, 1, 1)).commands);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.request.format, ExportFormat::Json);
}

// ---- what the overlay says ---------------------------------------------------------------

#[tokio::test]
async fn a_cut_result_says_how_many_rows_the_file_holds_and_how_to_get_the_rest() {
    let mut h = shown(truncated_set(1_000)).await;
    h.press("e");
    let text = words(&mut h);
    assert!(text.contains("Writes the first 1,000 rows"), "{text}");
    assert!(text.contains("cut at the row cap") && text.contains("load more") && text.contains("narrower query"), "{text}");
    let mut h = shown(ResultSet { truncated: false, ..truncated_set(1_000) }).await;
    h.press("e");
    let text = words(&mut h);
    assert!(text.contains("Writes the 1,000 rows held") && !text.contains("row cap"), "{text}");
}

#[tokio::test]
async fn each_formats_losses_show_for_that_format_only() {
    let mut h = shown(three()).await;
    h.press("e");
    // (the phrase only this format's line has, every other format's phrase)
    let phrases = [
        ("a value starting with = + - or @ gets a ' in front", "CSV"),
        ("integers that fit 64 bits are numbers", "JSON"),
        ("NULL and '' both show as an empty cell", "Excel"),
        ("NULL looks like the text NULL", "Markdown"),
        ("only INSERT statements", "SQL INSERT"),
    ];
    for (i, (_, name)) in phrases.iter().enumerate() {
        let text = words(&mut h);
        assert!(text.contains(&format!("● {name}")), "{name}: {text}");
        for (j, (other, _)) in phrases.iter().enumerate() {
            assert_eq!(text.contains(other), i == j, "{name}: {other:?} in {text}");
        }
        h.press("right");
    }
    // The details of each.
    h.press("1");
    let text = words(&mut h);
    assert!(text.contains("NULL is written as \\N"), "{text}");
    h.press("down down space");
    let text = words(&mut h);
    assert!(text.contains("NULL is an empty field") && text.contains("a reader may take it for ''") && !text.contains("NULL is written as \\N"), "{text}");
    h.press("shift+tab shift+tab 4");
    let text = words(&mut h);
    for loss in ["NULL looks like the text NULL", "line breaks become <br>", "| becomes \\|", "trailing spaces are padding"] {
        assert!(text.contains(loss), "{loss:?} in {text}");
    }
    h.press("3");
    let text = words(&mut h);
    for loss in ["dates and times become Excel dates", "numbers up to 15 digits become numbers", "no formula guard"] {
        assert!(text.contains(loss), "{loss:?} in {text}");
    }
}

#[tokio::test]
async fn the_excel_loss_counts_the_values_over_its_cell_limit() {
    let long = |n: usize| Cell::Value("x".repeat(n));
    let set = ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![long(40_000)], vec![long(32_767)], vec![long(50_000)]], ..rows_of(&[]) };
    let mut h = shown(set).await;
    h.press("e 3");
    let text = words(&mut h);
    assert!(text.contains("2 values here are over 32,767 characters") && text.contains("written whole") && text.contains("Excel may cut them"), "{text}");
    let mut h = shown(three()).await;
    h.press("e 3");
    let text = words(&mut h);
    assert!(text.contains("a value over 32,767 characters") && text.contains("written whole") && !text.contains("values here"), "{text}");
}

// ---- copy as -----------------------------------------------------------------------------------

const MARKDOWN: &str = "| id  | name |\n| --- | ---- |\n| 1   | a    |\n| 2   | NULL |\n| 3   | x\\|y |\n";

#[tokio::test]
async fn copying_a_three_row_result_as_markdown_sends_dbxs_markdown_to_the_clipboard() {
    let mut h = shown(three()).await;
    h.press("C");
    let text = words(&mut h);
    for entry in ["1 CSV", "2 JSON", "3 Markdown", "4 SQL INSERT"] {
        assert!(text.contains(entry), "{entry:?} in {text}");
    }
    assert!(!text.contains("Excel"), "a spreadsheet file is not text: {text}");
    let sent = h.press_fx("3");
    assert_eq!(copied(&sent), vec![MARKDOWN.to_string()]);
    assert!(h.app.ui.overlay.is_none());
    assert_eq!(notice_text(&h), format!("Sent Markdown (3 rows) to the terminal clipboard ({} B)", MARKDOWN.len()));
}

#[tokio::test]
async fn copy_as_takes_arrows_enter_and_clicks_and_esc_backs_out() {
    let mut h = shown(three()).await;
    h.press("C down");
    let sent = copied(&h.press_fx("enter"));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with('[') && sent[0].contains("\"name\": null"), "JSON: {}", sent[0]);
    h.press("C");
    assert!(copied(&h.press_fx("esc")).is_empty() && h.app.ui.overlay.is_none());
    h.press("C");
    screen(&mut h);
    let (x, y) = where_is(&mut h, "CSV").expect("the entry");
    let sent = copied(&click(&mut h.app, Rect::new(x, y, 1, 1)).commands);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("\"id\",\"name\"\n"), "{}", sent[0]);
    h.press("C 4");
    assert!(notice_text(&h).contains("SQL INSERT"), "{}", notice_text(&h));
}

#[tokio::test]
async fn a_cut_result_says_so_when_it_is_copied() {
    let mut h = shown(truncated_set(5)).await;
    h.press("C 1");
    let text = notice_text(&h);
    assert!(text.starts_with("Sent CSV (the first 5 rows, the result was cut at the row cap) to the terminal clipboard"), "{text}");
}

#[tokio::test]
async fn a_copy_over_75000_bytes_is_refused_and_the_notice_suggests_a_file() {
    // Too large before it is even formatted.
    let big = |n: usize| ResultSet { columns: vec![col("t", "text", CellKind::Text)], rows: vec![vec![Cell::Value("x".repeat(n))]], ..rows_of(&[]) };
    let mut h = shown(big(80_000)).await;
    h.press("C");
    let sent = h.press_fx("1");
    assert!(copied(&sent).is_empty(), "nothing is sent");
    let text = notice_text(&h);
    assert!(text.contains("Not copied") && text.contains("CSV") && text.contains("75.0 kB") && text.contains("export it to a file"), "{text}");
    assert!(h.app.ui.notice.as_ref().unwrap().failure);
    // Small as text, over the limit once JSON has escaped it.
    let mut h = shown(ResultSet { rows: vec![vec![Cell::Value("\"".repeat(40_000))]], ..big(0) }).await;
    h.press("C");
    assert!(copied(&h.press_fx("2")).is_empty());
    assert!(notice_text(&h).contains("Not copied") && notice_text(&h).contains("JSON") && notice_text(&h).contains("export it to a file"), "{}", notice_text(&h));
    // Just under it goes.
    let mut h = shown(big(74_000)).await;
    h.press("C");
    assert_eq!(copied(&h.press_fx("1")).len(), 1);
}

// ---- row copy: the leftover of slice 07 ------------------------------------------------------------

#[tokio::test]
async fn a_row_copy_says_when_the_row_is_lossy_and_a_plain_row_does_not() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("a", "text", CellKind::Text), col("b", "text", CellKind::Text), col("c", "text", CellKind::Text)],
        rows: vec![
            vec![v("x"), v("y"), v("z")],
            vec![v("x"), Cell::Null, v("")],
            vec![v("x"), Cell::Null, Cell::Null],
            vec![v(""), v(""), v("z")],
            vec![v("a\tb"), v("y"), v("z")],
            vec![v("x"), v("two\nlines"), v("z")],
        ],
        ..rows_of(&[])
    };
    let mut h = shown(set).await;
    h.press("Y");
    assert!(!notice_text(&h).contains("Lossy"), "a plain row: {}", notice_text(&h));
    assert!(notice_text(&h).starts_with("Sent row to the terminal clipboard (5 B)"), "{}", notice_text(&h));
    h.press("j Y");
    let text = notice_text(&h);
    assert!(text.starts_with("Sent row to the terminal clipboard") && text.contains("Lossy: NULL and an empty string both paste as an empty field"), "{text}");
    h.press("j Y");
    assert!(!notice_text(&h).contains("Lossy"), "two NULLs and no empty string are not mistaken for each other: {}", notice_text(&h));
    h.press("j Y");
    assert!(!notice_text(&h).contains("Lossy"), "an empty string alone is not lossy: {}", notice_text(&h));
    h.press("j Y");
    let text = notice_text(&h);
    assert!(text.contains("Lossy: a tab inside a cell splits it when pasted into a spreadsheet"), "{text}");
    h.press("j Y");
    let text = notice_text(&h);
    assert!(text.contains("Lossy: a line break inside a cell splits it when pasted into a spreadsheet"), "{text}");
}

#[tokio::test]
async fn sql_insert_names_the_columns_dbx_leaves_out_and_only_for_that_format() {
    let v = |s: &str| Cell::Value(s.into());
    let set = ResultSet {
        columns: vec![col("id", "integer", CellKind::Integer), col("doc", "tsvector", CellKind::Text), col("__DBX_ROWID", "text", CellKind::Text)],
        rows: vec![vec![v("1"), v("'a':1"), v("x")]],
        ran: "select id, doc from t".into(),
        ..rows_of(&[])
    };
    let mut h = shown(set).await;
    h.press("e");
    assert!(!words(&mut h).contains("left out of the INSERT"), "{}", words(&mut h));
    h.press("right right right right");
    let text = words(&mut h);
    assert!(text.contains("left out of the INSERT: doc, __DBX_ROWID"), "{text}");
    assert!(!words(&mut shown(three()).await).contains("left out"), "a result with no such column says nothing");
    let mut plain = shown(three()).await;
    plain.press("e right right right right");
    assert!(!words(&mut plain).contains("left out of the INSERT"), "{}", words(&mut plain));
}
