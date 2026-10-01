//! hem-tui: the ratatui front end for hem.
//!
//! The crate is split into state ([`app`]), rendering ([`ui`]), the
//! keymap ([`keymap`]), theming ([`theme`]) and configuration
//! ([`config`]).  [`run`] wires them to a real terminal; the rendering
//! functions also work against ratatui's `TestBackend` for tests.

pub mod app;
pub mod clipboard;
pub mod config;
pub mod keymap;
pub mod modal;
pub mod terminal;
pub mod theme;
pub mod ui;

use std::io::Write;
use std::path::PathBuf;

use hem_core::Source;

/// Everything the binary passes to the TUI.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub sources: Vec<Source>,
    pub merge: bool,
    pub config: config::Config,
    pub config_path: Option<PathBuf>,
    pub ascii: bool,
    pub mouse: bool,
    pub theme: Option<String>,
    pub initial_filter: hem_core::filter::FilterSpec,
    pub follow: bool,
    pub collapse: Option<bool>,
    pub debug_log: Option<PathBuf>,
}

