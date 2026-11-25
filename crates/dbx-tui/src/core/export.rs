//! Exporting a result: the formats, what a request carries to the Core, and
//! what the shell reports when the file is written (slice 19).
//!
//! The rows are the ones the client holds, as exact text. dbx's formats take
//! JSON values; the adapter converts (`DbxCore`, `export_bytes`).

use super::{Cell, Column, DbKind};

/// What a result can be written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
    Excel,
    Markdown,
    SqlInsert,
}

impl ExportFormat {
    /// In the order the overlay offers them.
    pub const ALL: [ExportFormat; 5] = [ExportFormat::Csv, ExportFormat::Json, ExportFormat::Excel, ExportFormat::Markdown, ExportFormat::SqlInsert];

    /// The formats a copy to the clipboard offers: the text ones (a spreadsheet
    /// file is binary).
    pub const COPYABLE: [ExportFormat; 4] = [ExportFormat::Csv, ExportFormat::Json, ExportFormat::Markdown, ExportFormat::SqlInsert];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Excel => "Excel",
            ExportFormat::Markdown => "Markdown",
            ExportFormat::SqlInsert => "SQL INSERT",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Excel => "xlsx",
            ExportFormat::Markdown => "md",
            ExportFormat::SqlInsert => "sql",
        }
    }
}

/// A result to write in one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub format: ExportFormat,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// The kind of database the rows came from: SQL INSERTs quote names and
    /// write values the way that database reads them.
    pub kind: Option<DbKind>,
    /// SQL INSERT: the table the rows are inserted into, and its schema
    /// (empty for none).
    pub table: String,
    pub schema: String,
    /// CSV: NULL is an empty field, not `\N`.
    pub null_empty: bool,
}

/// One write to a file: what to write, where, and whether an existing file
/// may be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportJob {
    pub request: ExportRequest,
    pub path: String,
    pub replace: bool,
}

/// What the shell reports after a write that succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub path: String,
    pub rows: usize,
    pub bytes: u64,
}

/// A write that did not happen, and why. Nothing was replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportError {
    /// The file is there already and the job did not allow replacing it.
    pub exists: bool,
    /// Names the path and says what went wrong.
    pub message: String,
}
