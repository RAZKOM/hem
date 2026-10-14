//! Centred overlays: palette, filter editor, go-to, settings, help, source
//! picker, line detail and messages.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use hem_core::timestamp::{format_ms, TsDisplay};

use super::logpane::{display_text, line_spans};
use super::widgets::{block, centered, dim, fit, group, pad, wrap_spans};
use crate::app::{App, Hit};
use crate::modal::{FilterEditorState, Modal, SettingItem};
use crate::theme::Theme;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let Some(modal) = app.modal.clone() else {
        return;
    };
    dim(frame.buffer_mut(), area, &app.theme);
    match modal {
        Modal::Prompt { .. } => {}
        Modal::Palette { input, selected } => palette(app, frame, area, &input.text, selected),
        Modal::FilterEditor(st) => filter_editor(app, frame, area, &st),
        Modal::Settings { selected } => settings(app, frame, area, selected),
        Modal::Help { scroll } => help(app, frame, area, scroll),
        Modal::Sources { selected } => sources(app, frame, area, selected),
        Modal::Detail {
            row_idx,
            row,
            scroll,
            selected_field,
        } => detail(app, frame, area, row_idx, row, scroll, selected_field),
        Modal::Message { title, lines } => message(app, frame, area, &title, &lines),
    }
}

fn frame_box(app: &mut App, frame: &mut Frame, area: Rect, title: &str, w: u16, h: u16) -> Rect {
    let r = centered(area, w, h);
    frame.render_widget(Clear, r);
    let b = block(title, &app.theme, app.ascii);
    let inner = b.inner(r);
    frame.render_widget(b, r);
    // Close button in the top-right corner.
    let close = if app.ascii { "[x]" } else { " ✕ " };
    let cx = r.x + r.width.saturating_sub(4);
    frame.render_widget(
        Paragraph::new(Span::styled(
            close,
            app.theme.fg_style(app.theme.chip_close),
        )),
        Rect {
            x: cx,
            y: r.y,
            width: 3,
            height: 1,
        },
    );
    app.hits.push((
        Rect {
            x: cx,
            y: r.y,
            width: 3,
            height: 1,
        },
        Hit::ModalClose,
    ));
    inner
}

fn footer(theme: &Theme, text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), theme.dim_style()))
}

fn palette(app: &mut App, frame: &mut Frame, area: Rect, query: &str, selected: usize) {
    let w = area.width.min(70);
    let h = area.height.min(20);
    let inner = frame_box(app, frame, area, "Command palette", w, h);
    let theme = app.theme.clone();
    let matches = app.palette_matches(query);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(" > ", theme.accent_style()),
        Span::styled(
            format!("{query:<w$}", w = inner.width.saturating_sub(4) as usize),
            theme.input(),
        ),
    ]));
    let list_h = inner.height.saturating_sub(2) as usize;
    let first = selected.saturating_sub(list_h.saturating_sub(1));
    for (i, (idx, _)) in matches.iter().enumerate().skip(first).take(list_h) {
        let item = &app.palette_items[*idx];
        let label = item.label(app);
        let keys = match item {
            crate::app::PaletteItem::Action(a) => app.keymap.keys_label(*a, app.ascii),
            _ => String::new(),
        };
        let lw = inner.width as usize;
        let text = pad(
            &format!(" {label}"),
            lw.saturating_sub(keys.width() + 2),
            app.ascii,
        );
        let style = if i == selected {
            theme.selection().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(
                format!("{keys} "),
                if i == selected {
                    style.fg(theme.accent)
                } else {
                    theme.dim_style()
                },
            ),
        ]));
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + lines.len() as u16 - 1,
                width: inner.width,
                height: 1,
            },
            Hit::ModalItem(i),
        ));
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching commands",
            theme.dim_style(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    frame.set_cursor_position((inner.x + 3 + query.width() as u16, inner.y));
}

