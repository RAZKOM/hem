//! Themes, colour-depth degradation and `NO_COLOR` support.

use std::collections::BTreeMap;

use hem_core::Level;
use ratatui::style::Color;

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Mono,
    Ansi16,
    Ansi256,
    TrueColor,
}

/// Named theme styles.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub depth: Depth,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    pub border_focus: Color,
    pub selection_bg: Color,
    pub cursor_bg: Color,
    pub timestamp: Color,
    pub levels: [Color; Level::COUNT],
    pub sources: [Color; 6],
    pub json_key: Color,
    pub string: Color,
    pub number: Color,
    pub url: Color,
    pub match_bg: Color,
    pub match_fg: Color,
    pub current_match_bg: Color,
    pub highlight_bg: Color,
    pub pin: Color,
    pub chip_bg: Color,
    pub chip_fg: Color,
    pub chip_close: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub tab_active_bg: Color,
    pub tab_active_fg: Color,
    pub tab_inactive_fg: Color,
    pub modal_bg: Color,
    pub modal_border: Color,
    pub title: Color,
    pub badge: Color,
    pub sparkline: Color,
    pub minimap_bg: Color,
    pub minimap_view: Color,
    pub scrollbar: Color,
    pub input_bg: Color,
    pub muted_bg: Color,
    pub warning: Color,
    pub success: Color,
}

fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            name: "dark".into(),
            depth: Depth::TrueColor,
            bg: rgb(0x14161c),
            fg: rgb(0xd8dbe2),
            dim: rgb(0x6b7280),
            accent: rgb(0x7aa2f7),
            border: rgb(0x2b2f3a),
            border_focus: rgb(0x7aa2f7),
            selection_bg: rgb(0x2a2f3d),
            cursor_bg: rgb(0x343a4d),
            timestamp: rgb(0x7f8597),
            levels: [
                rgb(0x6b7280), // trace
                rgb(0x8b93a7), // debug
                rgb(0x7dcfff), // info
                rgb(0xe0af68), // warn
                rgb(0xf7768e), // error
                rgb(0xff5d8f), // fatal
            ],
            sources: [
                rgb(0x9ece6a),
                rgb(0xbb9af7),
                rgb(0x7dcfff),
                rgb(0xe0af68),
                rgb(0xf7768e),
                rgb(0x73daca),
            ],
            json_key: rgb(0x7aa2f7),
            string: rgb(0x9ece6a),
            number: rgb(0xff9e64),
            url: rgb(0x73daca),
            match_bg: rgb(0x3b4261),
            match_fg: rgb(0xffffff),
            current_match_bg: rgb(0xe0af68),
            highlight_bg: rgb(0x2f3f2a),
            pin: rgb(0xe0af68),
            chip_bg: rgb(0x2b2f3a),
            chip_fg: rgb(0xc0caf5),
            chip_close: rgb(0xf7768e),
            status_bg: rgb(0x1b1e26),
            status_fg: rgb(0xa9b1d6),
            tab_active_bg: rgb(0x2b2f3a),
            tab_active_fg: rgb(0xffffff),
            tab_inactive_fg: rgb(0x7f8597),
            modal_bg: rgb(0x1b1e26),
            modal_border: rgb(0x7aa2f7),
            title: rgb(0xc0caf5),
            badge: rgb(0xbb9af7),
            sparkline: rgb(0x7aa2f7),
            minimap_bg: rgb(0x1b1e26),
            minimap_view: rgb(0xc0caf5),
            scrollbar: rgb(0x3b4261),
            input_bg: rgb(0x1f232d),
            muted_bg: rgb(0x1b1e26),
            warning: rgb(0xe0af68),
            success: rgb(0x9ece6a),
        }
    }

    pub fn light() -> Theme {
        Theme {
            name: "light".into(),
            depth: Depth::TrueColor,
            bg: rgb(0xfafafa),
            fg: rgb(0x1f2328),
            dim: rgb(0x8c959f),
            accent: rgb(0x0969da),
            border: rgb(0xd0d7de),
            border_focus: rgb(0x0969da),
            selection_bg: rgb(0xe6edf5),
            cursor_bg: rgb(0xd7e3f2),
            timestamp: rgb(0x6e7781),
            levels: [
                rgb(0x8c959f),
                rgb(0x6e7781),
                rgb(0x0969da),
                rgb(0x9a6700),
                rgb(0xcf222e),
                rgb(0xa40e26),
            ],
            sources: [
                rgb(0x1a7f37),
                rgb(0x8250df),
                rgb(0x0969da),
                rgb(0x9a6700),
                rgb(0xcf222e),
                rgb(0x0f766e),
            ],
            json_key: rgb(0x0550ae),
            string: rgb(0x116329),
            number: rgb(0x953800),
            url: rgb(0x0f766e),
            match_bg: rgb(0xfff8c5),
            match_fg: rgb(0x1f2328),
            current_match_bg: rgb(0xffd33d),
            highlight_bg: rgb(0xdafbe1),
            pin: rgb(0x9a6700),
            chip_bg: rgb(0xe6edf5),
            chip_fg: rgb(0x1f2328),
            chip_close: rgb(0xcf222e),
            status_bg: rgb(0xeef1f4),
            status_fg: rgb(0x424a53),
            tab_active_bg: rgb(0xe6edf5),
            tab_active_fg: rgb(0x1f2328),
            tab_inactive_fg: rgb(0x8c959f),
            modal_bg: rgb(0xffffff),
            modal_border: rgb(0x0969da),
            title: rgb(0x1f2328),
            badge: rgb(0x8250df),
            sparkline: rgb(0x0969da),
            minimap_bg: rgb(0xeef1f4),
            minimap_view: rgb(0x1f2328),
            scrollbar: rgb(0xd0d7de),
            input_bg: rgb(0xffffff),
            muted_bg: rgb(0xeef1f4),
            warning: rgb(0x9a6700),
            success: rgb(0x1a7f37),
        }
    }

    /// Pick a theme by name.
    pub fn by_name(name: &str) -> Option<Theme> {
        match name.to_ascii_lowercase().as_str() {
            "dark" | "default" => Some(Theme::dark()),
            "light" => Some(Theme::light()),
            _ => None,
        }
    }

    pub fn names() -> &'static [&'static str] {
        &["dark", "light"]
    }

    /// Apply `name = colour` overrides from the config.  Unknown names are
    /// returned so the caller can warn.
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<String, String>) -> Vec<String> {
        let mut unknown = Vec::new();
        for (k, v) in overrides {
            let Some(color) = parse_color(v) else {
                unknown.push(format!("{k} = {v:?} (not a colour)"));
                continue;
            };
            let slot: Option<&mut Color> = match k.as_str() {
                "bg" | "background" => Some(&mut self.bg),
                "fg" | "foreground" => Some(&mut self.fg),
                "dim" => Some(&mut self.dim),
                "accent" => Some(&mut self.accent),
                "border" => Some(&mut self.border),
                "border_focus" => Some(&mut self.border_focus),
                "selection" => Some(&mut self.selection_bg),
                "cursor" => Some(&mut self.cursor_bg),
                "timestamp" => Some(&mut self.timestamp),
                "trace" => Some(&mut self.levels[0]),
                "debug" => Some(&mut self.levels[1]),
                "info" => Some(&mut self.levels[2]),
                "warn" => Some(&mut self.levels[3]),
                "error" => Some(&mut self.levels[4]),
                "fatal" => Some(&mut self.levels[5]),
                "json_key" => Some(&mut self.json_key),
                "string" => Some(&mut self.string),
                "number" => Some(&mut self.number),
                "url" => Some(&mut self.url),
                "match" => Some(&mut self.match_bg),
                "current_match" => Some(&mut self.current_match_bg),
                "highlight" => Some(&mut self.highlight_bg),
                "pin" => Some(&mut self.pin),
                "status_bg" => Some(&mut self.status_bg),
                "status_fg" => Some(&mut self.status_fg),
                "modal_bg" => Some(&mut self.modal_bg),
                "modal_border" => Some(&mut self.modal_border),
                "sparkline" => Some(&mut self.sparkline),
                "minimap_bg" => Some(&mut self.minimap_bg),
                _ => None,
            };
            match slot {
                Some(s) => *s = color,
                None => unknown.push(k.clone()),
            }
        }
        unknown
    }

    // --- Convenience styles -------------------------------------------

    pub fn level_color(&self, level: Level) -> Color {
        self.levels[level.index()]
    }

}

/// Parse `#rrggbb`, `rgb(r,g,b)`, an ANSI name, or a 0-255 index.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(rgb(v));
        }
        if hex.len() == 3 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let r = ((v >> 8) & 0xF) * 17;
            let g = ((v >> 4) & 0xF) * 17;
            let b = (v & 0xF) * 17;
            return Some(Color::Rgb(r as u8, g as u8, b as u8));
        }
        return None;
    }
    if let Ok(n) = s.parse::<u8>() {
        return Some(Color::Indexed(n));
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" | "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "reset" | "default" | "none" => Color::Reset,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("#f00"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("red"), Some(Color::Red));
        assert_eq!(parse_color("42"), Some(Color::Indexed(42)));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn overrides() {
        let mut t = Theme::dark();
        let mut o = BTreeMap::new();
        o.insert("error".to_string(), "#00ff00".to_string());
        o.insert("bogus".to_string(), "#00ff00".to_string());
        let unknown = t.apply_overrides(&o);
        assert_eq!(t.levels[Level::Error.index()], Color::Rgb(0, 255, 0));
        assert_eq!(unknown, vec!["bogus".to_string()]);
    }
}
