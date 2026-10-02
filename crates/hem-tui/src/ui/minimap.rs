//! Timeline minimap: a vertical histogram of the whole view, coloured by
//! the highest severity in each bucket, with a viewport indicator.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use hem_core::timeline::Counts;
use hem_core::Level;

use crate::app::{App, Hit};

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const BLOCKS_ASCII: [char; 9] = [' ', '.', '.', ':', ':', '-', '=', '#', '@'];

/// Highest severity present in a bucket.
pub fn top_level(c: &Counts) -> Option<Level> {
    (0..Level::COUNT)
        .rev()
        .find(|&i| c[i] > 0)
        .and_then(Level::from_index)
}

/// Total lines in a bucket.
pub fn total(c: &Counts) -> u32 {
    c.iter().take(Level::COUNT + 1).sum()
}

/// Cells (one per terminal row) for a histogram of `n` rows: `(height
/// 0..=8, level)`.  Uses a log scale so that a handful of lines next to a
/// burst still shows up.
pub fn cells(hist: &hem_core::timeline::Histogram, n: usize) -> Vec<(u8, Option<Level>, u32)> {
    let sampled = hist.resample(n);
    if sampled.is_empty() {
        return vec![(0, None, 0); n];
    }
    let max = sampled.iter().map(total).max().unwrap_or(0).max(1) as f64;
    let lmax = (max + 1.0).ln();
    sampled
        .iter()
        .map(|c| {
            let t = total(c);
            let h = if t == 0 {
                0
            } else {
                let v = ((t as f64 + 1.0).ln() / lmax * 8.0).round() as u8;
                v.clamp(1, 8)
            };
            // Errors always show at least a visible bar so spikes are obvious.
            let lvl = top_level(c);
            let h = match lvl {
                Some(Level::Error | Level::Fatal) => h.max(3),
                Some(Level::Warn) => h.max(2),
                _ => h,
            };
            (h, lvl, t)
        })
        .collect()
}

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width < 2 {
        return;
    }
    let theme = app.theme.clone();
    let ascii = app.ascii;
    let n = area.height as usize;
    let t = app.tab();
    let hist = t.view.histogram();
    let cells = cells(&hist, n);
    let len = t.len();
    // Viewport indicator: which cells does the visible range cover?
    let (v0, v1) = if len == 0 {
        (0usize, 0usize)
    } else {
        let top = t.top.min(len - 1);
        let bottom = (t.top + t.last_height.max(1)).min(len).saturating_sub(1);
        if hist.is_time() && hist.total > 0 {
            let ts_of = |row: usize| -> Option<i64> {
                let start = row.saturating_sub(4);
                let chunk = t.view.chunk(start, row + 1);
                for k in (0..chunk.len()).rev() {
                    let m = hem_core::parse::parse_line(
                        chunk.line(k),
                        t.format_for(chunk.row(k).source()),
                        t.view.default_year(),
                    );
                    if let Some(ts) = m.ts {
                        return Some(ts);
                    }
                }
                None
            };
            let a = ts_of(top).map(|ts| hist.cell_of(ts, n)).unwrap_or(0);
            let b = ts_of(bottom).map(|ts| hist.cell_of(ts, n)).unwrap_or(a);
            (a.min(b), a.max(b))
        } else {
            let a = top * n / len.max(1);
            let b = bottom * n / len.max(1);
            (a.min(n - 1), b.min(n - 1))
        }
    };
    let glyphs = if ascii { &BLOCKS_ASCII } else { &BLOCKS };
    let bar_w = (area.width - 1) as usize;
    let mut lines = Vec::with_capacity(n);
    for (i, (h, lvl, _)) in cells.iter().enumerate() {
        let in_view = i >= v0 && i <= v1 && len > 0;
        let marker = if in_view {
            Span::styled(
                if ascii { ">" } else { "▐" },
                theme
                    .fg_style(theme.minimap_view)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(" ", Style::default())
        };
        let color = match lvl {
            Some(l) => theme.level_style(*l),
            None => theme.dim_style(),
        };
        let bar: String = std::iter::repeat_n(glyphs[(*h as usize).min(8)], bar_w).collect();
        let mut style = color;
        if !theme.is_mono() {
            style = style.bg(theme.minimap_bg);
        }
        if in_view && !theme.is_mono() {
            style = style.bg(theme.selection_bg);
        }
        lines.push(Line::from(vec![marker, Span::styled(bar, style)]));
        app.hits.push((
            Rect {
                x: area.x,
                y: area.y + i as u16,
                width: area.width,
                height: 1,
            },
            Hit::Minimap { cell: i, cells: n },
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use hem_core::timeline::Histogram;

    #[test]
    fn error_spikes_are_visible() {
        let mut h = Histogram::new_position(1000);
        for i in 0..1000 {
            h.add(i, Some(Level::Info));
        }
        h.add(500, Some(Level::Error));
        let c = cells(&h, 10);
        assert_eq!(c.len(), 10);
        assert_eq!(c[5].1, Some(Level::Error));
        assert!(c[5].0 >= 3);
        assert_eq!(hem_core::timeline::NO_LEVEL, 6);
    }
}
