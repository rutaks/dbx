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

fn restore_extras() {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
}
