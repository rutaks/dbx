//! Where the config file is, and reading it (slice 22). The text is parsed by
//! `app::config`; this is the part that touches the environment and the disk.
//!
//! The file is `$DBPANE_CONFIG` (`$DBX_TUI_CONFIG`, the name the variables had
//! before the rename, still works), else `$XDG_CONFIG_HOME/dbpane/config.toml`,
//! else `~/.config/dbpane/config.toml`: the same on macOS and Linux, as herdr
//! does, because it is a file people edit by hand and keep in their dotfiles,
//! where terminal tools look for it. (The store, which people do not edit, is in
//! `~/Library/Application Support/dbpane` on macOS.) A relative `HOME` or
//! `XDG_CONFIG_HOME` is ignored, as the store's are.

use std::path::{Path, PathBuf};

use crate::app::config::{self, Loaded, Problem};

/// The most a config file may be: a file this size is a mistake, not a config.
const MAX_BYTES: u64 = 1024 * 1024;

/// Where the path came from, for what is said when the file is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A variable the user set to name the file.
    Env(&'static str),
    /// The default place, in the config directory.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub from: Origin,
}

/// The variables that name the file, the dbpane one first.
const PATH_VARS: [&str; 2] = ["DBPANE_CONFIG", "DBX_TUI_CONFIG"];
/// The variables that name the theme, the dbpane one first.
const THEME_VARS: [&str; 2] = ["DBPANE_THEME", "DBX_TUI_THEME"];

fn set(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name).filter(|v| !v.is_empty())
}

/// Where the config file is, or why it cannot be known.
pub fn locate(env: &dyn Fn(&str) -> Option<String>) -> Result<ConfigFile, String> {
    for var in PATH_VARS {
        if let Some(path) = set(env, var) {
            return Ok(ConfigFile { path: PathBuf::from(path), from: Origin::Env(var) });
        }
    }
    let absolute = |name: &str| set(env, name).map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = match (absolute("XDG_CONFIG_HOME"), absolute("HOME")) {
        (Some(xdg), _) => xdg,
        (None, Some(home)) => home.join(".config"),
        (None, None) => return Err("HOME is not set to an absolute path, so there is no default config location. Set DBPANE_CONFIG to the file.".to_string()),
    };
    Ok(ConfigFile { path: dir.join("dbpane").join("config.toml"), from: Origin::Default })
}

/// The theme the environment names (`DBPANE_THEME`, else `DBX_TUI_THEME`): it
/// wins over the file, which is how tests pick one.
pub fn env_theme(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    THEME_VARS.iter().find_map(|v| set(env, v))
}

/// Reads and parses the file. A file that is not there is the defaults, and
/// not a problem, unless the user named it. Whether it was found is returned.
pub fn read(file: &ConfigFile) -> (Loaded, bool) {
    let fail = |message: String| Loaded { config: config::Config::default(), problems: vec![Problem { line: 0, column: 0, message }] };
    let path = file.path.as_path();
    match read_text(path) {
        Ok(text) => (config::parse(&text), true),
        Err(Failure::Missing) => match file.from {
            Origin::Default => (config::parse(""), false),
            Origin::Env(var) => (fail(format!("{var} names {}, which does not exist. dbpane started with the defaults.", path.display())), false),
        },
        Err(Failure::Other(why)) => (fail(format!("cannot read the config file {}: {why}. dbpane started with the defaults.", path.display())), true),
    }
}

enum Failure {
    Missing,
    Other(String),
}

