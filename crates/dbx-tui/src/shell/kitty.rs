//! `dbpane kitty-setup`: where the kitty configuration that releases kitty's
//! own Cmd shortcuts while this client has focus is, and the one line to add to
//! `kitty.conf`. The file ships in `extras/kitty/dbx-tui.conf` and is also
//! compiled in, so `--write` can put a copy in kitty's configuration directory
//! when the source tree is not where the binary runs.

use std::path::{Path, PathBuf};

/// The file as shipped.
pub const CONF: &str = include_str!("../../extras/kitty/dbx-tui.conf");

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("extras/kitty/dbx-tui.conf")
}

/// kitty's configuration directory: `KITTY_CONFIG_DIRECTORY`, else
/// `XDG_CONFIG_HOME/kitty`, else `~/.config/kitty`.
fn kitty_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(d) = env("KITTY_CONFIG_DIRECTORY") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = env("XDG_CONFIG_HOME") {
        return Some(Path::new(&d).join("kitty"));
    }
    env("HOME").map(|h| Path::new(&h).join(".config/kitty"))
}

/// What the subcommand prints, and the file it wrote when asked to.
pub fn setup(write: bool, env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let path = if write {
        let dir = kitty_dir(env).ok_or("cannot find kitty's configuration directory: set KITTY_CONFIG_DIRECTORY")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let path = dir.join("dbx-tui.conf");
        std::fs::write(&path, CONF).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        path
    } else {
        let path = shipped();
        if !path.exists() {
            return Err("the shipped file is not on this machine: run `dbpane kitty-setup --write` to put a copy in kitty's configuration directory".into());
        }
        path
    };
    Ok(format!(
        "{}\n\nAdd this line to kitty.conf, then restart kitty:\n\ninclude {}\n\nIt releases kitty's Cmd shortcuts (cmd+enter, cmd+f, cmd+l, ...) only while dbpane is the focused program.\n",
        if write { "Wrote the file:" } else { "The file:" },
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_names_the_file_and_the_one_include_line() {
        let text = setup(false, &|_| None).expect("the shipped file exists in the source tree");
        assert!(text.lines().any(|l| l.starts_with("include ") && l.ends_with("dbx-tui.conf")), "{text}");
    }

    #[test]
    fn write_puts_a_copy_in_kitty_s_directory() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap().to_string();
        let text = setup(true, &|k| (k == "KITTY_CONFIG_DIRECTORY").then(|| d.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("dbx-tui.conf")).unwrap(), CONF);
        assert!(text.contains(&format!("include {d}/dbx-tui.conf")), "{text}");
    }

    #[test]
    fn the_file_releases_only_cmd_keys_only_while_the_variable_is_set_and_keeps_cmd_v() {
        for line in CONF.lines().filter(|l| l.starts_with("map ")) {
            assert!(line.starts_with("map --when-focus-on var:dbx_tui cmd+"), "{line}");
        }
        assert!(!CONF.lines().any(|l| l.starts_with("map") && l.ends_with("cmd+v")), "cmd+v stays kitty's: it is how paste reaches us");
        for key in ["cmd+enter", "cmd+f", "cmd+l", "cmd+g", "cmd+d", "cmd+a", "cmd+z", "cmd+shift+z", "cmd+x", "cmd+c", "cmd+backspace", "cmd+/"] {
            assert!(CONF.lines().any(|l| l.ends_with(&format!(" {key}"))), "{key}");
        }
    }
}
