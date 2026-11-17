//! Shared by the layer 1 test binaries (`store` and `live`): a scratch data
//! directory, key files and `HOME`, with the process locked to them.
//!
//! Safety: these tests never touch the keychain or the real store. Every test
//! gets its own temporary data directory, passed to `DbxCore` explicitly, and
//! the store key always comes from `DBX_SECRET_KEY_FILE` (or `DBX_SECRET_KEY`),
//! which dbx consults before any key policy (`dbx-core/src/persistence/
//! secret_codec.rs`, `resolve_with_env`). Those variables are process-wide, so
//! every test holds one lock for its whole body and sets them inside the lock.
//! `HOME` is pointed at the scratch directory as well, so dbx never looks at
//! the real user's compatibility key file
//! (`~/Library/Application Support/dbx/secret.key`).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dbx_tui::core::{Connection, ConnectionDraft, DbKind, DbxCore, StoreOptions};

pub const PASSWORD: &str = "pw-Zq8!never-shown-4471";
pub const KEY_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
pub const KEY_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

pub static ENV: Mutex<()> = Mutex::new(());

/// A scratch data directory plus key files, with the process locked to it.
pub struct Scratch {
    pub _env: MutexGuard<'static, ()>,
    pub tmp: tempfile::TempDir,
}

impl Scratch {
    pub fn new() -> Self {
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
    pub fn use_key(&self, name: &str) {
        std::env::remove_var("DBX_SECRET_KEY");
        std::env::set_var("DBX_SECRET_KEY_FILE", self.tmp.path().join(name));
    }

    /// Points dbx at a key given as a value in `DBX_SECRET_KEY`, the other way
    /// of passing one in the environment.
    pub fn use_key_value(&self, key: &str) {
        std::env::remove_var("DBX_SECRET_KEY_FILE");
        std::env::set_var("DBX_SECRET_KEY", key);
    }

    /// No key in the environment. Only `managed_core` may be used like this:
    /// the client's own key policy would go to the keychain.
    pub fn use_no_key(&self) {
        std::env::remove_var("DBX_SECRET_KEY_FILE");
        std::env::remove_var("DBX_SECRET_KEY");
    }

    pub fn data_dir(&self) -> PathBuf {
        self.tmp.path().join("store")
    }

    pub fn core(&self) -> DbxCore {
        // The client's key policy reads the keychain when no key is given in
        // the environment: refuse to build one that could.
        match std::env::var_os("DBX_SECRET_KEY_FILE") {
            Some(key) => assert!(Path::new(&key).starts_with(self.tmp.path()), "the key must be a scratch key"),
            None => assert!(std::env::var_os("DBX_SECRET_KEY").is_some(), "a scratch key is always set"),
        }
        let home = std::env::var_os("HOME").expect("HOME");
        assert!(Path::new(&home).starts_with(self.tmp.path()), "HOME must be scratch");
        std::fs::create_dir_all(self.data_dir()).expect("scratch data dir");
        DbxCore::new(self.options(self.data_dir().as_os_str().to_str().unwrap()))
    }

    /// A `DbxCore` whose keys live only in the data directory (dbx-web's
    /// `ManagedDataDir` policy), so a missing key is `MISSING_MANAGED_KEY` and
    /// never a look at the keychain. For the key-refusal tests.
    pub fn managed_core(&self) -> DbxCore {
        let home = std::env::var_os("HOME").expect("HOME");
        assert!(Path::new(&home).starts_with(self.tmp.path()), "HOME must be scratch");
        std::fs::create_dir_all(self.data_dir()).expect("scratch data dir");
        DbxCore::with_managed_keys(self.options(self.data_dir().as_os_str().to_str().unwrap()))
    }

    /// Options as the client builds them, with `DBX_DATA_DIR` set to `value`.
    pub fn options(&self, value: &str) -> StoreOptions {
        let home = self.tmp.path().join("home");
        let vars = [("DBX_DATA_DIR", value.to_string()), ("HOME", home.to_string_lossy().into_owned())];
        StoreOptions::from_vars(|name| {
            vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.into()).or_else(|| std::env::var_os(name))
        })
            .expect("valid options")
    }
}

pub fn draft(name: &str) -> ConnectionDraft {
    ConnectionDraft {
        id: None,
        name: name.into(),
        kind: DbKind::Postgres,
        host: "127.0.0.1".into(),
        port: Some(55433),
        database: Some("pagila".into()),
        username: "postgres".into(),
        password: Some(PASSWORD.into()),
        tls: false,
        production: true,
    }
}

pub fn assert_no_secret(c: &Connection) {
    let fields = [c.id.0.as_str(), c.name.as_str(), c.host.as_str(), c.database.as_deref().unwrap_or("")];
    for f in fields {
        assert!(!f.contains(PASSWORD), "password in a field: {f:?}");
    }
    assert!(!format!("{c:?}").contains(PASSWORD), "password in Debug output");
}

