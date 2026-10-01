//! Terminal setup and guaranteed restore (on quit, error, panic, Ctrl+C).

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);

/// Enter raw mode + alternate screen (+ mouse capture), and install a panic
/// hook that restores the terminal before printing the panic.
pub fn setup(mouse: bool) -> Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableBracketedPaste,
        cursor::Hide
    )?;
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    ACTIVE.store(true, Ordering::SeqCst);
    MOUSE.store(mouse, Ordering::SeqCst);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));
    let backend = CrosstermBackend::new(out);
    let term = Terminal::new(backend)?;
    Ok(term)
}

/// Restore the terminal.  Safe to call multiple times.
pub fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    if MOUSE.load(Ordering::SeqCst) {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Is the terminal currently in TUI mode?
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}
