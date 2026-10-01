//! Templates view: a sortable table of mined log templates.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use hem_core::timestamp::{format_ms, TsDisplay};

use super::widgets::{fit, group, pad};
use crate::app::{App, Hit};
use crate::modal::TemplateSort;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let ascii = app.ascii;
    if area.height < 3 {
        return;
    }
    let list = app.sorted_templates();
    let stride = app.tab().clusters.as_ref().map(|c| c.stride()).unwrap_or(1);
    let st = app.tab().templates.clone();
    let width = area.width as usize;

    // Column widths.
    let w_count = 9usize;
    let w_level = 6usize;
    let w_time = if width >= 100 {
        17usize
    } else if width >= 80 {
        9
    } else {
        0
    };
    let w_tpl = width.saturating_sub(w_count + w_level + w_time * 2 + 5);

    // Header.
    let arrow = |s: TemplateSort| -> String {
        if st.sort == s {
            if st.ascending {
                if ascii {
                    " ^"
                } else {
                    " ▴"
                }
            } else if ascii {
                " v"
            } else {
                " ▾"
            }
            .to_string()
        } else {
            String::new()
        }
    };
    let hstyle = theme.title_style();
    let mut hx = area.x;
    let mut header: Vec<Span> = Vec::new();
    let cols: Vec<(String, usize, usize)> = {
        let mut v = vec![
            (
                format!(
                    "{:>w$}",
                    format!("count{}", arrow(TemplateSort::Count)),
                    w = w_count
                ),
                w_count,
                0,
            ),
            (
                pad(
                    &format!("level{}", arrow(TemplateSort::Level)),
                    w_level,
                    ascii,
                ),
                w_level,
                1,
            ),
        ];
        if w_time > 0 {
            v.push((
                pad(
                    &format!("first{}", arrow(TemplateSort::First)),
                    w_time,
                    ascii,
                ),
                w_time,
                2,
            ));
            v.push((
                pad(&format!("last{}", arrow(TemplateSort::Last)), w_time, ascii),
                w_time,
                3,
            ));
        }
        v.push((
            pad(
                &format!("template{}", arrow(TemplateSort::Template)),
                w_tpl,
                ascii,
            ),
            w_tpl,
            4,
        ));
        v
    };
    for (text, w, col) in &cols {
        header.push(Span::styled(format!(" {text}"), hstyle));
        app.hits.push((
            Rect {
                x: hx,
                y: area.y,
                width: (*w + 1) as u16,
                height: 1,
            },
            Hit::TemplateHeader(*col),
        ));
        hx += (*w + 1) as u16;
    }
    let mut lines = vec![Line::from(header).style(if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.muted_bg)
    })];

    // Rows.
    let body_h = (area.height as usize).saturating_sub(2);
    let selected = st.selected.min(list.len().saturating_sub(1));
    let first = if selected >= body_h {
        selected + 1 - body_h
    } else {
        0
    };
    for (i, t) in list.iter().enumerate().skip(first).take(body_h) {
        let count = t.count * stride;
        let count_s = if stride > 1 {
            format!("~{}", group(count))
        } else {
            group(count)
        };
        let lvl = t.max_level();
        let lvl_span = match lvl {
            Some(l) => Span::styled(pad(l.short(), w_level, ascii), theme.level_style(l)),
            None => Span::styled(pad("-", w_level, ascii), theme.dim_style()),
        };
        let fmt_time = |ts: Option<i64>, row: hem_core::view::Row| -> String {
            match ts {
                Some(ms) if w_time >= 17 => format_ms(ms, TsDisplay::Iso, None)
                    .chars()
                    .take(16)
                    .collect(),
                Some(ms) => format_ms(ms, TsDisplay::Time, None)
                    .chars()
                    .take(8)
                    .collect(),
                None => format!("L{}", row.line() + 1),
            }
        };
        let is_sel = i == selected;
        let style = if is_sel {
            theme.selection().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(
                format!(" {:>w$}", count_s, w = w_count),
                style.patch(theme.fg_style(theme.number)),
            ),
            Span::styled(" ", style),
            lvl_span.patch_style(style),
        ];
        if w_time > 0 {
            spans.push(Span::styled(
                format!(" {}", pad(&fmt_time(t.first_ts, t.first), w_time, ascii)),
                style.patch(theme.fg_style(theme.timestamp)),
            ));
            spans.push(Span::styled(
                format!(" {}", pad(&fmt_time(t.last_ts, t.last), w_time, ascii)),
                style.patch(theme.fg_style(theme.timestamp)),
            ));
        }
        let tpl = t.display();
        let tpl_spans = template_spans(&theme, &fit(&tpl, w_tpl, ascii), style);
        spans.push(Span::styled(" ", style));
        spans.extend(tpl_spans);
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used < width {
            spans.push(Span::styled(" ".repeat(width - used), style));
        }
        let y = area.y + 1 + (i - first) as u16;
        app.hits.push((
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            Hit::TemplateRow(i),
        ));
        lines.push(Line::from(spans));
    }
    if list.is_empty() {
        let msg = if app
            .tab()
            .clusters
            .as_ref()
            .map(|c| !c.is_complete())
            .unwrap_or(true)
        {
            "clustering…"
        } else {
            "no templates"
        };
        lines.push(Line::from(Span::styled(
            format!(" {msg}"),
            theme.dim_style(),
        )));
    }
    while lines.len() < area.height as usize - 1 {
        lines.push(Line::from(""));
    }

    // Footer.
    let mut foot = vec![Span::styled(" ", Style::default())];
    if st.filtering || !st.filter.text.is_empty() {
        foot.push(Span::styled(" / ", theme.accent_style()));
        foot.push(Span::styled(
            format!("{:<30}", st.filter.text),
            theme.input(),
        ));
        foot.push(Span::raw("  "));
    }
    let hint = format!(
        "{} templates · sort: {}{} · Enter show occurrences · s sort · r reverse · / filter · y copy · Esc back",
        group(list.len() as u64),
        st.sort.label(),
        if st.ascending { " ascending (rare first)" } else { "" }
    );
    foot.push(Span::styled(
        fit(
            &hint,
            width.saturating_sub(foot.iter().map(|s| s.content.width()).sum::<usize>()),
            ascii,
        ),
        theme.dim_style(),
    ));
    lines.truncate(area.height as usize - 1);
    lines.push(Line::from(foot).style(if theme.is_mono() {
        Style::default()
    } else {
        Style::default().bg(theme.muted_bg)
    }));
    frame.render_widget(Paragraph::new(lines), area);
    if st.filtering {
        frame.set_cursor_position((
            area.x + 4 + st.filter.text.width() as u16,
            area.y + area.height - 1,
        ));
    }
}

/// Colour `<*>` wildcards differently from literal tokens.
fn template_spans(theme: &crate::theme::Theme, text: &str, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(p) = rest.find("<*>") {
        if p > 0 {
            out.push(Span::styled(rest[..p].to_string(), base));
        }
        out.push(Span::styled(
            "<*>".to_string(),
            base.patch(theme.fg_style(theme.badge)),
        ));
        rest = &rest[p + 3..];
    }
    if !rest.is_empty() {
        out.push(Span::styled(rest.to_string(), base));
    }
    out
}
