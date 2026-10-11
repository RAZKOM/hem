//! Clipboard: the OS clipboard via `arboard`, with an OSC 52 fallback so
//! copying works over SSH and inside terminals that forward it.

use base64::Engine;

/// Result of a copy attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    /// Copied through the OS clipboard.
    System,
    /// The caller must write this OSC 52 sequence to the terminal.
    Osc52(String),
}

#[derive(Default)]
pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
    tried: bool,
    force_osc52: bool,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clipboard(system={})", self.inner.is_some())
    }
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            inner: None,
            tried: false,
            force_osc52: std::env::var_os("HEM_OSC52").is_some()
                || std::env::var_os("SSH_TTY").is_some(),
        }
    }
}

/// Build an OSC 52 "set clipboard" escape sequence.
pub fn osc52(text: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{b64}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_format() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
