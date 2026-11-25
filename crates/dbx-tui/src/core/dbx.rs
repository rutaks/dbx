//! The adapter to dbx: the only module that imports a dbx crate.
//!
//! Startup follows dbx's desktop app (`src-tauri/src/lib.rs:2092-2117`), the
//! head that uses the `PlatformDefault` key policy: `Storage::open_unmigrated`,
//! `PlatformDefault` with key creation allowed, the migration readiness check,
//! `AppState`, then the saved connections loaded into `state.configs`. (dbx-web
//! opens storage the same way but with `ManagedDataDir`, a key file in its
//! data directory.) Opening never creates a key: dbx creates one lazily, on
//! the first write that carries a secret.
//!
//! No secret leaves this module. `Connection` carries no password, and the
//! errors built here name dbx's error codes, never key material.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use dbx_core::connection::{
    connection_configs_pool_equivalent, connection_configs_session_credentials_compatible, AppState,
};
use dbx_core::runtime_config::{is_transient_runtime_config_id, TEST_PROBE_ID_PREFIX};
use dbx_core::models::connection::{ConnectionConfig, DatabaseType};
use dbx_core::persistence::secret_codec::SecretKeyPolicy;
use dbx_core::query::{execute_sql_statement_with_options_typed, QueryExecutionError, QueryExecutionOptions};
use dbx_core::query_cancel::RunningTaskMetadata;
use dbx_core::storage::{MigrationPreflight, Storage};
use tokio::sync::Mutex;

use super::*;

/// Where the store is. Built once, from the environment, by `from_env`; tests
/// build it directly so they never depend on the process environment for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
    /// The directory holding `dbx.db`, shared with every other dbx head.
    pub data_dir: PathBuf,
    /// Create the directory if it is missing. True for the desktop app's
    /// default (a fresh machine has none yet); false for an explicit
    /// `DBX_DATA_DIR`, where a missing directory is more likely a typo.
    pub create_if_missing: bool,
    /// Where dbx will take the store key from, for advice when it fails.
    pub key_source: KeySource,
}

/// Where dbx takes the store key from. `DBX_SECRET_KEY_FILE` and
/// `DBX_SECRET_KEY` win over every key policy (`secret_codec.rs:172-181`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    EnvFile,
    EnvValue,
    /// The compatibility key file if present, else the OS keychain or Secret Service.
    Platform,
}

impl StoreOptions {
    /// `DBX_DATA_DIR` if set, else the desktop app's data directory. The one
    /// place the client reads the environment to find its store. (The key
    /// variables `DBX_SECRET_KEY_FILE` and `DBX_SECRET_KEY` are read by dbx.)
    pub fn from_env() -> Result<Self, CoreError> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// `from_env` with the lookup injected, so tests need not change the
    /// process environment.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, CoreError> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        // dbx itself tests presence, not emptiness, for the key variables.
        let key_source = if var("DBX_SECRET_KEY_FILE").is_some() {
            KeySource::EnvFile
        } else if var("DBX_SECRET_KEY").is_some() {
            KeySource::EnvValue
        } else {
            KeySource::Platform
        };
        if let Some(dir) = set("DBX_DATA_DIR") {
            if !dir.is_absolute() {
                return Err(CoreError::new(
                    CoreErrorKind::Store,
                    format!(
                        "DBX_DATA_DIR is {:?}, which is not an absolute path (a leading ~ is not expanded). \
                         Set it to the full path of a dbx data directory, such as /Users/you/dbx-data.",
                        dir.as_os_str()
                    ),
                ));
            }
            return Ok(StoreOptions { data_dir: dir, create_if_missing: false, key_source });
        }
        let data_dir = desktop_data_dir(set("HOME"), set("XDG_DATA_HOME")).ok_or_else(|| {
            CoreError::new(
                CoreErrorKind::Store,
                "dbx-tui cannot tell where dbx keeps its store: HOME is not set. \
                 Set DBX_DATA_DIR to a dbx data directory and start again.",
            )
        })?;
        Ok(StoreOptions { data_dir, create_if_missing: true, key_source })
    }
}

/// Where the desktop app (Tauri identifier `com.dbx.app`) keeps its data:
/// Tauri's `app_data_dir`.
fn desktop_data_dir(home: Option<PathBuf>, xdg_data_home: Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/com.dbx.app"));
    }
    xdg_data_home.or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join("com.dbx.app"))
}

/// Whether a seeding tool may write to `dir`: never the desktop app's store
/// (under `home`), and outside the system temp directory only when `allow`
/// (`DBX_TUI_ALLOW_SEED=1`) is set. Paths are compared after resolving
/// symlinks, so `/tmp` and `/private/tmp` are one place.
#[cfg(feature = "test-support")]
pub fn check_seed_dir(dir: &Path, home: Option<&Path>, allow: bool) -> Result<(), String> {
    let dir = resolved(dir);
    // The desktop's directory name, wherever HOME points.
    if dir.components().any(|c| c.as_os_str() == "com.dbx.app") {
        return Err(format!("refusing to seed {}: that is the desktop app's store", dir.display()));
    }
    let desktops = [home.map(Path::to_path_buf), Some(PathBuf::from(std::env::var_os("HOME").unwrap_or_default()))];
    for home in desktops.into_iter().flatten().filter(|h| !h.as_os_str().is_empty()) {
        if let Some(desktop) = desktop_data_dir(Some(home), None) {
            if dir.starts_with(resolved(&desktop)) {
                return Err(format!("refusing to seed {}: that is the desktop app's store", dir.display()));
            }
        }
    }
    let temps = [std::env::temp_dir(), PathBuf::from("/tmp")];
    if allow || temps.iter().any(|t| dir.starts_with(resolved(t))) {
        return Ok(());
    }
    Err(format!(
        "refusing to seed {}: it is not under the system temp directory (set DBX_TUI_ALLOW_SEED=1 to allow it)",
        dir.display()
    ))
}

/// `path` with its longest existing ancestor's symlinks resolved.
#[cfg(feature = "test-support")]
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut base = path.to_path_buf();
    loop {
        if let Ok(real) = base.canonicalize() {
            return rest.iter().rev().fold(real, |acc: PathBuf, part| acc.join(part));
        }
        match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Talks to dbx. Starts lazily, on the first call: the first connection list
/// is the startup, so its failure reaches the app as that reply's error. A
/// failed start is not remembered, so the next call tries again.
pub struct DbxCore {
    options: Result<StoreOptions, CoreError>,
    state: Mutex<Option<Arc<AppState>>>,
    /// The state once open, readable without waiting on `state`'s lock:
    /// `cancel` is not async and must never miss because another call holds
    /// the lock for a moment.
    opened: std::sync::OnceLock<Arc<AppState>>,
    policy: SecretKeyPolicy,
    /// Held for the whole of a save or a remove: each reads the saved config,
    /// writes the store and then updates dbx's runtime list, and two of them
    /// interleaved can leave the runtime and the store disagreeing.
    mutations: Mutex<()>,
    /// Cancels for runs dbx had not registered yet. dbx's
    /// `RunningQueries::cancel` forgets a cancel for an id it does not know,
    /// and a run registers only after it has looked up the store and the
    /// connection, so `ctrl+c` right after `f5` would otherwise be lost. A
    /// run checks this, under the same lock, as it registers.
    early_cancels: std::sync::Mutex<std::collections::BTreeSet<RunId>>,
    /// Sessions closed: a run's wait for its session (`wait_for_session`)
    /// must not open one of them again.
    closed_sessions: std::sync::Mutex<std::collections::HashSet<SessionId>>,
    /// Sessions in which a `USE` has been sent (`SwitchedSessions`).
    switched: SwitchedSessions,
}

/// The most early cancels remembered. A cancel for a run that has already
/// finished lands here too and is never claimed; the oldest go first.
const EARLY_CANCELS: usize = 256;

/// Runs a future on a blocking-pool thread. dbx's store calls can block (the
/// macOS keychain read on a first password save waits for as long as its
/// prompt is up), and must not hold a runtime worker while they do.
async fn blocking<T: Send + 'static>(fut: impl std::future::Future<Output = T> + Send + 'static) -> Result<T, CoreError> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handle.block_on(fut))
        .await
        .map_err(|e| CoreError::new(CoreErrorKind::Other, format!("the store call stopped unexpectedly: {e}")))
}

impl DbxCore {
    pub fn new(options: StoreOptions) -> Self {
        DbxCore { options: Ok(options), state: Mutex::new(None), opened: std::sync::OnceLock::new(), policy: SecretKeyPolicy::PlatformDefault, mutations: Mutex::new(()), early_cancels: Default::default(), closed_sessions: Default::default(), switched: Default::default() }
    }

    /// The client's DbxCore: the store named by the environment. If that
    /// cannot be worked out, every call fails with the reason.
    pub fn from_env() -> Self {
        DbxCore { options: StoreOptions::from_env(), state: Mutex::new(None), opened: std::sync::OnceLock::new(), policy: SecretKeyPolicy::PlatformDefault, mutations: Mutex::new(()), early_cancels: Default::default(), closed_sessions: Default::default(), switched: Default::default() }
    }

    /// Keys only from the environment or the data directory, never the OS
    /// keychain (dbx-web's `ManagedDataDir` policy). Tests only: it is how a
    /// test reaches "no key available" without asking the keychain.
    #[cfg(feature = "test-support")]
    pub fn with_managed_keys(options: StoreOptions) -> Self {
        DbxCore { policy: SecretKeyPolicy::ManagedDataDir, ..Self::new(options) }
    }

    /// The directory the store lives in, when the environment named one the
    /// client can work with. Preferences are kept beside it.
    pub fn store_dir(&self) -> Option<&Path> {
        self.options.as_ref().ok().map(|o| o.data_dir.as_path())
    }

    fn data_dir(&self) -> &Path {
        self.options.as_ref().map(|o| o.data_dir.as_path()).unwrap_or(Path::new(""))
    }

    async fn state(&self) -> Result<Arc<AppState>, CoreError> {
        let mut slot = self.state.lock().await;
        if let Some(state) = slot.as_ref() {
            return Ok(state.clone());
        }
        let options = self.options.as_ref().map_err(Clone::clone)?;
        // Opening blocks in places: the macOS keychain read waits for as long
        // as its access prompt is up, and dbx sleeps between WAL retries. Run
        // it on a blocking-pool thread (16 MiB stack, like every runtime
        // thread) so timers, the event loop and signals never stall.
        let (dir, create, source, policy) =
            (options.data_dir.clone(), options.create_if_missing, options.key_source, self.policy);
        let handle = tokio::runtime::Handle::current();
        let state = tokio::task::spawn_blocking(move || handle.block_on(open(&dir, create, source, policy))).await.map_err(|e| {
            CoreError::new(CoreErrorKind::Other, format!("opening the store stopped unexpectedly: {e}"))
        })??;
        *slot = Some(state.clone());
        let _ = self.opened.set(state.clone());
        Ok(state)
    }

    /// Saves a connection config exactly as given, as another dbx head would
    /// have written it (one with SSH layers or options this client does not
    /// edit). Tests only (feature `test-support`).
    #[cfg(feature = "test-support")]
    pub async fn seed_config(&self, config: serde_json::Value) -> Result<ConnectionId, CoreError> {
        let state = self.state().await?;
        let config: ConnectionConfig = serde_json::from_value(config)
            .map_err(|e| CoreError::new(CoreErrorKind::Other, format!("dbx rejected the connection: {e}")))?;
        state.storage.save_connections(std::slice::from_ref(&config)).await.map_err(|raw| self.store_failure("seed a connection", &raw))?;
        let id = ConnectionId(config.id.clone());
        state.configs.write().await.insert(config.id.clone(), config);
        Ok(id)
    }

    /// dbx's runtime copy of a connection as JSON, password included. Tests
    /// only: it is how a test sees which fields an edit kept.
    #[cfg(feature = "test-support")]
    pub async fn runtime_config(&self, id: &ConnectionId) -> Option<serde_json::Value> {
        let state = self.state().await.ok()?;
        let configs = state.configs.read().await;
        configs.get(&id.0).and_then(|c| serde_json::to_value(c).ok())
    }

    /// Holds the lock `state()` takes, as another Core call does for a moment
    /// while it looks the state up. Tests only: a cancel must not depend on
    /// that lock being free.
    #[cfg(feature = "test-support")]
    pub async fn hold_state_lock(&self) -> impl Drop + '_ {
        self.state.lock().await
    }

    /// Every id in dbx's runtime connection list, temporary ones included.
    /// Tests only: `connections()` hides the temporary ones.
    #[cfg(feature = "test-support")]
    pub async fn runtime_config_ids(&self) -> Vec<String> {
        let Ok(state) = self.state().await else { return Vec::new() };
        let mut ids: Vec<String> = state.configs.read().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Runs `select 1` on a saved connection through dbx's pool, until `run`
    /// exists (slice 05). Tests only: it proves a stored password decrypts
    /// and that a connection's current settings are the ones in use.
    #[cfg(feature = "test-support")]
    pub async fn probe_select_one(&self, id: &ConnectionId) -> Result<(), CoreError> {
        let state = self.state().await?;
        let database = state.configs.read().await.get(&id.0).and_then(|c| c.database.clone()).unwrap_or_default();
        dbx_core::query::execute_sql_statement_with_options_typed(
            &state,
            &id.0,
            &database,
            "select 1",
            None,
            None,
            Default::default(),
        )
        .await
        .map(|_| ())
        .map_err(|e| CoreError::new(CoreErrorKind::Connect, e.to_string()))
    }

    /// Waits until a tab's session answers again, which is when the statement
    /// it was running has ended on the server. Returns whether it answered.
    ///
    /// After a cancel (or the connection's timeout, which cancels) dbx
    /// returns as soon as it has sent Postgres's cancel request, or given up
    /// sending it: a cancel that never reached the server still comes back as
    /// cancelled, within a fraction of a second, while the statement runs on.
    /// dbx keeps the session's connection on a cancel, and a statement sent on
    /// it queues behind the one still running, so a `select 1` there answers
    /// only once that has ended. Normally this costs one round trip.
    ///
    /// It never opens a session: only a session dbx still holds a pool for,
    /// and that has not been closed, is asked. A close that lands while it
    /// waits is made good afterwards (closed again), in case dbx reopened it.
    async fn wait_for_session(&self, state: &AppState, connection: &str, database: &str, session: SessionId) -> bool {
        let closed = || self.closed_sessions.lock().unwrap_or_else(|p| p.into_inner()).contains(&session);
        let id = session_id(session);
        if closed() || !session_pool_exists(state, connection, &id).await {
            return false;
        }
        let options = QueryExecutionOptions {
            max_rows: Some(1),
            timeout_secs: Some(0),
            client_session_id: Some(id.clone()),
            ..Default::default()
        };
        let answered = execute_sql_statement_with_options_typed(state, connection, database, "select 1", None, None, options).await.is_ok();
        if closed() {
            let _ = state.close_client_session_pool(connection, Some(database), &id).await;
            return false;
        }
        answered
    }

    /// The database a path names. An empty name (a connection with no database
    /// level) is no name: the connection's own database stands in, as it does
    /// for a run, and dbx is never handed `""` as if it were a database.
    async fn database_name(&self, connection: &ConnectionId, database: &str) -> String {
        if !database.is_empty() {
            return database.to_string();
        }
        let Ok(state) = self.state().await else { return String::new() };
        let own = state.configs.read().await.get(&connection.0).and_then(|c| c.database.clone());
        own.unwrap_or_default()
    }

    /// Runs one of dbx's metadata calls for connection `id`.
    ///
    /// dbx's metadata functions take no cancel token. The work runs on a
    /// blocking-pool thread (16 MiB stack, and a slow driver cannot hold a
    /// runtime worker), inside a `select!` against a token held by this future:
    /// when the caller stops waiting (the task is aborted, the future dropped)
    /// the token fires, dbx's future is dropped, and the thread is released.
    /// Without that, an abandoned listing of an unreachable server would keep a
    /// thread until the driver's own connect timeout.
    async fn metadata<T, Fut>(
        &self,
        id: &ConnectionId,
        work: impl FnOnce(Arc<AppState>) -> Fut + Send + 'static,
    ) -> Result<T, CoreError>
    where
        T: Send + 'static,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        let state = self.state().await?;
        // Known secrets, to blank out of whatever dbx says about a failure.
        let secret = state.configs.read().await.get(&id.0).map(|c| c.password.clone()).unwrap_or_default();
        let (token, cancelled) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::runtime::Handle::current();
        let task = tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                tokio::select! {
                    biased;
                    // The sender is only ever dropped, never sent on.
                    _ = cancelled => Err(CoreError::new(CoreErrorKind::Cancelled, "cancelled")),
                    result = work(state) => result.map_err(|raw| metadata_error(&raw, &secret)),
                }
            })
        });
        let result = task
            .await
            .map_err(|e| CoreError::new(CoreErrorKind::Other, format!("the schema call stopped unexpectedly: {e}")))?;
        drop(token);
        result
    }

    /// dbx answers a schema call on most other kinds with an empty list, which
    /// the tree would show as "(no schemas)": say plainly that it is not
    /// supported yet, without calling dbx.
    async fn ensure_browsable(&self, id: &ConnectionId) -> Result<(), CoreError> {
        let state = self.state().await?;
        let db_type = state.configs.read().await.get(&id.0).map(|c| c.db_type);
        match db_type {
            Some(db_type) if !capabilities_of(db_type).browse => Err(CoreError::new(
                CoreErrorKind::Other,
                format!("Browsing {} is not supported yet.", kind_of(db_type).display_name()),
            )),
            // A type that can be browsed, or no such connection (dbx says so itself).
            _ => Ok(()),
        }
    }

    /// The connection `id` as saved in the store right now. Loading decrypts
    /// every saved secret: they stay in the returned config, which the gate
    /// only reads flags from, and a failure is the store's error without
    /// them.
    async fn saved_config(&self, state: &AppState, id: &str) -> Result<Option<ConnectionConfig>, CoreError> {
        let saved = state
            .storage
            .load_connections()
            .await
            .map_err(|raw| self.store_failure("re-read the saved connections before a write", &raw))?;
        Ok(saved.into_iter().find(|c| c.id == id))
    }

    fn store_failure(&self, doing: &str, raw: &str) -> CoreError {
        let source = self.options.as_ref().map(|o| o.key_source).unwrap_or(KeySource::Platform);
        classify(raw, source).unwrap_or_else(|| store_error(self.data_dir(), doing, raw))
    }
}

