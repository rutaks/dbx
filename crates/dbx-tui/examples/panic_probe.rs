//! Enters the terminal exactly as the client does, then panics: on the main
//! thread by default, or inside a spawned task with the argument `task`. The
//! journey `panic-restores-terminal.sh` runs it to check the shell comes back
//! and the process exits instead of carrying on.

use std::time::Duration;

use dbx_tui::shell::TerminalGuard;

#[tokio::main]
async fn main() {
    let _guard = TerminalGuard::enter().expect("terminal");
    if std::env::args().nth(1).as_deref() == Some("task") {
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            panic!("probe panic in task");
        });
        // A client keeps running after a task dies; so does this, until the
        // panic hook stops the process.
        tokio::time::sleep(Duration::from_secs(5)).await;
        return;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    panic!("probe panic");
}
