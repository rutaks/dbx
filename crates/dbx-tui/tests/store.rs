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

mod common;

use std::path::Path;

use common::*;
use dbx_tui::core::{Connection, ConnectionDraft, Core, CoreErrorKind, DatabasePath, DbKind, DbxCore, SchemaPath, StoreOptions, TablePath};

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
async fn a_saved_connection_is_listed_after_a_reopen_without_its_password() {
    let s = Scratch::new();
    let seeded = s.core().save_connection(&draft("local pg")).await.expect("seed");
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
    assert_eq!((c.username.as_str(), c.tls), ("postgres", false));
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
    s.core().save_connection(&draft("one")).await.expect("seed under key A");
    s.core().save_connection(&draft("two")).await.expect("second write under key A");
    // Loading decrypts every saved password, so this fails under any other key.
    let names: Vec<String> = s.core().connections().await.expect("reopen").into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["one".to_string(), "two".to_string()]);
}

#[tokio::test]
async fn a_store_opened_under_a_different_key_is_a_key_error_that_names_no_secret() {
    let s = Scratch::new();
    s.core().save_connection(&draft("one")).await.expect("seed under key A");
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
    s.core().save_connection(&draft("one")).await.expect("seed under key A");
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
    s.core().save_connection(&draft("one")).await.expect("seed");
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

// ---- slice 03: save, edit, remove, test -------------------------------------------

/// A connection as the desktop app writes it: SSH tunnel, options and
/// preferences the TUI form has no fields for.
fn desktop_config(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": "from desktop",
        "db_type": "postgres",
        "host": "db.internal",
        "port": 5432,
        "username": "app",
        "password": PASSWORD,
        "database": "shop",
        "ssl": true,
        "note": "owned by the data team",
        "color": "#ff8800",
        "url_params": "application_name=desktop",
        "read_only": true,
        "visible_databases": ["shop", "shop_test"],
        "show_system_schemas": true,
        "connect_timeout_secs": 7,
        "is_production": false,
        "production_databases": ["shop"],
        "transport_layers": [{
            "type": "ssh", "id": "layer-1", "name": "bastion", "enabled": true,
            "host": "bastion.example.com", "port": 2222, "user": "jump",
            "password": "ssh-secret-91", "key_path": "/keys/id_ed25519",
        }],
    })
}

fn only(list: Vec<Connection>) -> Connection {
    assert_eq!(list.len(), 1, "{list:?}");
    list.into_iter().next().unwrap()
}

#[tokio::test]
async fn saving_a_new_connection_returns_it_without_its_password_and_lists_it_at_once() {
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&draft("local pg")).await.expect("save");
    assert_no_secret(&saved);
    assert!(!saved.id.0.is_empty(), "a new connection gets an id");
    assert_eq!(core.connections().await.unwrap(), vec![saved.clone()], "no restart needed to see it");
    assert_eq!(core.runtime_config_ids().await, vec![saved.id.0.clone()], "dbx's runtime list holds it too");
    // Two saves of new connections are two connections.
    let other = core.save_connection(&draft("second")).await.expect("save");
    assert_ne!(other.id, saved.id);
    assert_eq!(core.connections().await.unwrap().len(), 2);
}

#[tokio::test]
async fn the_first_password_save_on_a_fresh_store_with_a_key_file_creates_no_key_and_decrypts_on_reopen() {
    let s = Scratch::new();
    assert_eq!(std::fs::read_dir(s.data_dir().parent().unwrap()).unwrap().filter(|e| e.as_ref().unwrap().file_name() == "store").count(), 0);
    let saved = s.core().save_connection(&draft("fresh")).await.expect("first save that carries a password");
    // A new process: the password is read back and decrypted with the same key.
    let reopened = s.core();
    assert_eq!(only(reopened.connections().await.expect("reopen")), saved);
    let stored = reopened.runtime_config(&saved.id).await.expect("runtime config");
    assert_eq!(stored["password"], PASSWORD, "the password decrypted to what was typed");
    // dbx wrote no key of its own next to the store: the environment's key was used.
    for entry in std::fs::read_dir(s.data_dir()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(name.starts_with("dbx.db"), "unexpected {name:?} in the store directory");
    }
}

