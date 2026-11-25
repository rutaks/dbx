//! The command line: what `dbpane` was asked to do, and the text it prints for
//! `--version`, `--help` and `--licenses`. Pure: `main` reads the environment
//! and the file system and passes in what these need.

use crate::core::{StoreChoice, StoreInfo};

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// Start the client against the store `StoreChoice` names.
    Run(StoreChoice),
    Version,
    Help,
    Licenses,
    /// `kitty-setup [--write]`.
    KittySetup { write: bool },
}

/// The version and the dbx commit this binary was built from.
pub fn version_text() -> String {
    format!("dbpane {} (dbx {})", env!("CARGO_PKG_VERSION"), env!("DBPANE_DBX_COMMIT"))
}

/// `--help`, ending with the store that would be used now (`store` is why
/// it cannot be worked out, if it cannot).
pub fn help_text(store: Result<StoreInfo, String>) -> String {
    let in_force = match store {
        Ok(info) => format!("{}:\n  {}", info.describe(), info.path.display()),
        Err(why) => format!("not known: {why}\n  Name one with --store <dir> or DBX_DATA_DIR."),
    };
    format!(
        "dbpane: a terminal database client, built on dbx's core.\n\
         \n\
         Usage:\n\
         \x20 dbpane [--store dbx|own|<dir>]\n\
         \x20 dbpane kitty-setup [--write]\n\
         \x20 dbpane --version | --help | --licenses\n\
         \n\
         Options:\n\
         \x20 --store dbx|own|<dir>  Which store to use: the dbx desktop app's, dbpane's own,\n\
         \x20                        or the directory <dir> (an absolute path that exists).\n\
         \x20                        Without it: DBX_DATA_DIR if set, else dbpane's own store\n\
         \x20                        if it has one, else the dbx desktop app's if it has\n\
         \x20                        one, else a new dbpane store.\n\
         \x20 --version, -V          Print the version and the dbx commit it was built from.\n\
         \x20 --licenses             Print the licences and notices that ship with dbpane.\n\
         \x20 --help, -h             Print this text.\n\
         \n\
         Subcommands:\n\
         \x20 kitty-setup [--write]  Print (or, with --write, install) the kitty configuration\n\
         \x20                        that frees kitty's Cmd shortcuts while dbpane has focus.\n\
         \n\
         Environment:\n\
         \x20 DBX_DATA_DIR           The store directory (absolute, must exist). --store wins.\n\
         \x20 DBX_SECRET_KEY_FILE    A file holding the store's 64-hex-digit key. Wins over\n\
         \x20                        every other key source, so no keychain is asked.\n\
         \x20 DBX_SECRET_KEY         The same key as a value. DBX_SECRET_KEY_FILE comes first.\n\
         \x20 DBX_TUI_THEME          The colour theme: catppuccin (default) or terminal.\n\
         \x20 HOME, XDG_DATA_HOME    Where the default stores live (on Linux, $XDG_DATA_HOME\n\
         \x20                        or ~/.local/share; on macOS, ~/Library/Application Support).\n\
         \n\
         Store in use now: {in_force}\n"
    )
}

/// The notices and licences that ship with the binary.
const NOTICE: &str = include_str!("../../legal/NOTICE");
const THIRD_PARTY: &str = include_str!("../../legal/THIRD-PARTY.txt");
const LICENSE: &str = include_str!("../../legal/LICENSE");

/// `--licenses`: the NOTICE, the third-party list and dbx's licence.
pub fn licenses_text() -> String {
    let rule = "=".repeat(76);
    format!("{NOTICE}\n{rule}\n\n{THIRD_PARTY}\n{rule}\n\ndbx's LICENSE (Apache License, Version 2.0)\n\n{LICENSE}")
}

/// Reads the arguments after the program name. The first of `--help`,
/// `--version` and `--licenses` wins, wherever it is.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let args: Vec<String> = args.into_iter().collect();
    let unknown = |arg: &str| format!("unknown argument {arg:?}. Try dbpane --help.");
    if args.first().map(String::as_str) == Some("kitty-setup") {
        return match &args[1..] {
            [] => Ok(Cli::KittySetup { write: false }),
            [w] if w == "--write" => Ok(Cli::KittySetup { write: true }),
            [other, ..] => Err(unknown(other)),
        };
    }
    let informational = args.iter().find_map(|a| match a.as_str() {
        "--help" | "-h" => Some(Cli::Help),
        "--version" | "-V" => Some(Cli::Version),
        "--licenses" => Some(Cli::Licenses),
        _ => None,
    });
    if let Some(info) = informational {
        return Ok(info);
    }
    let mut store = StoreChoice::Auto;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value_of = |given: Option<&str>| -> Result<String, String> {
            let value = match given {
                Some(v) => Some(v.to_string()),
                None => rest.next().cloned(),
            };
            value.filter(|v| !v.is_empty()).ok_or_else(|| "--store needs a value: dbx, own or an absolute directory.".to_string())
        };
        match arg.as_str() {
            "--store" => store = store_choice(&value_of(None)?),
            other => match other.strip_prefix("--store=") {
                Some(v) => store = store_choice(&value_of(Some(v))?),
                None => return Err(unknown(other)),
            },
        }
    }
    Ok(Cli::Run(store))
}

