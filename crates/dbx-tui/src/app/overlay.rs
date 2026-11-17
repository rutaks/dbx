//! Overlays: boxes drawn over the panes that take all input until dismissed
//! (design section 3). One at a time, as `Ui.overlay: Option<Overlay>`; each
//! variant carries its own state. Slice 02 brings the first, `Error`.

use ratatui::layout::Rect;

use crate::core::{CoreError, CoreErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// A failure the client cannot work around, such as a store it cannot
    /// open. Blocking: the panes behind it take no input.
    Error(ErrorOverlay),
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

impl Overlay {
    /// The buttons along the bottom of the overlay, left to right. Their keys
    /// are the only keys the overlay answers to.
    pub fn buttons(&self) -> &'static [ButtonSpec] {
        match self {
            // No `esc`: a store the client cannot open leaves nothing behind
            // the overlay to go back to, so the choices are retry or quit.
            Overlay::Error(_) => &[
                ButtonSpec { key_label: "↵", key: "enter", label: "retry", action: OverlayAction::Retry },
                ButtonSpec { key_label: "q", key: "q", label: "quit", action: OverlayAction::Quit },
            ],
        }
    }
}
