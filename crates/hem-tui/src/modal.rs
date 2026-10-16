//! Modal state: text inputs, prompts, the filter editor and friends.

use hem_core::view::Row;
use hem_core::Level;

/// A single-line text input with a cursor (in chars).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

impl Input {
    pub fn with(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    fn byte_at(&self, idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_at(self.cursor);
        self.text.insert(b, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let b = self.byte_at(self.cursor - 1);
        self.text.remove(b);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let b = self.byte_at(self.cursor);
        self.text.remove(b);
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        while self.cursor > 0 && self.text.chars().nth(self.cursor - 1) == Some(' ') {
            self.backspace();
        }
        while self.cursor > 0 && self.text.chars().nth(self.cursor - 1) != Some(' ') {
            self.backspace();
        }
    }

    /// Delete everything before the cursor (Ctrl+U).
    pub fn kill_to_start(&mut self) {
        let b = self.byte_at(self.cursor);
        self.text.drain(..b);
        self.cursor = 0;
    }
}

/// What an inline prompt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Search,
    Include,
    Exclude,
    Field,
    OpenFile,
    Highlight,
    Goto,
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::Search => "/",
            PromptKind::Include => "& filter",
            PromptKind::Exclude => "! exclude",
            PromptKind::Field => "= field",
            PromptKind::OpenFile => "open",
            PromptKind::Highlight => "* highlight",
            PromptKind::Goto => "go to",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PromptKind::Search => "regex (smart case) · Enter: search · Esc: cancel",
            PromptKind::Include => "show only lines matching this regex",
            PromptKind::Exclude => "hide lines matching this regex",
            PromptKind::Field => "key=value · key!=value · key~regex · status>=500 · key?",
            PromptKind::OpenFile => "path to a log file (.gz ok)",
            PromptKind::Highlight => "value to highlight (request id, thread, trace id…)",
            PromptKind::Goto => "line number · 50% · timestamp (10:30, 2026-10-13T10:30:00Z, 5m)",
        }
    }
}

/// Filter editor fields.
#[derive(Debug, Clone, Default)]
pub struct FilterEditorState {
    pub include: Input,
    pub exclude: Input,
    pub fields: Input,
    pub since: Input,
    pub until: Input,
    pub level: Option<Level>,
    pub literal: bool,
    /// 0 include, 1 exclude, 2 fields, 3 level, 4 since, 5 until, 6 literal, 7 apply, 8 clear
    pub focus: usize,
    pub error: Option<String>,
}

impl FilterEditorState {
    pub const FIELDS: usize = 9;

    pub fn input_mut(&mut self) -> Option<&mut Input> {
        match self.focus {
            0 => Some(&mut self.include),
            1 => Some(&mut self.exclude),
            2 => Some(&mut self.fields),
            4 => Some(&mut self.since),
            5 => Some(&mut self.until),
            _ => None,
        }
    }

    pub fn cycle_level(&mut self, forward: bool) {
        let order: [Option<Level>; 7] = [
            None,
            Some(Level::Trace),
            Some(Level::Debug),
            Some(Level::Info),
            Some(Level::Warn),
            Some(Level::Error),
            Some(Level::Fatal),
        ];
        let i = order.iter().position(|l| *l == self.level).unwrap_or(0);
        let n = order.len();
        let j = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.level = order[j];
    }
}

/// Settings rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingItem {
    Theme,
    Collapse,
    Timestamp,
    Wrap,
    Follow,
    Columns,
    Minimap,
    Mouse,
    Ascii,
}

impl SettingItem {
    pub const ALL: [SettingItem; 9] = [
        SettingItem::Theme,
        SettingItem::Collapse,
        SettingItem::Timestamp,
        SettingItem::Wrap,
        SettingItem::Follow,
        SettingItem::Columns,
        SettingItem::Minimap,
        SettingItem::Mouse,
        SettingItem::Ascii,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingItem::Theme => "Theme",
            SettingItem::Collapse => "Collapse repeated lines",
            SettingItem::Timestamp => "Timestamp display",
            SettingItem::Wrap => "Wrap long lines",
            SettingItem::Follow => "Follow by default",
            SettingItem::Columns => "Field columns (JSON/logfmt)",
            SettingItem::Minimap => "Timeline minimap",
            SettingItem::Mouse => "Mouse support",
            SettingItem::Ascii => "ASCII box drawing",
        }
    }
}

