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

    pub fn clear(&mut self) {
        self.text.clear();
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
        selected: usize,
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
            Modal::Prompt { .. } | Modal::Palette { .. } => true,
            Modal::FilterEditor(s) => matches!(s.focus, 0 | 1 | 2 | 4 | 5),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_cycle_wraps() {
        let mut s = FilterEditorState::default();
        s.cycle_level(false);
        assert_eq!(s.level, Some(Level::Fatal));
        s.cycle_level(true);
        assert_eq!(s.level, None);
    }
}