fn read_text(path: &Path) -> Result<String, Failure> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Failure::Missing,
        _ => Failure::Other(e.to_string()),
    })?;
    if !meta.is_file() {
        return Err(Failure::Other("it is not a file".to_string()));
    }
    if meta.len() > MAX_BYTES {
        return Err(Failure::Other(format!("it is {} bytes, over the {MAX_BYTES} a config file may be", meta.len())));
    }
    // The file may have grown since the check: never read more than the limit.
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|e| Failure::Other(e.to_string()))?;
    std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_BYTES + 1), &mut bytes).map_err(|e| Failure::Other(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Failure::Other(format!("it is over the {MAX_BYTES} bytes a config file may be")));
    }
    String::from_utf8(bytes).map_err(|_| Failure::Other("it is not valid UTF-8 text".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_file_is_the_variable_then_xdg_then_home_dot_config_on_every_platform() {
        let at = |pairs: &[(&str, &str)]| locate(&env_of(pairs));
        let own = at(&[("DBPANE_CONFIG", "/etc/mine.toml"), ("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/a")]).unwrap();
        assert_eq!((own.path, own.from), (PathBuf::from("/etc/mine.toml"), Origin::Env("DBPANE_CONFIG")));
        let old = at(&[("DBX_TUI_CONFIG", "/etc/old.toml"), ("HOME", "/home/a")]).unwrap();
        assert_eq!((old.path, old.from), (PathBuf::from("/etc/old.toml"), Origin::Env("DBX_TUI_CONFIG")), "the old name still works");
        let both = at(&[("DBX_TUI_CONFIG", "/old.toml"), ("DBPANE_CONFIG", "/new.toml")]).unwrap();
        assert_eq!(both.path, PathBuf::from("/new.toml"), "the dbpane name wins");
        let xdg = at(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/a")]).unwrap();
        assert_eq!((xdg.path, xdg.from), (PathBuf::from("/xdg/dbpane/config.toml"), Origin::Default));
        let home = at(&[("HOME", "/home/a")]).unwrap();
        assert_eq!(home.path, PathBuf::from("/home/a/.config/dbpane/config.toml"), "also on macOS");
        let empty = at(&[("DBPANE_CONFIG", ""), ("HOME", "/home/a")]).unwrap();
        assert_eq!(empty.from, Origin::Default,"an empty variable is not set");
    }

    #[test]
    fn a_relative_home_or_xdg_directory_is_ignored_and_without_either_there_is_no_default() {
        let at = |pairs: &[(&str, &str)]| locate(&env_of(pairs));
        assert_eq!(at(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/a")]).unwrap().path, PathBuf::from("/home/a/.config/dbpane/config.toml"));
        let e = at(&[("HOME", "rel")]).unwrap_err();
        assert!(e.contains("DBPANE_CONFIG"), "{e}");
        assert!(at(&[]).is_err());
    }

    #[test]
    fn the_theme_comes_from_the_dbpane_variable_then_the_old_one() {
        let at = |pairs: &[(&str, &str)]| env_theme(&env_of(pairs));
        assert_eq!(at(&[("DBX_TUI_THEME", "terminal")]).as_deref(), Some("terminal"));
        assert_eq!(at(&[("DBX_TUI_THEME", "terminal"), ("DBPANE_THEME", "intellij-dark")]).as_deref(), Some("intellij-dark"));
        assert_eq!(at(&[("DBPANE_THEME", "")]), None);
        assert_eq!(at(&[]), None);
    }

    fn file_in(dir: &tempfile::TempDir, name: &str, from: Origin) -> ConfigFile {
        ConfigFile { path: dir.path().join(name), from }
    }

    #[test]
    fn a_file_is_read_and_parsed_and_a_missing_default_is_the_defaults_without_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[run]\nrow_cap = 250\nbogus = 1\n").unwrap();
        let (loaded, found) = read(&file_in(&dir, "config.toml", Origin::Default));
        assert!(found);
        assert_eq!(loaded.config.row_cap, 250);
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].line, 3);

        let (loaded, found) = read(&file_in(&dir, "absent.toml", Origin::Default));
        assert!(!found && loaded.problems.is_empty());
        assert_eq!(loaded.config, config::Config::default());
    }

    #[test]
    fn a_named_file_that_is_missing_unreadable_or_too_big_is_one_problem_and_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (loaded, found) = read(&file_in(&dir, "absent.toml", Origin::Env("DBPANE_CONFIG")));
        assert!(!found);
        assert_eq!(loaded.problems.len(), 1);
        assert!(loaded.problems[0].message.contains("DBPANE_CONFIG") && loaded.problems[0].message.contains("does not exist"), "{:?}", loaded.problems);
        assert_eq!(loaded.problems[0].line, 0, "a problem with no place in the file");
        assert_eq!(loaded.config, config::Config::default());

        std::fs::create_dir(dir.path().join("a_dir")).unwrap();
        let (loaded, _) = read(&file_in(&dir, "a_dir", Origin::Default));
        assert!(loaded.problems[0].message.contains("not a file"), "{:?}", loaded.problems);

        std::fs::write(dir.path().join("binary.toml"), [0xff, 0xfe, 0x00]).unwrap();
        let (loaded, _) = read(&file_in(&dir, "binary.toml", Origin::Default));
        assert!(loaded.problems[0].message.contains("UTF-8"), "{:?}", loaded.problems);

        std::fs::write(dir.path().join("big.toml"), vec![b'#'; MAX_BYTES as usize + 1]).unwrap();
        let (loaded, _) = read(&file_in(&dir, "big.toml", Origin::Default));
        assert!(loaded.problems[0].message.contains("bytes"), "{:?}", loaded.problems);
        assert_eq!(loaded.config, config::Config::default());
    }
}
