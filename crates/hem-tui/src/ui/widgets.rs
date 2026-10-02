//! Small drawing helpers shared by the views.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// A rectangle of `w`×`h` centred in `area` (clamped).
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Dim everything in `area` (used under modals).
pub fn dim(buf: &mut Buffer, area: Rect, theme: &Theme) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if !theme.is_mono() {
                    cell.set_fg(theme.dim);
                }
                cell.modifier.insert(Modifier::DIM);
            }
        }
    }
}

/// Border glyphs honouring `--ascii`.
pub fn border_set(ascii: bool) -> border::Set<'static> {
    if ascii {
        border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        }
    } else {
        border::ROUNDED
    }
}

/// A bordered block with a title.
pub fn block<'a>(title: &'a str, theme: &Theme, ascii: bool) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(border_set(ascii))
        .border_style(Style::default().fg(theme.modal_border))
        .title(Span::styled(format!(" {title} "), theme.title_style()))
        .style(theme.modal())
}

/// Truncate `s` to `width` columns, appending `…` (or `~` in ASCII mode).
pub fn fit(s: &str, width: usize, ascii: bool) -> String {
    if width == 0 {
        return String::new();
    }
    if s.width() <= width {
        return s.to_string();
    }
    let ell = if ascii { "~" } else { "…" };
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push_str(ell);
    out
}

/// Pad / truncate to exactly `width` columns.
pub fn pad(s: &str, width: usize, ascii: bool) -> String {
    let t = fit(s, width, ascii);
    let w = t.width();
    if w < width {
        format!("{t}{}", " ".repeat(width - w))
    } else {
        t
    }
}

/// Split a styled line into display lines of at most `width` columns.
pub fn wrap_spans<'a>(spans: Vec<Span<'a>>, width: usize) -> Vec<Line<'a>> {
    if width == 0 {
        return vec![Line::from(spans)];
    }
    let mut lines = Vec::new();
    let mut cur: Vec<Span<'a>> = Vec::new();
    let mut cur_w = 0usize;
    for span in spans {
        let style = span.style;
        let mut buf = String::new();
        for c in span.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if cur_w + cw > width {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), style));
                }
                lines.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            }
            buf.push(c);
            cur_w += cw;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(Line::from(cur));
    }
    lines
}

/// Skip `skip` columns then keep at most `width` columns of a span list.
pub fn clip_spans<'a>(spans: Vec<Span<'a>>, skip: usize, width: usize) -> Vec<Span<'a>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let end = skip + width;
    for span in spans {
        let style = span.style;
        let mut buf = String::new();
        for c in span.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            let (s, e) = (pos, pos + cw);
            pos = e;
            if e <= skip || s >= end {
                continue;
            }
            if s < skip || e > end {
                // Partially visible wide char: replace with a space.
                buf.push(' ');
            } else {
                buf.push(c);
            }
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, style));
        }
        if pos >= end {
            break;
        }
    }
    out
}

/// Format a number with thousands separators.
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Human-readable byte size.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
        assert_eq!(bytes(2_470_000_000), "2.3 GB");
        assert_eq!(fit("hello world", 5, false), "hell…");
        assert_eq!(fit("hi", 5, false), "hi");
        assert_eq!(pad("hi", 4, true), "hi  ");
        let spans = vec![Span::raw("abcdef"), Span::raw("ghij")];
        let lines = wrap_spans(spans.clone(), 4);
        assert_eq!(lines.len(), 3);
        let clipped = clip_spans(spans, 2, 5);
        let text: String = clipped.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "cdefg");
    }
}
