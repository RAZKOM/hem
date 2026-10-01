//! Actions, key bindings and the single source of truth for the help modal
//! and the command palette.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};

/// Everything the user can do.  Each action has a stable config name
/// (`snake_case` of the variant), a label and a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    Top,
    Bottom,
    ToggleFollow,
    Search,
    NextMatch,
    PrevMatch,
    ClearSearch,
    Filter,
    Exclude,
    FieldFilter,
    FilterEditor,
    ClearFilters,
    CycleLevel,
    NextError,
    PrevError,
    ToggleCollapse,
    Expand,
    Templates,
    Detail,
    Pin,
    HighlightValue,
    ClearHighlight,
    Copy,
    CopyView,
    ToggleWrap,
    ToggleColumns,
    CycleTimestamp,
    NextTab,
    PrevTab,
    Sources,
    OpenFile,
    Goto,
    Palette,
    Settings,
    Help,
    Close,
    Quit,
    SortTemplates,
    SortTemplatesReverse,
    Refresh,
}

/// Grouping used by the help modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Navigation,
    Search,
    Filters,
    Insight,
    View,
    App,
}

impl Action {
    pub const ALL: [Action; 48] = [
        Action::ScrollUp,
        Action::ScrollDown,
        Action::ScrollLeft,
        Action::ScrollRight,
        Action::PageUp,
        Action::PageDown,
        Action::HalfPageUp,
        Action::HalfPageDown,
        Action::Top,
        Action::Bottom,
        Action::ToggleFollow,
        Action::Search,
        Action::NextMatch,
        Action::PrevMatch,
        Action::ClearSearch,
        Action::Filter,
        Action::Exclude,
        Action::FieldFilter,
        Action::FilterEditor,
        Action::ClearFilters,
        Action::CycleLevel,
        Action::NextError,
        Action::PrevError,
        Action::ToggleCollapse,
        Action::Expand,
        Action::Templates,
        Action::Detail,
        Action::Pin,
        Action::HighlightValue,
        Action::ClearHighlight,
        Action::Copy,
        Action::CopyView,
        Action::ToggleWrap,
        Action::ToggleColumns,
        Action::CycleTimestamp,
        Action::NextTab,
        Action::PrevTab,
        Action::Sources,
        Action::OpenFile,
        Action::Goto,
        Action::Palette,
        Action::Settings,
        Action::Help,
        Action::Close,
        Action::Quit,
        Action::SortTemplates,
        Action::SortTemplatesReverse,
        Action::Refresh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::ScrollUp => "Scroll up",
            Action::ScrollDown => "Scroll down",
            Action::ScrollLeft => "Scroll left",
            Action::ScrollRight => "Scroll right",
            Action::PageUp => "Page up",
            Action::PageDown => "Page down",
            Action::HalfPageUp => "Half page up",
            Action::HalfPageDown => "Half page down",
            Action::Top => "Go to top",
            Action::Bottom => "Go to bottom",
            Action::ToggleFollow => "Toggle follow (tail)",
            Action::Search => "Search",
            Action::NextMatch => "Next match",
            Action::PrevMatch => "Previous match",
            Action::ClearSearch => "Clear search",
            Action::Filter => "Filter: show only matching lines",
            Action::Exclude => "Exclude: hide matching lines",
            Action::FieldFilter => "Field filter (key=value, status>=500)",
            Action::FilterEditor => "Filter editor",
            Action::ClearFilters => "Clear all filters",
            Action::CycleLevel => "Cycle minimum level",
            Action::NextError => "Next error",
            Action::PrevError => "Previous error",
            Action::ToggleCollapse => "Toggle repeat collapsing",
            Action::Expand => "Expand / collapse repeated run",
            Action::Templates => "Templates view",
            Action::Detail => "Line detail",
            Action::Pin => "Pin / unpin line",
            Action::HighlightValue => "Highlight lines sharing a value",
            Action::ClearHighlight => "Clear highlight",
            Action::Copy => "Copy line",
            Action::CopyView => "Copy visible lines",
            Action::ToggleWrap => "Toggle line wrap",
            Action::ToggleColumns => "Toggle field columns",
            Action::CycleTimestamp => "Cycle timestamp display",
            Action::NextTab => "Next source tab",
            Action::PrevTab => "Previous source tab",
            Action::Sources => "Source picker",
            Action::OpenFile => "Open file",
            Action::Goto => "Go to line / percent / time",
            Action::Palette => "Command palette",
            Action::Settings => "Settings",
            Action::Help => "Help",
            Action::Close => "Close / back",
            Action::Quit => "Quit",
            Action::SortTemplates => "Templates: cycle sort column",
            Action::SortTemplatesReverse => "Templates: reverse sort",
            Action::Refresh => "Re-check file now",
        }
    }

    pub fn category(self) -> Category {
        use Action::*;
        match self {
            ScrollUp | ScrollDown | ScrollLeft | ScrollRight | PageUp | PageDown | HalfPageUp
            | HalfPageDown | Top | Bottom | ToggleFollow | Goto | NextTab | PrevTab => {
                Category::Navigation
            }
            Search | NextMatch | PrevMatch | ClearSearch => Category::Search,
            Filter | Exclude | FieldFilter | FilterEditor | ClearFilters | CycleLevel => {
                Category::Filters
            }
            NextError | PrevError | ToggleCollapse | Expand | Templates | Detail | Pin
            | HighlightValue | ClearHighlight | SortTemplates | SortTemplatesReverse => {
                Category::Insight
            }
            ToggleWrap | ToggleColumns | CycleTimestamp => Category::View,
            Copy | CopyView | Sources | OpenFile | Palette | Settings | Help | Close | Quit
            | Refresh => Category::App,
        }
    }

    /// Config name (`snake_case`).
    pub fn name(self) -> String {
        serde_json_name(self)
    }

    pub fn from_name(s: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == s)
    }
}

