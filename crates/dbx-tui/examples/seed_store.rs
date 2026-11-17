//! Saves one connection into a store, for journeys and by-hand checks against
//! scratch stores. Goes through `DbxCore`, so it writes exactly as the client
//! would (encrypted with the key dbx resolves: set `DBX_SECRET_KEY_FILE`).
//!
//! Usage: seed_store DATA_DIR NAME KIND HOST PORT [DATABASE]
//! KIND is postgres, mysql or sqlite. The password saved is a fixed dummy.
//! DATA_DIR must be under the system temp directory (or set
//! `DBX_TUI_ALLOW_SEED=1`); the desktop app's store is always refused.

use dbx_tui::core::{ConnectionDraft, DbKind, DbxCore, StoreOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 5 {
        eprintln!("usage: seed_store DATA_DIR NAME KIND HOST PORT [DATABASE]");
        std::process::exit(2);
    }
    if std::env::var_os("DBX_SECRET_KEY_FILE").is_none() && std::env::var_os("DBX_SECRET_KEY").is_none() {
        eprintln!("seed_store: refusing to run without DBX_SECRET_KEY_FILE or DBX_SECRET_KEY (it would use the keychain)");
        std::process::exit(2);
    }
    let allow = std::env::var("DBX_TUI_ALLOW_SEED").as_deref() == Ok("1");
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Err(why) = dbx_tui::core::check_seed_dir(std::path::Path::new(&args[0]), home.as_deref(), allow) {
        eprintln!("seed_store: {why}");
        std::process::exit(2);
    }
    let kind = match args[2].as_str() {
        "postgres" => DbKind::Postgres,
        "mysql" => DbKind::MySql,
        "sqlite" => DbKind::Sqlite,
        other => DbKind::Other(other.to_string()),
    };
    let draft = ConnectionDraft {
        id: None,
        name: args[1].clone(),
        kind,
        host: args[3].clone(),
        port: args[4].parse().ok(),
        database: args.get(5).cloned(),
        username: "postgres".into(),
        password: Some("seeded-dummy-password".into()),
        production: false,
    };
    dbx_tui::shell::install_crypto_provider();
    let rt = dbx_tui::shell::runtime().expect("runtime");
    if let Err(e) = std::fs::create_dir_all(&args[0]) {
        eprintln!("seed_store: cannot create {}: {e}", args[0]);
        std::process::exit(1);
    }
    let options = StoreOptions::from_vars(|name| match name {
        "DBX_DATA_DIR" => Some(args[0].clone().into()),
        other => std::env::var_os(other),
    });
    let core = match options {
        Ok(options) => DbxCore::new(options),
        Err(e) => {
            eprintln!("seed_store: {e}");
            std::process::exit(2);
        }
    };
    match rt.block_on(core.seed_connection(&draft)) {
        Ok(c) => println!("seeded {} ({})", c.name, c.id.0),
        Err(e) => {
            eprintln!("seed_store: {e}");
            std::process::exit(1);
        }
    }
}
