//! Title bar, tabs, filter bar / prompt, and status bar.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::widgets::{bytes, fit, group};
use crate::app::{App, Chip, Hit};
use crate::keymap::Action;
use crate::modal::Modal;

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

pub fn filter_bar(app: &mut App, frame: &mut Frame, area: Rect, chips: &[(Chip, String)]) {
    let theme = app.theme.clone();
    let close = if app.ascii { " x" } else { " ✕" };
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    let mut hits = Vec::new();
    for (i, (chip, label)) in chips.iter().enumerate() {
        let text = format!(" {}", fit(label, 40, app.ascii));
        let w = (text.width() + close.width() + 1) as u16;
        let style = match chip {
            Chip::Search => theme.match_style(false),
            Chip::Highlight => theme.highlight(),
            _ => theme.chip(),
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::styled(close, style.fg(theme.chip_close)));
        spans.push(Span::styled(" ", style));
        spans.push(Span::raw(" "));
        hits.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Hit::Chip(i),
        ));
        x += w + 1;
    }
    if !chips.is_empty() {
        let clear = if app.ascii { "[x clear]" } else { "✕ clear" };
        let w = clear.width() as u16;
        spans.push(Span::styled(
            clear,
            theme.dim_style().add_modifier(Modifier::UNDERLINED),
        ));
        hits.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Hit::ClearChips,
        ));
    }
    let style = if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.muted_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    app.hits.extend(hits);
}

/// Inline prompt (search / filter / goto …) drawn in the filter-bar slot.
pub fn prompt(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let Some(Modal::Prompt { kind, input }) = &app.modal else {
        return;
    };
    let label = format!(" {} ", kind.label());
    let hint = kind.hint();
    let label_w = label.width();
    let avail = (area.width as usize).saturating_sub(label_w + 1);
    let text = &input.text;
    let hint_w = if text.is_empty() { hint.width() + 2 } else { 0 };
    let field_w = avail.saturating_sub(hint_w).max(10);
    // Keep the cursor visible by scrolling the field.
    let cursor_col: usize = text.chars().take(input.cursor).collect::<String>().width();
    let skip = cursor_col.saturating_sub(field_w.saturating_sub(1));
    let shown: String = text.chars().skip(skip).collect();
    let shown = fit(&shown, field_w, app.ascii);
    let mut spans = vec![
        Span::styled(label, theme.accent_style().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{shown:<field_w$}"), theme.input()),
    ];
    if text.is_empty() {
        spans.push(Span::styled(format!("  {hint}"), theme.dim_style()));
    }
    let style = if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.muted_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    let cx = area.x + (label_w + cursor_col - skip) as u16;
    if cx < area.x + area.width {
        frame.set_cursor_position((cx, area.y));
    }
    app.hits.push((area, Hit::Prompt));
}

pub fn status(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let t = app.tab();
    let n = t.len();
    let stats = t.view.stats();
    let mut left: Vec<Span> = vec![Span::raw(" ")];
    if app.templates_mode {
        let c = t.clusters.as_ref();
        let count = c.map(|c| c.templates().len()).unwrap_or(0);
        left.push(Span::raw(format!("{} templates", group(count as u64))));
        if let Some(c) = c {
            if c.is_sampled() {
                left.push(Span::styled(
                    format!(" · sampled 1/{}", c.stride()),
                    theme.dim_style(),
                ));
            }
        }
    } else {
        if !t.filter.is_empty() {
            left.push(Span::styled(
                format!("{} matching lines", group(stats.lines)),
                theme.accent_style(),
            ));
            left.push(Span::styled(" · ", theme.dim_style()));
        }
        let line_no = t
            .view
            .row(t.cursor)
            .map(|r| group(r.line() + 1))
            .unwrap_or_else(|| "-".into());
        left.push(Span::raw(format!("line {line_no}")));
        if t.collapse && n != stats.lines as usize {
            left.push(Span::styled(
                format!(" ({} rows)", group(n as u64)),
                theme.dim_style(),
            ));
        }
        let pct = ((t.cursor + 1) * 100).checked_div(n).unwrap_or(0).min(100);
        left.push(Span::styled(format!(" · {pct}%"), theme.dim_style()));
        if let Some(s) = &t.search {
            let total = s.count();
            let pos = t
                .current_match
                .map(|i| format!("{}/", i + 1))
                .unwrap_or_default();
            let dots = if s.is_complete() { "" } else { "…" };
            left.push(Span::styled(
                format!(" · {pos}{} matches{dots}", group(total as u64)),
                theme.match_style(false),
            ));
        }
        let errs = stats.levels[hem_core::Level::Error.index()]
            + stats.levels[hem_core::Level::Fatal.index()];
        let warns = stats.levels[hem_core::Level::Warn.index()];
        if errs > 0 {
            left.push(Span::styled(
                format!(" · {} errors", group(errs)),
                theme.level_style(hem_core::Level::Error),
            ));
        }
        if warns > 0 {
            left.push(Span::styled(
                format!(" · {} warnings", group(warns)),
                theme.level_style(hem_core::Level::Warn),
            ));
        }
    }
    let progress = app.progress_text();
    if !progress.is_empty() {
        left.push(Span::styled(
            format!(" · {progress}"),
            theme.fg_style(theme.warning),
        ));
    }

    // Right side: newest notice, else hints.
    let right: Vec<Span> = if let Some(nt) = app.notices.back() {
        vec![Span::styled(
            fit(&nt.text, (area.width as usize).saturating_sub(4), app.ascii),
            if nt.warning {
                theme.fg_style(theme.warning).add_modifier(Modifier::BOLD)
            } else {
                theme.accent_style()
            },
        )]
    } else {
        let help = app.keymap.keys_label(Action::Help, app.ascii);
        let pal = app.keymap.keys_label(Action::Palette, app.ascii);
        vec![Span::styled(
            format!("{help} help · {pal} commands"),
            theme.dim_style(),
        )]
    };
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let mut right = right;
    let mut right_w: usize = right.iter().map(|s| s.content.width()).sum();
    // Drop the right half rather than running the two together.
    let show_right = left_w + right_w + 3 <= area.width as usize;
    if !show_right {
        if app.notices.is_empty() {
            right.clear();
        } else {
            // A fresh notice matters more than the counters: show it alone.
            left = vec![Span::raw(" ")];
            left.append(&mut right);
        }
        right_w = 0;
    }
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let gap = (area.width as usize).saturating_sub(left_w + right_w + 1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.status()),
        area,
    );
    // Right-side hints are clickable.
    let hx = area.x + (area.width as usize).saturating_sub(right_w + 1) as u16;
    if show_right && app.notices.is_empty() && right_w + 1 < area.width as usize {
        app.hits.push((
            Rect {
                x: hx,
                y: area.y,
                width: 8,
                height: 1,
            },
            Hit::Action(Action::Help),
        ));
        app.hits.push((
            Rect {
                x: hx + 9,
                y: area.y,
                width: right_w.saturating_sub(9) as u16,
                height: 1,
            },
            Hit::Action(Action::Palette),
        ));
    }
}
