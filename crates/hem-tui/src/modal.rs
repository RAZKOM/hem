//! Modal state: text inputs, prompts, the filter editor and friends.

use hem_core::Level;

/// A single-line text input with a cursor (in chars).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

impl Input {
    pub fn len(&self) -> usize {
        self.text.chars().count()
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

