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
    policy: SecretKeyPolicy,
    /// Held for the whole of a save or a remove: each reads the saved config,
    /// writes the store and then updates dbx's runtime list, and two of them
    /// interleaved can leave the runtime and the store disagreeing.
    mutations: Mutex<()>,
}

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
        DbxCore { options: Ok(options), state: Mutex::new(None), policy: SecretKeyPolicy::PlatformDefault, mutations: Mutex::new(()) }
    }

    /// The client's DbxCore: the store named by the environment. If that
    /// cannot be worked out, every call fails with the reason.
    pub fn from_env() -> Self {
        DbxCore { options: StoreOptions::from_env(), state: Mutex::new(None), policy: SecretKeyPolicy::PlatformDefault, mutations: Mutex::new(()) }
    }

    /// Keys only from the environment or the data directory, never the OS
    /// keychain (dbx-web's `ManagedDataDir` policy). Tests only: it is how a
    /// test reaches "no key available" without asking the keychain.
    #[cfg(feature = "test-support")]
    pub fn with_managed_keys(options: StoreOptions) -> Self {
        DbxCore { policy: SecretKeyPolicy::ManagedDataDir, ..Self::new(options) }
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
        host: display_safe(&c.host),
        port: (c.port != 0).then_some(c.port),
        database: c.database.as_deref().filter(|d| !d.is_empty()).map(display_safe),
        username: display_safe(&c.username),
        tls: c.ssl,
        production: c.is_production,
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
    config.port = draft.port.unwrap_or(match config.db_type {
        DatabaseType::Postgres => 5432,
        DatabaseType::Mysql => 3306,
        _ => 0,
    });
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
    let mut out = display_safe(message.trim());
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "••••");
    }
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

fn not_yet<T>(slice: &str) -> Result<T, CoreError> {
    Err(CoreError::new(CoreErrorKind::Other, format!("not available yet (slice {slice})")))
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

    async fn databases(&self, _id: &ConnectionId) -> Result<Vec<String>, CoreError> {
        not_yet("04")
    }
    async fn schemas(&self, _at: &DatabasePath) -> Result<Vec<String>, CoreError> {
        not_yet("04")
    }
    async fn tables(&self, _at: &SchemaPath) -> Result<Vec<Table>, CoreError> {
        not_yet("04")
    }
    async fn columns(&self, _at: &TablePath) -> Result<Vec<ColumnInfo>, CoreError> {
        not_yet("04")
    }
    async fn run(&self, _run: RunId, _request: RunRequest) -> Result<ResultSet, CoreError> {
        not_yet("05")
    }
    fn cancel(&self, _run: RunId) {}
    async fn close_session(&self, _tab: TabId) -> Result<(), CoreError> {
        Ok(())
    }
    async fn history(&self, _query: HistoryQuery) -> Result<Vec<HistoryEntry>, CoreError> {
        not_yet("18")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| owned.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
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
}