/// Opens the store and builds dbx's runtime state, as the desktop does.
///
/// Opening a store is not read-only. dbx's `Storage::open_unmigrated` creates
/// `dbx.db` if it is missing, tightens the file's permissions, switches it to
/// WAL journal mode, and runs its schema DDL (`init_schema`: `CREATE ... IF NOT
/// EXISTS` and added columns), exactly as every dbx head does on start. On a
/// store already at this dbx version those statements change nothing. It
/// never migrates legacy data: a store that needs that is refused.
async fn open(dir: &Path, create_if_missing: bool, source: KeySource, policy: SecretKeyPolicy) -> Result<Arc<AppState>, CoreError> {
    if !create_if_missing && !dir.is_dir() {
        return Err(CoreError::new(
            CoreErrorKind::Store,
            format!(
                "DBX_DATA_DIR names {}, which does not exist or is not a directory. Check the path, or create \
                 the directory for a new store, then retry.",
                dir.display()
            ),
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| store_error(dir, "create the data directory", &e.to_string()))?;
    let storage = Storage::open_unmigrated(&dir.join("dbx.db"))
        .await
        .map_err(|raw| store_error(dir, "open the store", &raw))?
        .with_secret_key_policy(policy)
        .with_secret_key_creation(true);
    let preflight = storage.inspect_data_migration().await.map_err(|raw| store_error(dir, "inspect the store", &raw))?;
    if !preflight.is_ready() {
        return Err(not_ready(dir, &preflight, source));
    }
    let configs = storage
        .load_connections()
        .await
        .map_err(|raw| classify(&raw, source).unwrap_or_else(|| store_error(dir, "read saved connections", &raw)))?;
    let state = Arc::new(AppState::new_with_plugin_and_agent_dir_and_app_version(
        storage,
        dir.join("plugins"),
        dir.join("agents"),
        env!("CARGO_PKG_VERSION"),
    ));
    *state.configs.write().await = configs.into_iter().map(|c| (c.id.clone(), c)).collect::<HashMap<_, _>>();
    Ok(state)
}

// ---- errors ------------------------------------------------------------------

/// dbx's codes for a key it cannot find, read or use
/// (`dbx-core/src/persistence/secret_codec.rs` and `storage.rs`).
const KEY_CODES: &[&str] = &[
    "SECRET_KEY_INVALID",
    "SECRET_KEY_MISMATCH",
    "SECRET_KEY_UNAVAILABLE",
    "KEY_FILE_UNAVAILABLE",
    "KEYRING_ACCESS_FAILED",
    "KEYRING_WRITE_FAILED",
    "MISSING_EXTERNAL_KEY",
    "MISSING_MANAGED_KEY",
    "ENCRYPTED_DATA_KEY_MISSING",
    "KEY_PROVIDER_UNAVAILABLE",
];

/// What to do about a key failure, by dbx's code and by where the key came
/// from: a wrong key is not a locked keychain, and a key file is not one.
fn advice(code: &str, source: KeySource) -> String {
    let wrong = matches!(code, "SECRET_KEY_MISMATCH" | "ENCRYPTED_DATA_KEY_MISSING");
    match source {
        KeySource::EnvFile => match code {
            "KEY_FILE_UNAVAILABLE" => "dbx-tui cannot read the file DBX_SECRET_KEY_FILE names. Check the path and its \
                permissions, then start again."
                .into(),
            "SECRET_KEY_INVALID" => "The file DBX_SECRET_KEY_FILE names does not hold a valid key (64 hex characters). \
                Fix the file, then start again."
                .into(),
            _ => "DBX_SECRET_KEY_FILE does not name the key this store was written with. Point it at the right key \
                file, or unset it to use the key dbx keeps for you, then start again."
                .into(),
        },
        KeySource::EnvValue => match code {
            "SECRET_KEY_INVALID" => "DBX_SECRET_KEY is not a valid key (64 hex characters). Fix it, then start again."
                .into(),
            _ => "DBX_SECRET_KEY is not the key this store was written with. Set it to the right key, or unset it \
                to use the key dbx keeps for you, then start again."
                .into(),
        },
        KeySource::Platform if wrong || code == "SECRET_KEY_INVALID" => "The key dbx keeps for this user does not \
            open this store. Set DBX_SECRET_KEY_FILE to the key file this store was written with, then start again."
            .into(),
        KeySource::Platform if code == "KEY_FILE_UNAVAILABLE" => "dbx's key file in your user directory cannot be \
            read. Check its permissions, or set DBX_SECRET_KEY_FILE, then retry."
            .into(),
        KeySource::Platform if cfg!(target_os = "macos") => "Unlock your login keychain (and allow dbx-tui to use \
            the dbx key if asked), then retry. Or set DBX_SECRET_KEY_FILE to the store's key file and start again."
            .into(),
        KeySource::Platform => "Make sure the Secret Service (such as GNOME Keyring) is running and unlocked, then \
            retry. Or set DBX_SECRET_KEY_FILE to the store's key file and start again."
            .into(),
    }
}

/// The key code a raw dbx error starts with, if it is a key error.
fn key_code(raw: &str) -> Option<&'static str> {
    let raw = raw.trim_start();
    KEY_CODES.iter().copied().find(|code| raw.starts_with(code))
}

/// A raw dbx error from a store read, as a key error when it is one.
fn classify(raw: &str, source: KeySource) -> Option<CoreError> {
    if let Some(code) = key_code(raw) {
        return Some(key_error(code, None, source));
    }
    // A saved secret that will not decrypt: the key is not the one it was
    // written with (`SecretCodec::decrypt`).
    if raw.contains("secret decryption failed") {
        return Some(key_error("SECRET_KEY_MISMATCH", None, source));
    }
    None
}

fn key_error(code: &str, dbx_says: Option<&str>, source: KeySource) -> CoreError {
    let what = dbx_says.unwrap_or("dbx-tui cannot read the key that encrypts this store's saved passwords");
    CoreError::new(CoreErrorKind::Key, format!("{} ({code}). {}", what.trim_end_matches('.'), advice(code, source)))
}

fn store_error(dir: &Path, doing: &str, raw: &str) -> CoreError {
    // dbx's text for a store failure is SQLite's or the file system's; cap it
    // so a pathological message cannot fill the screen.
    let raw: String = display_safe(raw.trim().trim_end_matches('.')).chars().take(300).collect();
    CoreError::new(
        CoreErrorKind::Store,
        format!(
            "dbx-tui could not {doing} at {}: {raw}. Check that DBX_DATA_DIR names a dbx data directory you can \
             read and write, then retry.",
            dir.display()
        ),
    )
}

/// The store opened but is not ready for a head to use: either its key is
/// missing or wrong, or it holds data only the desktop app's upgrade wizard
/// may migrate. Never migrate it here.
fn not_ready(dir: &Path, p: &MigrationPreflight, source: KeySource) -> CoreError {
    let code = p.error_code.as_deref().unwrap_or("");
    if p.key_recovery_required || (key_code(code).is_some() && !p.needs_migration) {
        let code = key_code(code).unwrap_or("SECRET_KEY_UNAVAILABLE");
        return key_error(code, p.error_message.as_deref(), source);
    }
    let reason = if code.is_empty() { format!("{:?}", p.state).to_uppercase() } else { code.to_string() };
    CoreError::new(
        CoreErrorKind::Store,
        format!(
            "The store at {} needs the dbx desktop app's data upgrade before another program can use it ({reason}). \
             Open the dbx desktop app once to finish it, then retry.",
            dir.display()
        ),
    )
}

// ---- connections ---------------------------------------------------------------

/// The connection types this client browses and edits in its form so far.
/// dbx describes ninety; the adapter behind `Core::schemas`, completion and the
/// form is built and tested for these only. One line to grow.
const BROWSED_HERE: [DatabaseType; 1] = [DatabaseType::Postgres];

/// What dbx's descriptor says that its Rust manifest does not carry: the
/// tree-shape traits and `sqlExplain`, read from the manifest dbx bundles
/// (`crates/dbx-core/assets/database-drivers.manifest.json`, the same
/// `plugins/connection-types/*.yaml` data). `dbx_core::database_manifest` is
/// preferred for everything it does carry.
#[derive(Debug, Default)]
struct Descriptor {
    schema_aware: bool,
    tree_schema: bool,
    single_database: bool,
    sql_explain: bool,
}

fn descriptor(db_type: DatabaseType) -> Option<Descriptor> {
    static ALL: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let all = ALL.get_or_init(|| {
        let json = include_str!("../../../dbx-core/assets/database-drivers.manifest.json");
        serde_json::from_str(json).unwrap_or_default()
    });
    let entry = all.get("drivers")?.as_array()?.iter().find(|d| d["dbType"] == db_type.as_str())?;
    let flag = |group: &str, name: &str| entry[group][name].as_bool().unwrap_or(false);
    Some(Descriptor {
        schema_aware: flag("traits", "schemaAware"),
        tree_schema: flag("traits", "treeSchema"),
        single_database: flag("traits", "singleDatabase"),
        sql_explain: flag("capabilities", "sqlExplain"),
    })
}

/// What dbx says a connection type can do and how its tree is shaped.
fn capabilities_of(db_type: DatabaseType) -> Capabilities {
    use dbx_core::database_manifest::{self as manifest, DatabaseCapability as Flag};
    let Some(entry) = manifest::entry(&db_type) else { return Capabilities::default() };
    let extra = descriptor(db_type).unwrap_or_default();
    let here = BROWSED_HERE.contains(&db_type);
    Capabilities {
        databases: !(entry.local_file || extra.single_database),
        schemas: extra.schema_aware || extra.tree_schema,
        query: entry.capability(Flag::QueryExecution),
        browse: entry.capability(Flag::MetadataBrowse) && here,
        edit_data: entry.capability(Flag::TableDataEdit),
        explain: extra.sql_explain,
        edit_in_form: here,
        // A file database reports port 0: it has none.
        default_port: manifest::default_port(&db_type).filter(|p| *p != 0),
        dialect: match manifest::dialect_name(&db_type) {
            Some("PostgreSQL") => Dialect::PostgreSql,
            Some("MySQL" | "Doris" | "StarRocks") => Dialect::MySql,
            Some("SQLite") => Dialect::Sqlite,
            _ => Dialect::Generic,
        },
        single_connection: entry.single_connection_pool,
        dump_loses: matches!(db_type, DatabaseType::Postgres | DatabaseType::Gaussdb | DatabaseType::OpenGauss),
    }
}

fn kind_of(db_type: DatabaseType) -> DbKind {
    match db_type {
        DatabaseType::Postgres => DbKind::Postgres,
        DatabaseType::Mysql => DbKind::MySql,
        DatabaseType::Sqlite => DbKind::Sqlite,
        other => DbKind::Other(other.as_str().to_string()),
    }
}

fn db_type_name(kind: &DbKind) -> &str {
    match kind {
        DbKind::Postgres => DatabaseType::Postgres.as_str(),
        DbKind::MySql => DatabaseType::Mysql.as_str(),
        DbKind::Sqlite => DatabaseType::Sqlite.as_str(),
        DbKind::Other(name) => name,
    }
}

/// Our view of a saved connection. Copies only what the sidebar and the form
/// show: never the password, URL parameters, connection string or tunnel
/// secrets that `ConnectionConfig` carries.
fn to_connection(c: &ConnectionConfig) -> Connection {
    Connection {
        id: ConnectionId(c.id.clone()),
        name: display_safe(&c.name),
        kind: kind_of(c.db_type),
        capabilities: capabilities_of(c.db_type),
        host: display_safe(&c.host),
        port: (c.port != 0).then_some(c.port),
        database: c.database.as_deref().filter(|d| !d.is_empty()).map(display_safe),
        username: display_safe(&c.username),
        tls: c.ssl,
        production: c.is_production,
        production_databases: c.production_databases.iter().map(|d| display_safe(d)).collect(),
        query_timeout: query_timeout(c),
    }
}

/// The config to store for `draft`: the saved one with only the form's fields
/// changed, so whatever the form does not show (SSH layers, URL parameters,
/// notes, colours, timeouts) survives an edit made here. A new connection is
/// built from dbx's defaults. A blank password keeps the stored one.
fn config_from_draft(draft: &ConnectionDraft, existing: Option<ConnectionConfig>) -> Result<ConnectionConfig, CoreError> {
    let mut config = match existing {
        Some(config) => config,
        None => {
            let id = draft.id.as_ref().map_or_else(|| uuid::Uuid::new_v4().to_string(), |id| id.0.clone());
            let value = serde_json::json!({
                "id": id,
                "name": "",
                "db_type": db_type_name(&draft.kind),
                "host": "",
                "port": 0,
                "username": "",
                "password": "",
                "database": null,
                "ssl": false,
                "driver_profile": null,
            });
            // The error text is serde's and names the field, never its value.
            serde_json::from_value(value)
                .map_err(|e| CoreError::new(CoreErrorKind::Other, format!("dbx rejected the connection: {e}")))?
        }
    };
    config.name = draft.name.trim().to_string();
    config.host = draft.host.trim().to_string();
    config.port = draft.port.unwrap_or(capabilities_of(config.db_type).default_port.unwrap_or(0));
    config.username = draft.username.trim().to_string();
    config.database = draft.database.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
    config.ssl = draft.tls;
    config.is_production = draft.production;
    if let Some(password) = draft.password.as_ref().filter(|p| !p.is_empty()) {
        config.password = password.clone();
    }
    Ok(config.canonicalized())
}

/// dbx's words for a failed connection, with every secret we know of blanked
/// out and the length capped. Drivers can echo more than they should.
fn scrub(message: &str, secrets: &[&str]) -> String {
    // The secrets go first, as they were typed: cleaning the text first would
    // change a password that holds a control character, and it would no longer
    // be found.
    let mut out = message.trim().to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "••••");
    }
    let mut out = display_safe(&out);
    if out.chars().count() > 600 {
        out = out.chars().take(599).collect::<String>() + "…";
    }
    out
}

/// A temporary runtime entry for testing a connection that is not saved. Its
/// cleanup runs on every way out: `cleanup().await` on the normal paths, and
/// from `Drop` (as a spawned task) when the future is dropped mid-test.
struct Probe {
    state: Arc<AppState>,
    id: String,
    config: ConnectionConfig,
    armed: bool,
}

impl Probe {
    async fn release(state: &AppState, id: &str, config: &ConnectionConfig) {
        state.remove_connection_pools(id).await;
        state.reset_connection_transport_for_config(id, config).await;
        state.configs.write().await.remove(id);
        state.session_credentials.clear_connection(id);
    }

    async fn cleanup(&mut self) {
        Self::release(&self.state, &self.id, &self.config).await;
        self.armed = false;
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (state, id, config) = (self.state.clone(), self.id.clone(), self.config.clone());
            runtime.spawn(async move { Probe::release(&state, &id, &config).await });
        }
    }
}

// ---- schema ------------------------------------------------------------------

/// Schemas that belong to the server, not to the user's data.
fn is_system_schema(name: &str) -> bool {
    name == "information_schema" || name == "pg_catalog" || name.starts_with("pg_toast") || name.starts_with("pg_temp_")
}

/// dbx's `table_type` for a relation. Postgres says `BASE TABLE`, `VIEW`,
/// `MATERIALIZED_VIEW` and `FOREIGN TABLE`; other drivers say `TABLE`.
fn table_kind(table_type: &str) -> TableKind {
    match table_type.to_ascii_uppercase().replace(' ', "_").as_str() {
        "VIEW" => TableKind::View,
        "MATERIALIZED_VIEW" => TableKind::MaterializedView,
        "FOREIGN_TABLE" => TableKind::Foreign,
        _ => TableKind::Table,
    }
}

/// A failed metadata call as the tree shows it: dbx's own words, cleaned.
fn metadata_error(raw: &str, secret: &str) -> CoreError {
    let kind = if dbx_core::query::is_connection_error(raw) { CoreErrorKind::Connect } else { CoreErrorKind::Other };
    CoreError::new(kind, scrub(raw, &[secret]))
}

// ---- runs ------------------------------------------------------------------------

/// The query timeout a run asks dbx for, in seconds, or `None` for no limit.
/// The connection's own `query_timeout_secs` when it sets one; dbx fills a
/// connection that never set it with its default (60), which counts as unset.
///
/// The value must be passed in `QueryExecutionOptions.timeout_secs`: on the
/// single-statement path dbx's `operation_budget_for_pool_key` overwrites the
/// connection's query timeout with the per-call one, and an absent per-call
/// timeout means dbx's built-in 30 s (`QUERY_TIMEOUT`), not the connection's.
fn query_timeout(config: &ConnectionConfig) -> Option<u64> {
    let secs = config.effective_query_timeout_secs();
    (secs != 0 && config.query_timeout_secs != dbx_core::models::connection::default_query_timeout_secs()).then_some(secs)
}

/// dbx's session id for one session of a query tab. dbx treats the text
/// before a `:` as an owner and some suffixes (`:count`, `:export`) as kinds
/// of work, so it has neither.
fn session_id(session: SessionId) -> String {
    format!("dbx-tui-tab-{}-{}", session.tab.0, session.serial)
}

/// The id a run is registered under in dbx's running-query list.
fn run_key(run: RunId) -> String {
    format!("dbx-tui-run-{}", run.0)
}

/// Whether dbx holds a pool for client session `id` of `connection`: its
/// session pools are keyed `<connection>:…:session:<id>`.
async fn session_pool_exists(state: &AppState, connection: &str, id: &str) -> bool {
    let (prefix, suffix) = (format!("{connection}:"), format!(":session:{}", id.replace(':', "_")));
    state.with_connection_pools(|pools| pools.keys().any(|k| k.starts_with(&prefix) && k.ends_with(&suffix))).await
}

/// The connection's query timeout passed and the run was cancelled for it.
/// `stopped` says whether the session answered again afterwards, which is
/// when the statement has ended.
fn timeout_error(secs: u64, stopped: bool, statement: &str) -> CoreError {
    let how = if stopped {
        "and was stopped"
    } else {
        "and was cancelled; whether the server has stopped it is not known"
    };
    let mut e = CoreError::new(
        CoreErrorKind::Timeout,
        format!(
            "The statement ran longer than this connection's query timeout of {secs} s {how}. \
             The limit is the connection's \"Query timeout\" setting, set in the dbx desktop app."
        ),
    );
    e.statement = Some(statement.to_string());
    e
}

/// The statement a run executes: the selection if there is one, otherwise
/// the statement under the cursor as dbx splits statements for `db_type`
/// (`find_statement_at_cursor_for_database`, which counts the cursor in
/// UTF-16 units).
fn statement_to_run(request: &RunRequest, db_type: DatabaseType) -> String {
    if let Some((a, b)) = request.selection.filter(|(a, b)| a != b) {
        return text_between(&request.buffer, a, b);
    }
    statement_at(&request.buffer, request.cursor, db_type)
}

