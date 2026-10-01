//! Clipboard: the OS clipboard via `arboard`, with an OSC 52 fallback so
//! copying works over SSH and inside terminals that forward it.

use base64::Engine;

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

