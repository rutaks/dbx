//! The adapter to dbx. In slice 01 it is a stub: dbx dependencies arrive in
//! slice 02. This is the only module that will ever import a dbx crate.

use async_trait::async_trait;

use super::*;

/// Talks to dbx. Returns no connections and refuses everything else until
/// slice 02 wires it up.
#[derive(Debug, Default)]
pub struct DbxCore;

impl DbxCore {
    pub fn new() -> Self {
        Self
    }
}

fn not_wired<T>() -> Result<T, CoreError> {
    Err(CoreError::new(CoreErrorKind::Other, "dbx is not wired up yet"))
}

#[async_trait]
impl Core for DbxCore {
    async fn connections(&self) -> Result<Vec<Connection>, CoreError> {
        Ok(Vec::new())
    }
    async fn save_connection(&self, _draft: &ConnectionDraft) -> Result<Connection, CoreError> {
        not_wired()
    }
    async fn remove_connection(&self, _id: &ConnectionId) -> Result<(), CoreError> {
        not_wired()
    }
    async fn test_connection(&self, _draft: &ConnectionDraft) -> Result<(), CoreError> {
        not_wired()
    }
    async fn databases(&self, _id: &ConnectionId) -> Result<Vec<String>, CoreError> {
        not_wired()
    }
    async fn schemas(&self, _at: &DatabasePath) -> Result<Vec<String>, CoreError> {
        not_wired()
    }
    async fn tables(&self, _at: &SchemaPath) -> Result<Vec<Table>, CoreError> {
        not_wired()
    }
    async fn columns(&self, _at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError> {
        not_wired()
    }
    async fn run(&self, _run: RunId, _request: RunRequest) -> Result<ResultSet, CoreError> {
        not_wired()
    }
    fn cancel(&self, _run: RunId) {}
    async fn close_session(&self, _tab: TabId) -> Result<(), CoreError> {
        Ok(())
    }
    async fn history(&self, _query: HistoryQuery) -> Result<Vec<HistoryEntry>, CoreError> {
        Ok(Vec::new())
    }
}
