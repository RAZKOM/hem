//! Centred overlays: palette, filter editor, go-to, settings, help, source
//! picker, line detail and messages.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::widgets::{block, centered, pad};
use crate::app::{App, Hit};
use crate::modal::FilterEditorState;
use crate::theme::Theme;

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

