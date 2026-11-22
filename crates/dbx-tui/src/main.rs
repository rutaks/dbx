use std::sync::Arc;

use dbx_tui::core::DbxCore;
use dbx_tui::shell;

fn main() {
    // `dbx-tui kitty-setup [--write]` prints where kitty's include file is.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("kitty-setup") {
        let write = args.next().as_deref() == Some("--write");
        match shell::kitty::setup(write, &|k| std::env::var(k).ok()) {
            Ok(text) => print!("{text}"),
            Err(e) => {
                eprintln!("dbx-tui: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    shell::install_crypto_provider();
    let runtime = match shell::runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("dbx-tui: cannot start the async runtime: {e}");
            std::process::exit(1);
        }
    };
    // Where the store is comes from the environment, read in one place. A
    // failure there, like any store failure, is shown in the client.
    let core = Arc::new(DbxCore::from_env());
    // The layout preferences sit beside the store, if the store is known.
    let prefs = core.store_dir().map(shell::prefs::path_in);
    let result = runtime.block_on(shell::run(core, prefs));
    // The terminal is already restored (the guard is dropped inside `run`). Do not wait for Core calls still in
    // flight: quitting is immediate.
    runtime.shutdown_background();
    match result {
        Ok(Some(note)) => eprintln!("dbx-tui: {note}"),
        Ok(None) => {}
        Err(e) => {
            eprintln!("dbx-tui: {e}");
            std::process::exit(1);
        }
    }
}
