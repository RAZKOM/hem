//! hem-tui: the ratatui front end for hem.
//!
//! The crate is split into state ([`app`]), rendering ([`ui`]), the
//! keymap ([`keymap`]), theming ([`theme`]) and configuration
//! ([`config`]).  [`run`] wires them to a real terminal; the rendering
//! functions also work against ratatui's `TestBackend` for tests.

pub mod app;
pub mod keymap;
pub mod modal;

