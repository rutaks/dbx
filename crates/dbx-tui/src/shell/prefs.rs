//! The preferences file: where the user left the dividers, kept beside the
//! store as `dbpane-prefs.json`. The shell reads and writes it; `app` only
//! holds the values (`LayoutPrefs`).
//!
//! A missing, unreadable or corrupt file means the defaults, never an error
//! on screen: the layout is a convenience, and a file we cannot make sense of
//! is not worth interrupting anyone for. A value that is not a usable
//! proportion is dropped alone; the others stand.

use std::io;
use std::path::{Path, PathBuf};

use crate::app::layout::LayoutPrefs;

/// The file's name.
pub const FILE: &str = "dbpane-prefs.json";

/// The name it had before the product was named, still read as a fallback.
pub const OLD_FILE: &str = "dbx-tui-prefs.json";

/// Where the file lives for a store in `dir`.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// The layout the file holds, or the defaults.
pub fn load(path: &Path) -> LayoutPrefs {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Before dbpane had its name the file was `dbx-tui-prefs.json`: read
        // it while the new one is absent. Only the new name is ever written.
        Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::read_to_string(path.with_file_name(OLD_FILE)) {
            Ok(text) => text,
            Err(_) => return LayoutPrefs::default(),
        },
        Err(_) => return LayoutPrefs::default(),
    };
    parse(&text)
}

/// The layout in `text`, or the defaults when it is not JSON.
pub fn parse(text: &str) -> LayoutPrefs {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return LayoutPrefs::default() };
    let field = |name: &str| value.get(name).and_then(serde_json::Value::as_u64).and_then(|n| u16::try_from(n).ok());
    LayoutPrefs { sidebar: field("sidebar_permille"), editor: field("editor_permille") }.sane()
}

/// The file's text for `prefs`.
pub fn render(prefs: &LayoutPrefs) -> String {
    let mut map = serde_json::Map::new();
    if let Some(p) = prefs.sidebar {
        map.insert("sidebar_permille".into(), p.into());
    }
    if let Some(p) = prefs.editor {
        map.insert("editor_permille".into(), p.into());
    }
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

/// Writes the file whole or not at all: a temporary file beside it, then a
/// rename, so a crash never leaves half a file.
pub fn save(path: &Path, prefs: &LayoutPrefs) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, render(prefs))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_saved_is_read_back_and_a_missing_file_is_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        assert_eq!(load(&path), LayoutPrefs::default(), "no file");
        let prefs = LayoutPrefs { sidebar: Some(450), editor: Some(520) };
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path), prefs);
        save(&path, &LayoutPrefs { sidebar: None, editor: Some(300) }).unwrap();
        assert_eq!(load(&path), LayoutPrefs { sidebar: None, editor: Some(300) });
        assert!(!path.with_extension("json.tmp").exists(), "no temporary file is left");
    }

    #[test]
    fn the_old_file_name_is_read_when_the_new_one_is_absent_and_only_the_new_one_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        let old = dir.path().join(OLD_FILE);
        std::fs::write(&old, r#"{"sidebar_permille": 450, "editor_permille": 520}"#).unwrap();
        assert_eq!(load(&path), LayoutPrefs { sidebar: Some(450), editor: Some(520) });
        // Once the new file exists it wins, even over a different old one.
        save(&path, &LayoutPrefs { sidebar: Some(300), editor: None }).unwrap();
        assert_eq!(load(&path), LayoutPrefs { sidebar: Some(300), editor: None });
        assert!(std::fs::read_to_string(&old).unwrap().contains("450"), "the old file is left alone");
    }

    #[test]
    fn a_corrupt_file_or_an_unusable_value_is_the_default_and_never_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        for junk in ["", "not json", "[1,2]", "{\"sidebar_permille\": \"wide\"}", "\u{0}\u{1}", "{"] {
            std::fs::write(&path, junk).unwrap();
            assert_eq!(load(&path), LayoutPrefs::default(), "{junk:?}");
        }
        // Out of range and negative values are dropped one by one.
        std::fs::write(&path, r#"{"sidebar_permille": 0, "editor_permille": 400}"#).unwrap();
        assert_eq!(load(&path), LayoutPrefs { sidebar: None, editor: Some(400) });
        std::fs::write(&path, r#"{"sidebar_permille": 1000, "editor_permille": -5}"#).unwrap();
        assert_eq!(load(&path), LayoutPrefs::default());
        // A directory where the file should be reads as the defaults too.
        let blocked = dir.path().join("blocked.json");
        std::fs::create_dir(&blocked).unwrap();
        assert_eq!(load(&blocked), LayoutPrefs::default());
    }

    #[test]
    fn saving_where_it_cannot_is_an_error_to_ignore_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("no-such-dir").join(FILE);
        assert!(save(&gone, &LayoutPrefs::default()).is_err());
    }
}
