//! Records the dbx commit the binary is built from, for `dbpane --version`.
//! `DBPANE_DBX_COMMIT` overrides it (a build from a source archive, where
//! there is no git history to ask).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DBPANE_DBX_COMMIT");
    // `logs/HEAD` gains a line at every commit; `HEAD` alone changes only when the branch does.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    let commit = std::env::var("DBPANE_DBX_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            let out = Command::new("git").args(["rev-parse", "--short=12", "HEAD"]).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=DBPANE_DBX_COMMIT={commit}");
}