#[tokio::test]
async fn the_first_password_save_with_a_key_given_as_a_value_decrypts_on_reopen_under_that_key_only() {
    let s = Scratch::new();
    s.use_key_value(KEY_B);
    let saved = s.core().save_connection(&draft("fresh")).await.expect("save under an environment key");
    let reopened = s.core();
    assert_eq!(only(reopened.connections().await.expect("reopen")), saved);
    assert_eq!(reopened.runtime_config(&saved.id).await.unwrap()["password"], PASSWORD);
    s.use_key_value(KEY_A);
    let err = s.core().connections().await.expect_err("another key cannot open it");
    assert_eq!(err.kind, CoreErrorKind::Key);
    assert!(!err.message.contains(KEY_A) && !err.message.contains(KEY_B) && !err.message.contains(PASSWORD));
}

#[tokio::test]
async fn editing_changes_only_the_fields_the_form_shows_and_keeps_every_other_one() {
    let s = Scratch::new();
    let core = s.core();
    let id = core.seed_config(desktop_config("desk-1")).await.expect("seed a desktop connection");
    let before = core.runtime_config(&id).await.unwrap();

    // The form edits the name and nothing else.
    let listed = only(core.connections().await.unwrap());
    assert_eq!((listed.username.as_str(), listed.tls), ("app", true), "the form is filled from the connection");
    let edit = ConnectionDraft {
        id: Some(id.clone()),
        name: "renamed".into(),
        kind: listed.kind.clone(),
        host: listed.host.clone(),
        port: listed.port,
        database: listed.database.clone(),
        username: listed.username.clone(),
        password: None,
        tls: listed.tls,
        production: listed.production,
    };
    let saved = core.save_connection(&edit).await.expect("edit");
    assert_eq!((saved.id.clone(), saved.name.as_str()), (id.clone(), "renamed"));

    for store in [core.runtime_config(&id).await.unwrap(), s.core().runtime_config(&id).await.unwrap()] {
        let mut want = before.clone();
        want["name"] = "renamed".into();
        assert_eq!(store, want, "only the name changed, in dbx's runtime copy and in the store");
        assert_eq!(store["transport_layers"][0]["host"], "bastion.example.com");
        assert_eq!(store["transport_layers"][0]["password"], "ssh-secret-91", "the tunnel's secret survived");
        assert_eq!(store["note"], "owned by the data team");
        assert_eq!(store["read_only"], true);
    }
    assert_eq!(core.connections().await.unwrap().len(), 1, "an edit is not an add");
}

#[tokio::test]
async fn editing_with_a_blank_password_keeps_the_stored_one_and_a_typed_one_replaces_it() {
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&draft("pg")).await.unwrap();
    let edit = |name: &str, password: Option<&str>| ConnectionDraft {
        id: Some(saved.id.clone()),
        name: name.into(),
        password: password.map(Into::into),
        ..draft("pg")
    };
    core.save_connection(&edit("pg 2", None)).await.unwrap();
    core.save_connection(&edit("pg 3", Some(""))).await.unwrap();
    for c in [&core, &s.core()] {
        assert_eq!(c.runtime_config(&saved.id).await.unwrap()["password"], PASSWORD, "blank means unchanged");
    }
    core.save_connection(&edit("pg 4", Some("a-new-password"))).await.unwrap();
    assert_eq!(s.core().runtime_config(&saved.id).await.unwrap()["password"], "a-new-password");
}

#[tokio::test]
async fn removing_a_connection_deletes_it_from_the_store_and_from_dbxs_runtime_list() {
    let s = Scratch::new();
    let core = s.core();
    let keep = core.save_connection(&draft("keep")).await.unwrap();
    let gone = core.save_connection(&draft("gone")).await.unwrap();
    core.remove_connection(&gone.id).await.expect("remove");
    assert_eq!(core.connections().await.unwrap(), vec![keep.clone()]);
    assert_eq!(core.runtime_config_ids().await, vec![keep.id.0.clone()]);
    assert_eq!(s.core().connections().await.unwrap(), vec![keep], "gone after a restart too");
    // Removing what is already gone is not an error: another dbx head may have beaten us to it.
    core.remove_connection(&gone.id).await.expect("idempotent");
}

