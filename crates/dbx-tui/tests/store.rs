//! Layer 1: `DbxCore` against scratch stores, through the public Core seam.
//!
//! Run: `cargo test -p dbx-tui --test store -- --test-threads=1`
//!
//! Safety: these tests never touch the keychain or the real store. Every test
//! gets its own temporary data directory, passed to `DbxCore` explicitly, and
//! the store key always comes from `DBX_SECRET_KEY_FILE`, which dbx consults
//! before any key policy (`dbx-core/src/persistence/secret_codec.rs`,
//! `resolve_with_env`). That variable is process-wide, so every test holds one
//! lock for its whole body and sets it inside the lock. `HOME` is pointed at
//! the scratch directory as well, so dbx never looks at the real user's
//! compatibility key file (`~/Library/Application Support/dbx/secret.key`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dbx_tui::core::{Connection, ConnectionDraft, Core, CoreErrorKind, DbKind, DbxCore, StoreOptions};

const PASSWORD: &str = "pw-Zq8!never-shown-4471";
const KEY_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const KEY_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

static ENV: Mutex<()> = Mutex::new(());

/// A scratch data directory plus key files, with the process locked to it.
struct Scratch {
    _env: MutexGuard<'static, ()>,
    tmp: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let env = ENV.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        for (name, key) in [("key-a", KEY_A), ("key-b", KEY_B)] {
            std::fs::write(tmp.path().join(name), key).expect("key file");
        }
        // HOME too: dbx looks for a compatibility key file under HOME before
        // the keychain. With HOME scratch, it never even looks at the real one.
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("scratch home");
        std::env::set_var("HOME", &home);
        let s = Scratch { _env: env, tmp };
        s.use_key("key-a");
        s
    }

    /// Points dbx at one of the scratch key files. No test runs without one.
    fn use_key(&self, name: &str) {
        std::env::remove_var("DBX_SECRET_KEY");
        std::env::set_var("DBX_SECRET_KEY_FILE", self.tmp.path().join(name));
    }

    fn data_dir(&self) -> PathBuf {
        self.tmp.path().join("store")
    }

    fn core(&self) -> DbxCore {
        let key = std::env::var_os("DBX_SECRET_KEY_FILE").expect("a scratch key is always set");
        assert!(Path::new(&key).starts_with(self.tmp.path()), "the key must be a scratch key");
        let home = std::env::var_os("HOME").expect("HOME");
        assert!(Path::new(&home).starts_with(self.tmp.path()), "HOME must be scratch");
        std::fs::create_dir_all(self.data_dir()).expect("scratch data dir");
        DbxCore::new(self.options(self.data_dir().as_os_str().to_str().unwrap()))
    }

    /// Options as the client builds them, with `DBX_DATA_DIR` set to `value`.
    fn options(&self, value: &str) -> StoreOptions {
        let home = self.tmp.path().join("home");
        let vars = [("DBX_DATA_DIR", value.to_string()), ("HOME", home.to_string_lossy().into_owned())];
        StoreOptions::from_vars(|name| {
            vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.into()).or_else(|| std::env::var_os(name))
        })
            .expect("valid options")
    }
}

fn draft(name: &str) -> ConnectionDraft {
    ConnectionDraft {
        id: None,
        name: name.into(),
        kind: DbKind::Postgres,
        host: "127.0.0.1".into(),
        port: Some(55433),
        database: Some("pagila".into()),
        username: "postgres".into(),
        password: Some(PASSWORD.into()),
        production: true,
    }
}

fn assert_no_secret(c: &Connection) {
    let fields = [c.id.0.as_str(), c.name.as_str(), c.host.as_str(), c.database.as_deref().unwrap_or("")];
    for f in fields {
        assert!(!f.contains(PASSWORD), "password in a field: {f:?}");
    }
    assert!(!format!("{c:?}").contains(PASSWORD), "password in Debug output");
}

