//! Rendering.  Every function here draws from `&mut App` into a ratatui
//! frame and records clickable regions in `app.hits`.

pub mod bars;
pub mod line;
pub mod logpane;
pub mod minimap;
pub mod modals;
pub mod templates;
pub mod widgets;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use crate::app::App;

/// Log pane + minimap + scrollbar.
fn render_main(app: &mut App, frame: &mut Frame, area: Rect) {
    let minimap_w: u16 = if app.minimap && area.width >= 40 {
        4
    } else {
        0
    };
    let scrollbar_w: u16 = 1;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(scrollbar_w),
            Constraint::Length(minimap_w),
        ])
        .split(area);
    logpane::render(app, frame, chunks[0]);
    logpane::scrollbar(app, frame, chunks[1]);
    if minimap_w > 0 {
        minimap::render(app, frame, chunks[2]);
    }
}
