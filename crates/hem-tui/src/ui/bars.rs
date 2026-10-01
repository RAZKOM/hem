//! Title bar, tabs, filter bar / prompt, and status bar.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::widgets::{bytes, fit, group};
use crate::app::{App, Hit};
use crate::keymap::Action;

fn sep(app: &App) -> &'static str {
    if app.ascii {
        " - "
    } else {
        " ─ "
    }
}

pub fn title(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let t = app.tab();
    let sources = t.base.sources();
    let total_bytes: u64 = sources.iter().map(|s| s.total_bytes()).sum();
    let total_lines: usize = sources.iter().map(|s| s.line_count()).sum();
    let mut spans = vec![
        Span::styled(" hem", theme.title_style()),
        Span::styled(sep(app), theme.dim_style()),
        Span::styled(t.name.clone(), theme.accent_style()),
        Span::styled(sep(app), theme.dim_style()),
        Span::raw(bytes(total_bytes)),
        Span::styled(sep(app), theme.dim_style()),
        Span::raw(format!("{} lines", group(total_lines as u64))),
    ];
    if let Some(f) = sources.first() {
        if sources.len() == 1 {
            spans.push(Span::styled(sep(app), theme.dim_style()));
            spans.push(Span::styled(
                f.format().as_str().to_string(),
                theme.dim_style(),
            ));
        }
    }
    if t.follow {
        spans.push(Span::styled(sep(app), theme.dim_style()));
        spans.push(Span::styled(
            if app.ascii {
                "following *"
            } else {
                "following ●"
            },
            theme.fg_style(theme.success).add_modifier(Modifier::BOLD),
        ));
    }
    if sources.iter().any(|s| !s.is_complete()) {
        let p: f64 = sources.iter().map(|s| s.progress()).sum::<f64>() / sources.len() as f64;
        spans.push(Span::styled(sep(app), theme.dim_style()));
        spans.push(Span::styled(
            format!("indexing {:.0}%", p * 100.0),
            theme.fg_style(theme.warning),
        ));
    }
    let style = if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.status_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    // Clickable: title opens the palette.
    app.hits.push((
        Rect {
            x: area.x,
            y: area.y,
            width: 5,
            height: 1,
        },
        Hit::Action(Action::Palette),
    ));
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

