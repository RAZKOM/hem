//! Small drawing helpers shared by the views.

use unicode_width::UnicodeWidthStr;

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
