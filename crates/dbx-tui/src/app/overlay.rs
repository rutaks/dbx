//! Overlays: boxes drawn over the panes that take all input until dismissed
//! (design section 3). One at a time, as `Ui.overlay: Option<Overlay>`; each
//! variant carries its own state. Slice 02 brought `Error`; slice 03 adds the
//! connection form, the removal question and a plain failure notice.

use ratatui::layout::Rect;

use super::form::{ConnectionForm, Field};
use crate::core::{ConnectionId, CoreError, CoreErrorKind, TabId};

#[derive(Debug, Clone)]
pub enum Overlay {
    /// A failure the client cannot work around, such as a store it cannot
    /// open. Blocking: the panes behind it take no input.
    Error(ErrorOverlay),
    /// Add or edit a connection.
    Form(Box<ConnectionForm>),
    /// "Remove this connection?", naming it.
    ConfirmRemove(ConfirmRemove),
    /// Something the user asked for failed and there is nothing to retry from
    /// here: say why, and let them go on.
    Failure(FailureOverlay),
    /// A second cancel while the first has not landed: "close this tab's
    /// session?", naming the database.
    ConfirmDropSession(ConfirmDropSession),
    /// "Load more" on a statement that is read-only but not proven safe to
    /// repeat: re-run it?
    ConfirmRerun(ConfirmRerun),
    /// Closing a tab that has text, or a statement running: "close tab 2?",
    /// saying what goes with it.
    ConfirmCloseTab(ConfirmCloseTab),
    /// The adapter would not run a statement without a yes: on production,
    /// or dangerous. Red, naming where, why and exactly what.
    ConfirmRun(Box<ConfirmRun>),
    /// One cell's whole value, wrapped and scrollable.
    CellViewer(Box<super::viewer::CellViewer>),
    /// "What to run?" when the caret is inside a subquery.
    Chooser(Chooser),
    /// Every action with all its keys.
    Help(HelpOverlay),
    /// Earlier statements, shared with the desktop app: search, preview,
    /// insert or run again (slice 18).
    History(Box<super::history::HistoryOverlay>),
    /// Write the result to a file: format, path, and what the format loses
    /// (slice 19).
    Export(Box<super::export::ExportOverlay>),
    /// Which text format to copy the result to the clipboard as.
    CopyAs(super::export::CopyAsOverlay),
}

