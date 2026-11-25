//! Raw mode, the alternate screen, mouse and paste reporting: set up on entry,
//! undone on drop and on panic.

use std::io;

use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::DefaultTerminal;

/// Owns the terminal while the client runs. Dropping it restores the shell.
pub struct TerminalGuard {
    pub terminal: DefaultTerminal,
    /// The terminal speaks the kitty keyboard protocol (asked once, here, before
    /// the event stream starts) and the guard turned it on.
    pub keyboard_protocol: bool,
}

/// Set once the protocol is on, so the panic hook and signal paths pop it too.
static PROTOCOL_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set once xterm's modifyOtherKeys was asked for (terminals without the kitty
/// protocol, tmux among them: it passes Ctrl+Enter on only to a program that asks).
static MODIFY_OTHER_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set once the kitty user variable was written.
static USER_VAR_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set once the terminal is in the client's mode; a panic before that has
/// nothing to restore.
static ENTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes a panic anywhere end the process with code 101, after putting the
/// terminal back if it was entered. Call it at the top of `main`: a panic in a
/// spawned task would otherwise leave the main loop running, and a panic
/// before the terminal is entered would otherwise unwind through `main` to
/// Rust's own exit code. Installed once; later calls do nothing.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if ENTERED.load(std::sync::atomic::Ordering::SeqCst) {
                restore_extras();
                let _ = ratatui::try_restore();
            }
            previous(info);
            std::process::exit(101);
        }));
    });
}

/// The user variable kitty's `map --when-focus-on var:dbx_tui` looks at.
pub const USER_VAR: &str = "dbx_tui";

/// `OSC 1337 ; SetUserVar=NAME=BASE64 ST`: sets a kitty user variable.
pub fn set_user_var(name: &str, value: &str) -> String {
    format!("\u{1b}]1337;SetUserVar={name}={}\u{1b}\\", base64(value.as_bytes()))
}

/// The same with no value: kitty's own examples clear a variable this way
/// (`SetUserVar=in_editor`).
pub fn clear_user_var(name: &str) -> String {
    format!("\u{1b}]1337;SetUserVar={name}\u{1b}\\")
}

