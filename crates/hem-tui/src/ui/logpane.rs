//! The virtualised log pane.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use hem_core::format::Format;
use hem_core::parse::{parse_line, LineMeta};
use hem_core::timestamp::{format_ms, TsDisplay};
use hem_core::view::Row;

use super::line::{match_ranges, segments, Kind};
use super::widgets::{clip_spans, group, wrap_spans};
use crate::app::{App, Hit, MAX_EXPAND};
use crate::theme::Theme;

const SPARK: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SPARK_ASCII: [char; 9] = [' ', '.', '.', ':', ':', '-', '=', '#', '#'];

/// Style for a syntax kind.
pub fn style_for(theme: &Theme, kind: Kind) -> Style {
    match kind {
        Kind::Text => Style::default(),
        Kind::Timestamp => theme.fg_style(theme.timestamp),
        Kind::Level(l) => theme.level_style(l),
        Kind::Key => theme.fg_style(theme.json_key),
        Kind::Str => theme.fg_style(theme.string),
        Kind::Number => theme.fg_style(theme.number),
        Kind::Url => theme.fg_style(theme.url).add_modifier(Modifier::UNDERLINED),
        Kind::Punct => theme.dim_style(),
        Kind::Match => theme.match_style(false),
        Kind::CurrentMatch => theme.match_style(true),
        Kind::Highlight => theme.highlight(),
    }
}

/// Display text for a line: lossy UTF-8, tabs expanded, control bytes
/// replaced, optional timestamp reformatting.  Returns the text and the
/// metadata re-derived for it (spans stay valid).
pub fn display_text(
    raw: &[u8],
    meta: &LineMeta,
    format: Format,
    ts: TsDisplay,
    origin: Option<i64>,
    year: i64,
) -> (String, LineMeta) {
    let mut bytes: Vec<u8> = Vec::with_capacity(raw.len() + 8);
    let mut changed = false;
    if ts != TsDisplay::Original {
        if let (Some(ms), Some((s, e))) = (meta.ts, meta.ts_span) {
            let (s, e) = (s as usize, e as usize);
            if e <= raw.len() {
                bytes.extend_from_slice(&raw[..s]);
                bytes.extend_from_slice(format_ms(ms, ts, origin).as_bytes());
                bytes.extend_from_slice(&raw[e..]);
                changed = true;
            }
        }
    }
    if !changed {
        bytes.extend_from_slice(raw);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text
        .bytes()
        .any(|b| b == b'\t' || (b < 0x20 && b != b'\n') || b == 0x7f)
    {
        text = text
            .chars()
            .map(|c| match c {
                '\t' => ' ',
                c if (c as u32) < 0x20 || c == '\u{7f}' => '·',
                c => c,
            })
            .collect();
        changed = true;
    }
    if changed || text.len() != raw.len() {
        let meta2 = parse_line(text.as_bytes(), format, year);
        (text, meta2)
    } else {
        (text, *meta)
    }
}

/// Build the styled spans for one line (owned, so they can outlive `text`).
#[allow(clippy::too_many_arguments)]
pub fn line_spans(
    theme: &Theme,
    text: &str,
    meta: &LineMeta,
    format: Format,
    search: Option<&regex::bytes::Regex>,
    current: bool,
    highlight: Option<&regex::bytes::Regex>,
) -> Vec<Span<'static>> {
    let bytes = text.as_bytes();
    let matches = search.map(|re| match_ranges(re, bytes)).unwrap_or_default();
    let highlights = highlight
        .map(|re| match_ranges(re, bytes))
        .unwrap_or_default();
    let segs = segments(bytes, meta, format, &matches, current, &highlights);
    let mut spans = Vec::with_capacity(segs.len());
    for s in segs {
        // Align to char boundaries (segments are byte based and the text is
        // valid UTF-8, but overlays could split a multi-byte char).
        let (mut a, mut b) = (s.start, s.end);
        while a > 0 && !text.is_char_boundary(a) {
            a -= 1;
        }
        while b < text.len() && !text.is_char_boundary(b) {
            b += 1;
        }
        if a >= b {
            continue;
        }
        spans.push(Span::styled(
            text[a..b].to_string(),
            style_for(theme, s.kind),
        ));
    }
    spans
}

/// Column layout for field-column mode: first N keys of the top line.
fn column_keys(app: &App, rows: &[Row]) -> Vec<String> {
    let t = app.tab();
    for r in rows.iter().take(5) {
        let line = t.line_bytes(*r);
        let fmt = t.format_for(r.source());
        if !fmt.has_fields() {
            continue;
        }
        let fields = hem_core::fields::extract(&line, fmt);
        if !fields.is_empty() {
            let mut keys: Vec<String> = fields.into_iter().map(|(k, _)| k).collect();
            keys.truncate(8);
            return keys;
        }
    }
    Vec::new()
}