/// One thing the chooser offers to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// `subquery` or `statement`.
    pub label: &'static str,
    /// Its first line.
    pub preview: String,
    /// The text that runs, exactly.
    pub range: (crate::core::Cursor, crate::core::Cursor),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chooser {
    pub items: Vec<Candidate>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpOverlay {
    /// The first row shown.
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDropSession {
    /// The tab the question was asked in: a yes acts on it, whichever tab is
    /// active when it is answered.
    pub tab: TabId,
    /// `connection / database`, as the editor's title names it.
    pub target: String,
    /// The run ended while the question was open: the box only says so.
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRerun {
    /// The tab the question was asked in.
    pub tab: TabId,
    /// The statement's first non-blank line.
    pub statement: String,
}

/// What a confirmed statement was run as, so a yes runs it the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPurpose {
    /// A run of the editor's text, or of what the chooser picked.
    Plain,
    /// "Load more", with the cap it asked for.
    LoadMore(super::query::LoadMore),
    /// A page or the count of a browse tab.
    Browse(super::query::Fetch),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRun {
    /// The tab the run was in.
    pub tab: TabId,
    /// Why, and what a yes sends back.
    pub assessment: crate::core::Assessment,
    pub purpose: RunPurpose,
    /// The row cap the run asked for.
    pub row_cap: usize,
    /// The first line of the statement shown.
    pub scroll: usize,
    /// When the box opened: a `y` before `CONFIRM_GRACE` has passed is
    /// ignored (typing that was already under way must not confirm).
    pub opened: Option<std::time::Instant>,
}

/// How long a new confirmation box ignores `y`.
pub const CONFIRM_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmCloseTab {
    /// The tab to close: a yes closes it, whichever tab is active then.
    pub tab: TabId,
    /// How the bar names it: `tab 2 (local pg / pagila)`.
    pub name: String,
    /// It has text, which goes with it.
    pub text: bool,
    /// A statement is running in it, which is cancelled.
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRemove {
    pub id: ConnectionId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureOverlay {
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorOverlay {
    pub title: String,
    /// What happened and what to do, as the Core said it.
    pub message: String,
}

impl ErrorOverlay {
    /// The overlay for a store or key failure, or `None` for an error that
    /// belongs somewhere less drastic (a status line, the results pane).
    pub fn for_startup(error: &CoreError) -> Option<Self> {
        let title = match error.kind {
            CoreErrorKind::Key => "Store key unavailable",
            CoreErrorKind::Store => "Store unavailable",
            _ => return None,
        };
        Some(ErrorOverlay { title: title.into(), message: error.message.clone() })
    }
}

/// What a button in an overlay does. The key and the click take one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    /// Ask the Core again for what failed.
    Retry,
    Quit,
    /// The form: save, test, and `esc` (close, asking first if it was changed).
    FormSave,
    FormTest,
    FormClose,
    /// The "discard changes?" question: discard, or keep editing.
    DiscardYes,
    DiscardNo,
    /// The removal question: remove.
    RemoveYes,
    /// The load-more question: run the statement again.
    RerunYes,
    /// The second-cancel question: close the session.
    DropSessionYes,
    /// The close-tab question: close it.
    CloseTabYes,
    /// The run confirmation: run it, or not (`Not run.`).
    ConfirmRunYes,
    ConfirmRunNo,
    /// The statement chooser: run the highlighted entry, or one that was clicked.
    ChooserRun,
    ChooserPick(usize),
    /// The history: put the highlighted statement in the editor, run it, flip
    /// between this connection and all of them, or highlight the entry that
    /// was clicked.
    HistoryInsert,
    HistoryRun,
    HistoryScope,
    HistoryPick(usize),
    /// The export overlay: write the file; replace the file that is there;
    /// keep it and go back to the overlay; pick a format; focus a row; pick
    /// the CSV NULL literal (true: an empty field).
    ExportRun,
    ExportReplace,
    ExportKeep,
    ExportFormat(usize),
    ExportFocus(super::export::ExportField),
    ExportNull(bool),
    /// The copy-as list: copy in the highlighted format, or in the one clicked.
    CopyAsRun,
    CopyAsPick(usize),
    /// Close a notice or a question without doing anything.
    Dismiss,
    /// The cell viewer: copy the raw value, and switch between a JSON
    /// cell's pretty-printed and stored text.
    ViewerCopy,
    ViewerToggleRaw,
    /// The cell viewer's scroll hints: a line, a page, the ends.
    ViewerLineDown,
    ViewerLineUp,
    ViewerPageDown,
    ViewerPageUp,
    ViewerTop,
    ViewerBottom,
}

/// Where a form row was drawn, for clicks. Filled by `view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldHit {
    pub rect: Rect,
    pub field: Field,
}

/// A button as drawn: where it is, and what it does. Filled by `view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayButton {
    pub rect: Rect,
    pub action: OverlayAction,
}

/// A button's key, as shown (`↵`) and as matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonSpec {
    pub key_label: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub action: OverlayAction,
}

const fn button(key_label: &'static str, key: &'static str, label: &'static str, action: OverlayAction) -> ButtonSpec {
    ButtonSpec { key_label, key, label, action }
}

// No `esc` on the startup error: a store the client cannot open leaves nothing
// behind the overlay to go back to, so the choices are retry or quit.
const ERROR_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "retry", OverlayAction::Retry), button("q", "q", "quit", OverlayAction::Quit)];
const FORM_BUTTONS: &[ButtonSpec] = &[
    button("ctrl+t", "ctrl+t", "test", OverlayAction::FormTest),
    button("↵", "enter", "save", OverlayAction::FormSave),
    button("esc", "esc", "cancel", OverlayAction::FormClose),
];
const DISCARD_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "discard", OverlayAction::DiscardYes), button("esc", "esc", "keep editing", OverlayAction::DiscardNo)];
const REMOVE_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "remove", OverlayAction::RemoveYes), button("esc", "esc", "cancel", OverlayAction::Dismiss)];
const RERUN_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "re-run", OverlayAction::RerunYes), button("esc", "esc", "cancel", OverlayAction::Dismiss)];
const CLOSE_TAB_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "close tab", OverlayAction::CloseTabYes), button("esc", "esc", "keep it", OverlayAction::Dismiss)];
const STOPPED_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "close", OverlayAction::Dismiss), button("esc", "esc", "close", OverlayAction::Dismiss)];
const DROP_SESSION_BUTTONS: &[ButtonSpec] = &[
    button("↵", "enter", "new session", OverlayAction::DropSessionYes),
    button("esc", "esc", "keep waiting", OverlayAction::Dismiss),
];
// `esc` also closes a failure notice (see `App::on_overlay_key`): backing out
// one level always works.
const CHOOSER_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "run", OverlayAction::ChooserRun), button("esc", "esc", "cancel", OverlayAction::Dismiss)];
// No `enter`: a habit of `f5` then `enter` must not confirm.
const CONFIRM_RUN_BUTTONS: &[ButtonSpec] =
    &[button("y", "y", "run", OverlayAction::ConfirmRunYes), button("esc", "esc", "cancel", OverlayAction::ConfirmRunNo)];