fn serde_json_name(a: Action) -> String {
    // Use toml's serializer to get the `snake_case` rename without a JSON dep.
    let v = toml::Value::try_from(a).unwrap_or(toml::Value::String(String::new()));
    match v {
        toml::Value::String(s) => s,
        _ => String::new(),
    }
}

/// A key chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Key {
        Key { code, mods }
    }

    pub fn ch(c: char) -> Key {
        Key::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub fn ctrl(c: char) -> Key {
        Key::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Parse `ctrl+p`, `shift+tab`, `G`, `pgdn`, `enter`.
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let parts: Vec<&str> = s.split('+').collect();
        let (mods_part, key_part) = parts.split_at(parts.len() - 1);
        let mut mods = KeyModifiers::NONE;
        for m in mods_part {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" | "m" | "a" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                _ => return None,
            }
        }
        let k = key_part[0].trim();
        let code = match k.to_ascii_lowercase().as_str() {
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pgup" | "pageup" => KeyCode::PageUp,
            "pgdn" | "pagedown" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => {
                if mods.contains(KeyModifiers::SHIFT) {
                    mods.remove(KeyModifiers::SHIFT);
                    KeyCode::BackTab
                } else {
                    KeyCode::Tab
                }
            }
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "plus" => KeyCode::Char('+'),
            _ => {
                if let Some(n) = k
                    .strip_prefix(['f', 'F'])
                    .and_then(|n| n.parse::<u8>().ok())
                {
                    KeyCode::F(n)
                } else {
                    let mut it = k.chars();
                    let c = it.next()?;
                    if it.next().is_some() {
                        return None;
                    }
                    if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_alphabetic() {
                        mods.remove(KeyModifiers::SHIFT);
                        KeyCode::Char(c.to_ascii_uppercase())
                    } else {
                        KeyCode::Char(c)
                    }
                }
            }
        };
        Some(Key { code, mods })
    }

    /// Human-readable form for help text.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        let k = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::BackTab => "Shift+Tab".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Insert => "Ins".into(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        s.push_str(&k);
        s
    }

    /// ASCII-only variant (for `--ascii`).
    pub fn display_ascii(&self) -> String {
        self.display()
            .replace('↑', "Up")
            .replace('↓', "Down")
            .replace('←', "Left")
            .replace('→', "Right")
    }
}