/// Only kitty reads the variable; other terminals are not sent it.
fn in_kitty() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some() || std::env::var("TERM").is_ok_and(|t| t.contains("kitty"))
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        // Ours first, so ratatui's hook (installed by `try_init`, which
        // restores raw mode and the screen) runs ahead of it.
        install_panic_hook();
        let terminal = ratatui::try_init()?; // raw mode, alternate screen, restoring panic hook
        ENTERED.store(true, std::sync::atomic::Ordering::SeqCst);
        execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        // Asked once, now: it reads the terminal's answer, which must not race
        // the event stream. Disambiguated keys give Cmd (as SUPER), Ctrl+Enter
        // and the rest; a terminal that cannot says so and nothing is pushed.
        let keyboard_protocol = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if keyboard_protocol && execute!(io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)).is_ok() {
            PROTOCOL_ON.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if !keyboard_protocol && modify_other_keys_is_safe() {
            use std::io::Write;
            let mut out = io::stdout();
            // xterm's modifyOtherKeys, level 2: Ctrl+Enter, Ctrl+/ and the like
            // arrive with their modifiers where the terminal (or tmux, with
            // `extended-keys on`) can send them. A terminal that does not know
            // it ignores it.
            if out.write_all(b"\x1b[>4;2m").and_then(|_| out.flush()).is_ok() {
                MODIFY_OTHER_ON.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        if in_kitty() {
            use std::io::Write;
            let mut out = io::stdout();
            if out.write_all(set_user_var(USER_VAR, "1").as_bytes()).and_then(|_| out.flush()).is_ok() {
                USER_VAR_ON.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        Ok(Self { terminal, keyboard_protocol })
    }
}

/// crossterm 0.29 cannot parse xterm's `CSI 27;mods;code ~` form (it rejects
/// the key code 27), so modifyOtherKeys is only asked for where the terminal
/// answers in the `CSI code;mods u` form it does parse: inside tmux set to
/// `extended-keys-format csi-u`. Anywhere else, ctrl and alt chords would be lost.
fn modify_other_keys_is_safe() -> bool {
    if std::env::var_os("TMUX").is_none() {
        return false;
    }
    std::process::Command::new("tmux")
        .args(["show-options", "-sv", "extended-keys-format"])
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "csi-u")
        .unwrap_or(false)
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ENTERED.store(false, std::sync::atomic::Ordering::SeqCst);
        restore_extras();
        let _ = ratatui::try_restore();
    }
}

/// Base64 (RFC 4648, with padding). Written out here because the clipboard is
/// the only place the client needs it.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The OSC 52 sequence that sets the system clipboard to `text`:
/// `ESC ] 52 ; c ; <base64> BEL`. The text is cell data from a database and
/// may hold any bytes, escape sequences included; as base64 it is only ever
/// letters, digits, `+`, `/` and `=`, so the terminal can read it as nothing
/// but the clipboard's content. OSC 52 works over SSH and inside tmux (with
/// `set-clipboard on`) and kitty, which a native clipboard crate does not.
pub fn osc52(text: &str) -> String {
    format!("\u{1b}]52;c;{}\u{7}", base64(text.as_bytes()))
}

/// Writes the clipboard sequence for `text` to `out` and flushes.
pub fn write_clipboard(out: &mut impl io::Write, text: &str) -> io::Result<()> {
    out.write_all(osc52(text).as_bytes())?;
    out.flush()
}

/// The clipboard sink of the real terminal.
pub fn copy_to_terminal(text: &str) {
    // A terminal that ignores OSC 52 ignores this too; nothing to do on error.
    let _ = write_clipboard(&mut io::stdout().lock(), text);
}

fn restore_extras() {
    use std::io::Write;
    use std::sync::atomic::Ordering;
    // Every exit path comes through here: quit and signals (the guard's drop)
    // and panics (the hook).
    if PROTOCOL_ON.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    if MODIFY_OTHER_ON.swap(false, Ordering::SeqCst) {
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[>4;0m");
        let _ = out.flush();
    }
    if USER_VAR_ON.swap(false, Ordering::SeqCst) {
        let mut out = io::stdout();
        let _ = out.write_all(clear_user_var(USER_VAR).as_bytes());
        let _ = out.flush();
    }
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kitty_user_variable_is_set_with_base64_one_and_cleared_without_a_value() {
        assert_eq!(set_user_var("dbx_tui", "1"), "\u{1b}]1337;SetUserVar=dbx_tui=MQ==\u{1b}\\");
        assert_eq!(clear_user_var("dbx_tui"), "\u{1b}]1337;SetUserVar=dbx_tui\u{1b}\\");
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        for (text, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("hello", "aGVsbG8="), ("é😀", "w6nwn5iA")] {
            assert_eq!(base64(text.as_bytes()), want, "{text:?}");
        }
    }

    #[test]
    fn the_clipboard_sequence_carries_the_text_as_base64_and_nothing_else() {
        assert_eq!(osc52("hello"), "\u{1b}]52;c;aGVsbG8=\u{7}");
        // Cell data that is an escape sequence in its own right.
        let evil = "\u{1b}]0;pwned\u{7}\u{1b}[2J\u{1b}P1;2|aa\u{1b}\\ \u{9b}31m";
        let seq = osc52(evil);
        assert_eq!(seq.matches('\u{1b}').count(), 1, "only the introducer: {seq:?}");
        assert_eq!(seq.matches('\u{7}').count(), 1, "only the terminator: {seq:?}");
        assert!(seq.is_ascii(), "{seq:?}");
        let payload = seq.strip_prefix("\u{1b}]52;c;").unwrap().strip_suffix('\u{7}').unwrap();
        assert!(payload.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)), "{payload:?}");
        assert_eq!(osc52(""), "\u{1b}]52;c;\u{7}", "an empty string is still sent: it clears the clipboard");
    }

    #[test]
    fn writing_the_sequence_writes_exactly_it_and_flushes() {
        let mut out = Vec::new();
        write_clipboard(&mut out, "hi").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\u{1b}]52;c;aGk=\u{7}");
    }
}
