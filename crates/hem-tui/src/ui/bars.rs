//! Title bar, tabs, filter bar / prompt, and status bar.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::widgets::fit;
use crate::app::{App, Hit};

fn sep(app: &App) -> &'static str {
    if app.ascii {
        " - "
    } else {
        " ─ "
    }
}

pub fn tabs(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    let mut hits = Vec::new();
    for (i, t) in app.tabs.iter().enumerate() {
        let label = format!(" {} ", fit(&t.name, 24, app.ascii));
        let w = label.width() as u16;
        spans.push(Span::styled(label, theme.tab(i == app.active)));
        spans.push(Span::raw(" "));
        hits.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Hit::Tab(i),
        ));
        x += w + 1;
    }
    let style = if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.status_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    app.hits.extend(hits);
}