/// Ordered list of bindings.  Earlier entries win on conflicts.
#[derive(Debug, Clone)]
pub struct KeyMap {
    pub bindings: Vec<(Key, Action)>,
}

impl Default for KeyMap {
    fn default() -> Self {
        use Action::*;
        let b: Vec<(Key, Action)> = vec![
            (Key::ch('j'), ScrollDown),
            (Key::new(KeyCode::Down, KeyModifiers::NONE), ScrollDown),
            (Key::ch('k'), ScrollUp),
            (Key::new(KeyCode::Up, KeyModifiers::NONE), ScrollUp),
            (Key::ch('h'), ScrollLeft),
            (Key::new(KeyCode::Left, KeyModifiers::NONE), ScrollLeft),
            (Key::new(KeyCode::Right, KeyModifiers::NONE), ScrollRight),
            (Key::new(KeyCode::PageUp, KeyModifiers::NONE), PageUp),
            (Key::new(KeyCode::PageDown, KeyModifiers::NONE), PageDown),
            (Key::ctrl('b'), PageUp),
            (Key::ctrl('f'), PageDown),
            (Key::ctrl('u'), HalfPageUp),
            (Key::ctrl('d'), HalfPageDown),
            (Key::ch('g'), Top),
            (Key::new(KeyCode::Home, KeyModifiers::NONE), Top),
            (Key::ch('G'), Bottom),
            (Key::new(KeyCode::End, KeyModifiers::NONE), Bottom),
            (Key::ch('f'), ToggleFollow),
            (Key::ch('/'), Search),
            (Key::ch('n'), NextMatch),
            (Key::ch('N'), PrevMatch),
            (Key::ctrl('l'), ClearSearch),
            (Key::ch('&'), Filter),
            (Key::ch('!'), Exclude),
            (Key::ch('='), FieldFilter),
            (Key::ch('F'), FilterEditor),
            (Key::ch('X'), ClearFilters),
            (Key::ch('l'), CycleLevel),
            (Key::ch('e'), NextError),
            (Key::ch('E'), PrevError),
            (Key::ch('c'), ToggleCollapse),
            (Key::ch('x'), Expand),
            (Key::ch('t'), Templates),
            (Key::new(KeyCode::Enter, KeyModifiers::NONE), Detail),
            (Key::ch('p'), Pin),
            (Key::ch('*'), HighlightValue),
            (Key::ch('y'), Copy),
            (Key::ch('Y'), CopyView),
            (Key::ch('w'), ToggleWrap),
            (Key::ch('C'), ToggleColumns),
            (Key::ch('T'), CycleTimestamp),
            (Key::new(KeyCode::Tab, KeyModifiers::NONE), NextTab),
            (Key::new(KeyCode::BackTab, KeyModifiers::NONE), PrevTab),
            (Key::ch('S'), Sources),
            (Key::ch('o'), OpenFile),
            (Key::ch(':'), Palette),
            (Key::ctrl('p'), Palette),
            (Key::ch('L'), Goto),
            (Key::ch(','), Settings),
            (Key::ch('?'), Help),
            (Key::new(KeyCode::F(1), KeyModifiers::NONE), Help),
            (Key::new(KeyCode::Esc, KeyModifiers::NONE), Close),
            (Key::ch('q'), Quit),
            (Key::ctrl('c'), Quit),
            (Key::ch('s'), SortTemplates),
            (Key::ch('r'), SortTemplatesReverse),
            (Key::ch('R'), Refresh),
        ];
        KeyMap { bindings: b }
    }
}

