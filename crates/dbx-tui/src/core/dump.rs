//! Dumping a database, or one schema, to a `.sql` file (slice 20): what the
//! app asks for, the progress dbx reports while it works, and what the shell
//! reports when it has ended.
//!
//! dbx does the work (`database_export::export_database_sql_core`). The
//! adapter turns its progress into `DumpProgress`; the shell checks the path
//! before dbx is called and looks at the file afterwards, so the app can say
//! what is there without doing any I/O itself.

use std::sync::Arc;

use super::DatabasePath;

/// One dump, from the app's request to its end. dbx's export id is made
/// from it (with the process id), so two dumps never share a cancel flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DumpId(pub u64);

/// What goes in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpContent {
    StructureAndData,
    StructureOnly,
    DataOnly,
}

impl DumpContent {
    pub const ALL: [DumpContent; 3] = [DumpContent::StructureAndData, DumpContent::StructureOnly, DumpContent::DataOnly];

    pub fn label(self) -> &'static str {
        match self {
            DumpContent::StructureAndData => "structure and data",
            DumpContent::StructureOnly => "structure only",
            DumpContent::DataOnly => "data only",
        }
    }

    pub fn structure(self) -> bool {
        self != DumpContent::DataOnly
    }

    pub fn data(self) -> bool {
        self != DumpContent::StructureOnly
    }
}

/// A dump to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpRequest {
    pub at: DatabasePath,
    /// The one schema to dump; empty for the whole database (every schema).
    pub schema: String,
    pub content: DumpContent,
    /// `DROP TABLE IF EXISTS` before each table.
    pub drop_tables: bool,
    /// As typed: the shell expands `~/`.
    pub path: String,
    /// dbx refuses to touch a file that is there (`create_new`). False only
    /// after the user said yes to replacing the file, by name.
    pub prevent_overwrite: bool,
}

/// Where dbx is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpStatus {
    /// Listing what there is, before anything is written.
    Preparing,
    Running,
    Writing,
    Done,
    Cancelled,
}

/// One report of dbx's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpProgress {
    pub status: DumpStatus,
    /// What dbx is on, as it names it (`public: film`). Raw: cleaned when drawn.
    pub object: String,
    /// The object's place, from 0, and how many there are (both per schema
    /// when the whole database is dumped: dbx counts each schema's objects).
    pub index: usize,
    pub total: usize,
    pub rows: u64,
    /// Objects that failed and became `-- ERROR` comments in the file.
    pub errors: u64,
    /// The first of them, as dbx said it.
    pub first_error: Option<String>,
}

impl DumpProgress {
    pub fn new(status: DumpStatus) -> Self {
        DumpProgress { status, object: String::new(), index: 0, total: 0, rows: 0, errors: 0, first_error: None }
    }
}

/// Where the adapter sends progress as it comes.
pub type DumpSink = Arc<dyn Fn(DumpProgress) + Send + Sync>;

/// How a dump that dbx did not fail ended: its last progress (`Done` or
/// `Cancelled`), or nothing when dbx returned without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpEnd {
    pub last: Option<DumpProgress>,
}

impl DumpEnd {
    pub fn cancelled(&self) -> bool {
        self.last.as_ref().is_some_and(|p| p.status == DumpStatus::Cancelled)
    }
}

/// What is at the path once a dump has ended, as the shell found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLeft {
    /// Nothing is there.
    Nothing,
    /// The file that was there before is as it was (same file, size and time).
    Unchanged,
    /// A file of this many bytes that dbx wrote.
    Written(u64),
}

/// What the shell reports when a dump has ended without failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dumped {
    pub path: String,
    pub end: DumpEnd,
    pub file: FileLeft,
}

/// A dump that did not run, or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpFailure {
    /// The file is there and the request did not allow replacing it.
    pub exists: bool,
    /// Refused by the shell before dbx was called (no directory, no name).
    pub refused: bool,
    pub message: String,
    pub path: String,
    pub file: FileLeft,
}
