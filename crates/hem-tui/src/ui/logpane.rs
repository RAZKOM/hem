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
use crate::app::{App, ColumnLayout, Hit, PinPos, MAX_EXPAND};
use crate::keymap::Action;
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

/// Most pinned lines shown in the strip above the log.
pub const MAX_PIN_STRIP: usize = 5;

/// Width of the source-name column in merged views.
const SOURCE_TAG_W: usize = 11;

/// Height of the pinned strip (pinned lines plus a divider) for a log
/// column `height` rows tall; 0 when nothing is pinned or space is short.
pub fn pin_strip_height(app: &App, height: u16) -> u16 {
    let pins = app.tab().pins.len();
    if pins == 0 || height < 8 {
        return 0;
    }
    let room = (height as usize / 3).saturating_sub(1).max(1);
    (pins.min(MAX_PIN_STRIP).min(room) + 1) as u16
}

/// Is the column header shown for the active tab?
pub fn column_header_visible(app: &App) -> bool {
    let t = app.tab();
    t.columns && t.col_layout.as_ref().is_some_and(|l| !l.keys.is_empty())
}

/// The source-name column for merged views.
fn source_tag(app: &App, row: Row, theme: &Theme) -> Span<'static> {
    let name = app
        .tab()
        .base
        .source(row.source())
        .map(|s| s.name().to_string())
        .unwrap_or_default();
    let tag: String = name.chars().take(SOURCE_TAG_W - 1).collect();
    Span::styled(
        format!("{tag:<w$} ", w = SOURCE_TAG_W - 1),
        theme.source_style(row.source() as usize),
    )
}