/// The statement dbx's splitter picks for the caret (`find_statement_at_cursor_
/// for_database`, which counts the cursor in UTF-16 units). The one place a
/// statement is chosen: a run sends it, the editor outlines it.
fn statement_at(buffer: &str, cursor: Cursor, db_type: DatabaseType) -> String {
    dbx_core::sql::find_statement_at_cursor_for_database(buffer, utf16_offset(buffer, cursor), db_type)
}

/// dbx's result as ours. Each column's kind comes from the database's type
/// name (`column_types`); a column dbx gives no type name for is `Other`. Every cell is NULL or text:
/// numbers keep the digits dbx gave, never passing through a float.
fn result_set(result: dbx_core::db::QueryResult, ran: String, database_type: DatabaseType) -> ResultSet {
    let columns = result
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            // No type name, no kind: the cells' JSON types change with the
            // path dbx's driver took, so they cannot stand in for one.
            let db_type = result.column_types.get(i).cloned().unwrap_or_default();
            let kind = if db_type.is_empty() { CellKind::Other } else { kind_of_type(&db_type) };
            Column { name: name.clone(), db_type, kind }
        })
        .collect();
    let rows = result.rows.into_iter().map(|row| row.into_iter().map(cell).collect()).collect();
    let messages = result.messages.into_iter().map(|m| format!("{}: {}", m.severity, m.message)).collect();
    ResultSet {
        columns,
        rows,
        truncated: result.truncated,
        elapsed: std::time::Duration::from_millis(result.execution_time_ms.min(u64::MAX as u128) as u64),
        messages,
        affected: result.affected_rows,
        rerun: rerun_of(&ran, database_type),
        ran,
    }
}

/// What is known about running `sql` again, which "load more" needs because it
/// repeats the statement. Three steps:
///
/// 1. The statement must start with `SELECT`, `WITH`, `VALUES` or `TABLE`
///    (so `SHOW` and `EXPLAIN`, which dbx also calls read-only, are left out).
/// 2. dbx's `classify_sql_risk_for_database` must say read-only. It is a
///    denylist: it sees through `WITH ... DELETE ... RETURNING`, `SELECT
///    INTO`, locking clauses and several statements in one, but it calls
///    `select f()`, `select * from f()`, `select pg_notify(..)` and
///    `select set_config(.., false)` read-only, because it cannot know what a
///    function does. Failing it is `Write`.
/// 3. dbx's `prove_read_only_for_database` (an allowlist of pure functions)
///    decides between `Proven` and `ReadOnlyUnproven`.
fn rerun_of(sql: &str, database_type: DatabaseType) -> Rerun {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else if let Some(after) = rest.strip_prefix('(') {
            rest = after.trim_start();
        } else {
            break;
        }
    }
    let word: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    if !matches!(word.as_str(), "select" | "with" | "values" | "table") {
        return Rerun::Write;
    }
    // dbx calls `TABLE name` a write (it fails closed on what it cannot
    // read); it is `SELECT * FROM name`, and is judged as that.
    let judged = if word == "table" { format!("select * from {}", &rest[word.len()..]) } else { sql.to_string() };
    if !matches!(dbx_core::sql_risk::classify_sql_risk_for_database(&judged, database_type), Ok(dbx_core::sql_risk::SqlRisk::ReadOnly)) {
        return Rerun::Write;
    }
    match dbx_core::sql_risk::prove_read_only_for_database(&judged, database_type) {
        dbx_core::sql_risk::ReadProof::ProvenReadOnly => Rerun::Proven,
        dbx_core::sql_risk::ReadProof::Unproven => Rerun::ReadOnlyUnproven,
    }
}

/// Why a statement must wait for a yes, before the names of the connection
/// and the confirmation are added (`Assessment`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Verdict {
    production: bool,
    reasons: Vec<Reason>,
    named: Vec<String>,
    target_unknown: bool,
}

/// The gate (slice 17): whether `sql`, about to run on `database` of the
/// connection `config`, needs the user's yes, and why. `None` runs it.
///
/// 1. Anything but a read (a parse failure counts as a write) that dbx's
///    `targets_production_database` says reaches production. No setting
///    turns this off.
/// 2. A read dbx cannot prove read-only (`prove_read_only_for_database`:
///    it calls functions dbx cannot vouch for) on production. Stricter than
///    the desktop app, which lets every read through.
/// 3. What dbx's `is_dangerous_sql_for_database` flags, when the request
///    has not turned the check off.
///
/// A write on a `read_only` connection is let through to dbx, which refuses
/// it with its own error: asking first would only offer a yes that fails.
fn assess(config: &ConnectionConfig, database: &str, sql: &str, confirm_dangerous: bool, switched: bool) -> Option<Verdict> {
    use dbx_core::production_safety::{is_production_database, targets_production_database};
    use dbx_core::sql_risk::{classify_sql_risk_for_database, is_dangerous_sql_for_database, prove_read_only_for_database, ReadProof, SqlRisk};
    let db_type = config.db_type;
    // `EXPLAIN ANALYZE` executes the statement it explains: that statement is
    // judged too.
    let inner = explain_analyze_inner(sql);
    if config.read_only && [Some(sql), inner.as_deref()].into_iter().flatten().any(|s| dbx_core::query_execution_sql::is_write_sql_for_database(s, db_type)) {
        return None;
    }
    let first_of = |text: &str| match classify_sql_risk_for_database(text, db_type) {
        Err(_) => Some(Reason::Unparsed),
        Ok(SqlRisk::Write) => Some(Reason::Writes),
        Ok(SqlRisk::Ddl) => Some(Reason::ChangesSchema),
        Ok(SqlRisk::Transaction) => Some(Reason::TransactionControl),
        Ok(SqlRisk::ReadOnly) => match prove_read_only_for_database(text, db_type) {
            ReadProof::ProvenReadOnly => None,
            ReadProof::Unproven => Some(Reason::Unproven),
        },
    };
    let mut first = first_of(sql);
    let mut lead = Vec::new();
    if let Some(inner) = &inner {
        let inner_first = first_of(inner);
        if matches!(first, None | Some(Reason::Unproven)) {
            first = inner_first;
        }
        if inner_first.is_some() {
            lead.push(Reason::ExplainRuns);
        }
    }
    // A `USE` is named for what it does, not for "it writes data".
    let switches = switches_database(sql);
    if switches && only_switches(sql) {
        first = Some(Reason::SwitchesDatabase);
    }
    let dangerous = is_dangerous_sql_for_database(sql, db_type) || inner.as_deref().is_some_and(|i| is_dangerous_sql_for_database(i, db_type));
    // A session that has switched database may be anywhere the connection
    // lists as production: judged against the tab's database, a write would
    // pass. Whatever is not proven read-only is aimed at production.
    let listed = config.is_production || !config.production_databases.is_empty();
    let by_sql = first.is_some() && targets_production_database(config, database, sql);
    let by_session = first.is_some() && !by_sql && switched && listed;
    let production = by_sql || by_session;
    let mut reasons = lead;
    if production {
        reasons.extend(first);
        if switches && first != Some(Reason::SwitchesDatabase) {
            reasons.push(Reason::SwitchesDatabase);
        }
        if dangerous {
            reasons.push(Reason::Dangerous);
        }
        if by_session {
            reasons.push(Reason::SessionSwitched);
        }
    } else if dangerous && confirm_dangerous {
        if first == Some(Reason::Unparsed) {
            reasons.push(Reason::Unparsed);
        }
        reasons.push(Reason::Dangerous);
    } else {
        return None;
    }
    // The target was found through the SQL: say which marked databases it
    // names, asking dbx one marked name at a time. A name no database has
    // tells whether dbx could not work out the target at all.
    let (mut named, mut target_unknown) = (Vec::new(), false);
    if by_sql && !is_production_database(config, database) {
        let only = |name: &str| {
            let mut one = config.clone();
            one.is_production = false;
            one.production_databases = vec![name.to_string()];
            targets_production_database(&one, database, sql)
        };
        target_unknown = only("\u{1}dbx-tui: no such database");
        if !target_unknown {
            named = config.production_databases.iter().filter(|d| only(d)).map(|d| display_safe(d)).collect();
        }
    }
    Some(Verdict { production, reasons, named, target_unknown })
}

/// The gate on a run: `Err` with what the user is asked, unless `statement`
/// needs no yes, or the request carries a yes for exactly this statement,
/// connection, `database` and session. A yes for anything else asks again.
fn gate(config: &ConnectionConfig, request: &RunRequest, statement: &str, database: &str, switched: bool) -> Result<(), Box<Assessment>> {
    let this = Confirmation {
        statement: statement.to_string(),
        connection: request.connection.clone(),
        database: database.to_string(),
        session: request.session,
    };
    let verdict = assess(config, database, statement, request.confirm_dangerous, switched);
    let mut verdict = match (&request.confirmation, verdict) {
        (Some(yes), _) if *yes == this => return Ok(()),
        (None, None) => return Ok(()),
        (None, Some(v)) => v,
        // A yes given for something else: the client's view is out of date,
        // so it is asked again, whatever this statement is.
        (Some(_), v) => {
            let production = dbx_core::production_safety::is_production_database(config, database);
            let mut v = v.unwrap_or(Verdict { production, reasons: Vec::new(), named: Vec::new(), target_unknown: false });
            v.reasons.push(Reason::Changed);
            v
        }
    };
    verdict.reasons.dedup();
    Err(Box::new(Assessment {
        production: verdict.production,
        reasons: verdict.reasons,
        named: verdict.named,
        target_unknown: verdict.target_unknown,
        connection_name: display_safe(&config.name),
        confirmation: this,
    }))
}

/// The statements of `sql`, comments gone (MySQL's executable comments
/// opened), split at `;`. A `;` inside a literal splits it too: that can only
/// make the checks below see more, never less.
fn statements_of(sql: &str) -> Vec<String> {
    dbx_core::query_execution_sql::strip_sql_comments(sql)
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The first word of a statement, as written.
fn first_word(statement: &str) -> &str {
    let end = statement.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(statement.len());
    &statement[..end]
}

/// Whether any statement of `sql` begins with `USE`: it switches the
/// session's database (MySQL, SQL Server and kin), and the switch lasts.
fn switches_database(sql: &str) -> bool {
    statements_of(sql).iter().any(|s| first_word(s).eq_ignore_ascii_case("use"))
}

/// Whether dbx proves `sql` (and what an `EXPLAIN ANALYZE` of it runs)
/// read-only, and it switches nothing: the statements that need no second
/// look at the saved flags.
fn proven_read_only(sql: &str, db_type: DatabaseType) -> bool {
    use dbx_core::sql_risk::{classify_sql_risk_for_database, prove_read_only_for_database, ReadProof, SqlRisk};
    let proven = |text: &str| {
        matches!(classify_sql_risk_for_database(text, db_type), Ok(SqlRisk::ReadOnly))
            && prove_read_only_for_database(text, db_type) == ReadProof::ProvenReadOnly
    };
    !switches_database(sql) && (proven(sql) || explain_analyze_inner(sql).is_some_and(|inner| proven(&inner)))
}

/// Whether every statement of `sql` is a `USE`.
fn only_switches(sql: &str) -> bool {
    let all = statements_of(sql);
    !all.is_empty() && all.iter().all(|s| first_word(s).eq_ignore_ascii_case("use"))
}

/// The sessions in which a statement that switches the database has run. A
/// `USE` lasts in the session, but the gate judges a run against the tab's
/// database: for such a session, on a connection that lists a production
/// database, nothing not proven read-only is assumed to be elsewhere.
#[derive(Default)]
struct SwitchedSessions(std::sync::Mutex<std::collections::HashSet<SessionId>>);

impl SwitchedSessions {
    /// `sql` is about to be sent in `session`.
    fn note(&self, session: SessionId, sql: &str) {
        if switches_database(sql) {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(session);
        }
    }
    fn contains(&self, session: SessionId) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).contains(&session)
    }
    /// The session is closed: a new one starts where the tab's database is.
    fn forget(&self, session: SessionId) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(&session);
    }
}

/// The statement an `EXPLAIN` with `ANALYZE` in any option form will run:
/// `explain analyze s`, `explain (analyze) s`, `explain (analyze true,
/// verbose) s`, `explain analyze verbose s`, MySQL's `explain analyze
/// format=tree s`. `None` when it is not an EXPLAIN, or has no ANALYZE (or
/// `analyze false`).
fn explain_analyze_inner(sql: &str) -> Option<String> {
    let stripped = dbx_core::query_execution_sql::strip_sql_comments(sql);
    let mut rest = stripped.trim_start();
    let word = first_word(rest);
    if !word.eq_ignore_ascii_case("explain") {
        return None;
    }
    rest = rest[word.len()..].trim_start();
    let mut analyze = false;
    loop {
        if let Some(open) = rest.strip_prefix('(') {
            let (options, after) = open.split_once(')')?;
            for option in options.split(',') {
                let mut parts = option.split_whitespace();
                let name = parts.next().unwrap_or("").to_ascii_lowercase();
                if name == "analyze" || name == "analyse" {
                    analyze = !matches!(parts.next().map(str::to_ascii_lowercase).as_deref(), Some("false" | "off" | "0"));
                }
            }
            rest = after.trim_start();
            continue;
        }
        let word = first_word(rest);
        match word.to_ascii_lowercase().as_str() {
            "analyze" | "analyse" => analyze = true,
            "verbose" => {}
            "format" => {
                // `format=tree`, `format = json`
                let after = rest[word.len()..].trim_start().strip_prefix('=')?.trim_start();
                rest = &after[first_word(after).len()..];
                rest = rest.trim_start();
                continue;
            }
            _ => break,
        }
        rest = rest[word.len()..].trim_start();
    }
    (analyze && !rest.is_empty()).then(|| rest.to_string())
}

/// One cell as text. dbx sends numbers it cannot hold exactly as strings
/// already; a JSON number is printed as dbx parsed it, which for integers is
/// exact and for floats is the shortest text that reads back the same.
fn cell(value: serde_json::Value) -> Cell {
    match value {
        serde_json::Value::Null => Cell::Null,
        serde_json::Value::String(s) => Cell::Value(s),
        other => Cell::Value(other.to_string()),
    }
}

/// A column's kind from the database's name for its type. Postgres names
/// (`int4`, `timestamptz`) and the spelled-out SQL names other drivers use
/// (`BIGINT UNSIGNED`, `DATETIME`) both count. Arrays and anything unknown
/// are `Other`.
fn kind_of_type(db_type: &str) -> CellKind {
    let t = db_type.trim().to_ascii_lowercase();
    if t.starts_with('_') || t.ends_with("[]") {
        return CellKind::Other;
    }
    let base = t.split(['(', ' ']).next().unwrap_or("");
    match base {
        "int2" | "int4" | "int8" | "smallint" | "integer" | "int" | "bigint" | "tinyint" | "mediumint" | "serial"
        | "bigserial" | "smallserial" | "oid" => CellKind::Integer,
        "numeric" | "decimal" | "money" => CellKind::Decimal,
        "float4" | "float8" | "real" | "double" | "float" => CellKind::Float,
        // `bit` and `varbit` are bit strings (`101`), not truth values.
        "bool" | "boolean" => CellKind::Bool,
        "text" | "varchar" | "bpchar" | "char" | "character" | "name" | "citext" | "string" | "enum" | "set"
        | "tinytext" | "mediumtext" | "longtext" => CellKind::Text,
        "date" | "time" | "timetz" | "timestamp" | "timestamptz" | "interval" | "datetime" | "year" => CellKind::Temporal,
        "json" | "jsonb" => CellKind::Json,
        "bytea" | "blob" | "binary" | "varbinary" | "tinyblob" | "mediumblob" | "longblob" => CellKind::Binary,
        _ => CellKind::Other,
    }
}


/// A failed run as ours: the kind from dbx's error variant (a connection
/// failure is `Connect` whatever variant carries it), dbx's message without
/// the sentence saying it left the SQL out (we show the SQL), the password
/// blanked out, and the statement that ran with the position dbx found in it.
fn run_error(error: QueryExecutionError, statement: &str, secret: &str, timeout: Option<u64>) -> CoreError {
    let (kind, position) = match &error {
        QueryExecutionError::Canceled { .. } => (CoreErrorKind::Cancelled, None),
        QueryExecutionError::Timeout(_) => (CoreErrorKind::Timeout, None),
        QueryExecutionError::SqlWithPosition { position, .. } => {
            (CoreErrorKind::Sql, Some(SqlPosition { line: position.line, column: position.column }))
        }
        QueryExecutionError::Sql(_) => (CoreErrorKind::Sql, None),
        _ => (CoreErrorKind::Other, None),
    };
    let raw = error.into_legacy_string();
    let kind = if kind != CoreErrorKind::Cancelled && dbx_core::query::is_connection_error(&raw) {
        CoreErrorKind::Connect
    } else {
        kind
    };
    let message = match (kind, timeout) {
        (CoreErrorKind::Timeout, Some(secs)) => format!(
            "The statement ran longer than this connection's query timeout of {secs} s and was stopped. \
             The limit is the connection's \"Query timeout\" setting, set in the dbx desktop app."
        ),
        _ => run_message(&raw, secret),
    };
    let mut e = CoreError::new(kind, message);
    e.statement = Some(statement.to_string());
    e.position = position;
    e
}

/// The sentence dbx appends to a SQL error in place of the SQL.
const SQL_OMITTED: &str = "SQL text omitted from user-facing error";

/// dbx's words for a failed run: the omitted-SQL sentence dropped, the
/// password blanked out, control characters replaced line by line (the
/// server's DETAIL and HINT lines stay lines), and the length capped.
fn run_message(raw: &str, secret: &str) -> String {
    let mut text = match raw.find(SQL_OMITTED) {
        Some(at) => raw[..at].to_string(),
        None => raw.to_string(),
    };
    if !secret.is_empty() {
        text = text.replace(secret, "••••");
    }
    let mut out: String = text.trim().lines().map(|l| display_safe(l.trim_end())).collect::<Vec<_>>().join("\n");
    if out.chars().count() > 4_000 {
        out = out.chars().take(3_999).collect::<String>() + "…";
    }
    out
}

impl Syntax for DbxCore {
    fn capabilities(&self, kind: &DbKind) -> Capabilities {
        database_type_of(kind).map(capabilities_of).unwrap_or_default()
    }

    fn type_names(&self, kind: &DbKind) -> Vec<String> {
        type_names_for(kind)
    }

    fn statement_range(&self, kind: &DbKind, buffer: &str, cursor: Cursor) -> Option<(Cursor, Cursor)> {
        statement_range_for(kind, buffer, cursor)
    }

