use std::sync::Arc;

use dbx_tui::core::DbxCore;

#[tokio::main]
async fn main() {
    if let Err(e) = dbx_tui::shell::run(Arc::new(DbxCore::new())).await {
        eprintln!("dbx-tui: {e}");
        std::process::exit(1);
    }
}