#[tokio::test]
async fn a_store_that_holds_encrypted_data_but_has_no_key_refuses_the_save_as_a_key_error_and_writes_nothing() {
    let s = Scratch::new();
    // A connection with a password, saved under key A, in a store whose keys are
    // only ever read from the environment or the data directory.
    let core = s.managed_core();
    let existing = core.save_connection(&draft("existing")).await.expect("save under key A");

    // The key goes missing while the client runs: the variable is unset. (dbx
    // notices, because the set of places a key can come from has changed.)
    // A store like this cannot be opened by the client in the first place, so
    // this is the only way a save meets it: the key vanishing mid-session.
    s.use_no_key();
    let err = core.save_connection(&draft("new one")).await.expect_err("no key, encrypted data present");
    assert_eq!(err.kind, CoreErrorKind::Key, "{err:?}");
    assert!(err.message.contains("ENCRYPTED_DATA_KEY_MISSING"), "dbx's code is shown: {}", err.message);
    for secret in [PASSWORD, KEY_A, KEY_B] {
        assert!(!err.message.contains(secret), "secret in {:?}", err.message);
    }
    assert!(!s.data_dir().join(".dbx/secret.key").exists(), "no key was made to get past it");

    // Nothing was written, and nothing is listed that is not stored.
    assert_eq!(core.connections().await.unwrap(), vec![existing.clone()], "the runtime list did not change");
    s.use_key("key-a");
    assert_eq!(s.managed_core().connections().await.unwrap(), vec![existing], "the store did not change");
}

/// A TCP server that accepts connections and never says a word: a database
/// that is up but will not answer.
async fn silent_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    (addr, task)
}

#[tokio::test]
async fn a_failed_test_reports_the_reason_and_leaves_no_temporary_entry_behind() {
    let s = Scratch::new();
    let core = s.core();
    let kept = core.save_connection(&draft("kept")).await.unwrap();
    // Nothing listens on port 1.
    let unreachable = ConnectionDraft { port: Some(1), ..draft("unsaved") };
    let err = core.test_connection(&unreachable).await.expect_err("nothing listens there");
    assert_eq!(err.kind, CoreErrorKind::Connect, "{err:?}");
    assert!(!err.message.is_empty());
    assert!(!err.message.contains(PASSWORD), "{}", err.message);
    assert_eq!(core.runtime_config_ids().await, vec![kept.id.0.clone()], "the temporary entry is gone");
    assert_eq!(core.connections().await.unwrap(), vec![kept.clone()], "and the test saved nothing");
    assert_eq!(s.core().connections().await.unwrap(), vec![kept], "not in the store either");
}

#[tokio::test]
async fn a_test_that_is_abandoned_while_it_waits_still_cleans_up_its_temporary_entry() {
    use std::time::{Duration, Instant};
    let s = Scratch::new();
    let core = s.core();
    let (addr, server) = silent_server().await;
    let hanging = ConnectionDraft { host: addr.ip().to_string(), port: Some(addr.port()), ..draft("hangs") };

    // The caller gives up and drops the future mid-test (a cancelled task, a timeout).
    let gave_up = tokio::time::timeout(Duration::from_millis(500), core.test_connection(&hanging)).await;
    assert!(gave_up.is_err(), "the server never answers, so the test is still waiting");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ids = core.runtime_config_ids().await;
        if ids.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "the temporary entry was never cleaned up: {ids:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
}