    fn table_page_sql(&self, kind: &DbKind, table: &TablePath, order_by: &[String], limit: usize, offset: usize) -> String {
        page_sql_for(kind, table, order_by, limit, offset)
    }

    fn table_count_sql(&self, kind: &DbKind, table: &TablePath) -> String {
        count_sql_for(kind, table)
    }

    fn export_bytes(&self, request: &ExportRequest) -> Result<Vec<u8>, String> {
        export_bytes_for(request)
    }

    fn export_skipped_columns(&self, request: &ExportRequest) -> Vec<String> {
        export_skipped_columns_for(request)
    }
}

/// A result in one format, by dbx's formats (slice 19), from the exact text
/// the client holds.
///
/// dbx's formats take JSON values, so each cell is converted: NULL is `null`;
/// an integer column's text is a JSON number only when it is the exact text of
/// an `i64` or `u64`; everything else is a string holding the exact text. A row
/// longer than the header loses the cells past it (they have no column); a
/// shorter one stays short, since a missing cell is not NULL. Cell by cell
/// what each format then does is listed in the README.
pub(super) fn export_bytes_for(request: &ExportRequest) -> Result<Vec<u8>, String> {
    use dbx_core::csv_export::{default_csv_null_literal, format_csv_with_options, CsvQuoteMode};
    use dbx_core::text_export::{format_json, format_markdown, QueryResultTextExportData};
    use dbx_core::xlsx_export::{build_xlsx_workbook, XlsxWorksheetData};

    let names: Vec<String> = request.columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Vec<serde_json::Value>> = request
        .rows
        .iter()
        .map(|row| row.iter().take(request.columns.len()).enumerate().map(|(c, cell)| json_cell(&request.columns[c], cell)).collect())
        .collect();
    match request.format {
        ExportFormat::Csv => {
            let null = default_csv_null_literal();
            let literal = if request.null_empty { None } else { Some(null.as_str()) };
            Ok(format_csv_with_options(&names, &rows, CsvQuoteMode::All, literal).into_bytes())
        }
        ExportFormat::Json => format_json(&QueryResultTextExportData { title: None, columns: names, rows }).map(String::into_bytes),
        ExportFormat::Markdown => Ok(format_markdown(&QueryResultTextExportData { title: None, columns: names, rows }).into_bytes()),
        ExportFormat::Excel => {
            let mut sheet = XlsxWorksheetData::new(Some("Result".into()), names, rows);
            sheet.column_types = request.columns.iter().map(|c| c.db_type.clone()).collect();
            build_xlsx_workbook(&sheet)
        }
        ExportFormat::SqlInsert => {
            if rows.is_empty() {
                return Err("the result has no rows, so there is nothing to insert".into());
            }
            // dbx leaves some columns out (see `export_skipped_columns_for`);
            // they are dropped here, with their cells, so that what dbx does is
            // known and told, and its raw-name rules still apply.
            let skipped = export_skipped_columns_for(request);
            let kept = ExportRequest {
                columns: request.columns.iter().filter(|c| !skipped.contains(&c.name)).cloned().collect(),
                rows: request
                    .rows
                    .iter()
                    .map(|row| row.iter().zip(&request.columns).filter(|(_, c)| !skipped.contains(&c.name)).map(|(cell, _)| cell.clone()).collect())
                    .collect(),
                ..request.clone()
            };
            if kept.columns.is_empty() {
                return Err(format!("every column would be left out of the INSERT ({}), so there is nothing to insert", skipped.join(", ")));
            }
            insert_statements(&kept, true).map(String::into_bytes)
        }
    }
}

/// The columns dbx's SQL INSERT leaves out, by name, in column order: dbx's
/// own `is_export_insert_column` decides (Postgres `tsvector`, the synthetic
/// row id column, SQL Server `timestamp`/`rowversion`). It is private, so each
/// column is offered alone to the INSERT builder: an empty answer means it was
/// skipped. A column the builder refuses for another reason is not counted.
pub(super) fn export_skipped_columns_for(request: &ExportRequest) -> Vec<String> {
    request
        .columns
        .iter()
        .enumerate()
        .filter(|(c, column)| {
            let probe = ExportRequest {
                format: ExportFormat::SqlInsert,
                columns: vec![(*column).clone()],
                rows: vec![vec![request.rows.first().and_then(|r| r.get(*c)).cloned().unwrap_or(Cell::Null)]],
                kind: request.kind.clone(),
                table: "t".into(),
                schema: String::new(),
                null_empty: false,
            };
            insert_statements(&probe, false).map(|text| text.is_empty()).unwrap_or(false)
        })
        .map(|(_, column)| column.name.clone())
        .collect()
}

/// The INSERT text of `request` by dbx's builder. Every name goes through
/// `guarded_all`/`guarded` here, the one place names reach it. With
/// `guard_names` off the raw names are used (the skip probe: dbx's rules
/// match raw names, `__DBX_ROWID` for one).
fn insert_statements(request: &ExportRequest, guard_names: bool) -> Result<String, String> {
    use dbx_core::database_export::{build_export_sql_insert, BuildExportInsertStatementsOptions, BuildExportSqlInsertOptions, SqlInsertDialect};
    let db_type = request.kind.as_ref().and_then(database_type_of);
    let names: Vec<String> = request.columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Vec<serde_json::Value>> = request
        .rows
        .iter()
        .map(|row| row.iter().take(request.columns.len()).enumerate().map(|(c, cell)| json_cell(&request.columns[c], cell)).collect())
        .collect();
    let insert = BuildExportInsertStatementsOptions {
        database_type: db_type,
        identifier_quote: None,
        schema: Some(guarded_if(guard_names, db_type, &request.schema)).filter(|s| !s.is_empty()),
        table_name: Some(guarded_if(guard_names, db_type, &request.table)),
        qualified_table_name: None,
        columns: guarded_all_if(guard_names, db_type, &names),
        column_types: request.columns.iter().map(|c| Some(c.db_type.clone()).filter(|t| !t.is_empty())).collect(),
        column_extras: Vec::new(),
        spatial_columns: Vec::new(),
        spatial_values: Vec::new(),
        rows,
        batch_size: None,
        preserve_original_language: false,
    };
    build_export_sql_insert(BuildExportSqlInsertOptions { insert, insert_dialect: SqlInsertDialect::Source, exclude_columns: Vec::new() })
}

/// One cell as a JSON value, for dbx's formats (see `export_bytes_for`).
fn json_cell(column: &Column, cell: &Cell) -> serde_json::Value {
    use serde_json::Value;
    let Cell::Value(text) = cell else { return Value::Null };
    if column.kind == CellKind::Integer {
        if let Some(n) = text.parse::<i64>().ok().filter(|n| n.to_string() == *text) {
            return Value::from(n);
        }
        if let Some(n) = text.parse::<u64>().ok().filter(|n| n.to_string() == *text) {
            return Value::from(n);
        }
    }
    Value::String(text.clone())
}

/// The select of one page of `table`, built by dbx's table-data select
/// (`build_table_data_select_sql`). dbx orders by nothing of its own:
/// `fallback_order_columns` is never read, and `primary_keys` only decides
/// whether Postgres adds a `ctid` column when row ids are asked for (they are
/// not). So the order is ours, `order_by`, quoted the way dbx quotes names.
/// `large_value_preview_size` is never set: every value comes whole.
fn page_sql_for(kind: &DbKind, table: &TablePath, order_by: &[String], limit: usize, offset: usize) -> String {
    use dbx_core::sql_dialect::{build_table_data_select_sql, quote_table_identifier, TableDataSelectSqlOptions};
    let db_type = database_type_of(kind);
    let order = guarded_all(db_type, order_by).iter().map(|c| quote_table_identifier(db_type, c)).collect::<Vec<_>>().join(", ");
    build_table_data_select_sql(TableDataSelectSqlOptions {
        database_type: db_type,
        schema: Some(guarded(db_type, &table.schema.schema)).filter(|s| !s.is_empty()),
        table_name: guarded(db_type, &table.table),
        primary_keys: guarded_all(db_type, order_by),
        order_by: Some(order).filter(|o| !o.is_empty()),
        limit: Some(limit),
        offset: Some(offset),
        ..Default::default()
    })
}