impl KeyMap {
    /// Apply config overrides: `action = "key"` or `action = ["k1", "k2"]`.
    /// Overridden actions lose their default bindings.  Returns errors as
    /// human-readable strings.
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<String, KeySpec>) -> Vec<String> {
        let mut errors = Vec::new();
        for (name, spec) in overrides {
            let Some(action) = Action::from_name(name) else {
                errors.push(format!("keys.{name}: unknown action"));
                continue;
            };
            let keys: Vec<&str> = match spec {
                KeySpec::One(s) => vec![s.as_str()],
                KeySpec::Many(v) => v.iter().map(String::as_str).collect(),
            };
            let mut parsed = Vec::new();
            for k in keys {
                match Key::parse(k) {
                    Some(key) => parsed.push(key),
                    None => errors.push(format!("keys.{name}: cannot parse key {k:?}")),
                }
            }
            self.bindings.retain(|(_, a)| *a != action);
            for key in parsed {
                self.bindings.retain(|(k, _)| *k != key);
                self.bindings.insert(0, (key, action));
            }
        }
        errors
    }

    pub fn lookup(&self, key: Key) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, a)| *a)
    }

    /// All keys bound to an action.
    pub fn keys_for(&self, action: Action) -> Vec<Key> {
        self.bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Joined key names for display.
    pub fn keys_label(&self, action: Action, ascii: bool) -> String {
        let v: Vec<String> = self
            .keys_for(action)
            .iter()
            .map(|k| {
                if ascii {
                    k.display_ascii()
                } else {
                    k.display()
                }
            })
            .collect();
        v.join(" / ")
    }

    /// Rows for the help modal, grouped by category.
    pub fn help_rows(&self, ascii: bool) -> Vec<(Category, Vec<(String, &'static str)>)> {
        let cats = [
            Category::Navigation,
            Category::Search,
            Category::Filters,
            Category::Insight,
            Category::View,
            Category::App,
        ];
        cats.iter()
            .map(|c| {
                let rows = Action::ALL
                    .iter()
                    .filter(|a| a.category() == *c)
                    .map(|a| (self.keys_label(*a, ascii), a.label()))
                    .collect();
                (*c, rows)
            })
            .collect()
    }
}

/// A key spec in the config: one string or a list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum KeySpec {
    One(String),
    Many(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keys() {
        assert_eq!(Key::parse("ctrl+p"), Some(Key::ctrl('p')));
        assert_eq!(Key::parse("G"), Some(Key::ch('G')));
        assert_eq!(Key::parse("shift+g"), Some(Key::ch('G')));
        assert_eq!(
            Key::parse("shift+tab"),
            Some(Key::new(KeyCode::BackTab, KeyModifiers::NONE))
        );
        assert_eq!(
            Key::parse("pgdn"),
            Some(Key::new(KeyCode::PageDown, KeyModifiers::NONE))
        );
        assert_eq!(
            Key::parse("f5"),
            Some(Key::new(KeyCode::F(5), KeyModifiers::NONE))
        );
        assert_eq!(Key::parse("bogus"), None);
        assert_eq!(Key::parse("hyper+x"), None);
    }

    #[test]
    fn names_roundtrip() {
        for a in Action::ALL {
            assert_eq!(Action::from_name(&a.name()), Some(a), "{a:?}");
        }
        assert_eq!(Action::NextError.name(), "next_error");
    }

    #[test]
    fn defaults_and_overrides() {
        let mut km = KeyMap::default();
        assert_eq!(km.lookup(Key::ch('e')), Some(Action::NextError));
        let mut o = BTreeMap::new();
        o.insert(
            "next_error".to_string(),
            KeySpec::Many(vec!["ctrl+n".into(), "F8".into()]),
        );
        o.insert("nope".to_string(), KeySpec::One("x".into()));
        let errs = km.apply_overrides(&o);
        assert_eq!(errs.len(), 1);
        assert_eq!(km.lookup(Key::ch('e')), None);
        assert_eq!(km.lookup(Key::ctrl('n')), Some(Action::NextError));
        assert_eq!(km.keys_label(Action::NextError, true), "F8 / Ctrl+n");
    }

    #[test]
    fn help_covers_every_action() {
        let km = KeyMap::default();
        let rows = km.help_rows(false);
        let n: usize = rows.iter().map(|(_, r)| r.len()).sum();
        assert_eq!(n, Action::ALL.len());
    }
}
