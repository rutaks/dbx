//! Which store dbpane is using, and why. Plain data: `app` draws it on the
//! welcome screen and `main` prints it in `--help`; `DbxCore` decides it
//! (`StoreOptions::resolve` in `core/dbx.rs`).

use std::path::PathBuf;

/// Which store the person asked for (`--store`), or left to dbpane.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StoreChoice {
    /// dbpane's own store if it exists, else the dbx desktop app's if it
    /// exists, else dbpane's own (made on first use).
    #[default]
    Auto,
    /// `--store dbx`: the dbx desktop app's store.
    Dbx,
    /// `--store own`: dbpane's own store.
    Own,
    /// `--store <dir>`: a directory.
    Dir(PathBuf),
}

/// Whose store it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// The dbx desktop app's, shared with it.
    Shared,
    /// dbpane's own.
    Own,
    /// Named by a directory.
    Named,
}

/// How it came to be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreSource {
    /// dbpane looked: the dbx desktop app's store exists, or it does not.
    Auto,
    /// `--store`.
    Flag,
    /// `DBX_DATA_DIR`.
    Env,
}

/// The store in force: what the welcome screen and `--help` say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreInfo {
    pub kind: StoreKind,
    pub source: StoreSource,
    pub path: PathBuf,
}

impl StoreInfo {
    /// A short account of whose store this is and why, without the path.
    pub fn describe(&self) -> &'static str {
        match (self.kind, self.source) {
            (StoreKind::Shared, StoreSource::Auto) => "the dbx desktop app's store (found, so shared with it)",
            (StoreKind::Shared, _) => "the dbx desktop app's store (--store dbx)",
            (StoreKind::Own, StoreSource::Auto) => "dbpane's own store (kept: it exists, or no dbx desktop store was found)",
            (StoreKind::Own, _) => "dbpane's own store (--store own)",
            (StoreKind::Named, StoreSource::Env) => "the store DBX_DATA_DIR names",
            (StoreKind::Named, _) => "the store --store names",
        }
    }
}

use std::ffi::OsString;
use std::path::Path;

use super::{CoreError, CoreErrorKind, KeySource, StoreOptions};

/// The directory the dbx desktop app keeps its data in (Tauri's
/// `app_data_dir` for `com.dbx.app`), and the one dbpane keeps its own in.
fn data_dir(app: &str, home: Option<PathBuf>, xdg_data_home: Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support").join(app));
    }
    xdg_data_home.or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join(app))
}

/// Where the dbx desktop app keeps its store.
pub(super) fn desktop_data_dir(home: Option<PathBuf>, xdg_data_home: Option<PathBuf>) -> Option<PathBuf> {
    data_dir("com.dbx.app", home, xdg_data_home)
}

/// Where dbpane keeps a store of its own.
pub(super) fn own_data_dir(home: Option<PathBuf>, xdg_data_home: Option<PathBuf>) -> Option<PathBuf> {
    data_dir("dbpane", home, xdg_data_home)
}

/// Whether `dir` holds a dbx store (a `dbx.db` file).
pub fn has_dbx_store(dir: &Path) -> bool {
    dir.join("dbx.db").is_file()
}