/// `name` made safe to hand to dbx's identifier quoting. For Postgres, GaussDB
/// and openGauss dbx returns a name unchanged when it starts and ends with a
/// quote (`"`, a backtick, or `[` and `]`), taking it for one already quoted,
/// and skips the doubling of any `"` inside: a table named
/// `"x"; delete from t; --"` would then end the identifier and run the rest.
/// The name is quoted here, `"` doubled, so dbx's pass-through returns it
/// whole. Every other kind (dbx doubles the quote there) and an empty name get
/// the raw name.
fn guarded(db_type: Option<DatabaseType>, name: &str) -> String {
    let passes_quoted_names_through = matches!(db_type, Some(DatabaseType::Postgres | DatabaseType::Gaussdb | DatabaseType::OpenGauss));
    if !passes_quoted_names_through || name.is_empty() {
        return name.to_string();
    }
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `guarded` for every name of a list.
fn guarded_all(db_type: Option<DatabaseType>, names: &[String]) -> Vec<String> {
    names.iter().map(|n| guarded(db_type, n)).collect()
}

/// `guarded`, or the raw name when `on` is off: only for building text that is
/// looked at and never run (the skip probe of `export_skipped_columns_for`).
fn guarded_if(on: bool, db_type: Option<DatabaseType>, name: &str) -> String {
    if on {
        guarded(db_type, name)
    } else {
        name.to_string()
    }
}

fn guarded_all_if(on: bool, db_type: Option<DatabaseType>, names: &[String]) -> Vec<String> {
    names.iter().map(|n| guarded_if(on, db_type, n)).collect()
}

/// dbx's count statement for `table` (`build_data_grid_count_sql`).
fn count_sql_for(kind: &DbKind, table: &TablePath) -> String {
    use dbx_core::data_grid_sql::{build_data_grid_count_sql, DataGridCountSqlOptions};
    let db_type = database_type_of(kind);
    build_data_grid_count_sql(DataGridCountSqlOptions {
        database_type: db_type,
        identifier_quote: None,
        catalog: None,
        database: None,
        schema: Some(guarded(db_type, &table.schema.schema)).filter(|s| !s.is_empty()),
        table_name: guarded(db_type, &table.table),
        where_input: None,
        count_hint: None,
    })
}

/// dbx's `analyze_sql_references` for `sql`, in our own types. Its spans are
/// 1-based lines and columns (the column of a character, with the end
/// inclusive); ours are 0-based with the end after the last character.
fn references_for(kind: &DbKind, sql: &str) -> Result<References, String> {
    use dbx_core::sql_analysis::{analyze_sql_references, SqlTextSpan};
    let span = |s: SqlTextSpan| {
        let start = Cursor { row: s.start_line.saturating_sub(1), col: s.start_column.saturating_sub(1) };
        let end = Cursor { row: s.end_line.saturating_sub(1), col: s.end_column };
        Span { start, end: if (end.row, end.col) < (start.row, start.col) { start } else { end } }
    };
    let analysis = analyze_sql_references(sql, Some(kind.label()))?;
    Ok(References {
        tables: analysis
            .tables
            .into_iter()
            .map(|t| TableReference { name: t.name, schema: t.schema, alias: t.alias, span: span(t.span), scope: t.scope_id })
            .collect(),
        columns: analysis
            .columns
            .into_iter()
            .map(|c| ColumnReference { name: c.name, qualifier: c.qualifier, span: span(c.span), scope: c.scope_id })
            .collect(),
        scopes: analysis.scopes.into_iter().map(|s| (s.id, s.parent_id)).collect(),
        unknown_qualifiers: analysis.unknown_qualifiers.into_iter().map(|u| (u.qualifier, span(u.span))).collect(),
    })
}

/// The type names of a kind of database, from dbx's dialect files: each
/// type's name and its aliases. `list_dialect_type_names` gives only the
/// names (Postgres's `character varying`, not `varchar`; `integer`, not
/// `int`), and what people type is mostly the alias.
fn type_names_for(kind: &DbKind) -> Vec<String> {
    use dbx_core::sql_dialect::dialect_loader::DialectRegistry;
    let name = kind.display_name();
    // Also what loads the compiled-in dialect files the registry reads.
    let mut names = dbx_core::sql_dialect::dialect_types::list_dialect_type_names(name);
    if let Some(yaml) = DialectRegistry::global().get_yaml(name) {
        names.extend(yaml.types.into_iter().flat_map(|t| t.aliases));
    }
    names
}

/// dbx's database type for one of our kinds, if dbx has it.
fn database_type_of(kind: &DbKind) -> Option<DatabaseType> {
    match kind {
        DbKind::Postgres => Some(DatabaseType::Postgres),
        DbKind::MySql => Some(DatabaseType::Mysql),
        DbKind::Sqlite => Some(DatabaseType::Sqlite),
        DbKind::Other(name) => DatabaseType::ALL.iter().copied().find(|t| t.as_str() == name),
    }
}

/// Where the statement `statement_to_run` would pick for the caret is.
/// It is `statement_at`, the function a run uses, and its text located in the
/// buffer (`locate_statement`), so the outline and the run cannot disagree.
fn statement_range_for(kind: &DbKind, buffer: &str, cursor: Cursor) -> Option<(Cursor, Cursor)> {
    let db_type = database_type_of(kind)?;
    locate_statement(buffer, cursor, &statement_at(buffer, cursor, db_type))
}

#[async_trait]
impl Core for DbxCore {
    async fn connections(&self) -> Result<Vec<Connection>, CoreError> {
        let state = self.state().await?;
        let configs = state.configs.read().await;
        // A connection being tested is in dbx's runtime list under a temporary
        // id for as long as the test runs; it is not a saved connection.
        let mut list: Vec<Connection> =
            configs.iter().filter(|(id, _)| !is_transient_runtime_config_id(id)).map(|(_, c)| to_connection(c)).collect();
        sort_connections(&mut list);
        Ok(list)
    }

    async fn save_connection(&self, draft: &ConnectionDraft) -> Result<Connection, CoreError> {
        let state = self.state().await?;
        let _one_at_a_time = self.mutations.lock().await;
        let existing = match &draft.id {
            Some(id) => state.configs.read().await.get(&id.0).cloned(),
            None => None,
        };
        let config = config_from_draft(draft, existing)?;
        // The store first: if it refuses (no key, a locked file), the runtime
        // list must not move ahead of it.
        let (st, to_store) = (state.clone(), config.clone());
        blocking(async move { st.storage.save_connections(std::slice::from_ref(&to_store)).await })
            .await?
            .map_err(|raw| self.store_failure("save the connection", &raw))?;
        // dbx does not re-sync its runtime from the store: do what its own
        // heads do after a save (`dbx-web/src/routes/connection.rs`).
        let old = state.configs.write().await.insert(config.id.clone(), config.clone());
        if let Some(old) = old {
            if !connection_configs_session_credentials_compatible(&old, &config) {
                state.session_credentials.clear_connection(&config.id);
            }
            if !connection_configs_pool_equivalent(&old, &config) {
                // Pools opened with the old host, port or password must not serve the next run.
                let _ = state.remove_connection_pools_detached_checked(&config.id).await;
            }
        }
        Ok(to_connection(&config))
    }

    async fn remove_connection(&self, id: &ConnectionId) -> Result<(), CoreError> {
        let state = self.state().await?;
        let _one_at_a_time = self.mutations.lock().await;
        let (st, gone) = (state.clone(), id.0.clone());
        blocking(async move { st.storage.delete_connections(std::slice::from_ref(&gone)).await })
            .await?
            .map_err(|raw| self.store_failure("remove the connection", &raw))?;
        state.configs.write().await.remove(&id.0);
        state.session_credentials.clear_connection(&id.0);
        let _ = state.remove_connection_pools_detached_checked(&id.0).await;
        Ok(())
    }

    async fn test_connection(&self, draft: &ConnectionDraft) -> Result<(), CoreError> {
        let state = self.state().await?;
        let existing = match &draft.id {
            Some(id) => state.configs.read().await.get(&id.0).cloned(),
            None => None,
        };
        let mut config = config_from_draft(draft, existing)?;
        let secret = config.password.clone();
        config.id = format!("{TEST_PROBE_ID_PREFIX}{}", uuid::Uuid::new_v4());
        state.configs.write().await.insert(config.id.clone(), config.clone());
        let database = config.database.clone();
        let mut probe = Probe { state: state.clone(), id: config.id.clone(), config, armed: true };
        let result = state.get_or_create_pool(&probe.id, database.as_deref()).await;
        probe.cleanup().await;
        result.map(|_| ()).map_err(|raw| CoreError::new(CoreErrorKind::Connect, scrub(&raw, &[&secret])))
    }

    async fn databases(&self, id: &ConnectionId) -> Result<Vec<String>, CoreError> {
        self.ensure_browsable(id).await?;
        let conn = id.0.clone();
        let list = self
            .metadata(id, move |state| async move { dbx_core::schema::list_databases_core(&state, &conn).await })
            .await?;
        // Names stay exactly as the server has them: they are the paths of the
        // nodes below, and the painter makes whatever is drawn safe.
        Ok(list.into_iter().map(|d| d.name).collect())
    }
    async fn schemas(&self, at: &DatabasePath) -> Result<Vec<String>, CoreError> {
        self.ensure_browsable(&at.connection).await?;
        let (conn, database) = (at.connection.0.clone(), self.database_name(&at.connection, &at.database).await);
        let list = self
            .metadata(&at.connection, move |state| async move {
                dbx_core::schema::list_schemas_core(&state, &conn, &database).await
            })
            .await?;
        Ok(list.into_iter().filter(|name| !is_system_schema(name)).collect())
    }
    async fn tables(&self, at: &SchemaPath) -> Result<Vec<Table>, CoreError> {
        self.ensure_browsable(&at.database.connection).await?;
        let (conn, database, schema) = (at.database.connection.0.clone(), self.database_name(&at.database.connection, &at.database.database).await, at.schema.clone());
        let list = self
            .metadata(&at.database.connection, move |state| async move {
                dbx_core::schema::list_tables_core(&state, &conn, &database, &schema, None, None, None, None, None).await
            })
            .await?;
        // A table with several INHERITS parents comes back once per parent: one
        // row per name, the first, or two rows would share one node identity.
        let mut seen = std::collections::HashSet::new();
        Ok(list
            .into_iter()
            // A partition of a partitioned table has its parent named; the
            // parent is listed, and its partitions would bury everything else.
            .filter(|t| t.parent_name.is_none())
            .filter(|t| seen.insert(t.name.clone()))
            .map(|t| Table { kind: table_kind(&t.table_type), name: t.name })
            .collect())
    }
    async fn columns(&self, at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError> {
        self.ensure_browsable(&at.schema.database.connection).await?;
        let schema = &at.schema;
        let (conn, database, schema_name, table) =
            (
                schema.database.connection.0.clone(),
                self.database_name(&schema.database.connection, &schema.database.database).await,
                schema.schema.clone(),
                at.table.clone(),
            );
        let list = self
            .metadata(&schema.database.connection, move |state| async move {
                dbx_core::schema::get_columns_core(&state, &conn, &database, &schema_name, &table).await
            })
            .await?;
        Ok(list
            .into_iter()
            .map(|c| ColumnInfo {
                name: c.name,
                db_type: c.data_type,
                nullable: c.is_nullable,
                primary_key: c.is_primary_key,
                default: c.column_default,
            })
            .collect())
    }
    async fn foreign_keys(&self, at: &TablePath) -> Result<Vec<ForeignKey>, CoreError> {
        self.ensure_browsable(&at.schema.database.connection).await?;
        let schema = &at.schema;
        let (conn, database, schema_name, table) =
            (
                schema.database.connection.0.clone(),
                self.database_name(&schema.database.connection, &schema.database.database).await,
                schema.schema.clone(),
                at.table.clone(),
            );
        let list = self
            .metadata(&schema.database.connection, move |state| async move {
                dbx_core::schema::list_foreign_keys_core(&state, &conn, &database, &schema_name, &table).await
            })
            .await?;
        Ok(list
            .into_iter()
            .map(|k| ForeignKey { name: k.name, column: k.column, ref_schema: k.ref_schema, ref_table: k.ref_table, ref_column: k.ref_column })
            .collect())
    }
    async fn analyze_references(&self, kind: &DbKind, sql: &str) -> Result<Option<References>, CoreError> {
        let (kind, sql) = (kind.clone(), sql.to_string());
        // A parse of one statement: CPU only, off the runtime's workers.
        let parsed = tokio::task::spawn_blocking(move || references_for(&kind, &sql))
            .await
            .map_err(|e| CoreError::new(CoreErrorKind::Other, format!("the analysis stopped unexpectedly: {e}")))?;
        // A statement that does not parse is an answer, not a failure.
        Ok(parsed.ok())
    }
    async fn complete(&self, request: &CompletionRequest) -> Result<Completion, CoreError> {
        use dbx_core::db::{CompletionAssistantCandidateKind as K, CompletionAssistantMatchMode, CompletionAssistantObjectKind as O, CompletionAssistantRequest};
        self.ensure_browsable(&request.at.connection).await?;
        let object_kinds = match request.kind {
            SearchKind::Schema => vec![O::Schema],
            SearchKind::Relation => vec![O::Table, O::View],
            SearchKind::Function => vec![O::Function],
        };
        let limit = request.limit.clamp(1, 1000);
        let search = CompletionAssistantRequest {
            connection_id: request.at.connection.0.clone(),
            database: self.database_name(&request.at.connection, &request.at.database).await,
            schema: request.schema.clone(),
            object_kinds,
            mask: request.prefix.clone(),
            case_sensitive: false,
            global_search: request.global,
            max_results: Some(limit),
            search_in_comments: false,
            search_in_definitions: false,
            parent_schema: None,
            parent_name: None,
            match_mode: Some(CompletionAssistantMatchMode::Prefix),
        };
        let conn = request.at.connection.clone();
        let response = self
            .metadata(&conn, move |state| async move { dbx_core::schema::completion_assistant_search_core(&state, search).await })
            .await?;
        let candidates = response
            .candidates
            .into_iter()
            .filter_map(|c| {
                let kind = match c.kind {
                    K::Schema => CandidateKind::Schema,
                    K::Table => CandidateKind::Table,
                    K::View => CandidateKind::View,
                    K::MaterializedView => CandidateKind::MaterializedView,
                    K::Function => CandidateKind::Function,
                    K::Procedure => CandidateKind::Procedure,
                    _ => return None,
                };
                let detail = c.data_type.filter(|d| !d.is_empty());
                Some(Candidate { name: c.name, kind, schema: c.schema, detail })
            })
            .collect();
        Ok(Completion { candidates, incomplete: response.incomplete })
    }
    async fn current_schema(&self, at: &DatabasePath) -> Result<Option<String>, CoreError> {
        self.ensure_browsable(&at.connection).await?;
        let (conn, database) = (at.connection.0.clone(), self.database_name(&at.connection, &at.database).await);
        self.metadata(&at.connection, move |state| async move {
            // Through the query path: dbx has no getter for it. Its own pool
            // for the call, not the tab's session: the search path of a
            // session may have been `SET`, and this asks what a fresh
            // connection resolves names in.
            let options = QueryExecutionOptions { max_rows: Some(1), timeout_secs: Some(0), ..Default::default() };
            let result = execute_sql_statement_with_options_typed(&state, &conn, &database, "select current_schema()", None, None, options)
                .await
                .map_err(|e| e.to_string())?;
            Ok(result.rows.first().and_then(|r| r.first()).and_then(|v| v.as_str()).map(str::to_string))
        })
        .await
    }
    async fn release(&self, at: &DatabasePath) -> Result<(), CoreError> {
        let state = self.state().await?;
        let postgres = state.configs.read().await.get(&at.connection.0).is_some_and(|c| kind_of(c.db_type) == DbKind::Postgres);
        if !postgres {
            return Ok(());
        }
        // dbx keeps one pool per (connection, database) for a Postgres
        // connection, under the key "<connection id>:<database>"
        // (`base_pool_key_for_with_catalog` in dbx-core's connection module),
        // and `remove_pool_by_key` closes that one pool and its bookkeeping
        // (keepalive task, activity, cancel context) and waits for the close.
        // The connection's own pool, under the bare connection id, and the
        // pools of other databases are not touched. A key with no pool is a
        // no-op.
        let database = self.database_name(&at.connection, &at.database).await;
        state.remove_pool_by_key(&format!("{}:{database}", at.connection.0)).await;
        Ok(())
    }
    /// Runs one statement on the tab's own database session (dbx's
    /// `client_session_id`), so `SET`, temporary tables and transactions last
    /// from one run of a tab to the next, and runs of one tab queue behind
    /// each other. The run is registered with dbx's running-query list under
    /// its `RunId`, so `cancel` can reach it; the registration ends on every
    /// way out, including this future being dropped (`RegisteredQuery`'s
    /// `Drop`).
    async fn run(&self, run: RunId, request: RunRequest) -> Result<ResultSet, CoreError> {
        let state = self.state().await?;
        let config = state.configs.read().await.get(&request.connection.0).cloned();
        let Some(config) = config else {
            return Err(CoreError::new(CoreErrorKind::Other, "That connection is no longer saved."));
        };
        let statement = statement_to_run(&request, config.db_type);
        if statement.trim().is_empty() {
            return Err(CoreError::new(CoreErrorKind::Other, "There is no statement at the cursor to run."));
        }
        let database = request.database.clone().filter(|d| !d.is_empty()).or_else(|| config.database.clone()).unwrap_or_default();
        // The gate (slice 17), on the exact text about to be sent, with the
        // config as it is now (the desktop app can change the flags).
        // Nothing reaches dbx without the matching yes.
        let switched = self.switched.contains(request.session);
        let mut gated = gate(&config, &request, &statement, &database, switched);
        // A flag set in the desktop while this client runs is not in the
        // runtime copy. Before letting through anything not proven
        // read-only, look at the saved connection once more. A statement
        // already confirmed for this exact target has been asked about.
        if gated.is_ok() && request.confirmation.is_none() && !proven_read_only(&statement, config.db_type) {
            if let Some(saved) = self.saved_config(&state, &request.connection.0).await? {
                gated = gate(&saved, &request, &statement, &database, switched);
            }
        }
        gated.map_err(|a| CoreError::needs_confirmation(*a))?;
        // From here the statement is sent: a switch of database lasts in the
        // session, and the gate remembers it.
        self.switched.note(request.session, &statement);
        let session = session_id(request.session);
        let registered = {
            let mut early = self.early_cancels.lock().unwrap_or_else(|p| p.into_inner());
            if early.remove(&run) {
                let mut e = CoreError::new(CoreErrorKind::Cancelled, "Cancelled before it started.");
                e.statement = Some(statement);
                return Err(e);
            }
            state.running_queries.register_task(
                run_key(run),
                RunningTaskMetadata::query(request.connection.0.clone(), database.clone(), Some(session.clone())),
            )
        };
        let timeout = query_timeout(&config);
        let options = QueryExecutionOptions {
            max_rows: Some(request.row_cap),
            // Always "no limit" to dbx (absent would be its 30 s default): the
            // connection's own limit is kept here, below, because dbx's
            // timeout on Postgres discards the session's connection (its
            // `pool_error_action`), losing the session's SET values, temporary
            // tables and transaction, and leaving nothing to tell when the
            // statement has really stopped.
            timeout_secs: Some(0),
            client_session_id: Some(session.clone()),
            execution_id: Some(run_key(run)),
            ..Default::default()
        };
        let token = registered.token();
        let execution = execute_sql_statement_with_options_typed(
            &state,
            &request.connection.0,
            &database,
            &statement,
            None,
            Some(registered.token()),
            options,
        );
        tokio::pin!(execution);
        // The connection's query timeout: when it passes, the run is
        // cancelled through its token, exactly as a user's cancel is (dbx
        // keeps the connection on a cancel), and its outcome is a timeout.
        let mut timed_out = false;
        let result = match timeout {
            Some(secs) => tokio::select! {
                result = &mut execution => result,
                _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {
                    timed_out = true;
                    token.cancel();
                    execution.await
                }
            },
            None => execution.await,
        };
        registered.finish_with_late_cancel(&result, false);
        let stopped = match &result {
            // Nothing reached the server: there is nothing to wait for.
            Err(QueryExecutionError::Canceled { operation_outcome: dbx_core::db::agent_driver::AgentOperationOutcome::NotStarted, .. }) => true,
            Err(QueryExecutionError::Canceled { .. } | QueryExecutionError::Timeout(_)) => {
                self.wait_for_session(&state, &request.connection.0, &database, request.session).await
            }
            _ => true,
        };
        match (result, timeout) {
            (Err(QueryExecutionError::Canceled { .. }), Some(secs)) if timed_out => Err(timeout_error(secs, stopped, &statement)),
            (result, _) => result.map(|r| result_set(r, statement.clone(), config.db_type)).map_err(|e| run_error(e, &statement, &config.password, timeout)),
        }
    }
    fn cancel(&self, run: RunId) {
        // Under the lock `run` registers with, so a cancel is either seen by
        // dbx or remembered for the run to find: never neither.
        let mut early = self.early_cancels.lock().unwrap_or_else(|p| p.into_inner());
        let landed = self.opened.get().is_some_and(|state| state.running_queries.cancel(&run_key(run)));
        if !landed {
            early.insert(run);
            while early.len() > EARLY_CANCELS {
                early.pop_first();
            }
        }
    }
    /// Closes the one server connection dbx keeps for this tab's session on
    /// that database (its pool is keyed `<connection>:<database>:session:<id>`).
    /// `release` never touches it: that removes only the database's shared
    /// pool, keyed without the session, so collapsing a database in the
    /// sidebar cannot end a tab's session, even mid-run.
    async fn close_session(&self, session: SessionId, at: &DatabasePath) -> Result<(), CoreError> {
        let state = self.state().await?;
        // Before the close, so a run's wait for this session (which could
        // otherwise open it again) sees it is gone.
        self.closed_sessions.lock().unwrap_or_else(|p| p.into_inner()).insert(session);
        self.switched.forget(session);
        state
            .close_client_session_pool(&at.connection.0, Some(self.database_name(&at.connection, &at.database).await.as_str()).filter(|d| !d.is_empty()), &session_id(session))
            .await
            .map(|_| ())
            .map_err(|raw| CoreError::new(CoreErrorKind::Other, scrub(&raw, &[])))
    }
    /// Saved through the call the desktop app uses (`save_history_entry`), in
    /// the shape it writes: an RFC 3339 UTC time with milliseconds, the
    /// activity kind its history panel filters by, source `sql`.
    async fn save_history(&self, entry: &NewHistoryEntry) -> Result<(), CoreError> {
        let state = self.state().await?;
        let saved = dbx_core::history::HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            connection_id: entry.connection.0.clone(),
            connection_name: entry.connection_name.clone(),
            database: entry.database.clone(),
            sql: entry.sql.clone(),
            executed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            execution_time_ms: entry.elapsed.as_millis(),
            success: entry.success,
            error: entry.error.clone(),
            activity_kind: activity_kind(&entry.sql).to_string(),
            operation: primary_operation(&entry.sql),
            target: String::new(),
            affected_rows: entry.affected.map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
            rollback_sql: None,
            details_json: None,
            source: "sql".to_string(),
            mcp_tool_name: None,
            mcp_request_json: None,
            mcp_response_json: None,
            mcp_session_id: None,
        };
        state.storage.save_history_entry(&saved).await.map_err(|raw| CoreError::new(CoreErrorKind::Store, scrub(&raw, &[])))
    }

    async fn history(&self, query: &HistoryQuery) -> Result<HistoryPage, CoreError> {
        let state = self.state().await?;
        let request = dbx_core::history::HistorySearchRequest {
            search_text: query.search_text.clone(),
            connections: query
                .connection
                .iter()
                .map(|c| dbx_core::history::HistoryConnectionFilter { connection_id: c.0.clone(), connection_name: String::new() })
                .collect(),
            cursor: query.cursor.as_ref().map(|c| dbx_core::history::HistoryCursor { executed_at: c.executed_at.clone(), id: c.id.clone() }),
            limit: query.limit,
            // The MCP server's statements are not ours to show.
            source: Some("sql".to_string()),
            ..Default::default()
        };
        let found = state.storage.search_history_entries(request).await.map_err(|raw| CoreError::new(CoreErrorKind::Store, scrub(&raw, &[])))?;
        Ok(HistoryPage {
            entries: found.entries.into_iter().map(history_entry).collect(),
            next: found.next_cursor.map(|c| HistoryCursor { executed_at: c.executed_at, id: c.id }),
            total: found.total,
        })
    }

    /// The desktop app's sequence (`src-tauri/src/commands/database_export.rs`):
    /// dbx's export on `spawn_export_task` (the blocking pool, driven by this
    /// 16 MiB-stack runtime), then the export's own session closed, then its
    /// cancel flag cleared. All three happen inside the task, so they happen
    /// even if nobody waits for the answer any more.
    ///
    /// dbx records the destination directory's filesystem identity in the store
    /// (`ensure_export_destination_dir`), and makes a missing directory; the
    /// shell refuses a missing one before this is called.
    async fn dump(&self, id: DumpId, request: DumpRequest, progress: DumpSink) -> Result<DumpEnd, CoreError> {
        let state = self.state().await?;
        let config = state.configs.read().await.get(&request.at.connection.0).cloned();
        let Some(config) = config else {
            return Err(CoreError::new(CoreErrorKind::Other, "That connection is no longer saved."));
        };
        let database = self.database_name(&request.at.connection, &request.at.database).await;
        // dbx quotes the column names it reads from its own catalog with a
        // function that passes a name through unescaped when it starts and
        // ends with a quote (Postgres, GaussDB, openGauss), in its read query
        // and its INSERT column list. We cannot guard names we do not hand to
        // it, so a dump with such a column is refused, naming them.
        if matches!(config.db_type, DatabaseType::Postgres | DatabaseType::Gaussdb | DatabaseType::OpenGauss) {
            let found = quoted_columns(&state, &request.at.connection.0, &database, &request.schema, &config).await?;
            if !found.is_empty() {
                return Err(CoreError::new(CoreErrorKind::Refused, refusal_for_quoted_columns(&found, &request.schema)));
            }
        }
        let export_id = dump_key(id);
        let export = dump_request(&export_id, &request, &database)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        dbx_core::export_runtime::spawn_export_task(async move {
            let last = std::sync::Mutex::new(None::<DumpProgress>);
            let sent = std::sync::Mutex::new(None::<std::time::Instant>);
            let result = dbx_core::database_export::export_database_sql_core(&state, &export, |p| {
                let ours = dump_progress(p, &config.password);
                if matches!(ours.status, DumpStatus::Done | DumpStatus::Cancelled) {
                    *last.lock().unwrap_or_else(|p| p.into_inner()) = Some(ours.clone());
                } else {
                    // dbx reports every page: a few a second are enough to
                    // watch, and the app redraws for each one it gets.
                    let mut at = sent.lock().unwrap_or_else(|p| p.into_inner());
                    let now = std::time::Instant::now();
                    if at.is_some_and(|t| now.duration_since(t) < DUMP_PROGRESS_EVERY) && ours.status == DumpStatus::Running {
                        return;
                    }
                    *at = Some(now);
                }
                progress(ours);
            })
            .await;
            let database = Some(export.database.as_str()).filter(|d| !d.is_empty());
            let session = dbx_core::database_export::database_export_client_session_id(&export_id);
            let _ = state.close_client_session_pool_with_completion(&export.connection_id, database, &session).await;
            dbx_core::database_export::clear_export_cancelled(&export_id).await;
            let end = result.map(|()| DumpEnd { last: last.into_inner().unwrap_or_else(|p| p.into_inner()) });
            let _ = tx.send(end.map_err(|raw| {
                let kind = if dbx_core::query::is_connection_error(&raw) { CoreErrorKind::Connect } else { CoreErrorKind::Other };
                CoreError::new(kind, scrub(&raw, &[&config.password]))
            }));
        });
        rx.await.unwrap_or_else(|_| Err(CoreError::new(CoreErrorKind::Other, "The dump stopped before it finished.")))
    }

    fn cancel_dump(&self, id: DumpId) {
        // dbx's flag is a set behind a lock; the async function never waits.
        futures::executor::block_on(dbx_core::database_export::set_export_cancelled(&dump_key(id)));
    }
}

/// How often running progress is passed on, at most.
const DUMP_PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// dbx's export id for one dump: its own per process.
fn dump_key(id: DumpId) -> String {
    format!("dbx-tui-dump-{}-{}", std::process::id(), id.0)
}

/// The columns (`schema.table.column`) in the dump's scope whose names dbx's
/// quoting would pass through unescaped: a name of two or more characters that
/// starts and ends with `"`, a backtick, or `[` and `]`. At most 6 are
/// listed. System schemas are left out. The schema is a literal made safe
/// here; the statement is a plain select, which the gate lets through.
async fn quoted_columns(state: &Arc<AppState>, connection: &str, database: &str, schema: &str, config: &ConnectionConfig) -> Result<Vec<String>, CoreError> {
    let scope = if schema.is_empty() { String::new() } else { format!(" and n.nspname = '{}'", schema.replace('\'', "''")) };
    let sql = format!(
        "select n.nspname || '.' || c.relname || '.' || a.attname \
         from pg_catalog.pg_attribute a \
         join pg_catalog.pg_class c on c.oid = a.attrelid \
         join pg_catalog.pg_namespace n on n.oid = c.relnamespace \
         where a.attnum > 0 and not a.attisdropped and c.relkind in ('r', 'p', 'v', 'm', 'f') \
         and length(a.attname) >= 2 \
         and ((left(a.attname, 1) = '\"' and right(a.attname, 1) = '\"') \
           or (left(a.attname, 1) = '`' and right(a.attname, 1) = '`') \
           or (left(a.attname, 1) = '[' and right(a.attname, 1) = ']')) \
         and n.nspname not in ('pg_catalog', 'information_schema') \
         and n.nspname not like 'pg\\_toast%' and n.nspname not like 'pg\\_temp\\_%'{scope} \
         order by 1 limit 7"
    );
    let options = QueryExecutionOptions { max_rows: Some(7), timeout_secs: Some(0), ..Default::default() };
    let result = execute_sql_statement_with_options_typed(state, connection, database, &sql, None, None, options)
        .await
        .map_err(|e| run_error(e, &sql, &config.password, None))?;
    let set = result_set(result, sql, config.db_type);
    Ok(set.rows.into_iter().filter_map(|row| match row.into_iter().next() {
        Some(Cell::Value(name)) => Some(name),
        _ => None,
    }).collect())
}

/// The refusal's words: which columns, and what to do.
fn refusal_for_quoted_columns(found: &[String], schema: &str) -> String {
    let shown: Vec<String> = found.iter().take(6).map(|n| display_safe(n)).collect();
    let more = if found.len() > 6 { " and more" } else { "" };
    let scope = if schema.is_empty() { "In the whole database" } else { "In that schema" };
    format!(
        "Not dumped: {scope} a column is named with quotes around it ({}{more}), and dbx's dump would write its name unescaped, which can break or change the statements. Rename the column, or dump a schema without it.",
        shown.join(", ")
    )
}

/// dbx's request for a dump: structure, data and the objects (views,
/// functions, sequences) that go with structure; `fail_on_error` off, as the
/// desktop's dialog has it; plain SQL, never compressed or split. Built as
/// JSON so every option this client does not set keeps dbx's own default.
fn dump_request(export_id: &str, request: &DumpRequest, database: &str) -> Result<dbx_core::database_export::DatabaseExportRequest, CoreError> {
    let value = serde_json::json!({
        "exportId": export_id,
        "connectionId": request.at.connection.0,
        "database": database,
        "schema": request.schema,
        "filePath": request.path,
        "includeStructure": request.content.structure(),
        "includeData": request.content.data(),
        "includeObjects": request.content.structure(),
        "dropTableIfExists": request.drop_tables,
        "failOnError": false,
        "preventOverwrite": request.prevent_overwrite,
        "batchSize": dbx_core::database_export::DATABASE_EXPORT_INSERT_BATCH_SIZE,
    });
    serde_json::from_value(value).map_err(|e| CoreError::new(CoreErrorKind::Other, format!("dbx rejected the dump request: {e}")))
}

fn dump_progress(p: dbx_core::database_export::ExportProgress, secret: &str) -> DumpProgress {
    use dbx_core::database_export::ExportStatus as S;
    let status = match p.status {
        S::PreparingSnapshot => DumpStatus::Preparing,
        S::Running if p.preparing => DumpStatus::Preparing,
        S::Writing => DumpStatus::Writing,
        S::Done => DumpStatus::Done,
        S::Cancelled => DumpStatus::Cancelled,
        // dbx's export never sends `Error` itself (a failure is its return
        // value); a head that relays one would.
        S::Running | S::Error => DumpStatus::Running,
    };
    DumpProgress {
        status,
        object: p.current_object,
        index: p.object_index,
        total: p.total_objects,
        rows: p.rows_exported,
        errors: p.error_count,
        first_error: p.error_summary.map(|e| scrub(&e, &[secret])),
    }
}

/// The store's entry as ours. Text another program wrote is kept whole (the
/// statement is what a user may insert); the painter replaces control
/// characters when it is drawn.
fn history_entry(e: dbx_core::history::HistoryEntry) -> HistoryEntry {
    HistoryEntry {
        id: e.id,
        connection: ConnectionId(e.connection_id),
        connection_name: e.connection_name,
        database: e.database,
        sql: e.sql,
        when: local_time(&e.executed_at),
        elapsed: Duration::from_millis(u64::try_from(e.execution_time_ms).unwrap_or(u64::MAX)),
        success: e.success,
        error: e.error.filter(|m| !m.trim().is_empty()),
        affected: e.affected_rows.and_then(|n| u64::try_from(n).ok()),
    }
}

/// An RFC 3339 time as local `2026-11-25 14:03`; text that is not one is
/// shown as it is (cut to the length of one), never hidden.
fn local_time(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(at) => at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        Err(_) => rfc3339.chars().take(16).collect(),
    }
}