#[tokio::test]
async fn a_test_in_flight_is_not_listed_as_a_connection_while_it_runs() {
    use std::time::Duration;
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    let (addr, server) = silent_server().await;
    let hanging = ConnectionDraft { host: addr.ip().to_string(), port: Some(addr.port()), ..draft("hangs") };
    let task = tokio::spawn({
        let core = core.clone();
        async move { core.test_connection(&hanging).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let ids = core.runtime_config_ids().await;
    assert_eq!(ids.len(), 1, "the probe is in dbx's runtime list while it runs: {ids:?}");
    assert!(ids[0].starts_with("__test_"), "{ids:?}");
    assert_eq!(core.connections().await.unwrap(), vec![], "but the client never lists it");
    task.abort();
    let _ = task.await;
    server.abort();
}

#[tokio::test]
async fn an_edit_keeps_init_script_connection_secrets_and_save_password_false() {
    let s = Scratch::new();
    let core = s.core();
    let mut config = desktop_config("desk-2");
    config["init_script"] = "SET search_path = shop".into();
    config["connection_secrets"] = serde_json::json!({"extra": "tunnel-token-77"});
    config["save_password"] = false.into();
    config["password"] = "".into();
    let id = core.seed_config(config).await.expect("seed");
    let before = core.runtime_config(&id).await.unwrap();
    let listed = only(core.connections().await.unwrap());
    let edit = ConnectionDraft {
        id: Some(id.clone()),
        name: "renamed".into(),
        kind: listed.kind,
        host: listed.host,
        port: listed.port,
        database: listed.database,
        username: listed.username,
        password: None,
        tls: listed.tls,
        production: listed.production,
    };
    core.save_connection(&edit).await.expect("edit");
    for store in [core.runtime_config(&id).await.unwrap(), s.core().runtime_config(&id).await.unwrap()] {
        assert_eq!(store["init_script"], "SET search_path = shop");
        assert_eq!(store["connection_secrets"]["extra"], "tunnel-token-77");
        assert_eq!(store["save_password"], false);
        assert_eq!(store["name"], "renamed");
        assert_eq!(store["transport_layers"], before["transport_layers"]);
    }
}

#[tokio::test]
async fn control_characters_in_a_stored_name_or_host_never_reach_the_client() {
    let s = Scratch::new();
    let core = s.core();
    let mut config = desktop_config("evil-1");
    config["name"] = "evil\u{1b}]52;c;AAAA\u{7}name".into();
    config["host"] = "h\u{1b}[2Jost".into();
    config["username"] = "u\u{7}ser".into();
    config["database"] = "d\u{1b}b".into();
    core.seed_config(config).await.unwrap();
    let c = only(core.connections().await.unwrap());
    for text in [&c.name, &c.host, &c.username, c.database.as_ref().unwrap()] {
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }
    assert!(c.name.contains("evil") && c.name.contains("name"), "{:?}", c.name);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_and_a_remove_of_the_same_connection_never_leave_the_runtime_and_the_store_disagreeing() {
    let s = Scratch::new();
    let core = std::sync::Arc::new(s.core());
    for round in 0..300 {
        let saved = core.save_connection(&draft("target")).await.unwrap();
        let edit = ConnectionDraft { id: Some(saved.id.clone()), name: format!("edit {round}"), password: None, ..draft("target") };
        let (a, b) = tokio::join!(
            {
                let core = core.clone();
                let edit = edit.clone();
                tokio::spawn(async move {
                    // Several edits, so one of them is likely to straddle the remove.
                    let mut last = Ok(());
                    for _ in 0..3 {
                        last = core.save_connection(&edit).await.map(|_| ());
                    }
                    last
                })
            },
            {
                let core = core.clone();
                let id = saved.id.clone();
                tokio::spawn(async move { core.remove_connection(&id).await })
            },
        );
        a.unwrap().ok();
        b.unwrap().expect("remove");
        let runtime = core.connections().await.unwrap();
        let stored = s.core().connections().await.unwrap();
        assert_eq!(runtime, stored, "round {round}: runtime and store disagree");
        for c in &runtime {
            core.remove_connection(&c.id).await.unwrap();
        }
    }
}

// ---- slice 04: abandoning a metadata call -----------------------------------

#[test]
fn abandoning_a_listing_that_hangs_stops_dbxs_work_at_once() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::io::AsyncReadExt;
    let s = Scratch::new();
    let core = Arc::new(s.core());
    // The client's own runtime, so the blocking threads have the stack dbx needs.
    let rt = dbx_tui::shell::runtime().expect("runtime");
    rt.block_on(async {
        // A server that accepts, never answers, and counts the sockets still open.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted, open) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (a, o) = (accepted.clone(), open.clone());
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                a.fetch_add(1, SeqCst);
                o.fetch_add(1, SeqCst);
                let o = o.clone();
                tokio::spawn(async move {
                    let mut sink = [0u8; 256];
                    while matches!(socket.read(&mut sink).await, Ok(n) if n > 0) {}
                    o.fetch_sub(1, SeqCst);
                });
            }
        });
        let id = core.save_connection(&ConnectionDraft { port: Some(port), ..draft("hangs") }).await.unwrap().id;

        // The server never answers: the listing waits. Give up on it.
        let waited = tokio::time::timeout(Duration::from_millis(500), core.databases(&id)).await;
        assert!(waited.is_err(), "the listing is still waiting after 500 ms");
        assert!(accepted.load(SeqCst) >= 1, "dbx reached the server");

        // Dropping the future must stop dbx's work: its sockets close. Left to
        // run, it would keep them until dbx's own connect timeout (10 s).
        let gave_up = Instant::now();
        while open.load(SeqCst) > 0 && gave_up.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(open.load(SeqCst), 0, "{} socket(s) still open 3 s after the listing was abandoned", open.load(SeqCst));
        server.abort();
    });
}

