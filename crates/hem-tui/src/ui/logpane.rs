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
use crate::app::{App, Hit};
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