#[tokio::test]
async fn an_empty_directory_opens_and_returns_zero_connections() {
    let s = Scratch::new();
    let core = s.core();
    assert_eq!(std::fs::read_dir(s.data_dir()).unwrap().count(), 0, "empty");
    let list = core.connections().await.expect("open an empty directory");
    assert_eq!(list, vec![]);
    assert!(s.data_dir().join("dbx.db").exists(), "the store was created");
    // Opening writes nothing else: no key file (keys are created on the first
    // write of a secret, never on open), no plugin or agent directories.
    for entry in std::fs::read_dir(s.data_dir()).expect("store dir") {
        let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
        assert!(name.starts_with("dbx.db"), "unexpected {name:?} in a freshly opened store");
    }
}

#[tokio::test]
async fn a_store_seeded_with_one_connection_returns_it_without_its_password() {
    let s = Scratch::new();
    let seeded = s.core().seed_connection(&draft("local pg")).await.expect("seed");
    assert_no_secret(&seeded);

    // A fresh DbxCore, as a later start of the client would be.
    let list = s.core().connections().await.expect("reopen");
    assert_eq!(list.len(), 1);
    let c = &list[0];
    assert_eq!(c.id, seeded.id);
    assert_eq!(c.name, "local pg");
    assert_eq!(c.kind, DbKind::Postgres);
    assert_eq!(c.host, "127.0.0.1");
    assert_eq!(c.port, Some(55433));
    assert_eq!(c.database.as_deref(), Some("pagila"));
    assert!(c.production);
    assert_no_secret(c);

    // And dbx stored it encrypted: the plain password is nowhere in the files.
    for entry in std::fs::read_dir(s.data_dir()).expect("store dir") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            let bytes = std::fs::read(&path).expect("read store file");
            let hit = bytes.windows(PASSWORD.len()).any(|w| w == PASSWORD.as_bytes());
            assert!(!hit, "plain password found in {}", path.display());
        }
    }
}

#[tokio::test]
async fn with_dbx_secret_key_file_set_a_store_written_under_that_key_reopens() {
    let s = Scratch::new();
    s.core().seed_connection(&draft("one")).await.expect("seed under key A");
    s.core().seed_connection(&draft("two")).await.expect("second write under key A");
    // Loading decrypts every saved password, so this fails under any other key.
    let names: Vec<String> = s.core().connections().await.expect("reopen").into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["one".to_string(), "two".to_string()]);
}

#[tokio::test]
async fn a_store_opened_under_a_different_key_is_a_key_error_that_names_no_secret() {
    let s = Scratch::new();
    s.core().seed_connection(&draft("one")).await.expect("seed under key A");
    s.use_key("key-b");
    let err = s.core().connections().await.expect_err("key B cannot decrypt key A's store");
    assert_eq!(err.kind, CoreErrorKind::Key, "{err:?}");
    for secret in [PASSWORD, KEY_A, KEY_B] {
        assert!(!err.message.contains(secret), "secret in {:?}", err.message);
    }
    assert!(err.message.contains("DBX_SECRET_KEY_FILE"), "says what to do: {}", err.message);
    assert!(!err.message.contains("keychain"), "the key came from a file, not the keychain: {}", err.message);
}

#[tokio::test]
async fn a_store_file_that_is_not_a_database_is_a_store_error() {
    let s = Scratch::new();
    std::fs::create_dir_all(s.data_dir()).unwrap();
    std::fs::write(s.data_dir().join("dbx.db"), b"this is not a sqlite database, it is just text").unwrap();
    let err = s.core().connections().await.expect_err("garbage store");
    assert_eq!(err.kind, CoreErrorKind::Store, "{err:?}");
    assert!(err.message.contains("DBX_DATA_DIR"), "says what to do: {}", err.message);
}

#[tokio::test]
async fn a_failed_start_is_retried_on_the_next_call() {
    let s = Scratch::new();
    s.core().seed_connection(&draft("one")).await.expect("seed under key A");
    s.use_key("key-b");
    let core = s.core();
    assert_eq!(core.connections().await.unwrap_err().kind, CoreErrorKind::Key);
    s.use_key("key-a");
    assert_eq!(core.connections().await.expect("retry with the right key").len(), 1);
}