#[tokio::test]
async fn a_connection_that_is_not_postgres_says_it_cannot_be_browsed_yet_and_dbx_is_not_asked() {
    let s = Scratch::new();
    let core = s.core();
    // Nothing listens on port 1: a call that reached dbx would fail to connect.
    // SQLite is browsed since slice 23a (tests/sqlite.rs) and MySQL since
    // slice 23b (tests/mysql.rs); Doris and the other MySQL-family types are
    // types of their own in dbx.
    for (kind, name) in [(DbKind::Other("doris".into()), "doris")] {
        let draft = ConnectionDraft { kind: kind.clone(), port: Some(1), ..draft("other") };
        let id = core.save_connection(&draft).await.expect("save").id;
        let expected = format!("Browsing {name} is not supported yet.");
        let at = DatabasePath { connection: id.clone(), database: "x".into() };
        let schema = SchemaPath { database: at.clone(), schema: "s".into() };
        let table = TablePath { schema: schema.clone(), table: "t".into() };
        assert_eq!(core.databases(&id).await.unwrap_err().message, expected);
        assert_eq!(core.schemas(&at).await.unwrap_err().message, expected);
        assert_eq!(core.tables(&schema).await.unwrap_err().message, expected);
        assert_eq!(core.columns(&table).await.unwrap_err().message, expected);
        core.release(&at).await.expect("releasing nothing is fine");
    }
}

// ---- history (slice 18) ----------------------------------------------------------
//
// The history is the desktop app's: one table in the shared store, written and
// searched through dbx's own persistence. Scratch stores only.

use dbx_tui::core::{ConnectionId, HistoryCursor, HistoryQuery, NewHistoryEntry};
use std::time::Duration;

fn new_entry(connection: &str, name: &str, sql: &str) -> NewHistoryEntry {
    NewHistoryEntry {
        connection: ConnectionId(connection.into()),
        connection_name: name.into(),
        database: "pagila".into(),
        sql: sql.into(),
        elapsed: Duration::from_millis(42),
        success: true,
        error: None,
        affected: None,
    }
}

fn search(text: &str, connection: Option<&str>) -> HistoryQuery {
    HistoryQuery { search_text: text.into(), connection: connection.map(|c| ConnectionId(c.into())), cursor: None, limit: 50 }
}

/// dbx's own handle on the same store, as the desktop app has it (opened after
/// `core` has created and migrated the store).
async fn desktop_storage(s: &Scratch) -> dbx_core::storage::Storage {
    dbx_core::storage::Storage::open(&s.data_dir().join("dbx.db")).await.expect("open the scratch store as the desktop would")
}

