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
use crate::modal::Modal;

/// Draw one frame.
pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    app.size = (area.width, area.height);
    app.hits.clear();
    app.tab().view.resume();
    frame.render_widget(
        ratatui::widgets::Block::default().style(app.theme.base()),
        area,
    );
    if area.height < 3 || area.width < 10 {
        return;
    }

    let show_tabs = app.tabs.len() > 1;
    let prompt_open = matches!(app.modal, Some(Modal::Prompt { .. }));
    let chips = app.compute_chips();
    let show_filter_bar = prompt_open || !chips.is_empty();

    let mut constraints = vec![Constraint::Length(1)];
    if show_tabs {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    if show_filter_bar {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut i = 0;
    bars::title(app, frame, chunks[i]);
    i += 1;
    if show_tabs {
        bars::tabs(app, frame, chunks[i]);
        i += 1;
    }
    let main = chunks[i];
    i += 1;
    if app.templates_mode {
        templates::render(app, frame, main);
    } else {
        render_main(app, frame, main);
    }
    if show_filter_bar {
        if prompt_open {
            bars::prompt(app, frame, chunks[i]);
        } else {
            bars::filter_bar(app, frame, chunks[i], &chips);
        }
        i += 1;
    }
    bars::status(app, frame, chunks[i]);

    if app.modal.is_some() && !prompt_open {
        modals::render(app, frame, area);
    }
}

/// Pinned strip + column header + log pane, then scrollbar and minimap.
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
    if app.tab().columns {
        app.tab_mut().ensure_columns();
    }
    let log = chunks[0];
    let pin_h = logpane::pin_strip_height(app, log.height);
    let header_h = u16::from(logpane::column_header_visible(app));
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(pin_h),
            Constraint::Length(header_h),
            Constraint::Min(1),
        ])
        .split(log);
    if pin_h > 0 {
        logpane::pins(app, frame, parts[0]);
    }
    if header_h > 0 {
        logpane::column_header(app, frame, parts[1]);
    }
    logpane::render(app, frame, parts[2]);
    logpane::scrollbar(app, frame, chunks[1]);
    if minimap_w > 0 {
        minimap::render(app, frame, chunks[2]);
    }
}