#[tokio::test]
async fn a_store_awaiting_the_desktop_upgrade_is_refused_untouched_with_a_store_error() {
    let s = Scratch::new();
    // An older desktop app left its pre-database files: only the desktop's
    // upgrade wizard may migrate them.
    std::fs::create_dir_all(s.data_dir()).unwrap();
    let legacy = s.data_dir().join("sidebar_layout.json");
    std::fs::write(&legacy, b"{}").unwrap();
    let err = s.core().connections().await.expect_err("not ready");
    assert_eq!(err.kind, CoreErrorKind::Store, "{err:?}");
    assert!(err.message.contains("desktop app"), "says what to do: {}", err.message);
    assert_eq!(std::fs::read(&legacy).unwrap(), b"{}", "left as it was");
}

/// Holds an exclusive SQLite lock on a store until dropped, from another
/// process, as a second dbx program or a slow disk might.
struct LockHolder(std::process::Child);

impl LockHolder {
    fn hold(db: &Path) -> Self {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};
        let mut child = Command::new("sqlite3")
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("sqlite3 (part of macOS; on Linux install it)");
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(b"PRAGMA locking_mode=EXCLUSIVE;\nBEGIN EXCLUSIVE;\nSELECT 'locked';\n").unwrap();
        stdin.flush().unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("locked") {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "sqlite3 exited before locking");
        }
        LockHolder(child)
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        drop(self.0.stdin.take()); // end of input: sqlite3 exits and unlocks
        let _ = self.0.wait();
    }
}

// A single-threaded runtime: if opening the store blocked the thread it runs
// on, nothing else on the runtime (timers, the event loop) could run.
#[tokio::test(flavor = "current_thread")]
async fn while_the_store_is_slow_to_open_the_runtime_keeps_running() {
    use std::time::{Duration, Instant};
    let s = Scratch::new();
    s.core().seed_connection(&draft("one")).await.expect("seed");
    let lock = LockHolder::hold(&s.data_dir().join("dbx.db"));

    let core = std::sync::Arc::new(s.core());
    let pending = tokio::spawn({
        let core = core.clone();
        async move { core.connections().await }
    });
    let started = Instant::now();
    for tick in 0..10 {
        let before = Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let late = before.elapsed();
        assert!(late < Duration::from_millis(500), "tick {tick} stalled for {late:?}");
    }
    assert!(!pending.is_finished(), "the open was still waiting on the lock");
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(lock);
    let list = pending.await.unwrap().expect("opens once the lock is gone");
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn dbx_data_dir_must_be_an_absolute_path() {
    let s = Scratch::new();
    for given in ["~/dbx-store", "relative/store", "./store"] {
        let vars = [("DBX_DATA_DIR", given), ("HOME", "/nonexistent-home")];
        let err = StoreOptions::from_vars(|n| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| v.into()))
            .expect_err(given);
        assert_eq!(err.kind, CoreErrorKind::Store, "{given}");
        assert!(err.message.contains(given), "names what was given: {}", err.message);
        assert!(err.message.contains("absolute"), "says what is expected: {}", err.message);
    }
    drop(s);
}

#[tokio::test]
async fn a_dbx_data_dir_that_does_not_exist_is_reported_not_created() {
    let s = Scratch::new();
    let missing = s.tmp.path().join("no-such-dir");
    let core = DbxCore::new(s.options(missing.to_str().unwrap()));
    let err = core.connections().await.expect_err("missing override");
    assert_eq!(err.kind, CoreErrorKind::Store, "{err:?}");
    assert!(err.message.contains("does not exist"), "{}", err.message);
    assert!(err.message.contains("no-such-dir"), "{}", err.message);
    assert!(!missing.exists(), "nothing was created");
}