/// Draw the log pane.
pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = app.theme.clone();
    let ascii = app.ascii;
    let ts_display = app.ts_display;
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }
    let active = app.active;
    let (top, cursor, hscroll, wrap, columns, merged, origin, year) = {
        let t = app.tab();
        (
            t.top,
            t.cursor,
            t.hscroll,
            t.wrap,
            t.columns,
            t.base.is_merged(),
            t.view.stats().ts_min,
            t.view.default_year(),
        )
    };
    let n = app.tab().len();
    let search_re = app.tab().search.as_ref().map(|s| s.matcher().regex.clone());
    let highlight_re = app.tab().highlight.as_ref().map(|m| m.regex.clone());
    let current_match_row = app
        .tab()
        .search
        .as_ref()
        .and_then(|s| {
            app.tab()
                .current_match
                .and_then(|i| s.matches().get(i).copied())
        })
        .map(|r| r as usize);

    let mut visible: Vec<(usize, u16, u16)> = Vec::new();
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut hits: Vec<(Rect, Hit)> = Vec::new();

    if n == 0 {
        let t = app.tab();
        let msg = if !t.base.is_complete() || !t.view.is_complete() {
            "indexing…"
        } else if !t.filter.is_empty() {
            "no lines match the current filters  (X clears them)"
        } else if t.base.is_live() {
            "waiting for data…"
        } else {
            "empty"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, theme.dim_style()))),
            Rect {
                x: area.x + 1,
                y: area.y + area.height / 2,
                width: area.width.saturating_sub(1),
                height: 1,
            },
        );
        let t = app.tab_mut();
        t.last_visible.clear();
        t.last_height = height;
        return;
    }

    let top = top.min(n - 1);
    let rows = app.tab().view.rows(top, (top + height).min(n));
    let col_keys = if columns {
        column_keys(app, &rows)
    } else {
        Vec::new()
    };
    let chunk = app.tab().view.chunk(top, (top + height).min(n));

    let mut y = 0usize;
    let src_count = app.tab().base.sources().len();
    for (k, row) in rows.iter().enumerate() {
        if y >= height {
            break;
        }
        let row_idx = top + k;
        let raw: Vec<u8> = chunk.line(k).to_vec();
        let fmt = app.tab().format_for(row.source());
        let meta = app.tab().meta_for(*row, &raw);
        let is_cursor = row_idx == cursor;
        let is_pinned = app.tab().is_pinned(*row);
        let is_current_match = current_match_row == Some(row_idx);
        let expanded = app.tab().expanded.contains(&row_idx);

        let mut prefix: Vec<Span> = Vec::new();
        // Gutter: pin marker.
        prefix.push(Span::styled(
            if is_pinned {
                if ascii {
                    "*"
                } else {
                    "▍"
                }
            } else {
                " "
            },
            theme.fg_style(theme.pin),
        ));
        if merged && src_count > 1 {
            let name = app
                .tab()
                .base
                .source(row.source())
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            let tag: String = name.chars().take(10).collect();
            prefix.push(Span::styled(
                format!("{tag:<10} "),
                theme.source_style(row.source() as usize),
            ));
        }
        if row.count() > 1 {
            let badge = if ascii {
                format!("x{} ", group(row.count() as u64))
            } else {
                format!("×{} ", group(row.count() as u64))
            };
            prefix.push(Span::styled(
                badge,
                theme.fg_style(theme.badge).add_modifier(Modifier::BOLD),
            ));
            let spark = app.tab_mut().sparkline(row_idx, 8);
            if !spark.is_empty() {
                let chars = if ascii { &SPARK_ASCII } else { &SPARK };
                let s: String = spark.iter().map(|h| chars[(*h as usize).min(8)]).collect();
                prefix.push(Span::styled(s, theme.fg_style(theme.sparkline)));
                prefix.push(Span::raw(" "));
            }
            let arrow = if ascii {
                if expanded {
                    "- "
                } else {
                    "+ "
                }
            } else if expanded {
                "▾ "
            } else {
                "▸ "
            };
            prefix.push(Span::styled(arrow, theme.dim_style()));
        }

        let body: Vec<Span<'static>> = if columns && !col_keys.is_empty() && fmt.has_fields() {
            column_spans(&theme, &raw, fmt, &col_keys, width)
        } else {
            let (text, meta2) = display_text(&raw, &meta, fmt, ts_display, origin, year);
            line_spans(
                &theme,
                &text,
                &meta2,
                fmt,
                search_re.as_ref(),
                is_current_match,
                highlight_re.as_ref(),
            )
        };
        let mut spans = prefix;
        spans.extend(body);

        let row_style = if is_cursor {
            theme.selection()
        } else if is_pinned && !theme.is_mono() {
            Style::default().bg(theme.muted_bg)
        } else {
            Style::default()
        };
        let display_lines: Vec<Line> = if wrap {
            wrap_spans(spans, width)
        } else {
            vec![Line::from(clip_spans(spans, hscroll, width))]
        };
        let start_y = y;
        for dl in display_lines {
            if y >= height {
                break;
            }
            lines.push(dl.style(row_style));
            hits.push((
                Rect {
                    x: area.x,
                    y: area.y + y as u16,
                    width: area.width,
                    height: 1,
                },
                Hit::Row(row_idx),
            ));
            y += 1;
        }
        // Expanded members.
        if expanded && y < height {
            let members = app.tab_mut().members(row_idx);
            for m in members.iter().skip(1).take(MAX_EXPAND) {
                if y >= height {
                    break;
                }
                let mraw = app.tab().line_bytes(*m);
                let mmeta = app.tab().meta_for(*m, &mraw);
                let (text, meta2) = display_text(&mraw, &mmeta, fmt, ts_display, origin, year);
                let mut spans: Vec<Span<'static>> = vec![Span::styled(
                    if ascii { " | " } else { " │ " },
                    theme.dim_style(),
                )];
                spans.extend(line_spans(
                    &theme,
                    &text,
                    &meta2,
                    fmt,
                    search_re.as_ref(),
                    false,
                    highlight_re.as_ref(),
                ));
                let dls = if wrap {
                    wrap_spans(spans, width)
                } else {
                    vec![Line::from(clip_spans(spans, hscroll, width))]
                };
                for dl in dls {
                    if y >= height {
                        break;
                    }
                    lines.push(dl);
                    hits.push((
                        Rect {
                            x: area.x,
                            y: area.y + y as u16,
                            width: area.width,
                            height: 1,
                        },
                        Hit::Row(row_idx),
                    ));
                    y += 1;
                }
            }
            if members.len() > MAX_EXPAND && y < height {
                lines.push(Line::from(Span::styled(
                    format!(
                        "   … {} more (open detail for all)",
                        row.count() as usize - MAX_EXPAND
                    ),
                    theme.dim_style(),
                )));
                y += 1;
            }
        }
        visible.push((row_idx, start_y as u16, (y - start_y) as u16));
    }

    frame.render_widget(Paragraph::new(lines), area);
    app.hits.extend(hits);
    let t = &mut app.tabs[active];
    t.last_visible = visible;
    t.last_height = height;
}

