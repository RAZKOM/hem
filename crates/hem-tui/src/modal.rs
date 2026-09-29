//! Modal state: text inputs, prompts, the filter editor and friends.

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

