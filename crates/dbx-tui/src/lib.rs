//! dbpane (this package is `dbx-tui`): a terminal database client, built as a head on dbx's core.
//!
//! Three modules (design section 1): `shell` owns the terminal and tokio,
//! `app` is pure state, update and view, and `core` is the seam to dbx.

pub mod app;
pub mod core;
pub mod shell;

#[cfg(test)]
mod harness;