async fn pause() {
    // Entries are ordered by a time with millisecond resolution.
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn an_entry_the_client_saves_is_found_again_by_a_substring_and_by_connection_newest_first() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    core.save_history(&new_entry("conn-a", "local pg", "select * from Film where id = 1")).await.expect("save a");
    pause().await;
    core.save_history(&new_entry("conn-b", "other pg", "select count(*) from film")).await.expect("save b");
    pause().await;
    let failed = NewHistoryEntry { success: false, error: Some("relation \"nope\" does not exist".into()), ..new_entry("conn-a", "local pg", "select * from nope") };
    core.save_history(&failed).await.expect("save c");
    pause().await;
    let write = NewHistoryEntry { affected: Some(3), ..new_entry("conn-a", "local pg", "-- fix\nupdate film set title = 'x'") };
    core.save_history(&write).await.expect("save d");

    // A substring, without regard to case, over both connections: newest first.
    let found = core.history(&search("FILM", None)).await.expect("search");
    let sqls: Vec<&str> = found.entries.iter().map(|e| e.sql.as_str()).collect();
    assert_eq!(sqls, ["-- fix\nupdate film set title = 'x'", "select count(*) from film", "select * from Film where id = 1"]);
    assert_eq!(found.total, 3);
    assert_eq!(found.next, None);

    // This connection only.
    let mine = core.history(&search("film", Some("conn-a"))).await.expect("search a");
    assert_eq!(mine.entries.len(), 2);
    assert!(mine.entries.iter().all(|e| e.connection == ConnectionId("conn-a".into()) && e.connection_name == "local pg"));

    // The connection's name is searched too, and the database.
    assert_eq!(core.history(&search("other", None)).await.unwrap().entries.len(), 1);
    assert_eq!(core.history(&search("pagila", None)).await.unwrap().total, 4);
    assert_eq!(core.history(&search("zzz-nothing", None)).await.unwrap().entries.len(), 0);
    // `%` and `_` are text, not wildcards.
    assert_eq!(core.history(&search("%", None)).await.unwrap().entries.len(), 0);

    // What was saved: the failure with its error, the write with its rows.
    let all = core.history(&search("", None)).await.expect("all");
    let by = |sql: &str| all.entries.iter().find(|e| e.sql.contains(sql)).unwrap_or_else(|| panic!("{sql}")).clone();
    let f = by("from nope");
    assert!(!f.success && f.error.as_deref() == Some("relation \"nope\" does not exist"), "{f:?}");
    assert_eq!(by("update film").affected, Some(3));
    assert_eq!(by("count(*)").elapsed, Duration::from_millis(42));
    assert!(by("count(*)").success && by("count(*)").error.is_none() && by("count(*)").affected.is_none());
    assert!(!f.id.is_empty() && !f.when.is_empty());
}

#[tokio::test]
async fn what_the_client_saves_has_the_shape_the_desktop_writes() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    core.save_history(&new_entry("conn-a", "local pg", "select 1")).await.unwrap();
    core.save_history(&new_entry("conn-a", "local pg", "-- c\ndelete from t where id = 1")).await.unwrap();
    core.save_history(&new_entry("conn-a", "local pg", "create table x (a int)")).await.unwrap();
    let storage = desktop_storage(&s).await;
    let found = storage
        .search_history_entries(dbx_core::history::HistorySearchRequest { limit: 50, ..Default::default() })
        .await
        .expect("the desktop's own search");
    assert_eq!(found.entries.len(), 3);
    for e in &found.entries {
        assert_eq!(e.source, "sql");
        assert_eq!(e.connection_id, "conn-a");
        // JavaScript's `toISOString`: `2026-11-25T10:00:00.000Z`.
        let t = &e.executed_at;
        assert!(t.len() == 24 && t.ends_with('Z') && t.as_bytes()[10] == b'T' && t.as_bytes()[19] == b'.', "{t}");
        assert!(!e.id.is_empty());
        assert_eq!(e.execution_time_ms, 42);
        assert!(e.mcp_tool_name.is_none() && e.rollback_sql.is_none());
    }
    let kind = |sql: &str| found.entries.iter().find(|e| e.sql.contains(sql)).unwrap().activity_kind.clone();
    assert_eq!(kind("select 1"), "query");
    assert_eq!(kind("delete from"), "data_change", "the desktop's panel files it by this");
    assert_eq!(kind("create table"), "schema_change");
}