impl StoreOptions {
    /// Chooses the store. In order: `--store` (`choice`), `DBX_DATA_DIR`, then
    /// dbpane's own store if it already has one (`has_store` says whether a
    /// directory holds a `dbx.db`), else the dbx desktop app's if it has one,
    /// else dbpane's own (made on first use). Reads the environment
    /// only through `var` and the file system only through `has_store`.
    pub fn resolve(
        choice: &StoreChoice,
        var: impl Fn(&str) -> Option<OsString>,
        has_store: impl Fn(&Path) -> bool,
    ) -> Result<Self, CoreError> {
        let set = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        // dbx itself tests presence, not emptiness, for the key variables.
        let key_source = if var("DBX_SECRET_KEY_FILE").is_some() {
            KeySource::EnvFile
        } else if var("DBX_SECRET_KEY").is_some() {
            KeySource::EnvValue
        } else {
            KeySource::Platform
        };
        let named = |dir: PathBuf, source: StoreSource, given_by: &str| {
            if !dir.is_absolute() {
                return Err(CoreError::new(
                    CoreErrorKind::Store,
                    format!(
                        "{given_by} is {:?}, which is not an absolute path (a leading ~ is not expanded). \
                         Set it to the full path of a store directory, such as /Users/you/dbpane-data.",
                        dir.as_os_str()
                    ),
                ));
            }
            Ok(StoreOptions { data_dir: dir, create_if_missing: false, key_source, kind: StoreKind::Named, source })
        };
        let at = |dir: Option<PathBuf>, kind: StoreKind, source: StoreSource| {
            let data_dir = dir.ok_or_else(|| {
                CoreError::new(
                    CoreErrorKind::Store,
                    "dbpane cannot tell where its store is: HOME is not set. \
                     Set DBX_DATA_DIR, or start with --store <dir>, to name a store directory.",
                )
            })?;
            // Without a key from the environment, dbpane's own store keeps
            // its key in a file inside it (`StoreOptions::key_policy`).
            let key_source = match (kind, key_source) {
                (StoreKind::Own, KeySource::Platform) => KeySource::StoreFile,
                (_, other) => other,
            };
            Ok(StoreOptions { data_dir, create_if_missing: true, key_source, kind, source })
        };
        // A relative HOME or XDG_DATA_HOME would put the store wherever the
        // process happens to start: treat it as unset (the XDG spec says the
        // same of a relative XDG_DATA_HOME).
        let absolute = |name: &str| set(name).filter(|p| p.is_absolute());
        let home = || (absolute("HOME"), absolute("XDG_DATA_HOME"));
        match choice {
            StoreChoice::Dir(dir) => return named(dir.clone(), StoreSource::Flag, "--store"),
            StoreChoice::Dbx => {
                let (h, x) = home();
                return at(desktop_data_dir(h, x), StoreKind::Shared, StoreSource::Flag);
            }
            StoreChoice::Own => {
                let (h, x) = home();
                return at(own_data_dir(h, x), StoreKind::Own, StoreSource::Flag);
            }
            StoreChoice::Auto => {}
        }
        if let Some(dir) = set("DBX_DATA_DIR") {
            return named(dir, StoreSource::Env, "DBX_DATA_DIR");
        }
        let (h, x) = home();
        // Sticky: once dbpane has made its own store it stays in use, even if
        // the dbx desktop app is installed (and makes a store) later.
        let own = own_data_dir(h.clone(), x.clone());
        if own.as_deref().is_some_and(&has_store) {
            return at(own, StoreKind::Own, StoreSource::Auto);
        }
        match desktop_data_dir(h, x).filter(|d| has_store(d)) {
            Some(desktop) => at(Some(desktop), StoreKind::Shared, StoreSource::Auto),
            None => at(own, StoreKind::Own, StoreSource::Auto),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup over fixed variables.
    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(*v))
    }

    const HOME: &str = "/scratch/home";

    fn desktop() -> PathBuf {
        desktop_data_dir(Some(HOME.into()), None).unwrap()
    }

    fn own() -> PathBuf {
        own_data_dir(Some(HOME.into()), None).unwrap()
    }

    /// `has_store` where exactly these directories hold a `dbx.db`.
    fn stores(dirs: Vec<PathBuf>) -> impl Fn(&Path) -> bool {
        move |dir| dirs.iter().any(|d| d == dir)
    }

    fn resolve(choice: StoreChoice, v: &[(&str, &str)], present: Vec<PathBuf>) -> Result<StoreOptions, CoreError> {
        StoreOptions::resolve(&choice, vars(v), stores(present))
    }

