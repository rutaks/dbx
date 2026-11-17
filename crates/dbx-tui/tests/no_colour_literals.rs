//! Look and feel rule (design section 12): colour comes from palette roles,
//! so no colour literal may appear outside the theme module.

use std::fs;
use std::path::{Path, PathBuf};

const THEME_MODULE: &str = "src/app/theme.rs";

const FORBIDDEN: &[&str] = &[
    "Color::", "Rgb(", "Indexed(", "Stylize", ".black()", ".red()", ".green()", ".yellow()",
    ".blue()", ".magenta()", ".cyan()", ".gray()", ".white()", ".on_black()", ".on_red()",
    ".on_green()", ".on_yellow()", ".on_blue()", ".on_magenta()", ".on_cyan()", ".on_gray()",
    ".on_white()", "palette::", "tailwind::", "material::", "Color as ", "style::*",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn offences(source: &str) -> Vec<&'static str> {
    FORBIDDEN.iter().copied().filter(|p| source.contains(p)).collect()
}

#[test]
fn the_scanner_flags_colour_literals() {
    assert!(!offences("let s = Style::default().fg(Color::Red);").is_empty());
    assert!(!offences("Style::new().red()").is_empty());
    assert!(!offences("Color::Rgb(1, 2, 3)").is_empty());
    assert!(!offences("use ratatui::style::palette::tailwind::RED;").is_empty());
    assert!(!offences("use ratatui::style::palette::material::BLUE;").is_empty());
    assert!(!offences("use ratatui::style::Color as C; let x = C::Red;").is_empty());
    assert!(!offences("use ratatui::style::*;").is_empty());
    assert!(offences("Style::default().fg(theme.color(Role::Accent))").is_empty());
}

#[test]
fn no_colour_literal_appears_outside_the_theme_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("examples"), &mut files);
    let mut bad = Vec::new();
    for file in files {
        if file.strip_prefix(root).is_ok_and(|rel| rel == Path::new(THEME_MODULE)) {
            continue;
        }
        let found = offences(&fs::read_to_string(&file).expect("read source"));
        if !found.is_empty() {
            bad.push(format!("{}: {found:?}", file.display()));
        }
    }
    assert!(bad.is_empty(), "colour literals outside {THEME_MODULE}:\n{}", bad.join("\n"));
}