/// Draw the pinned strip: pinned lines stay on screen however far the log
/// scrolls.  Click one to jump to it.
pub fn pins(app: &mut App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = app.theme.clone();
    let ascii = app.ascii;
    let width = area.width as usize;
    let shown = area.height as usize - 1;
    let (origin, year) = {
        let t = app.tab();
        (t.view.stats().ts_min, t.view.default_year())
    };
    let merged = app.tab().base.is_merged() && app.tab().base.sources().len() > 1;
    let order = app.tab().pins_in_order();
    let cursor = app.tab().cursor;
    let highlight_re = app.tab().highlight.as_ref().map(|m| m.regex.clone());
    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
    let mut hidden = 0usize;
    for (k, &(idx, pos)) in order.iter().enumerate() {
        if pos == PinPos::Hidden {
            hidden += 1;
        }
        if k >= shown {
            continue;
        }
        let (src, line) = app.tab().pins[idx];
        let row = Row::new(line, src, 1);
        let raw = app.tab().line_bytes(row);
        let fmt = app.tab().format_for(src);
        let meta = app.tab().meta_for(row, &raw);
        let (text, meta2) = display_text(&raw, &meta, fmt, app.ts_display, origin, year);
        let at_cursor = pos == PinPos::Row(cursor);
        let mut spans = vec![Span::styled(
            if ascii { "*" } else { "▍" },
            theme.fg_style(theme.pin).add_modifier(Modifier::BOLD),
        )];
        if merged {
            spans.push(source_tag(app, row, &theme));
        }
        spans.push(Span::styled(
            format!("{:>8} ", group(line + 1)),
            theme.fg_style(theme.pin),
        ));
        let body = line_spans(
            &theme,
            &text,
            &meta2,
            fmt,
            None,
            false,
            highlight_re.as_ref(),
        );
        if pos == PinPos::Hidden {
            spans.push(Span::styled("filtered out ", theme.dim_style()));
            spans.extend(body.into_iter().map(|s| {
                let c = s.content.into_owned();
                Span::styled(c, theme.dim_style())
            }));
        } else {
            spans.extend(body);
        }
        let style = if at_cursor {
            theme.selection()
        } else if theme.is_mono() {
            Style::default()
        } else {
            Style::default().bg(theme.muted_bg)
        };
        lines.push(Line::from(clip_spans(spans, 0, width)).style(style));
        app.hits.push((
            Rect {
                x: area.x,
                y: area.y + k as u16,
                width: area.width,
                height: 1,
            },
            Hit::Pin(idx),
        ));
    }
    // Divider with a legend.
    let mut legend = format!(
        " {} pinned · {} / {} jump · {} unpin ",
        order.len(),
        app.keymap.keys_label(Action::NextPin, ascii),
        app.keymap.keys_label(Action::PrevPin, ascii),
        app.keymap.keys_label(Action::Pin, ascii),
    );
    if order.len() > shown {
        legend.push_str(&format!("· {} more not shown ", order.len() - shown));
    }
    if hidden > 0 {
        legend.push_str(&format!("· {hidden} filtered out "));
    }
    let rule = if ascii { "-" } else { "─" };
    let legend = super::widgets::fit(&legend, width.saturating_sub(2), ascii);
    let fill = width.saturating_sub(2 + legend.width());
    lines.push(Line::from(vec![
        Span::styled(rule.repeat(2), theme.fg_style(theme.border)),
        Span::styled(legend, theme.fg_style(theme.pin)),
        Span::styled(rule.repeat(fill), theme.fg_style(theme.border)),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}

/// Width of the row prefix in column mode, so the header lines up.
fn column_prefix_width(merged: bool, layout: &ColumnLayout) -> usize {
    1 + if merged { SOURCE_TAG_W } else { 0 }
        + if layout.count_w > 0 {
            layout.count_w + 1
        } else {
            0
        }
}

/// Draw the field-column header (key names), scrolled with the rows.
pub fn column_header(app: &mut App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = app.theme.clone();
    let ascii = app.ascii;
    let t = app.tab();
    let Some(layout) = t.col_layout.clone() else {
        return;
    };
    let merged = t.base.is_merged() && t.base.sources().len() > 1;
    let hscroll = t.hscroll;
    let style = theme.fg_style(theme.json_key).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    if merged {
        spans.push(Span::styled(
            format!("{:<w$}", "source", w = SOURCE_TAG_W),
            style,
        ));
    }
    if layout.count_w > 0 {
        spans.push(Span::styled(
            format!("{:>w$} ", "count", w = layout.count_w),
            style,
        ));
    }
    debug_assert_eq!(
        spans.iter().map(|s| s.content.width()).sum::<usize>(),
        column_prefix_width(merged, &layout)
    );
    for (k, w) in layout.keys.iter().zip(&layout.widths) {
        let cell = if *w == 0 {
            k.clone()
        } else {
            super::widgets::pad(k, *w, ascii)
        };
        spans.push(Span::styled(format!("{cell} "), style));
    }
    let mut spans = clip_spans(spans, hscroll, area.width as usize);
    if hscroll > 0 {
        // Show that the table is scrolled sideways.
        let arrow = if ascii { "<" } else { "◂" };
        if let Some(first) = spans.first_mut() {
            let rest: String = first.content.chars().skip(1).collect();
            *first = Span::styled(rest, first.style);
        }
        spans.insert(0, Span::styled(arrow, theme.accent_style()));
    }
    let bg = if theme.is_mono() {
        Style::default().add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().bg(theme.muted_bg)
    };
    frame.render_widget(Paragraph::new(Line::from(spans).style(bg)), area);
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
    let layout: Option<ColumnLayout> = if columns {
        app.tab().col_layout.clone().filter(|l| !l.keys.is_empty())
    } else {
        None
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
            prefix.push(source_tag(app, *row, &theme));
        }
        // Column mode: structured rows get a fixed-width prefix so every
        // cell lines up under the header.
        let col_layout = layout.as_ref().filter(|_| fmt.has_fields());
        if let Some(l) = col_layout {
            if l.count_w > 0 {
                let badge = if row.count() > 1 {
                    format!(
                        "{}{}",
                        if ascii { "x" } else { "×" },
                        group(row.count() as u64)
                    )
                } else {
                    String::new()
                };
                prefix.push(Span::styled(
                    format!("{badge:>w$} ", w = l.count_w),
                    theme.fg_style(theme.badge).add_modifier(Modifier::BOLD),
                ));
            }
        } else if row.count() > 1 {
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

        let body: Vec<Span<'static>> = if let Some(l) = col_layout {
            column_spans(
                &theme,
                &raw,
                fmt,
                l,
                ascii,
                search_re.as_ref(),
                is_current_match,
                highlight_re.as_ref(),
            )
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

/// Split `text` into spans with search / highlight matches overlaid on
/// `base`.
fn overlay(
    theme: &Theme,
    text: String,
    base: Style,
    search: Option<&regex::bytes::Regex>,
    current: bool,
    highlight: Option<&regex::bytes::Regex>,
) -> Vec<Span<'static>> {
    let bytes = text.as_bytes();
    let mut marks: Vec<(usize, usize, Style)> = Vec::new();
    if let Some(re) = highlight {
        for (a, b) in match_ranges(re, bytes) {
            marks.push((a, b, theme.highlight()));
        }
    }
    if let Some(re) = search {
        for (a, b) in match_ranges(re, bytes) {
            marks.push((a, b, theme.match_style(current)));
        }
    }
    if marks.is_empty() {
        return vec![Span::styled(text, base)];
    }
    marks.sort_by_key(|m| m.0);
    let mut out = Vec::new();
    let mut pos = 0usize;
    for (a, b, style) in marks {
        let (a, b) = (a.max(pos), b.min(text.len()));
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        if a > pos {
            out.push(Span::styled(text[pos..a].to_string(), base));
        }
        out.push(Span::styled(text[a..b].to_string(), style));
        pos = b;
    }
    if pos < text.len() {
        out.push(Span::styled(text[pos..].to_string(), base));
    }
    out
}

/// A field value made safe for one terminal cell row.
fn cell_text(v: &str) -> String {
    v.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Field-column rendering for structured lines: one padded cell per layout
/// key (blank when the line lacks it), the message unpadded, then any
/// other fields as dim `key=value` pairs.
#[allow(clippy::too_many_arguments)]
fn column_spans(
    theme: &Theme,
    raw: &[u8],
    fmt: Format,
    layout: &ColumnLayout,
    ascii: bool,
    search: Option<&regex::bytes::Regex>,
    current: bool,
    highlight: Option<&regex::bytes::Regex>,
) -> Vec<Span<'static>> {
    let fields = hem_core::fields::extract(raw, fmt);
    let mut spans = Vec::new();
    let mut used = vec![false; fields.len()];
    for (k, w) in layout.keys.iter().zip(&layout.widths) {
        let v = match ColumnLayout::field_for(&fields, k) {
            Some(i) => {
                used[i] = true;
                cell_text(&fields[i].1)
            }
            None => String::new(),
        };
        let style = match hem_core::Level::parse(&v) {
            Some(l) if k.contains("level") || k.contains("severity") => theme.level_style(l),
            _ if v.parse::<f64>().is_ok() => theme.fg_style(theme.number),
            _ => Style::default(),
        };
        let cell = if *w == 0 {
            v
        } else {
            super::widgets::pad(&v, *w, ascii)
        };
        spans.extend(overlay(theme, cell, style, search, current, highlight));
        spans.push(Span::raw(" "));
    }
    // Remaining fields inline.
    let rest: Vec<String> = fields
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|((k, v), _)| format!("{k}={}", cell_text(v)))
        .collect();
    if !rest.is_empty() {
        spans.extend(overlay(
            theme,
            rest.join(" "),
            theme.dim_style(),
            search,
            current,
            highlight,
        ));
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