fn filter_editor(app: &mut App, frame: &mut Frame, area: Rect, st: &FilterEditorState) {
    let w = area.width.min(72);
    let h = area.height.min(17);
    let inner = frame_box(app, frame, area, "Filter editor", w, h);
    let theme = app.theme.clone();
    let lw = 10usize;
    let fw = (inner.width as usize).saturating_sub(lw + 3);
    let field = |label: &str, value: &str, idx: usize, focus: usize| -> Line<'static> {
        let focused = idx == focus;
        let style = if focused {
            theme.input().add_modifier(Modifier::BOLD)
        } else {
            theme.input()
        };
        Line::from(vec![
            Span::styled(
                format!(" {label:<lw$} "),
                if focused {
                    theme.accent_style()
                } else {
                    Style::default()
                },
            ),
            Span::styled(pad(value, fw, true), style),
        ])
    };
    let level = st
        .level
        .map(|l| l.to_string())
        .unwrap_or_else(|| "any".into());
    let level_text = if app.ascii {
        format!("< {level} >")
    } else {
        format!("◂ {level} ▸")
    };
    let literal_text = if st.literal {
        "[x] literal (no regex)"
    } else {
        "[ ] literal (no regex)"
    };
    let mut lines = vec![
        field("include", &st.include.text, 0, st.focus),
        field("exclude", &st.exclude.text, 1, st.focus),
        field("fields", &st.fields.text, 2, st.focus),
        Line::from(vec![
            Span::styled(
                format!(" {:<lw$} ", "level ≥"),
                if st.focus == 3 {
                    theme.accent_style()
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                level_text,
                if st.focus == 3 {
                    theme.selection()
                } else {
                    Style::default()
                },
            ),
        ]),
        field("since", &st.since.text, 4, st.focus),
        field("until", &st.until.text, 5, st.focus),
        Line::from(vec![
            Span::raw(format!(" {:<lw$} ", "")),
            Span::styled(
                literal_text,
                if st.focus == 6 {
                    theme.selection()
                } else {
                    Style::default()
                },
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw(format!(" {:<lw$} ", "")),
            Span::styled(
                " Apply ",
                if st.focus == 7 {
                    theme.selection().add_modifier(Modifier::BOLD)
                } else {
                    theme.chip()
                },
            ),
            Span::raw("  "),
            Span::styled(
                " Clear all ",
                if st.focus == 8 {
                    theme.selection().add_modifier(Modifier::BOLD)
                } else {
                    theme.chip()
                },
            ),
        ]),
        Line::from(""),
        footer(
            &theme,
            " Tab/↑↓ move · Enter apply · separate several patterns with && · Esc cancel",
        ),
    ];
    if let Some(e) = &st.error {
        lines.push(Line::from(Span::styled(
            format!(" {e}"),
            theme.fg_style(theme.warning),
        )));
    }
    for (i, y) in [0u16, 1, 2, 3, 4, 5, 6].iter().enumerate() {
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + y,
                width: inner.width,
                height: 1,
            },
            Hit::ModalItem(i),
        ));
    }
    app.hits.push((
        Rect {
            x: inner.x + lw as u16 + 2,
            y: inner.y + 8,
            width: 7,
            height: 1,
        },
        Hit::ModalItem(7),
    ));
    app.hits.push((
        Rect {
            x: inner.x + lw as u16 + 11,
            y: inner.y + 8,
            width: 11,
            height: 1,
        },
        Hit::ModalItem(8),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
    // Cursor in the focused text field.
    let cursor_in = |input: &crate::modal::Input| -> u16 {
        input
            .text
            .chars()
            .take(input.cursor)
            .collect::<String>()
            .width() as u16
    };
    let cy = match st.focus {
        0 => Some((0, cursor_in(&st.include))),
        1 => Some((1, cursor_in(&st.exclude))),
        2 => Some((2, cursor_in(&st.fields))),
        4 => Some((4, cursor_in(&st.since))),
        5 => Some((5, cursor_in(&st.until))),
        _ => None,
    };
    if let Some((row, col)) = cy {
        frame.set_cursor_position((inner.x + lw as u16 + 2 + col.min(fw as u16), inner.y + row));
    }
}

fn settings(app: &mut App, frame: &mut Frame, area: Rect, selected: usize) {
    let w = area.width.min(60);
    let h = area.height.min(15);
    let inner = frame_box(app, frame, area, "Settings", w, h);
    let theme = app.theme.clone();
    let mut lines = Vec::new();
    for (i, item) in SettingItem::ALL.iter().enumerate() {
        let value = match item {
            SettingItem::Theme => theme.name.clone(),
            SettingItem::Collapse => onoff(app.config.defaults.collapse),
            SettingItem::Timestamp => format!("{:?}", app.ts_display).to_lowercase(),
            SettingItem::Wrap => onoff(app.config.defaults.wrap),
            SettingItem::Follow => onoff(app.config.defaults.follow),
            SettingItem::Columns => onoff(app.config.defaults.columns),
            SettingItem::Minimap => onoff(app.config.defaults.minimap),
            SettingItem::Mouse => onoff(app.config.defaults.mouse),
            SettingItem::Ascii => onoff(app.config.defaults.ascii),
        };
        let lw = (inner.width as usize).saturating_sub(16);
        let style = if i == selected {
            theme.selection().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(pad(&format!(" {}", item.label()), lw, app.ascii), style),
            Span::styled(
                format!(" {value:<13}"),
                if i == selected {
                    style.fg(theme.accent)
                } else {
                    theme.accent_style()
                },
            ),
        ]));
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            },
            Hit::ModalItem(i),
        ));
    }
    lines.push(Line::from(""));
    let path = app
        .config_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no config path)".into());
    lines.push(footer(
        &theme,
        " Enter/Space/←→ change · changes are saved to:",
    ));
    lines.push(footer(
        &theme,
        &format!(" {}", fit(&path, inner.width as usize - 2, app.ascii)),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn onoff(b: bool) -> String {
    if b { "on" } else { "off" }.to_string()
}

fn help(app: &mut App, frame: &mut Frame, area: Rect, scroll: usize) {
    let w = area.width.min(80);
    let h = area.height.min(40);
    let inner = frame_box(app, frame, area, "Help: keys", w, h);
    let theme = app.theme.clone();
    let mut lines: Vec<Line> = Vec::new();
    for (cat, rows) in app.keymap.help_rows(app.ascii) {
        lines.push(Line::from(Span::styled(
            format!(" {}", cat.title()),
            theme.title_style(),
        )));
        for (keys, label) in rows {
            lines.push(Line::from(vec![
                Span::styled(format!("   {keys:<18} "), theme.accent_style()),
                Span::raw(label.to_string()),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(" Filters", theme.title_style())));
    for l in [
        "   include/exclude take regexes (smart case: all-lowercase = case-insensitive)",
        "   field filters: status>=500  user=bob  msg~time.?out  err!=  trace_id?",
        "   go to: 1234 (line) · 50% · 10:30 · 2026-10-13T10:30:00Z · 15m (ago)",
        "   templates view: Enter shows a template's occurrences; sort ascending to find rare lines",
    ] {
        lines.push(Line::from(Span::raw(l)));
    }
    lines.push(Line::from(""));
    lines.push(footer(&theme, " j/k scroll · Esc close"));
    let total = lines.len();
    let visible = inner.height as usize;
    let scroll = scroll.min(total.saturating_sub(visible));
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

fn sources(app: &mut App, frame: &mut Frame, area: Rect, selected: usize) {
    let w = area.width.min(70);
    let h = area.height.min((app.tabs.len() + 7) as u16);
    let inner = frame_box(app, frame, area, "Sources", w, h);
    let theme = app.theme.clone();
    let mut lines = Vec::new();
    let n = app.tabs.len();
    for (i, t) in app.tabs.iter().enumerate() {
        let srcs = t.base.sources();
        let info = if t.base.is_merged() {
            format!("merged · {} sources", srcs.len())
        } else {
            let s = &srcs[0];
            format!(
                "{} · {} lines · {}{}",
                super::widgets::bytes(s.total_bytes()),
                group(s.line_count() as u64),
                s.format(),
                if s.is_complete() { "" } else { " · indexing" }
            )
        };
        let style = if i == selected {
            theme.selection().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let active = if i == app.active {
            if app.ascii {
                "*"
            } else {
                "●"
            }
        } else {
            " "
        };
        let lw = inner.width as usize;
        let text = pad(&format!(" {active} {}  ", t.name), 28.min(lw), app.ascii);
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(
                fit(&info, lw.saturating_sub(28), app.ascii),
                if i == selected {
                    style
                } else {
                    theme.dim_style()
                },
            ),
        ]));
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            },
            Hit::ModalItem(i),
        ));
    }
    let extra = [
        ("merge all sources by timestamp", n),
        ("open another file…", n + 1),
    ];
    for (label, idx) in extra {
        let style = if idx == selected {
            theme.selection().add_modifier(Modifier::BOLD)
        } else {
            theme.accent_style()
        };
        lines.push(Line::from(Span::styled(
            pad(&format!("   + {label}"), inner.width as usize, app.ascii),
            style,
        )));
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + idx as u16,
                width: inner.width,
                height: 1,
            },
            Hit::ModalItem(idx),
        ));
    }
    lines.push(Line::from(""));
    lines.push(footer(
        &theme,
        " Enter switch · d close source · Tab in the main view also switches",
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

#[allow(clippy::too_many_arguments)]
fn detail(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    row_idx: usize,
    row: hem_core::view::Row,
    scroll: usize,
    selected_field: usize,
) {
    let w = area.width.min(100);
    let h = area.height.min(32);
    let title = format!(
        "Line {} · {}",
        group(row.line() + 1),
        app.tab()
            .base
            .source(row.source())
            .map(|s| s.name().to_string())
            .unwrap_or_default()
    );
    let inner = frame_box(app, frame, area, &title, w, h);
    let theme = app.theme.clone();
    let t = app.tab();
    let raw = t.line_bytes(row);
    let fmt = t.format_for(row.source()).for_line(&raw);
    let meta = t.meta_for(row, &raw);
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Header: timestamp, level, format.
    let mut head = vec![Span::raw(" ")];
    if let Some(ms) = meta.ts {
        head.push(Span::styled(
            format_ms(ms, TsDisplay::Iso, None),
            theme.fg_style(theme.timestamp),
        ));
        head.push(Span::raw("  "));
    }
    if let Some(l) = meta.level {
        head.push(Span::styled(l.as_str(), theme.level_style(l)));
        head.push(Span::raw("  "));
    }
    head.push(Span::styled(fmt.as_str(), theme.dim_style()));
    if row.count() > 1 {
        head.push(Span::styled(
            format!("  ×{} repeats", group(row.count() as u64)),
            theme.fg_style(theme.badge),
        ));
    }
    if t.is_pinned(row) {
        head.push(Span::styled("  pinned", theme.fg_style(theme.pin)));
    }
    lines.push(Line::from(head));
    lines.push(Line::from(""));

    // Body: pretty JSON or the raw line (wrapped).
    let pretty = if fmt == hem_core::format::Format::Json {
        serde_json::from_slice::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
    } else {
        None
    };
    match pretty {
        Some(p) => {
            for l in p.lines() {
                let l: String = l.to_string();
                let m = hem_core::parse::LineMeta::default();
                let spans = line_spans(
                    &theme,
                    &l,
                    &m,
                    hem_core::format::Format::Json,
                    None,
                    false,
                    None,
                );
                let owned: Vec<Span<'static>> = spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.into_owned(), s.style))
                    .collect();
                for wl in wrap_spans(owned, width.saturating_sub(2)) {
                    let mut v = vec![Span::raw(" ")];
                    v.extend(wl.spans);
                    lines.push(Line::from(v));
                }
            }
        }
        None => {
            let (text, meta2) = display_text(
                &raw,
                &meta,
                fmt,
                TsDisplay::Original,
                None,
                t.view.default_year(),
            );
            let spans = line_spans(&theme, &text, &meta2, fmt, None, false, None);
            let owned: Vec<Span<'static>> = spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style))
                .collect();
            for wl in wrap_spans(owned, width.saturating_sub(2)) {
                let mut v = vec![Span::raw(" ")];
                v.extend(wl.spans);
                lines.push(Line::from(v));
            }
        }
    }

    // Fields table.
    let fields = hem_core::fields::extract(&raw, fmt);
    let mut field_rows: Vec<usize> = Vec::new();
    if !fields.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Fields  (Tab selects · * highlights lines with the same value)",
            theme.title_style(),
        )));
        let kw = fields
            .iter()
            .map(|(k, _)| k.width())
            .max()
            .unwrap_or(4)
            .min(30);
        for (i, (k, v)) in fields.iter().enumerate() {
            let style = if i == selected_field {
                theme.selection()
            } else {
                Style::default()
            };
            field_rows.push(lines.len());
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}", pad(k, kw, true)),
                    style.patch(theme.fg_style(theme.json_key)),
                ),
                Span::styled("  ", style),
                Span::styled(fit(v, width.saturating_sub(kw + 6), app.ascii), style),
            ]));
        }
    }

    // Context.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Context", theme.title_style())));
    let n = t.len();
    let from = row_idx.saturating_sub(4);
    let to = (row_idx + 5).min(n);
    let chunk = t.view.chunk(from, to);
    for k in 0..chunk.len() {
        let r = chunk.row(k);
        let is_this = from + k == row_idx;
        let cfmt = t.format_for(r.source());
        let craw = chunk.line(k);
        let cmeta = hem_core::parse::parse_line(craw, cfmt, t.view.default_year());
        let (text, meta2) = display_text(
            craw,
            &cmeta,
            cfmt,
            TsDisplay::Original,
            None,
            t.view.default_year(),
        );
        let spans = line_spans(&theme, &text, &meta2, cfmt, None, false, None);
        let mut v = vec![Span::styled(
            if is_this {
                if app.ascii {
                    " > "
                } else {
                    " ▶ "
                }
            } else {
                "   "
            },
            theme.accent_style(),
        )];
        v.extend(
            spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style)),
        );
        let mut line = Line::from(v);
        if is_this {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    lines.push(Line::from(""));
    lines.push(footer(
        &theme,
        " j/k scroll · n/N next/prev line · p pin · y copy · Esc close",
    ));

    let visible = inner.height as usize;
    let scroll = scroll.min(lines.len().saturating_sub(visible));
    for (i, fr) in field_rows.iter().enumerate() {
        if *fr >= scroll && *fr < scroll + visible {
            app.hits.push((
                Rect {
                    x: inner.x,
                    y: inner.y + (*fr - scroll) as u16,
                    width: inner.width,
                    height: 1,
                },
                Hit::ModalItem(i),
            ));
        }
    }
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

fn message(app: &mut App, frame: &mut Frame, area: Rect, title: &str, body: &[String]) {
    let w = area.width.min(70);
    let h = area.height.min(body.len() as u16 + 4);
    let inner = frame_box(app, frame, area, title, w, h);
    let lines: Vec<Line> = body.iter().map(|l| Line::from(format!(" {l}"))).collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