/// Column to sort the templates table by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateSort {
    Count,
    Level,
    First,
    Last,
    Template,
}

impl TemplateSort {
    pub const ALL: [TemplateSort; 5] = [
        TemplateSort::Count,
        TemplateSort::Level,
        TemplateSort::First,
        TemplateSort::Last,
        TemplateSort::Template,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TemplateSort::Count => "count",
            TemplateSort::Level => "level",
            TemplateSort::First => "first seen",
            TemplateSort::Last => "last seen",
            TemplateSort::Template => "template",
        }
    }

    pub fn next(self) -> TemplateSort {
        let i = TemplateSort::ALL
            .iter()
            .position(|s| *s == self)
            .unwrap_or(0);
        TemplateSort::ALL[(i + 1) % TemplateSort::ALL.len()]
    }
}

#[derive(Debug, Clone)]
pub struct TemplatesState {
    pub sort: TemplateSort,
    pub ascending: bool,
    pub selected: usize,
    pub scroll: usize,
    pub filter: Input,
    pub filtering: bool,
}

impl Default for TemplatesState {
    fn default() -> Self {
        Self {
            sort: TemplateSort::Count,
            ascending: false,
            selected: 0,
            scroll: 0,
            filter: Input::default(),
            filtering: false,
        }
    }
}

/// Which overlay is open.
#[derive(Debug, Clone)]
pub enum Modal {
    Prompt {
        kind: PromptKind,
        input: Input,
    },
    Palette {
        input: Input,
        selected: usize,
    },
    FilterEditor(Box<FilterEditorState>),
    Settings {
        selected: usize,
    },
    Help {
        scroll: usize,
    },
    Sources {
        /// Index into the matching items (see `App::source_matches`).
        selected: usize,
        filter: Input,
    },
    Detail {
        row_idx: usize,
        row: Row,
        scroll: usize,
        selected_field: usize,
    },
    Message {
        title: String,
        lines: Vec<String>,
    },
}

impl Modal {
    /// Does this modal currently own a text input?
    pub fn has_text_input(&self) -> bool {
        match self {
            Modal::Prompt { .. } | Modal::Palette { .. } | Modal::Sources { .. } => true,
            Modal::FilterEditor(s) => matches!(s.focus, 0 | 1 | 2 | 4 | 5),
            _ => false,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Modal::Prompt { kind, .. } => kind.label().to_string(),
            Modal::Palette { .. } => "Command palette".into(),
            Modal::FilterEditor(_) => "Filter editor".into(),
            Modal::Settings { .. } => "Settings".into(),
            Modal::Help { .. } => "Help".into(),
            Modal::Sources { .. } => "Sources".into(),
            Modal::Detail { .. } => "Line detail".into(),
            Modal::Message { title, .. } => title.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_editing() {
        let mut i = Input::with("héllo");
        assert_eq!(i.cursor, 5);
        i.left();
        i.left();
        i.insert('X');
        assert_eq!(i.text, "hélXlo");
        i.backspace();
        assert_eq!(i.text, "héllo");
        i.home();
        i.delete();
        assert_eq!(i.text, "éllo");
        i.end();
        i.delete_word();
        assert_eq!(i.text, "");
        let mut i = Input::with("a b c");
        i.kill_to_start();
        assert_eq!(i.text, "");
    }

    #[test]
    fn level_cycle_wraps() {
        let mut s = FilterEditorState::default();
        s.cycle_level(false);
        assert_eq!(s.level, Some(Level::Fatal));
        s.cycle_level(true);
        assert_eq!(s.level, None);
    }
}