/// The desktop's `primarySqlOperation`: the first keyword of the first
/// statement once comments are gone, upper-cased, else `SQL`.
fn primary_operation(sql: &str) -> String {
    let first = statements_of(sql).into_iter().next().unwrap_or_default();
    let word: String = first.chars().take_while(char::is_ascii_alphabetic).collect();
    if word.is_empty() {
        "SQL".to_string()
    } else {
        word.to_ascii_uppercase()
    }
}

/// What the desktop's history panel files a statement under: its
/// `classifySqlActivityKind` (`apps/desktop/src/lib/history/historyActivityKind.ts`),
/// on the statements with comments removed.
fn activity_kind(sql: &str) -> &'static str {
    let statements = statements_of(sql);
    let starts = |s: &str, words: &[&str]| {
        let first = s.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
        words.iter().any(|w| first.eq_ignore_ascii_case(w))
    };
    if statements.iter().any(|s| starts(s, &["create", "alter", "drop", "rename"])) {
        "schema_change"
    } else if statements.iter().any(|s| starts(s, &["insert", "update", "delete", "merge", "replace", "truncate"])) {
        "data_change"
    } else {
        "query"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn film() -> TablePath {
        TablePath {
            schema: crate::core::SchemaPath {
                database: DatabasePath { connection: ConnectionId("c".into()), database: "pagila".into() },
                schema: "public".into(),
            },
            table: "film".into(),
        }
    }

    #[test]
    fn the_page_select_for_film_on_postgres_is_dbxs_own_with_the_key_as_its_order() {
        let key = vec!["film_id".to_string()];
        assert_eq!(
            page_sql_for(&DbKind::Postgres, &film(), &key, 1000, 0),
            r#"SELECT * FROM "public"."film" ORDER BY "film_id" LIMIT 1000;"#
        );
        assert_eq!(
            page_sql_for(&DbKind::Postgres, &film(), &key, 1000, 1000),
            r#"SELECT * FROM "public"."film" ORDER BY "film_id" LIMIT 1000 OFFSET 1000;"#
        );
        // dbx orders by nothing of its own, whatever `fallback_order_columns` says.
        assert_eq!(page_sql_for(&DbKind::Postgres, &film(), &[], 1000, 0), r#"SELECT * FROM "public"."film" LIMIT 1000;"#);
        let two = vec!["a".to_string(), "Mixed Case".to_string()];
        assert_eq!(
            page_sql_for(&DbKind::Postgres, &film(), &two, 5, 0),
            r#"SELECT * FROM "public"."film" ORDER BY "a", "Mixed Case" LIMIT 5;"#
        );
    }

    fn named(schema: &str, table: &str) -> TablePath {
        let mut t = film();
        t.schema.schema = schema.into();
        t.table = table.into();
        t
    }

    #[test]
    fn a_name_with_a_quote_inside_is_doubled_in_the_page_and_count_selects() {
        let t = named("public", r#"a"b"#);
        assert_eq!(page_sql_for(&DbKind::Postgres, &t, &[], 5, 0), r#"SELECT * FROM "public"."a""b" LIMIT 5;"#);
        assert_eq!(count_sql_for(&DbKind::Postgres, &t), r#"SELECT COUNT(*) AS cnt FROM "public"."a""b""#);
    }

    #[test]
    fn a_name_that_starts_and_ends_with_a_quote_is_never_taken_as_already_quoted() {
        // dbx passes such a name through unchanged on Postgres, which would end the identifier.
        let t = named("public", r#""x"; delete from public.film; --""#);
        let want = r##""public"."""x""; delete from public.film; --""""##;
        assert_eq!(page_sql_for(&DbKind::Postgres, &t, &[], 5, 0), format!("SELECT * FROM {want} LIMIT 5;"));
        assert_eq!(count_sql_for(&DbKind::Postgres, &t), format!("SELECT COUNT(*) AS cnt FROM {want}"));
        for other in ["gaussdb", "opengauss"] {
            let kind = DbKind::Other(other.into());
            assert!(page_sql_for(&kind, &t, &[], 5, 0).contains(r##""""x""; delete from public.film; --""""##), "{other}");
            assert!(count_sql_for(&kind, &t).contains(r##""""x""; delete from public.film; --""""##), "{other}");
        }
    }

    #[test]
    fn a_schema_and_an_order_column_that_start_and_end_with_a_quote_are_quoted_whole() {
        let t = named(r#""s"; drop table t; --""#, "film");
        let order = vec![r#""id" desc; --""#.to_string()];
        assert_eq!(
            page_sql_for(&DbKind::Postgres, &t, &order, 5, 0),
            r#"SELECT * FROM """s""; drop table t; --"""."film" ORDER BY """id"" desc; --""" LIMIT 5;"#
        );
        assert_eq!(count_sql_for(&DbKind::Postgres, &t), r#"SELECT COUNT(*) AS cnt FROM """s""; drop table t; --"""."film""#);
    }

    #[test]
    fn mysql_and_sqlite_names_keep_dbxs_own_quoting() {
        let t = named("", "a`b");
        assert_eq!(page_sql_for(&DbKind::MySql, &t, &[], 5, 0), "SELECT * FROM `a``b` LIMIT 5;");
        let q = named("", r#"a"b"#);
        assert_eq!(page_sql_for(&DbKind::Sqlite, &q, &[], 5, 0), r#"SELECT * FROM "a""b" LIMIT 5;"#);
    }

    #[test]
    fn the_count_select_for_film_on_postgres_is_dbxs_own() {
        assert_eq!(count_sql_for(&DbKind::Postgres, &film()), r#"SELECT COUNT(*) AS cnt FROM "public"."film""#);
    }

    #[test]
    fn only_selects_dbx_proves_read_only_are_proven_rerunnable() {
        for sql in [
            "select 1",
            "SELECT * FROM film WHERE x = 1;",
            "  /* c */ select 1",
            "-- note\nselect 1",
            "(select 1) union (select 2)",
            "with a as (select 1) select * from a",
            "select id, upper(title) from film order by id",
            "values (1)",
            "values (1), (2)",
            "table film",
        ] {
            assert_eq!(rerun_of(sql, DatabaseType::Postgres), Rerun::Proven, "{sql:?} can be run again");
        }
    }

    #[test]
    fn a_select_that_calls_a_function_dbx_cannot_vouch_for_is_read_only_but_unproven() {
        for sql in [
            "select f()",
            "select * from f()",
            "select pg_notify('a', 'b')",
            "select set_config('x.y', '1', false)",
            "select my_func(id) from film",
        ] {
            assert_eq!(rerun_of(sql, DatabaseType::Postgres), Rerun::ReadOnlyUnproven, "{sql:?} asks first");
        }
    }

    #[test]
    fn writes_and_what_is_not_a_query_are_never_rerun() {
        for sql in [
            "update t set a = 1 returning *",
            "insert into t values (1) returning id",
            "delete from t returning *",
            "with d as (delete from t returning *) select * from d",
            "select * into newt from t",
            "select * from t for update",
            "select * from t for share",
            "select nextval('s')",
            "explain analyze delete from t",
            "show search_path",
            "create table x as select 1",
            "",
            "select 1; delete from t",
        ] {
            assert_eq!(rerun_of(sql, DatabaseType::Postgres), Rerun::Write, "{sql:?} must not be run again");
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| owned.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    fn request(buffer: &str, row: usize, col: usize) -> RunRequest {
        RunRequest {
            connection: ConnectionId("c".into()),
            database: Some("d".into()),
            session: SessionId { tab: TabId(1), serial: 1 },
            buffer: buffer.into(),
            cursor: Cursor { row, col },
            selection: None,
            row_cap: 1000,
            confirm_dangerous: true,
            confirmation: None,
        }
    }

    /// What a statement is, ignoring the spaces and `;` around it.
    fn bare(s: &str) -> &str {
        s.trim().trim_end_matches(';').trim()
    }

    #[test]
    fn the_statement_under_the_cursor_runs_not_the_whole_buffer() {
        let buffer = "select 1;\nselect 2\n  from film;\nselect 3;";
        for (row, col) in [(1, 0), (1, 4), (2, 5)] {
            let ran = statement_to_run(&request(buffer, row, col), DatabaseType::Postgres);
            assert_eq!(bare(&ran), "select 2\n  from film", "cursor at {row},{col}");
        }
        assert_eq!(bare(&statement_to_run(&request(buffer, 0, 3), DatabaseType::Postgres)), "select 1");
        assert_eq!(bare(&statement_to_run(&request(buffer, 3, 9), DatabaseType::Postgres)), "select 3", "at the very end");
    }

    #[test]
    fn a_cursor_after_a_multi_byte_character_still_picks_the_statement_it_is_in() {
        // Counted in bytes, the cursor at the end of the first line would land
        // inside the second statement; counted in characters, too early.
        let buffer = "select '😀😀😀😀', 'éé';\nselect 2;";
        let first = statement_to_run(&request(buffer, 0, 20), DatabaseType::Postgres);
        assert_eq!(bare(&first), "select '😀😀😀😀', 'éé'");
        let second = statement_to_run(&request(buffer, 1, 2), DatabaseType::Postgres);
        assert_eq!(bare(&second), "select 2");
    }

    #[test]
    fn a_selection_runs_exactly_the_selected_text() {
        let mut r = request("select 1;\nselect title from film;", 1, 0);
        r.selection = Some((Cursor { row: 1, col: 0 }, Cursor { row: 1, col: 12 }));
        assert_eq!(statement_to_run(&r, DatabaseType::Postgres), "select title");
    }

    fn query_result(value: serde_json::Value) -> dbx_core::db::QueryResult {
        serde_json::from_value(value).expect("a QueryResult")
    }

    #[test]
    fn cells_are_null_or_exact_text_and_kinds_come_from_the_database_type_names() {
        let r = query_result(serde_json::json!({
            "columns": ["id", "big", "exact", "approx", "flag", "label", "at", "doc", "bin", "uid"],
            "column_types": ["int4", "int8", "numeric", "float8", "bool", "text", "timestamptz", "jsonb", "bytea", "uuid"],
            "rows": [
                [1, "9007199254740993", "12345678901234567890.123456789", 0.1, true, "NULL", "2024-03-05T11:45:12+00:00", "{\"a\": 1}", "\\xdead", "a0ee"],
                ["2", null, null, 0.30000000000000004, false, "", null, null, null, null]
            ],
            "affected_rows": 0,
            "execution_time_ms": 12,
            "truncated": true,
            "messages": [{"severity": "NOTICE", "message": "hello"}]
        }));
        let set = result_set(r, "select ...".into(), DatabaseType::Postgres);
        let kinds: Vec<CellKind> = set.columns.iter().map(|c| c.kind).collect();
        use CellKind::*;
        assert_eq!(kinds, vec![Integer, Integer, Decimal, Float, Bool, Text, Temporal, Json, Binary, Other]);
        assert_eq!(set.columns[1].name, "big");
        assert_eq!(set.columns[1].db_type, "int8");
        let v = |s: &str| Cell::Value(s.into());
        assert_eq!(
            set.rows[0],
            vec![
                v("1"),
                v("9007199254740993"),
                v("12345678901234567890.123456789"),
                v("0.1"),
                v("true"),
                v("NULL"),
                v("2024-03-05T11:45:12+00:00"),
                v("{\"a\": 1}"),
                v("\\xdead"),
                v("a0ee")
            ]
        );
        assert_eq!(set.rows[1][0], v("2"), "an integer that came as a string is the same text");
        assert_eq!(set.rows[1][1], Cell::Null);
        assert_eq!(set.rows[1][3], v("0.30000000000000004"));
        assert_eq!(set.rows[1][5], v(""), "the empty string is not NULL");
        assert!(set.truncated);
        assert_eq!(set.elapsed, std::time::Duration::from_millis(12));
        assert_eq!(set.messages, vec!["NOTICE: hello".to_string()]);
        assert_eq!(set.ran, "select ...");
    }

    #[test]
    fn without_column_types_every_kind_is_other_whatever_json_type_the_cells_have() {
        // dbx's JSON types change with its protocol path, so they say nothing.
        let r = query_result(serde_json::json!({
            "columns": ["n", "s", "b", "nothing"],
            "column_types": [],
            "rows": [[null, "x", true, null], [3, "y", false, null]],
            "affected_rows": 0,
            "execution_time_ms": 1
        }));
        let set = result_set(r, String::new(), DatabaseType::Postgres);
        assert!(set.columns.iter().all(|c| c.kind == CellKind::Other && c.db_type.is_empty()), "{:?}", set.columns);
        assert_eq!(set.rows[1][0], Cell::Value("3".into()));
        // Types for only some columns: the ones without are `Other`.
        let r = query_result(serde_json::json!({
            "columns": ["a", "b"], "column_types": ["int4"], "rows": [[1, 2]], "affected_rows": 0, "execution_time_ms": 1
        }));
        let kinds: Vec<CellKind> = result_set(r, String::new(), DatabaseType::Postgres).columns.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![CellKind::Integer, CellKind::Other]);
    }

    #[test]
    fn a_bit_string_is_not_a_boolean() {
        for t in ["bit", "varbit", "bit(3)", "bit varying"] {
            assert_eq!(kind_of_type(t), CellKind::Other, "{t}");
        }
        assert_eq!(kind_of_type("bool"), CellKind::Bool);
        assert_eq!(kind_of_type("boolean"), CellKind::Bool);
    }

    #[test]
    fn a_statement_without_a_result_set_reports_the_rows_it_changed() {
        let r = query_result(serde_json::json!({ "columns": [], "rows": [], "affected_rows": 3, "execution_time_ms": 4 }));
        let set = result_set(r, "delete from t".into(), DatabaseType::Postgres);
        assert!(set.columns.is_empty());
        assert_eq!(set.affected, 3);
    }

    #[test]
    fn a_sql_error_keeps_dbxs_position_and_the_statement_and_drops_the_omitted_sql_sentence() {
        let position = dbx_core::sql_error_position::SqlErrorPosition { line: 2, column: 8, offset: 17 };
        let raw = "ERROR: syntax error at or near \"form\"\nSQL text omitted from user-facing error; enable debug SQL diagnostics to inspect the original statement.";
        let e = run_error(QueryExecutionError::SqlWithPosition { message: raw.into(), position }, "select 1\nselect form x", "", None);
        assert_eq!(e.kind, CoreErrorKind::Sql);
        assert_eq!(e.message, "ERROR: syntax error at or near \"form\"");
        assert_eq!(e.position, Some(SqlPosition { line: 2, column: 8 }));
        assert_eq!(e.statement.as_deref(), Some("select 1\nselect form x"));

        let plain = run_error(QueryExecutionError::Sql("relation \"nope\" does not exist".into()), "select * from nope", "", None);
        assert_eq!((plain.kind, plain.position), (CoreErrorKind::Sql, None));
        assert_eq!(plain.statement.as_deref(), Some("select * from nope"));
    }

    #[test]
    fn timeouts_cancels_and_lost_connections_keep_their_kind_and_never_show_the_password() {
        assert_eq!(run_error(QueryExecutionError::Timeout("Query timed out after 30s".into()), "s", "", None).kind, CoreErrorKind::Timeout);
        let cancelled = QueryExecutionError::Canceled {
            stage: dbx_core::db::agent_driver::AgentErrorStage::Execute,
            operation_outcome: dbx_core::db::agent_driver::AgentOperationOutcome::Unknown,
        };
        assert_eq!(run_error(cancelled, "s", "", None).kind, CoreErrorKind::Cancelled);
        let lost = run_error(QueryExecutionError::Legacy("PostgreSQL connection failed: Connection refused (hunter2)".into()), "s", "hunter2", None);
        assert_eq!(lost.kind, CoreErrorKind::Connect);
        assert!(!lost.message.contains("hunter2") && !format!("{lost:?}").contains("hunter2"), "{lost:?}");
        let other = run_error(QueryExecutionError::Legacy("something odd\u{1b}[31m".into()), "s", "", None);
        assert_eq!(other.kind, CoreErrorKind::Other);
        assert!(!other.message.chars().any(char::is_control), "{other:?}");
    }

    #[test]
    fn a_run_has_no_time_limit_unless_the_connection_sets_its_own() {
        let mut config = config_from_draft(
            &ConnectionDraft {
                id: None,
                name: "c".into(),
                kind: DbKind::Postgres,
                host: "h".into(),
                port: None,
                database: None,
                username: "u".into(),
                password: None,
                tls: false,
                production: false,
            },
            None,
        )
        .unwrap();
        assert_eq!(config.query_timeout_secs, 60, "dbx's default for a connection that never set one");
        assert_eq!(query_timeout(&config), None, "dbx's default is not a choice the user made");
        config.query_timeout_secs = 1;
        assert_eq!(query_timeout(&config), Some(1));
        config.query_timeout_secs = 600;
        assert_eq!(query_timeout(&config), Some(600));
        config.query_timeout_secs = 0;
        assert_eq!(query_timeout(&config), None, "0 is the desktop app's 'no limit'");
    }

    #[test]
    fn a_timeout_says_which_limit_applied_and_where_it_is_set() {
        let e = run_error(QueryExecutionError::Timeout("Query timed out after 1s".into()), "select pg_sleep(5)", "", Some(1));
        assert_eq!(e.kind, CoreErrorKind::Timeout);
        assert!(e.message.contains("1 s") && e.message.contains("query timeout") && e.message.contains("connection"), "{}", e.message);
        let e = run_error(QueryExecutionError::Timeout("Pool checkout timed out".into()), "s", "", None);
        assert!(e.message.contains("Pool checkout timed out"), "dbx's own words when no limit of ours applied: {}", e.message);
    }

    #[test]
    fn dbx_data_dir_wins_and_otherwise_the_store_is_the_desktop_apps() {
        let set = StoreOptions::from_vars(vars(&[("DBX_DATA_DIR", "/tmp/scratch"), ("HOME", "/Users/x")])).unwrap();
        assert_eq!(set.data_dir, PathBuf::from("/tmp/scratch"));
        assert!(!set.create_if_missing, "an explicit directory must already exist");
        let empty = StoreOptions::from_vars(vars(&[("DBX_DATA_DIR", ""), ("HOME", "/Users/x")])).unwrap();
        let default = StoreOptions::from_vars(vars(&[("HOME", "/Users/x")])).unwrap();
        assert_eq!(empty, default, "an empty DBX_DATA_DIR counts as unset");
        assert!(default.create_if_missing, "a fresh machine gets its store");
        if cfg!(target_os = "macos") {
            assert_eq!(default.data_dir, PathBuf::from("/Users/x/Library/Application Support/com.dbx.app"));
        } else {
            assert_eq!(default.data_dir, PathBuf::from("/Users/x/.local/share/com.dbx.app"));
        }
        let err = StoreOptions::from_vars(vars(&[])).unwrap_err();
        assert_eq!(err.kind, CoreErrorKind::Store);
        assert!(err.message.contains("DBX_DATA_DIR"), "{}", err.message);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn seeding_is_refused_outside_the_temp_dir_unless_allowed_and_always_in_the_desktop_store() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let scratch = tmp.path().join("store");
        assert!(check_seed_dir(&scratch, Some(&home), false).is_ok(), "a temp dir is fine");
        let elsewhere = PathBuf::from("/Users/someone/work/store");
        assert!(check_seed_dir(&elsewhere, Some(&home), false).is_err(), "outside temp needs the override");
        assert!(check_seed_dir(&elsewhere, Some(&home), true).is_ok(), "DBX_TUI_ALLOW_SEED=1");
        let desktop = desktop_data_dir(Some(home.clone()), None).unwrap();
        for allow in [false, true] {
            let err = check_seed_dir(&desktop, Some(&home), allow).unwrap_err();
            assert!(err.contains("desktop"), "{err}");
        }
        let real = desktop_data_dir(Some(PathBuf::from("/Users/x")), None).unwrap();
        assert!(check_seed_dir(&real, Some(Path::new("/Users/x")), true).is_err());
        // Even when HOME points elsewhere, a desktop-named directory is refused.
        assert!(check_seed_dir(&real, Some(&home), true).is_err(), "HOME moved, override set");
    }

    #[test]
    fn system_schemas_are_told_from_the_users_even_when_dbx_lists_them() {
        for hidden in ["pg_catalog", "information_schema", "pg_toast", "pg_toast_temp_1", "pg_temp_3"] {
            assert!(is_system_schema(hidden), "{hidden}");
        }
        // Lookalikes are the user's: matching is exact, or by the toast and temp prefixes.
        for shown in ["public", "pg_catalog2", "my_information_schema", "pg_tools", "Pg_Catalog", "other"] {
            assert!(!is_system_schema(shown), "{shown}");
        }
    }

    #[test]
    fn dbxs_relation_types_map_to_our_kinds() {
        for (dbx, kind) in [
            ("BASE TABLE", TableKind::Table),
            ("TABLE", TableKind::Table),
            ("VIEW", TableKind::View),
            ("MATERIALIZED_VIEW", TableKind::MaterializedView),
            ("FOREIGN TABLE", TableKind::Foreign),
            ("something new", TableKind::Table),
        ] {
            assert_eq!(table_kind(dbx), kind, "{dbx}");
        }
    }

    #[test]
    fn a_metadata_failure_hides_the_password_and_connection_errors_are_told_from_others() {
        let e = metadata_error("PostgreSQL connection failed: password authentication failed (hunter2)\u{1b}[31m", "hunter2");
        assert!(!e.message.contains("hunter2") && !e.message.chars().any(char::is_control), "{e:?}");
        assert_eq!(e.kind, CoreErrorKind::Connect);
        assert_eq!(metadata_error("relation \"x\" does not exist", "").kind, CoreErrorKind::Other);
    }

    #[test]
    fn a_password_with_a_control_character_in_it_is_still_hidden() {
        let shown = scrub("auth failed for pw\u{1b}[31mx and then pw\u{1b}[31mx again", &["pw\u{1b}[31mx"]);
        assert!(!shown.contains("pw") && !shown.contains('['), "{shown}");
        assert_eq!(shown, "auth failed for •••• and then •••• again");
    }

    #[test]
    fn a_connection_failure_message_never_carries_the_password_and_never_runs_long() {
        let raw = "error connecting to postgres://u:hunter2@db:5432/x: password authentication failed (hunter2)";
        let shown = scrub(raw, &["hunter2", ""]);
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("password authentication failed"), "the reason stays: {shown}");
        assert_eq!(scrub("  spaces trimmed \n", &[]), "spaces trimmed");
        let hostile = scrub("bad \u{1b}]52;c;AAAA\u{7} text \u{9b}", &[]);
        assert!(!hostile.chars().any(char::is_control), "{hostile:?}");
        assert!(hostile.contains("bad") && hostile.contains("text"));
        let long = scrub(&"x".repeat(5_000), &[]);
        assert_eq!(long.chars().count(), 600);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn key_advice_depends_on_the_error_and_on_where_the_key_came_from() {
        let file = advice("SECRET_KEY_MISMATCH", KeySource::EnvFile);
        assert!(file.contains("DBX_SECRET_KEY_FILE") && !file.contains("keychain"), "{file}");
        let value = advice("SECRET_KEY_INVALID", KeySource::EnvValue);
        assert!(value.contains("DBX_SECRET_KEY") && !value.contains("DBX_SECRET_KEY_FILE"), "{value}");
        let unreadable = advice("KEY_FILE_UNAVAILABLE", KeySource::EnvFile);
        assert!(unreadable.contains("read"), "{unreadable}");
        let platform = advice("KEYRING_ACCESS_FAILED", KeySource::Platform);
        if cfg!(target_os = "macos") {
            assert!(platform.contains("Unlock your login keychain"), "{platform}");
        } else {
            assert!(platform.contains("Secret Service") && !platform.contains("login keychain"), "{platform}");
        }
        let wrong_platform_key = advice("SECRET_KEY_MISMATCH", KeySource::Platform);
        assert!(!wrong_platform_key.contains("Unlock"), "a wrong key is not a locked one: {wrong_platform_key}");
        assert!(wrong_platform_key.contains("DBX_SECRET_KEY_FILE"), "{wrong_platform_key}");
    }

    #[test]
    fn dbx_key_failures_are_key_errors_and_other_store_failures_are_not() {
        for raw in ["SECRET_KEY_MISMATCH", "KEYRING_ACCESS_FAILED: user canceled", "secret decryption failed"] {
            let e = classify(raw, KeySource::EnvFile).unwrap_or_else(|| panic!("{raw}"));
            assert_eq!(e.kind, CoreErrorKind::Key, "{raw}");
            assert!(e.message.contains("DBX_SECRET_KEY_FILE"), "{}", e.message);
        }
        assert!(classify("database is locked", KeySource::Platform).is_none());
    }

    #[test]
    fn dbxs_type_names_for_postgres_include_the_ones_a_create_table_uses() {
        let names: Vec<String> = type_names_for(&DbKind::Postgres).iter().map(|n| n.to_lowercase()).collect();
        for wanted in ["varchar", "timestamptz", "integer", "text", "jsonb", "uuid"] {
            assert!(names.iter().any(|n| n == wanted), "{wanted} is missing from {names:?}");
        }
        let mysql: Vec<String> = type_names_for(&DbKind::MySql).iter().map(|n| n.to_lowercase()).collect();
        assert!(mysql.iter().any(|n| n == "varchar"), "{mysql:?}");
        assert!(!mysql.iter().any(|n| n == "timestamptz"), "MySQL has no timestamptz: {mysql:?}");
    }

    /// Every caret position of `buffer`, as the editor can put it.
    fn carets(buffer: &str) -> Vec<Cursor> {
        buffer.split('\n').enumerate().flat_map(|(row, line)| (0..=line.chars().count()).map(move |col| Cursor { row, col })).collect()
    }

    #[test]
    fn the_outlined_statement_is_the_statement_a_run_sends_at_every_caret_position() {
        let buffers = [
            "select 1;\nselect 2\n  from film;\nselect 3;",
            "select '😀😀', 'éé';\nselect 2;\n\n\ncreate table t (a int);\ninsert into t values (1);",
            "select 1;\nselect 1;\nselect 2;\nselect 1;",
            "select $$a;b$$;\nselect 'x;y', 2 -- c;\n;select 3",
            "",
            "   \n  ",
        ];
        let (mut placed, mut duplicated) = (0, 0);
        for buffer in buffers {
            for cursor in carets(buffer) {
                let sent = statement_to_run(&RunRequest { cursor, ..request(buffer, 0, 0) }, DatabaseType::Postgres);
                let Some((a, b)) = statement_range_for(&DbKind::Postgres, buffer, cursor) else { continue };
                placed += 1;
                assert_eq!(text_between(buffer, a, b), sent.trim(), "{buffer:?} caret {cursor:?}");
                // The right copy, when the statement is duplicated: the one
                // the caret is in or at the edge of (else no outline at all).
                if buffer.matches(sent.trim()).count() > 1 {
                    let key = |c: Cursor| (c.row, c.col);
                    assert!(key(a) <= key(cursor) && key(cursor) <= key(b), "{buffer:?} caret {cursor:?} outside its outline {a:?}..{b:?}");
                    duplicated += 1;
                }
            }
        }
        assert!(placed > 100, "only {placed} positions were outlined");
        assert!(duplicated > 5, "only {duplicated} positions of duplicated statements were outlined");
    }

    #[test]
    fn a_statement_that_is_not_duplicated_is_outlined_at_every_caret_position() {
        let buffer = "select 1;\nselect 2\n  from film;\n\nselect 3;";
        for cursor in carets(buffer) {
            assert!(statement_range_for(&DbKind::Postgres, buffer, cursor).is_some(), "no outline at {cursor:?}");
        }
        assert_eq!(statement_range_for(&DbKind::Postgres, buffer, Cursor { row: 2, col: 3 }), Some((Cursor { row: 1, col: 0 }, Cursor { row: 2, col: 11 })), "dbx's text leaves the `;` out");
    }

    #[test]
    fn an_empty_buffer_or_an_unknown_kind_has_no_outline() {
        assert_eq!(statement_range_for(&DbKind::Postgres, "", Cursor::default()), None);
        assert_eq!(statement_range_for(&DbKind::Postgres, "  \n ", Cursor::default()), None);
        assert_eq!(statement_range_for(&DbKind::Other("no-such-database".into()), "select 1", Cursor::default()), None);
    }

    #[test]
    fn dbxs_analysis_names_the_tables_and_aliases_of_a_statement_that_parses_and_says_so_for_one_that_does_not() {
        let sql = "select f.title\nfrom film f join public.film_actor as fa on fa.film_id = f.film_id";
        let refs = references_for(&DbKind::Postgres, sql).expect("it parses");
        let tables: Vec<(&str, Option<&str>, Option<&str>)> = refs.tables.iter().map(|t| (t.name.as_str(), t.schema.as_deref(), t.alias.as_deref())).collect();
        assert_eq!(tables, [("film", None, Some("f")), ("film_actor", Some("public"), Some("fa"))]);
        // Spans are in the rows and columns of the text, from 0, the end after the last character.
        assert_eq!((refs.tables[0].span.start, refs.tables[0].span.end), (Cursor { row: 1, col: 5 }, Cursor { row: 1, col: 9 }));
        assert!(refs.columns.iter().any(|c| c.name == "title" && c.qualifier.as_deref() == Some("f")), "{:?}", refs.columns);
        assert!(refs.scopes.iter().any(|(_, parent)| parent.is_none()));
        assert!(references_for(&DbKind::Postgres, "select a from film f join").is_err(), "a half-typed statement does not parse");
    }

    // ---- the gate (slice 17) ---------------------------------------------------

    /// A Postgres connection config: marked production or not, with these
    /// databases marked, and maybe read-only.
    fn config(production: bool, marked: &[&str], read_only: bool) -> ConnectionConfig {
        let value = serde_json::json!({
            "id": "c", "name": "prod pg", "db_type": "postgres", "host": "h", "port": 5432,
            "username": "u", "password": "", "database": "edge", "ssl": false, "driver_profile": null,
            "is_production": production, "production_databases": marked, "read_only": read_only,
        });
        serde_json::from_value(value).expect("a config")
    }

    fn production() -> ConnectionConfig {
        config(true, &[], false)
    }

    fn plain() -> ConnectionConfig {
        config(false, &[], false)
    }

    /// The reasons asked about, or `None` when it runs.
    fn asked(config: &ConnectionConfig, database: &str, sql: &str, setting: bool) -> Option<(bool, Vec<Reason>)> {
        assess(config, database, sql, setting, false).map(|v| (v.production, v.reasons))
    }

    #[test]
    fn a_proven_select_runs_on_production() {
        assert_eq!(asked(&production(), "edge", "select 1", true), None);
        assert_eq!(asked(&production(), "edge", "select * from t where id = 1", false), None);
    }

    #[test]
    fn a_select_calling_a_function_dbx_cannot_prove_is_confirmed_on_production_only() {
        // A function dbx knows nothing of: read-only to its classifier, not proven.
        assert_eq!(asked(&production(), "edge", "select my_func(1)", true), Some((true, vec![Reason::Unproven])));
        assert_eq!(asked(&plain(), "edge", "select my_func(1)", true), None);
    }

    #[test]
    fn dbx_calls_terminating_a_backend_a_dangerous_write() {
        // The slice expected "functions not proven"; dbx knows this one and says more.
        let terminate = "select pg_terminate_backend(1)";
        assert_eq!(asked(&production(), "edge", terminate, false), Some((true, vec![Reason::Writes, Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", terminate, true), Some((false, vec![Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", terminate, false), None);
    }

    #[test]
    fn a_scoped_delete_is_confirmed_on_production_and_runs_elsewhere() {
        assert_eq!(asked(&production(), "edge", "delete from t where id = 1", true), Some((true, vec![Reason::Writes])));
        assert_eq!(asked(&production(), "edge", "delete from t where id = 1", false), Some((true, vec![Reason::Writes])), "no setting turns it off");
        assert_eq!(asked(&plain(), "edge", "delete from t where id = 1", true), None);
    }

    #[test]
    fn an_unscoped_delete_outside_production_follows_the_setting() {
        assert_eq!(asked(&plain(), "edge", "delete from t", true), Some((false, vec![Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "delete from t", false), None);
        // On production it is asked either way, and says both.
        assert_eq!(asked(&production(), "edge", "delete from t", false), Some((true, vec![Reason::Writes, Reason::Dangerous])));
    }

    #[test]
    fn ddl_outside_production_is_dangerous() {
        assert_eq!(asked(&plain(), "edge", "drop table t", true), Some((false, vec![Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "create table t (a int)", true), Some((false, vec![Reason::Dangerous])), "unlike the desktop's regex");
        assert_eq!(asked(&production(), "edge", "drop table t", true), Some((true, vec![Reason::ChangesSchema, Reason::Dangerous])));
    }

    #[test]
    fn transaction_control_is_confirmed_on_production() {
        // dbx also flags transaction control dangerous, so outside production
        // it follows the setting.
        assert_eq!(asked(&production(), "edge", "commit", false), Some((true, vec![Reason::TransactionControl, Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "commit", true), Some((false, vec![Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "commit", false), None);
    }

    #[test]
    fn dbx_flags_set_dangerous_so_it_follows_the_setting_outside_production() {
        let set = "set search_path to public";
        assert_eq!(asked(&plain(), "edge", set, true).map(|(p, r)| (p, r.contains(&Reason::Dangerous))), Some((false, true)));
        assert_eq!(asked(&plain(), "edge", set, false), None);
    }

    #[test]
    fn what_dbx_cannot_parse_is_a_write() {
        // dbx's classifier falls back to keywords rather than fail, and
        // calls it a write; its danger test fails closed.
        assert_eq!(asked(&production(), "edge", "delet from t", false), Some((true, vec![Reason::Writes, Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "delet from t", true), Some((false, vec![Reason::Dangerous])));
    }

    #[test]
    fn a_database_marked_production_is_confirmed_only_when_it_is_the_target() {
        let marked = config(false, &["edge"], false);
        let sql = "update t set a = 1 where id = 1";
        assert_eq!(asked(&marked, "edge", sql, true), Some((true, vec![Reason::Writes])));
        assert_eq!(asked(&marked, "pagila", sql, true), None);
    }

    #[test]
    fn a_marked_database_named_in_the_sql_is_named_in_the_verdict() {
        // On Postgres `edge.t` is a schema: a statement reaches only the
        // database it runs on.
        let marked = config(false, &["edge", "other"], false);
        assert_eq!(asked(&marked, "pagila", "delete from edge.t where id = 1", true), None);
        // On MySQL it is a database.
        let mut mysql = marked.clone();
        mysql.db_type = DatabaseType::Mysql;
        let v = assess(&mysql, "staging", "delete from edge.t where id = 1", true, false).expect("asked");
        assert!(v.production);
        assert_eq!((v.named, v.target_unknown), (vec!["edge".to_string()], false));
        assert_eq!(asked(&mysql, "staging", "delete from staging.t where id = 1", true), None);
    }

    #[test]
    fn generated_browse_selects_run_on_production() {
        let key = vec!["film_id".to_string()];
        assert_eq!(asked(&production(), "pagila", &page_sql_for(&DbKind::Postgres, &film(), &key, 1000, 1000), true), None);
        assert_eq!(asked(&production(), "pagila", &count_sql_for(&DbKind::Postgres, &film()), true), None);
    }

    #[test]
    fn a_write_on_a_read_only_connection_is_left_to_dbx_to_refuse() {
        assert_eq!(asked(&config(true, &[], true), "edge", "delete from t where id = 1", true), None);
        assert_eq!(asked(&config(false, &[], true), "edge", "drop table t", true), None);
    }

    fn mysql(production: bool, marked: &[&str]) -> ConnectionConfig {
        let mut c = config(production, marked, false);
        c.db_type = DatabaseType::Mysql;
        c
    }

    fn asked_after_use(config: &ConnectionConfig, database: &str, sql: &str) -> Option<(bool, Vec<Reason>)> {
        assess(config, database, sql, true, true).map(|v| (v.production, v.reasons))
    }

    #[test]
    fn a_use_names_what_it_does_not_that_it_writes() {
        let c = mysql(false, &["prod"]);
        assert_eq!(asked(&c, "staging", "use prod", true), Some((true, vec![Reason::SwitchesDatabase])));
        assert_eq!(asked(&c, "staging", "USE `prod`;", true), Some((true, vec![Reason::SwitchesDatabase])));
        // Into a database not marked, nothing is asked: the session is tracked instead.
        assert_eq!(asked(&c, "staging", "use other", true), None);
        // With more after it, the rest is judged as well.
        let both = asked(&c, "staging", "use prod; delete from t where id = 1", true).expect("asked");
        assert!(both.1.contains(&Reason::SwitchesDatabase) && both.1.contains(&Reason::Writes), "{both:?}");
    }

    #[test]
    fn in_a_session_that_switched_database_anything_not_proven_is_aimed_at_production() {
        let c = mysql(false, &["prod"]);
        let sql = "delete from t where id = 1";
        assert_eq!(asked(&c, "staging", sql, true), None, "before any USE, it is judged on the tab's database");
        assert_eq!(asked_after_use(&c, "staging", sql), Some((true, vec![Reason::Writes, Reason::SessionSwitched])));
        assert_eq!(asked_after_use(&c, "staging", "select 1"), None, "a proven read is not held up");
        assert_eq!(asked_after_use(&c, "staging", "select my_func(1)"), Some((true, vec![Reason::Unproven, Reason::SessionSwitched])));
        assert_eq!(asked_after_use(&c, "staging", "commit").map(|a| a.0), Some(true));
        // A connection with no production database has nothing to be moved onto.
        assert_eq!(asked_after_use(&mysql(false, &[]), "staging", sql), None);
        // A connection marked production is production whatever the session did.
        assert_eq!(asked_after_use(&mysql(true, &[]), "staging", sql), Some((true, vec![Reason::Writes])));
    }

    #[test]
    fn what_switches_the_database_is_seen_in_any_statement_of_the_text() {
        for yes in ["use prod", "USE prod", "  use `prod`", "/* hi */ use prod", "-- x\nuse prod", "select 1; use prod", "select 1;\n  Use [prod]", "/*!40101 use prod */"] {
            assert!(switches_database(yes), "{yes}");
        }
        for no in ["select 1", "select 'use prod'", "-- use prod\nselect 1", "update t set a = 1", "select * from users", "useless", "select 1; select 2"] {
            assert!(!switches_database(no), "{no}");
        }
    }

    #[test]
    fn a_session_is_marked_when_a_switch_is_sent_and_stays_marked_until_it_closes() {
        let (a, b) = (SessionId { tab: TabId(1), serial: 1 }, SessionId { tab: TabId(1), serial: 2 });
        let marks = SwitchedSessions::default();
        marks.note(a, "select 1");
        assert!(!marks.contains(a));
        marks.note(a, "select 1; use prod");
        marks.note(a, "select 2");
        assert!(marks.contains(a), "stays");
        assert!(!marks.contains(b), "another session of the tab is unaffected");
        marks.forget(a);
        assert!(!marks.contains(a));
    }

    #[test]
    fn the_gate_asks_for_a_write_in_a_switched_session_and_a_yes_for_it_runs() {
        let c = mysql(false, &["prod"]);
        let sql = "delete from t where id = 1";
        assert_eq!(gate(&c, &whole(sql), sql, "staging", false), Ok(()));
        let a = gate(&c, &whole(sql), sql, "staging", true).expect_err("asked");
        assert!(a.production);
        assert_eq!(a.reasons, [Reason::Writes, Reason::SessionSwitched]);
    }

    #[test]
    fn explain_analyze_is_judged_as_the_statement_it_runs() {
        for form in [
            "explain analyze delete from t where id = 1",
            "explain (analyze) delete from t where id = 1",
            "explain (analyze true, verbose) delete from t where id = 1",
            "explain (format json, analyze on) delete from t where id = 1",
            "explain analyze verbose delete from t where id = 1",
            "/* c */ explain analyze delete from t where id = 1",
        ] {
            assert_eq!(asked(&production(), "edge", form, true), Some((true, vec![Reason::ExplainRuns, Reason::Writes])), "{form}");
            assert_eq!(asked(&plain(), "edge", form, true), None, "scoped, not production: {form}");
        }
        // dbx's parser does not take the ANALYSE spelling, so it also calls it dangerous.
        assert_eq!(
            asked(&production(), "edge", "EXPLAIN ANALYSE delete from t where id = 1", true),
            Some((true, vec![Reason::ExplainRuns, Reason::Writes, Reason::Dangerous]))
        );
        // Not production: the inner statement's danger counts.
        assert_eq!(asked(&plain(), "edge", "explain analyze delete from t", true), Some((false, vec![Reason::ExplainRuns, Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "explain (analyze) drop table t", true), Some((false, vec![Reason::ExplainRuns, Reason::Dangerous])));
        assert_eq!(asked(&plain(), "edge", "explain analyze delete from t", false), None, "the setting still turns it off");
    }

    #[test]
    fn explain_without_analyze_and_explain_analyze_of_a_read_run() {
        for form in [
            "explain delete from t where id = 1",
            "explain (analyze false) delete from t",
            "explain (analyze off, verbose) delete from t",
            "explain (verbose) delete from t",
            "explain analyze select 1",
            "explain (analyze) select * from t where id = 1",
        ] {
            assert_eq!(asked(&plain(), "edge", form, true), None, "{form}");
        }
        assert_eq!(asked(&production(), "edge", "explain (analyze) select 1", true), None);
        assert_eq!(asked(&production(), "edge", "explain analyze select my_func(1)", true).map(|a| a.1), Some(vec![Reason::ExplainRuns, Reason::Unproven]));
        assert_eq!(explain_analyze_inner("explain (analyze) delete from t").as_deref(), Some("delete from t"));
        assert_eq!(explain_analyze_inner("explain delete from t"), None);
    }

    fn whole(sql: &str) -> RunRequest {
        let end = Cursor { row: 0, col: sql.chars().count() };
        RunRequest { connection: ConnectionId("c".into()), database: Some("edge".into()), selection: Some((Cursor::default(), end)), ..request(sql, 0, 0) }
    }

    fn yes(sql: &str) -> Confirmation {
        Confirmation { statement: sql.into(), connection: ConnectionId("c".into()), database: "edge".into(), session: SessionId { tab: TabId(1), serial: 1 } }
    }

    #[test]
    fn the_gate_refuses_without_a_yes_and_names_what_a_yes_must_send() {
        let sql = "delete from t where id = 1";
        let a = gate(&production(), &whole(sql), sql, "edge", false).expect_err("asked");
        assert!(a.production);
        assert_eq!(a.reasons, [Reason::Writes]);
        assert_eq!(a.connection_name, "prod pg");
        assert_eq!(a.confirmation, yes(sql));
        assert_eq!(gate(&production(), &whole("select 1"), "select 1", "edge", false), Ok(()));
    }

    #[test]
    fn a_yes_counts_only_for_the_exact_statement_connection_database_and_session() {
        let sql = "delete from t where id = 1";
        let sent = |confirmation: Confirmation, statement: &str| {
            let request = RunRequest { confirmation: Some(confirmation), ..whole(statement) };
            gate(&production(), &request, statement, "edge", false)
        };
        assert_eq!(sent(yes(sql), sql), Ok(()));
        let other = "delete from t where id = 2";
        let a = sent(yes(sql), other).expect_err("a yes for id = 1 does not run id = 2");
        assert!(a.reasons.contains(&Reason::Changed), "{:?}", a.reasons);
        assert_eq!(a.confirmation.statement, other);
        let moved = Confirmation { session: SessionId { tab: TabId(1), serial: 2 }, ..yes(sql) };
        assert!(sent(moved, sql).is_err(), "another session");
        assert!(sent(Confirmation { database: "pagila".into(), ..yes(sql) }, sql).is_err(), "another database");
        assert!(sent(Confirmation { connection: ConnectionId("d".into()), ..yes(sql) }, sql).is_err(), "another connection");
        // A stale yes asks again even where no yes would be needed.
        let a = sent(yes(sql), "select 1").expect_err("asks again");
        assert_eq!(a.reasons, [Reason::Changed]);
    }

    // ---- slice 19 review: names in the SQL INSERT export -------------------------

    fn insert_request(kind: DbKind, columns: &[(&str, &str)], table: &str, schema: &str) -> ExportRequest {
        ExportRequest {
            format: ExportFormat::SqlInsert,
            columns: columns.iter().map(|(n, t)| Column { name: (*n).into(), db_type: (*t).into(), kind: CellKind::Text }).collect(),
            rows: vec![columns.iter().map(|_| Cell::Value("v".into())).collect()],
            kind: Some(kind),
            table: table.into(),
            schema: schema.into(),
            null_empty: false,
        }
    }

    fn insert_text(request: &ExportRequest) -> String {
        String::from_utf8(export_bytes_for(request).expect("built")).expect("UTF-8")
    }

    #[test]
    fn insert_names_that_start_and_end_with_a_quote_are_quoted_whole_on_the_kinds_dbx_passes_them_through() {
        let hostile = r#""x"; drop table t; --""#;
        for kind in [DbKind::Postgres, DbKind::Other("gaussdb".into()), DbKind::Other("opengauss".into())] {
            let request = insert_request(kind.clone(), &[(hostile, "text"), (r#""a""#, "text")], r#""t"; drop table u; --""#, r#""s""#);
            let sql = insert_text(&request);
            assert!(sql.contains(r##"("""x""; drop table t; --""", """a""")"##), "{kind:?}: {sql}");
            assert!(sql.contains(r##"INSERT INTO """s"""."""t""; drop table u; --""" ("##), "{kind:?}: {sql}");
            assert!(!sql.contains(r#"("x"; drop"#) && !sql.contains(r#", "a")"#), "{kind:?}: {sql}");
        }
    }

    #[test]
    fn insert_names_keep_dbxs_own_quoting_elsewhere_and_other_formats_keep_raw_headers() {
        let sql = insert_text(&insert_request(DbKind::MySql, &[("a`b", "text")], "t", ""));
        assert!(sql.contains("(`a``b`)"), "{sql}");
        let mut csv = insert_request(DbKind::Postgres, &[(r#""a""#, "text")], "t", "");
        csv.format = ExportFormat::Csv;
        assert!(String::from_utf8(export_bytes_for(&csv).unwrap()).unwrap().starts_with(r#""""a""""#), "CSV headers stay raw");
    }

    #[test]
    fn columns_dbx_will_not_insert_are_found_by_name() {
        let request = insert_request(DbKind::Postgres, &[("id", "integer"), ("doc", "tsvector"), ("__DBX_ROWID", "text"), ("body", "text")], "t", "");
        assert_eq!(export_skipped_columns_for(&request), vec!["doc".to_string(), "__DBX_ROWID".to_string()]);
        let sql = insert_text(&request);
        assert!(sql.contains(r#"("id", "body")"#) && !sql.contains("doc"), "{sql}");
        let mysql = insert_request(DbKind::MySql, &[("doc", "tsvector")], "t", "");
        assert!(export_skipped_columns_for(&mysql).is_empty());
        let all = insert_request(DbKind::Postgres, &[("doc", "tsvector")], "t", "");
        assert!(export_bytes_for(&all).is_err(), "nothing to insert is refused, not written empty");
    }

    /// Every name handed to a dbx SQL builder goes through `guarded` (or
    /// `guarded_all`). The builders are called once each, from known places,
    /// and each line that carries a name to one says so.
    #[test]
    fn every_name_given_to_a_dbx_sql_builder_goes_through_guarded() {
        let source = include_str!("dbx.rs");
        let production = &source[..source.find("#[cfg(test)]\nmod tests").expect("tests module")];
        for builder in ["build_table_data_select_sql(", "build_data_grid_count_sql(", "build_export_sql_insert("] {
            let calls = production.lines().filter(|l| !l.trim_start().starts_with("//") && !l.contains("use ") && l.contains(builder)).count();
            assert_eq!(calls, 1, "{builder} is called from one known place: register a new call here and guard its names");
        }
        // The bodies of the three functions that hold the builder calls.
        let mut bodies = String::new();
        let mut rest = production.to_string();
        for name in ["fn page_sql_for(", "fn count_sql_for(", "fn insert_statements("] {
            let start = production.find(name).unwrap_or_else(|| panic!("{name} is where the builder is called"));
            let end = start + production[start..].find("\n}\n").expect("end of function");
            bodies.push_str(&production[start..end]);
            bodies.push('\n');
            rest = rest.replace(&production[start..end], "");
        }
        // No other code quotes a name for SQL.
        for line in rest.lines().filter(|l| !l.trim_start().starts_with("//") && !l.contains("use ")) {
            assert!(!line.contains("quote_table") && !line.contains("quote_ident"), "a name is quoted outside the guarded builders: {line}");
        }
        let mut seen = 0;
        for (n, line) in bodies.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with("use ") {
                continue;
            }
            let carries_name = ["schema:", "table_name:", "order_by:", "primary_keys:", "qualified_table_name:"].iter().any(|f| t.starts_with(f))
                || (t.starts_with("columns:") && t.contains("names"))
                || t.contains("quote_table_identifier(")
                || t.contains("quote_table_data_identifier(")
                || t.contains("quote_identifier(");
            // (`schema: String::new()` is our own ExportRequest, not a dbx builder.)
            if carries_name && !t.starts_with("qualified_table_name: None") && !t.starts_with("schema: String::new()") {
                seen += 1;
                // `order` is built on the line above from `guarded_all`.
                assert!(t.contains("guarded") || t.starts_with("order_by: Some(order)"), "line {}: a name goes to dbx without `guarded`: {t}", n + 1);
            }
        }
        assert!(seen >= 6, "the scan looked at the call sites ({seen})");

        // `dump_request` is registered here with no guard, on purpose: it hands
        // dbx the database, the schema and the file path as values, and dbx's
        // dump builds every table and column statement from its own catalog,
        // with its own `quote_table_identifier`, so there is no name of ours
        // to guard. The gap that leaves (a column named like an already-quoted
        // identifier passes through unescaped) is closed by refusing the
        // dump: `dump` asks the catalog (`quoted_columns`) before it builds the
        // request.
        let calls = production.lines().filter(|l| !l.trim_start().starts_with("//") && l.contains("dump_request(") && !l.contains("fn dump_request(")).count();
        assert_eq!(calls, 1, "dump_request is called from one known place (the dump), after the catalog scan");
        let dump = production.find("async fn dump(").expect("the dump");
        let scan = production[dump..].find("quoted_columns(").expect("the dump scans the catalog for quoted column names");
        let build = production[dump..].find("dump_request(&export_id").expect("and then builds the request");
        assert!(scan < build, "the scan comes before the request is built");
    }
}