#[tokio::test]
async fn an_entry_written_the_desktops_way_is_read_back_and_shown_correctly() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    let storage = desktop_storage(&s).await;
    let desktop = |id: &str, connection_id: &str, name: &str, sql: &str, at: &str, source: &str| dbx_core::history::HistoryEntry {
        id: id.into(),
        connection_id: connection_id.into(),
        connection_name: name.into(),
        database: "pagila".into(),
        sql: sql.into(),
        executed_at: at.into(),
        execution_time_ms: 1234,
        success: false,
        error: Some("duplicate key value violates unique constraint".into()),
        activity_kind: "data_change".into(),
        operation: "UPDATE".into(),
        target: String::new(),
        affected_rows: Some(4),
        rollback_sql: None,
        details_json: None,
        source: source.into(),
        mcp_tool_name: None,
        mcp_request_json: None,
        mcp_response_json: None,
        mcp_session_id: None,
    };
    let stamp = "2026-11-24T08:30:15.123Z";
    storage.save_history_entry(&desktop("d-1", "conn-a", "local pg", "update film\n  set title = 'x'\n where id = 1", stamp, "sql")).await.unwrap();
    // An entry an old desktop wrote has no connection id.
    storage.save_history_entry(&desktop("d-2", "", "legacy pg", "select 'legacy'", "2026-11-23T08:30:15.000Z", "sql")).await.unwrap();
    // The MCP server's entries are not ours.
    storage.save_history_entry(&desktop("d-3", "conn-a", "local pg", "select 'from mcp'", "2026-11-25T08:30:15.000Z", "mcp")).await.unwrap();
    // Text another program wrote can hold anything.
    storage.save_history_entry(&desktop("d-4", "conn-a", "local pg", "select '\u{1b}[2J'", "not a time at all, really", "sql")).await.unwrap();

    let found = core.history(&search("", None)).await.expect("search");
    let ids: Vec<&str> = found.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["d-4", "d-1", "d-2"], "newest first by the stored time; the MCP entry is not shown");
    assert_eq!(found.total, 3);

    let e = &found.entries[1];
    assert_eq!((e.connection.0.as_str(), e.connection_name.as_str(), e.database.as_str()), ("conn-a", "local pg", "pagila"));
    assert_eq!(e.sql, "update film\n  set title = 'x'\n where id = 1", "the statement is exactly as stored");
    assert!(!e.success);
    assert_eq!(e.error.as_deref(), Some("duplicate key value violates unique constraint"));
    assert_eq!((e.elapsed, e.affected), (Duration::from_millis(1234), Some(4)));
    let local = chrono::DateTime::parse_from_rfc3339(stamp).unwrap().with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();
    assert_eq!(e.when, local, "shown in local time, to the minute");

    let legacy = &found.entries[2];
    assert_eq!((legacy.connection.0.as_str(), legacy.connection_name.as_str()), ("", "legacy pg"));
    // A time that is not one is shown cut, not hidden; the text is kept whole for the painter.
    assert_eq!(found.entries[0].when, "not a time at al");
    assert!(found.entries[0].sql.contains('\u{1b}'));

    // This connection's id does not match the legacy entry (it has none).
    let mine = core.history(&search("", Some("conn-a"))).await.unwrap();
    assert_eq!(mine.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["d-4", "d-1"]);
}

#[tokio::test]
async fn pages_follow_the_cursor_and_the_total_counts_every_page() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    for i in 0..5 {
        core.save_history(&new_entry("conn-a", "local pg", &format!("select {i}"))).await.unwrap();
        pause().await;
    }
    let first = core.history(&HistoryQuery { limit: 2, ..search("", None) }).await.unwrap();
    assert_eq!(first.entries.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(), ["select 4", "select 3"]);
    assert_eq!(first.total, 5, "the total counts every page");
    let cursor: HistoryCursor = first.next.clone().expect("more");
    let second = core.history(&HistoryQuery { limit: 2, cursor: Some(cursor), ..search("", None) }).await.unwrap();
    assert_eq!(second.entries.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(), ["select 2", "select 1"]);
    let third = core.history(&HistoryQuery { limit: 2, cursor: second.next.clone(), ..search("", None) }).await.unwrap();
    assert_eq!(third.entries.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(), ["select 0"]);
    assert_eq!(third.next, None);
}

