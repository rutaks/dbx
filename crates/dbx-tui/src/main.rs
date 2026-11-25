use std::sync::Arc;

use dbx_tui::core::{DbxCore, StoreChoice, StoreOptions};
use dbx_tui::shell::{self, cli};

fn main() {
    // First, so a panic before the terminal is entered also exits 101.
    shell::install_panic_hook();
    let choice = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Cli::Run(choice)) => choice,
        Ok(cli::Cli::Version) => return println!("{}", cli::version_text()),
        Ok(cli::Cli::Licenses) => return print!("{}", cli::licenses_text()),
        Ok(cli::Cli::Help) => {
            // The store in force is the one the environment (and nothing
            // else) would pick: the same resolver the client uses.
            let store = StoreOptions::from_env(&StoreChoice::Auto).map(|o| o.info()).map_err(|e| e.message);
            return print!("{}", cli::help_text(store));
        }
        // `dbpane kitty-setup [--write]` prints where kitty's include file is.
        Ok(cli::Cli::KittySetup { write }) => {
            match shell::kitty::setup(write, &|k| std::env::var(k).ok()) {
                Ok(text) => print!("{text}"),
                Err(e) => {
                    eprintln!("dbpane: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Err(e) => {
            eprintln!("dbpane: {e}");
            std::process::exit(2);
        }
    };
    shell::install_crypto_provider();
    let runtime = match shell::runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("dbpane: cannot start the async runtime: {e}");
            std::process::exit(1);
        }
    };
    // Where the store is comes from `--store` and the environment, read in one
    // place. A failure there, like any store failure, is shown in the client.
    let core = Arc::new(DbxCore::from_env(&choice));
    // The layout preferences sit beside the store, if the store is known.
    let prefs = core.store_dir().map(shell::prefs::path_in);
    let store = core.store_info();
    let result = runtime.block_on(shell::run(core, prefs, store));
    // The terminal is already restored (the guard is dropped inside `run`). Do not wait for Core calls still in
    // flight: quitting is immediate.
    runtime.shutdown_background();
    match result {
        Ok(Some(note)) => eprintln!("dbpane: {note}"),
        Ok(None) => {}
        Err(e) => {
            eprintln!("dbpane: {e}");
            std::process::exit(1);
        }
    }
}
