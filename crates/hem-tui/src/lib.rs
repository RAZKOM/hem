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
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};

use app::App;
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

/// Run the full-screen application until the user quits.
pub fn run(opts: RunOptions) -> Result<()> {
    let debug = opts.debug_log.as_ref().and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut app = App::new(opts)?;
    app.debug = debug;
    let mut term = terminal::setup(app.mouse)?;
    let size = term.size().map(|s| (s.width, s.height)).unwrap_or((0, 0));
    app.log_debug(&format!(
        "tui started: {}x{} · {} source(s) · theme {} · depth {:?}",
        size.0,
        size.1,
        app.tabs.len(),
        app.theme.name,
        app.theme.depth
    ));
    let result = event_loop(&mut term, &mut app);
    terminal::restore();
    result
}

fn event_loop(term: &mut terminal::Term, app: &mut App) -> Result<()> {
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let frame_budget = Duration::from_millis(16);
    loop {
        if app.quit {
            break;
        }
        app.tick();
        if app.take_dirty() || app.animating() {
            // Coalesce redraws to ~60 fps.
            let since = last_draw.elapsed();
            if since < frame_budget {
                std::thread::sleep(frame_budget - since);
            }
            term.draw(|f| ui::render(app, f))?;
            if last_draw.elapsed() > Duration::from_millis(900) {
                app.log_debug("frame drawn");
            }
            last_draw = Instant::now();
            if let Some(req) = app.take_osc52() {
                let mut out = std::io::stdout();
                let _ = out.write_all(req.as_bytes());
                let _ = out.flush();
            }
        }
        let timeout = if app.busy() || app.animating() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(200)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) => {
                    if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        app.handle_key(k);
                    }
                }
                Event::Mouse(m) => app.handle_mouse(m),
                Event::Resize(w, h) => app.handle_resize(w, h),
                Event::Paste(s) => app.handle_paste(&s),
                _ => {}
            }
            // Drain any burst of pending events before redrawing.
            while event::poll(Duration::ZERO)? {
                match event::read()? {
                    Event::Key(k) => {
                        if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                            app.handle_key(k);
                        }
                    }
                    Event::Mouse(m) => app.handle_mouse(m),
                    Event::Resize(w, h) => app.handle_resize(w, h),
                    Event::Paste(s) => app.handle_paste(&s),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
