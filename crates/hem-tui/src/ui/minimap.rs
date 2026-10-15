//! Timeline minimap: a vertical histogram of the whole view, coloured by
//! the highest severity in each bucket, with a viewport indicator.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use hem_core::timeline::Counts;
use hem_core::Level;

use crate::app::{App, Hit, PinPos, Tab};

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

/// Cells (one per terminal row) for a histogram of `n` rows:
/// `(length 0..=8, level, lines)`.
///
/// Length is the bar length in eighths of the bar width, on a log scale so
/// a handful of lines next to a burst still shows up.
///
/// A row takes the colour of the most severe level that is *unusually
/// dense* there: well above that level's average per row (three standard
/// deviations, treating counts as Poisson). With errors sprinkled evenly
/// through a log, plain "highest severity present" would paint every row
/// red; this way only bursts light up, while a single error in an otherwise
/// clean log still does. Rows with nothing unusual take the colour of
/// their most severe routine level (info, debug or trace).
pub fn cells(hist: &hem_core::timeline::Histogram, n: usize) -> Vec<(u8, Option<Level>, u32)> {
    let sampled = hist.resample(n);
    if sampled.is_empty() {
        return vec![(0, None, 0); n];
    }
    let max = sampled.iter().map(total).max().unwrap_or(0).max(1) as f64;
    let lmax = (max + 1.0).ln();
    let busy_rows = sampled.iter().filter(|c| total(c) > 0).count().max(1) as f64;
    let mut threshold = [0f64; Level::COUNT];
    for (l, thr) in threshold.iter_mut().enumerate() {
        let mean = sampled.iter().map(|c| c[l] as f64).sum::<f64>() / busy_rows;
        // Rarer than once per row on average: every occurrence is news.
        *thr = if mean < 1.0 {
            1.0
        } else {
            mean + 3.0 * mean.sqrt()
        };
    }
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
            let notable = [Level::Fatal, Level::Error, Level::Warn]
                .into_iter()
                .find(|l| c[l.index()] > 0 && c[l.index()] as f64 >= threshold[l.index()]);
            let lvl = notable.or_else(|| {
                (0..=Level::Info.index())
                    .rev()
                    .find(|&i| c[i] > 0)
                    .and_then(Level::from_index)
            });
            // Bursts always get a visible bar, even in a quiet stretch.
            let h = match notable {
                Some(Level::Error | Level::Fatal) => h.max(3),
                Some(Level::Warn) => h.max(2),
                _ => h,
            };
            (h, lvl, t)
        })
        .collect()
}

/// A horizontal bar `width` cells wide, filled to `len / 8` of its width.
fn bar(len: u8, width: usize, ascii: bool) -> String {
    const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = len as usize * width; // len/8 of width, in eighths of a cell
    let full = eighths / 8;
    let rem = eighths % 8;
    let mut s = String::with_capacity(width * 3);
    for _ in 0..full.min(width) {
        s.push(if ascii { '#' } else { '█' });
    }
    if full < width {
        s.push(if ascii {
            if rem >= 4 {
                '+'
            } else {
                ' '
            }
        } else {
            PARTIAL[rem]
        });
        for _ in full + 1..width {
            s.push(' ');
        }
    }
    s
}

/// Timestamp of a row, inherited from up to four rows above it.
fn row_ts(t: &Tab, row: usize) -> Option<i64> {
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
}

/// Most pins marked on the minimap (each costs a row lookup per frame).
const MAX_PIN_MARKS: usize = 64;

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
            let a = row_ts(t, top).map(|ts| hist.cell_of(ts, n)).unwrap_or(0);
            let b = row_ts(t, bottom).map(|ts| hist.cell_of(ts, n)).unwrap_or(a);
            (a.min(b), a.max(b))
        } else {
            let a = top * n / len.max(1);
            let b = bottom * n / len.max(1);
            (a.min(n - 1), b.min(n - 1))
        }
    };
    // Pinned lines, as cells.
    let mut pinned = vec![false; n];
    if len > 0 {
        let rows = t.pin_pos.iter().filter_map(|p| match p {
            PinPos::Row(r) => Some(*r),
            _ => None,
        });
        for r in rows.take(MAX_PIN_MARKS) {
            let cell = if hist.is_time() && hist.total > 0 {
                row_ts(t, r).map(|ts| hist.cell_of(ts, n))
            } else {
                (r * n).checked_div(len)
            };
            if let Some(c) = cell {
                pinned[c.min(n - 1)] = true;
            }
        }
    }
    let bar_w = (area.width - 1) as usize;
    let mut lines = Vec::with_capacity(n);
    for (i, (h, lvl, _)) in cells.iter().enumerate() {
        let in_view = i >= v0 && i <= v1 && len > 0;
        let marker = if pinned[i] {
            let mut s = theme.fg_style(theme.pin).add_modifier(Modifier::BOLD);
            if in_view && !theme.is_mono() {
                s = s.bg(theme.selection_bg);
            }
            Span::styled(if ascii { "*" } else { "◆" }, s)
        } else if in_view {
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
        let bar: String = bar(*h, bar_w, ascii);
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
    fn only_bursts_light_up_when_errors_are_everywhere() {
        let mut h = Histogram::new_position(10_000);
        for i in 0..10_000i64 {
            // A steady 3% error rate everywhere...
            let lvl = if i % 33 == 0 {
                Level::Error
            } else {
                Level::Info
            };
            h.add(i, Some(lvl));
        }
        // ...plus a burst in the middle.
        for _ in 0..300 {
            h.add(5_050, Some(Level::Error));
        }
        let c = cells(&h, 20);
        let red: Vec<usize> = (0..20).filter(|&i| c[i].1 == Some(Level::Error)).collect();
        assert_eq!(red, vec![10], "{c:?}");
        assert!(c.iter().all(|x| x.1.is_some()));
    }

    #[test]
    fn bars_have_the_right_length() {
        assert_eq!(bar(8, 3, false), "███");
        assert_eq!(bar(4, 3, false), "█▌ ");
        assert_eq!(bar(0, 3, false), "   ");
        assert_eq!(bar(8, 3, true), "###");
        assert_eq!(bar(4, 2, true), "# ");
    }

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
