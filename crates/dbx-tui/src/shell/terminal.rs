//! Raw mode, the alternate screen, mouse and paste reporting: set up on entry,
//! undone on drop and on panic.

use std::io;

use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;
use ratatui::DefaultTerminal;

/// Owns the terminal while the client runs. Dropping it restores the shell.
pub struct TerminalGuard {
    pub terminal: DefaultTerminal,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        let terminal = ratatui::try_init()?; // raw mode, alternate screen, restoring panic hook
        // Chain our own restore ahead of ratatui's hook, so a panic also
        // switches mouse and paste reporting off before the message prints.
        // Then end the process: a panic inside a spawned task would otherwise
        // leave the main loop running against a terminal that is already
        // restored.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_extras();
            previous(info);
            std::process::exit(101);
        }));
        execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
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
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
}

#[cfg(test)]
mod tests {
    use super::*;

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