/// Field-column rendering for structured lines.
fn column_spans(
    theme: &Theme,
    raw: &[u8],
    fmt: Format,
    keys: &[String],
    width: usize,
) -> Vec<Span<'static>> {
    let fields = hem_core::fields::extract(raw, fmt);
    let mut spans = Vec::new();
    let colw = (width / keys.len().max(1)).clamp(8, 40);
    let mut used = 0usize;
    for k in keys {
        let v = fields
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let cell = super::widgets::pad(v, colw - 1, true);
        let style = match hem_core::Level::parse(v) {
            Some(l) if k.contains("level") || k.contains("severity") => theme.level_style(l),
            _ if v.parse::<f64>().is_ok() => theme.fg_style(theme.number),
            _ => Style::default(),
        };
        spans.push(Span::styled(format!("{cell} "), style));
        used += colw;
        if used >= width {
            break;
        }
    }
    // Remaining fields inline.
    let rest: Vec<String> = fields
        .iter()
        .filter(|(k, _)| !keys.contains(k))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !rest.is_empty() {
        spans.push(Span::styled(rest.join(" "), theme.dim_style()));
    }
    spans
}

/// Draw the vertical scrollbar.
pub fn scrollbar(app: &mut App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = &app.theme;
    let t = app.tab();
    let n = t.len().max(1);
    let h = area.height as usize;
    let thumb_h = ((h * h) / n.max(h)).clamp(1, h);
    let thumb_y = if n > h {
        (t.top * (h - thumb_h)) / (n - h).max(1)
    } else {
        0
    };
    let track = if app.ascii { "|" } else { "│" };
    let thumb = if app.ascii { "#" } else { "┃" };
    let mut lines = Vec::with_capacity(h);
    for y in 0..h {
        if y >= thumb_y && y < thumb_y + thumb_h {
            lines.push(Line::from(Span::styled(
                thumb,
                theme.fg_style(theme.accent),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                track,
                theme.fg_style(theme.scrollbar),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
    app.hits.push((area, Hit::Scrollbar));
    // Clicking the track jumps proportionally.
    for y in 0..h {
        app.hits.push((
            Rect {
                x: area.x,
                y: area.y + y as u16,
                width: 1,
                height: 1,
            },
            Hit::Track { cell: y, cells: h },
        ));
    }
}

/// Width of a string in terminal columns (helper for tests).
pub fn display_width(s: &str) -> usize {
    s.width()
}