#[tokio::test]
async fn the_store_trims_to_the_desktops_retention_setting_when_a_save_lands() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    desktop_storage(&s).await.save_history_retention_limit(200).await.expect("the desktop's setting");
    for i in 0..203 {
        core.save_history(&new_entry("conn-a", "local pg", &format!("select {i}"))).await.unwrap();
    }
    assert_eq!(core.history(&search("", None)).await.unwrap().total, 200, "the client set no limit of its own");
}

#[tokio::test]
async fn the_operation_is_the_first_keyword_after_comments_like_the_desktops_and_the_error_keeps_its_lines() {
    let s = Scratch::new();
    let core = s.core();
    core.connections().await.expect("open");
    for sql in ["select 1", "-- c\n  delete from t", "/* x */ ; Update t set a = 1", "(select 1)", "-- only a comment"] {
        core.save_history(&new_entry("conn-a", "local pg", sql)).await.unwrap();
    }
    let failed = NewHistoryEntry { success: false, error: Some("line one\nline two".into()), ..new_entry("conn-a", "local pg", "insert into t values (1)") };
    core.save_history(&failed).await.unwrap();
    let storage = desktop_storage(&s).await;
    let found = storage.search_history_entries(dbx_core::history::HistorySearchRequest { limit: 50, ..Default::default() }).await.unwrap();
    let op = |sql: &str| found.entries.iter().find(|e| e.sql == sql).unwrap().operation.clone();
    assert_eq!(op("select 1"), "SELECT");
    assert_eq!(op("-- c\n  delete from t"), "DELETE");
    assert_eq!(op("/* x */ ; Update t set a = 1"), "UPDATE");
    assert_eq!(op("(select 1)"), "SQL");
    assert_eq!(op("-- only a comment"), "SQL");
    assert_eq!(op("insert into t values (1)"), "INSERT");
    let e = found.entries.iter().find(|e| e.sql.starts_with("insert")).unwrap();
    assert_eq!(e.error.as_deref(), Some("line one\nline two"));
}

/// dbpane's own store keeps its key in `<store>/.dbx/secret.key` and never
/// asks the keychain. Safe because: `Scratch` holds the process lock and
/// points `HOME` at a scratch directory; the environment's key variables are
/// removed (this is the one test that runs with none); and the options come
/// from `StoreChoice::Own`, whose policy is dbx's `ManagedDataDir` (asserted
/// via the key source), which reads only the file inside the store and never
/// the keychain or the per-user key file.
#[tokio::test]
async fn dbpanes_own_store_makes_and_keeps_its_key_file_and_names_it_when_it_goes() {
    use std::os::unix::fs::PermissionsExt;
    use dbx_tui::core::{KeySource, StoreChoice};

    let s = Scratch::new();
    s.use_no_key();
    std::env::remove_var("XDG_DATA_HOME");
    let home = s.tmp.path().join("home");
    let options = || {
        let home = home.clone();
        StoreOptions::resolve(&StoreChoice::Own, move |name| (name == "HOME").then(|| home.clone().into_os_string()), |_| false).unwrap()
    };
    let first = options();
    assert_eq!(first.key_source, KeySource::StoreFile, "the own store's key is a file in the store");
    assert!(first.data_dir.starts_with(&home), "{}", first.data_dir.display());
    let dir = first.data_dir.clone();

    let core = DbxCore::new(first);
    let saved = core.save_connection(&draft("own")).await.expect("first run makes the store and its key");
    let key = dir.join(".dbx/secret.key");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert!(key.is_file(), "the key file was made");
    assert_eq!(mode(&key), 0o600);
    assert_eq!(mode(&dir.join(".dbx")), 0o700);

    let listed = DbxCore::new(options()).connections().await.expect("a second open reads the same key");
    assert_eq!(listed, vec![saved]);

    std::fs::remove_file(&key).unwrap();
    let err = DbxCore::new(options()).connections().await.expect_err("the key is gone, the data is encrypted");
    assert_eq!(err.kind, CoreErrorKind::Key, "{err:?}");
    assert!(err.message.contains(".dbx/secret.key") && err.message.contains("DBX_SECRET_KEY_FILE"), "{}", err.message);
    assert!(!err.message.contains(PASSWORD));
}