const HELP_BUTTONS: &[ButtonSpec] = &[button("esc", "esc", "close", OverlayAction::Dismiss)];
// The history's own keys are read by `App::on_history_key` (the run key is
// the live one), and its buttons are drawn with the live key labels.
const HISTORY_BUTTONS: &[ButtonSpec] = &[];
const FAILURE_BUTTONS: &[ButtonSpec] = &[button("↵", "enter", "close", OverlayAction::Dismiss)];
const EXPORT_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "export", OverlayAction::ExportRun), button("esc", "esc", "cancel", OverlayAction::Dismiss)];
// While the write is out, `enter` would send it again: only `esc` is offered.
const EXPORT_WRITING_BUTTONS: &[ButtonSpec] = &[button("esc", "esc", "close", OverlayAction::Dismiss)];
// No `enter`: it is the key that sent the export.
const EXPORT_REPLACE_BUTTONS: &[ButtonSpec] =
    &[button("y", "y", "replace", OverlayAction::ExportReplace), button("esc", "esc", "keep the file", OverlayAction::ExportKeep)];
const COPY_AS_BUTTONS: &[ButtonSpec] =
    &[button("↵", "enter", "copy", OverlayAction::CopyAsRun), button("esc", "esc", "cancel", OverlayAction::Dismiss)];

impl Overlay {
    /// The buttons along the bottom of the overlay, left to right. Their keys
    /// are the only keys the overlay answers to (the form also takes text).
    pub fn buttons(&self) -> &'static [ButtonSpec] {
        match self {
            Overlay::Error(_) => ERROR_BUTTONS,
            // A form that is saving takes no input, and offers no keys.
            Overlay::Form(form) if form.locked() => &[],
            Overlay::Form(form) if form.confirm_discard => DISCARD_BUTTONS,
            Overlay::Form(_) => FORM_BUTTONS,
            Overlay::ConfirmRemove(_) => REMOVE_BUTTONS,
            Overlay::Failure(_) => FAILURE_BUTTONS,
            Overlay::ConfirmRerun(_) => RERUN_BUTTONS,
            Overlay::ConfirmCloseTab(_) => CLOSE_TAB_BUTTONS,
            Overlay::ConfirmRun(_) => CONFIRM_RUN_BUTTONS,
            Overlay::ConfirmDropSession(ask) if ask.stopped => STOPPED_BUTTONS,
            Overlay::ConfirmDropSession(_) => DROP_SESSION_BUTTONS,
            // Drawn with the live keys, not from this table.
            Overlay::Chooser(_) => CHOOSER_BUTTONS,
            Overlay::Help(_) => HELP_BUTTONS,
            Overlay::History(_) => HISTORY_BUTTONS,
            Overlay::Export(o) => match &o.stage {
                super::export::Stage::Writing(_) => EXPORT_WRITING_BUTTONS,
                super::export::Stage::Replace(_) => EXPORT_REPLACE_BUTTONS,
                _ => EXPORT_BUTTONS,
            },
            Overlay::CopyAs(_) => COPY_AS_BUTTONS,
            Overlay::CellViewer(_) => &[],
        }
    }
}
