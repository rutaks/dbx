//! Layer 1 against the Docker fixtures: `DbxCore` saving, editing and testing
//! connections to a real Postgres.
//!
//! Needs `tests/fixtures/up.sh` first (Postgres on 127.0.0.1:55433, user
//! `postgres`, password `dbxtui`). Run:
//! `cargo test -p dbx-tui --test live -- --test-threads=1`
//!
//! Scratch store, scratch key and scratch HOME, as in `store.rs` (see
//! `tests/common/mod.rs`): never the keychain, never a real store.

mod common;

use common::*;
use dbx_tui::core::{ConnectionDraft, Core, CoreErrorKind};

const FIXTURE_PASSWORD: &str = "dbxtui";
const WRONG_PASSWORD: &str = "wrong-pw-Qx7-never-shown";

fn fixture(name: &str, password: &str) -> ConnectionDraft {
    ConnectionDraft { password: Some(password.into()), production: false, ..draft(name) }
}

/// Fails with a pointer to `up.sh` when the fixtures are not up.
async fn require_fixtures() {
    let up = tokio::net::TcpStream::connect("127.0.0.1:55433").await.is_ok();
    assert!(up, "the fixture Postgres is not listening on 127.0.0.1:55433: run crates/dbx-tui/tests/fixtures/up.sh");
}

#[tokio::test]
async fn a_saved_connection_runs_after_the_store_is_reopened_which_proves_the_password_round_tripped() {
    require_fixtures().await;
    let s = Scratch::new();
    let saved = s.core().save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.expect("save");

    // A new process: dbx reads the store and decrypts the password with the key.
    let reopened = s.core();
    let list = reopened.connections().await.expect("reopen");
    assert_eq!(list, vec![saved.clone()]);
    reopened.probe_select_one(&saved.id).await.expect("a run succeeds with the decrypted password");
}

#[tokio::test]
async fn a_test_with_the_right_password_succeeds_and_leaves_nothing_behind() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    core.test_connection(&fixture("unsaved", FIXTURE_PASSWORD)).await.expect("the fixture accepts it");
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new(), "no temporary entry");
    assert_eq!(core.connections().await.unwrap(), vec![], "and nothing was saved");
}

#[tokio::test]
async fn a_test_with_a_wrong_password_fails_with_dbxs_message_and_leaves_no_temporary_entry() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let err = core.test_connection(&fixture("unsaved", WRONG_PASSWORD)).await.expect_err("wrong password");
    assert_eq!(err.kind, CoreErrorKind::Connect, "{err:?}");
    assert!(err.message.to_lowercase().contains("password authentication failed"), "dbx's message: {}", err.message);
    assert!(!err.message.contains(WRONG_PASSWORD), "no secret in it: {}", err.message);
    assert!(!format!("{err:?}").contains(WRONG_PASSWORD));
    assert_eq!(core.runtime_config_ids().await, Vec::<String>::new(), "dbx's runtime list has no temporary entry");
    assert_eq!(s.core().connections().await.unwrap(), vec![]);
}

#[tokio::test]
async fn testing_an_edit_with_a_blank_password_uses_the_stored_password() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    let blank = ConnectionDraft { id: Some(saved.id.clone()), password: None, ..fixture("pagila", "") };
    core.test_connection(&blank).await.expect("the stored password is used");
    let wrong = ConnectionDraft { password: Some(WRONG_PASSWORD.into()), ..blank };
    core.test_connection(&wrong).await.expect_err("a typed password wins over the stored one");
    assert_eq!(core.runtime_config_ids().await, vec![saved.id.0.clone()], "only the saved connection is listed");
}

#[tokio::test]
async fn editing_the_port_of_a_connected_connection_makes_the_next_run_use_the_new_port() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    core.probe_select_one(&saved.id).await.expect("connected: a pool for port 55433 now exists");

    // Nothing listens on port 1. If the old pool were kept, this run would still succeed.
    let edit = |port| ConnectionDraft { id: Some(saved.id.clone()), port: Some(port), password: None, ..fixture("pagila", "") };
    core.save_connection(&edit(1)).await.expect("edit the port");
    let err = core.probe_select_one(&saved.id).await.expect_err("the next run uses port 1, not the old pool");
    assert_eq!(err.kind, CoreErrorKind::Connect);

    core.save_connection(&edit(55433)).await.expect("edit it back");
    core.probe_select_one(&saved.id).await.expect("and the next run is back on the fixture");
}

#[tokio::test]
async fn editing_only_the_name_of_a_connected_connection_keeps_it_connected() {
    require_fixtures().await;
    let s = Scratch::new();
    let core = s.core();
    let saved = core.save_connection(&fixture("pagila", FIXTURE_PASSWORD)).await.unwrap();
    core.probe_select_one(&saved.id).await.unwrap();
    let rename = ConnectionDraft { id: Some(saved.id.clone()), password: None, ..fixture("renamed", "") };
    core.save_connection(&rename).await.expect("rename");
    core.probe_select_one(&saved.id).await.expect("still works");
}