    #[test]
    fn the_own_directory_is_dbpane_beside_the_desktop_apps() {
        if cfg!(target_os = "macos") {
            assert_eq!(own(), PathBuf::from("/scratch/home/Library/Application Support/dbpane"));
            assert_eq!(desktop(), PathBuf::from("/scratch/home/Library/Application Support/com.dbx.app"));
        } else {
            assert_eq!(own(), PathBuf::from("/scratch/home/.local/share/dbpane"));
            assert_eq!(own_data_dir(Some(HOME.into()), Some("/xdg".into())), Some(PathBuf::from("/xdg/dbpane")));
        }
    }

    #[test]
    fn with_a_dbx_store_present_and_nothing_chosen_the_store_is_shared() {
        let o = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![desktop()]).unwrap();
        assert_eq!(o.data_dir, desktop());
        assert_eq!((o.kind, o.source), (StoreKind::Shared, StoreSource::Auto));
        assert!(o.create_if_missing);
    }

    #[test]
    fn dbpanes_own_store_once_it_exists_stays_the_one_even_when_a_dbx_store_appears() {
        let both = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![desktop(), own()]).unwrap();
        assert_eq!(both.data_dir, own());
        assert_eq!((both.kind, both.source), (StoreKind::Own, StoreSource::Auto));
    }

    #[test]
    fn a_relative_home_or_xdg_data_home_counts_as_unset() {
        let e = resolve(StoreChoice::Auto, &[("HOME", "relative/home")], vec![]).unwrap_err();
        assert!(e.message.contains("HOME is not set"), "{}", e.message);
        let e = resolve(StoreChoice::Own, &[("HOME", "")], vec![]).unwrap_err();
        assert!(e.message.contains("HOME is not set"), "{}", e.message);
        let o = resolve(StoreChoice::Own, &[("HOME", HOME), ("XDG_DATA_HOME", "rel/xdg")], vec![]).unwrap();
        assert_eq!(o.data_dir, own(), "a relative XDG_DATA_HOME is ignored");
        if !cfg!(target_os = "macos") {
            let o = resolve(StoreChoice::Own, &[("HOME", HOME), ("XDG_DATA_HOME", "/xdg")], vec![]).unwrap();
            assert_eq!(o.data_dir, PathBuf::from("/xdg/dbpane"));
        }
    }

    #[test]
    fn with_no_dbx_store_the_store_is_dbpanes_own() {
        let o = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![]).unwrap();
        assert_eq!(o.data_dir, own());
        assert_eq!((o.kind, o.source), (StoreKind::Own, StoreSource::Auto));
        assert!(o.create_if_missing, "a stranger's first run makes the directory");
        // An own store that already exists stays the one in use.
        let again = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![own()]).unwrap();
        assert_eq!(again.data_dir, own());
    }

    #[test]
    fn store_own_with_a_dbx_store_present_uses_the_own_directory() {
        let o = resolve(StoreChoice::Own, &[("HOME", HOME)], vec![desktop()]).unwrap();
        assert_eq!(o.data_dir, own());
        assert_eq!((o.kind, o.source), (StoreKind::Own, StoreSource::Flag));
    }

    #[test]
    fn store_dbx_uses_the_desktop_directory_even_when_it_has_no_store_yet() {
        let o = resolve(StoreChoice::Dbx, &[("HOME", HOME)], vec![]).unwrap();
        assert_eq!(o.data_dir, desktop());
        assert_eq!((o.kind, o.source), (StoreKind::Shared, StoreSource::Flag));
    }

    #[test]
    fn dbx_data_dir_wins_over_both_automatic_choices() {
        for present in [vec![], vec![desktop()], vec![desktop(), own()]] {
            let o = resolve(StoreChoice::Auto, &[("HOME", HOME), ("DBX_DATA_DIR", "/scratch/named")], present).unwrap();
            assert_eq!(o.data_dir, PathBuf::from("/scratch/named"));
            assert_eq!((o.kind, o.source), (StoreKind::Named, StoreSource::Env));
            assert!(!o.create_if_missing, "a named directory must already exist");
        }
        let empty = resolve(StoreChoice::Auto, &[("HOME", HOME), ("DBX_DATA_DIR", "")], vec![]).unwrap();
        assert_eq!(empty.data_dir, own(), "an empty DBX_DATA_DIR counts as unset");
    }

    #[test]
    fn store_with_a_directory_names_it_and_beats_dbx_data_dir() {
        let o = resolve(StoreChoice::Dir("/scratch/flag".into()), &[("HOME", HOME), ("DBX_DATA_DIR", "/scratch/env")], vec![desktop()]).unwrap();
        assert_eq!(o.data_dir, PathBuf::from("/scratch/flag"));
        assert_eq!((o.kind, o.source), (StoreKind::Named, StoreSource::Flag));
        assert!(!o.create_if_missing);
    }

    #[test]
    fn a_directory_that_is_not_absolute_is_refused_naming_where_it_came_from() {
        let e = resolve(StoreChoice::Dir("relative/store".into()), &[("HOME", HOME)], vec![]).unwrap_err();
        assert_eq!(e.kind, CoreErrorKind::Store);
        assert!(e.message.contains("relative/store") && e.message.contains("absolute") && e.message.contains("--store"), "{}", e.message);
        let e = resolve(StoreChoice::Auto, &[("HOME", HOME), ("DBX_DATA_DIR", "~/store")], vec![]).unwrap_err();
        assert!(e.message.contains("DBX_DATA_DIR") && e.message.contains("absolute"), "{}", e.message);
    }

    #[test]
    fn without_a_home_the_error_says_how_to_choose_a_store() {
        let e = resolve(StoreChoice::Auto, &[], vec![]).unwrap_err();
        assert_eq!(e.kind, CoreErrorKind::Store);
        assert!(e.message.contains("DBX_DATA_DIR") && e.message.contains("--store"), "{}", e.message);
        // A named directory needs no home.
        assert!(resolve(StoreChoice::Dir("/scratch/x".into()), &[], vec![]).is_ok());
    }

    #[test]
    fn the_key_source_follows_the_key_variables() {
        let key = |v: &[(&str, &str)]| resolve(StoreChoice::Dbx, v, vec![]).unwrap().key_source;
        assert_eq!(key(&[("HOME", HOME)]), KeySource::Platform);
        assert_eq!(key(&[("HOME", HOME), ("DBX_SECRET_KEY_FILE", "/k")]), KeySource::EnvFile);
        assert_eq!(key(&[("HOME", HOME), ("DBX_SECRET_KEY", "k")]), KeySource::EnvValue);
        // dbpane's own store keeps its key in a file unless the environment gives one.
        let own = |v: &[(&str, &str)]| resolve(StoreChoice::Own, v, vec![]).unwrap().key_source;
        assert_eq!(own(&[("HOME", HOME)]), KeySource::StoreFile);
        assert_eq!(own(&[("HOME", HOME), ("DBX_SECRET_KEY_FILE", "/k")]), KeySource::EnvFile);
    }

    #[test]
    fn the_store_in_force_is_described_with_its_path() {
        let o = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![]).unwrap();
        let info = o.info();
        assert_eq!(info.path, own());
        assert!(info.describe().contains("dbpane's own store"), "{}", info.describe());
        let shared = resolve(StoreChoice::Auto, &[("HOME", HOME)], vec![desktop()]).unwrap().info();
        assert!(shared.describe().contains("dbx desktop"), "{}", shared.describe());
    }

    #[test]
    fn the_presence_check_on_disk_looks_for_dbx_db_in_a_temp_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!has_dbx_store(tmp.path()));
        std::fs::write(tmp.path().join("dbx.db"), b"").unwrap();
        assert!(has_dbx_store(tmp.path()));
        let nested = tmp.path().join("dbx.db-dir");
        std::fs::create_dir(&nested).unwrap();
        assert!(!has_dbx_store(&nested), "a directory without a dbx.db is not a store");
    }
}