/// `dbx` and `own` name the two default stores; anything else is a directory.
fn store_choice(value: &str) -> StoreChoice {
    match value {
        "dbx" => StoreChoice::Dbx,
        "own" => StoreChoice::Own,
        dir => StoreChoice::Dir(dir.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::core::{StoreKind, StoreSource};

    fn parsed(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn no_arguments_start_the_client_with_the_store_left_to_dbpane() {
        assert_eq!(parsed(&[]), Ok(Cli::Run(StoreChoice::Auto)));
    }

    #[test]
    fn store_takes_dbx_own_or_a_directory_in_either_form() {
        assert_eq!(parsed(&["--store", "dbx"]), Ok(Cli::Run(StoreChoice::Dbx)));
        assert_eq!(parsed(&["--store", "own"]), Ok(Cli::Run(StoreChoice::Own)));
        assert_eq!(parsed(&["--store=own"]), Ok(Cli::Run(StoreChoice::Own)));
        assert_eq!(parsed(&["--store", "/data/db"]), Ok(Cli::Run(StoreChoice::Dir(PathBuf::from("/data/db")))));
        assert_eq!(parsed(&["--store=/data/db"]), Ok(Cli::Run(StoreChoice::Dir(PathBuf::from("/data/db")))));
    }

    #[test]
    fn store_without_a_value_says_what_it_takes() {
        let e = parsed(&["--store"]).unwrap_err();
        assert!(e.contains("--store") && e.contains("dbx") && e.contains("own"), "{e}");
        let e = parsed(&["--store="]).unwrap_err();
        assert!(e.contains("--store"), "{e}");
    }

    #[test]
    fn version_help_and_licenses_are_asked_for_by_flag() {
        assert_eq!(parsed(&["--version"]), Ok(Cli::Version));
        assert_eq!(parsed(&["-V"]), Ok(Cli::Version));
        assert_eq!(parsed(&["--help"]), Ok(Cli::Help));
        assert_eq!(parsed(&["-h"]), Ok(Cli::Help));
        assert_eq!(parsed(&["--licenses"]), Ok(Cli::Licenses));
        assert_eq!(parsed(&["--store", "own", "--version"]), Ok(Cli::Version), "an informational flag wins over the rest");
        assert_eq!(parsed(&["--nope", "--licenses"]), Ok(Cli::Licenses), "even over a flag that would be refused");
    }

    #[test]
    fn the_kitty_subcommand_is_still_there() {
        assert_eq!(parsed(&["kitty-setup"]), Ok(Cli::KittySetup { write: false }));
        assert_eq!(parsed(&["kitty-setup", "--write"]), Ok(Cli::KittySetup { write: true }));
    }

    #[test]
    fn an_unknown_argument_is_refused_and_points_at_help() {
        for bad in [vec!["--nope"], vec!["stray"], vec!["kitty-setup", "--wat"]] {
            let e = parsed(&bad).unwrap_err();
            assert!(e.contains(bad.last().unwrap()) && e.contains("--help"), "{e}");
        }
    }

    #[test]
    fn the_version_names_dbpane_the_package_version_and_the_dbx_commit() {
        let text = version_text();
        assert!(text.starts_with("dbpane "), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains("dbx ") && text.len() > "dbpane 0.0.0 (dbx )".len(), "the commit is there: {text}");
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn help_lists_every_flag_and_environment_variable_and_names_the_store_in_force() {
        let info = StoreInfo { kind: StoreKind::Own, source: StoreSource::Auto, path: PathBuf::from("/scratch/home/dbpane") };
        let text = help_text(Ok(info));
        for needle in [
            "dbpane",
            "--store",
            "dbx|own|<dir>",
            "--version",
            "--help",
            "--licenses",
            "kitty-setup",
            "DBX_DATA_DIR",
            "DBX_SECRET_KEY_FILE",
            "DBX_SECRET_KEY",
            "DBX_TUI_THEME",
            "/scratch/home/dbpane",
            "dbpane's own store",
        ] {
            assert!(text.contains(needle), "help lacks {needle}:\n{text}");
        }
        let failed = help_text(Err("HOME is not set".into()));
        assert!(failed.contains("HOME is not set") && failed.contains("--store"), "{failed}");
    }

    #[test]
    fn the_licences_name_dbx_its_fork_herdr_and_every_direct_dependency() {
        let text = licenses_text();
        assert!(text.contains("Apache License") && text.contains("Version 2.0"), "dbx's licence");
        assert!(text.contains("modified work") && text.contains("https://github.com/rutaks/dbx"), "the NOTICE says what dbpane is");
        assert!(text.contains("herdr"), "the theme attribution");
        // Every crate named in this package's [dependencies] is listed, so
        // adding one without listing it fails here.
        let manifest = include_str!("../../Cargo.toml");
        let deps = manifest.split("[dependencies]").nth(1).unwrap().split("\n[").next().unwrap();
        let names: Vec<&str> = deps
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with(' ') && l.contains('='))
            .filter_map(|l| l.split('=').next())
            .map(str::trim)
            .collect();
        assert!(names.len() > 10, "{names:?}");
        for name in names {
            assert!(text.contains(&format!("\n{name} ")) || text.contains(&format!("\n{name}\t")), "the third-party list lacks {name}");
        }
    }
}
