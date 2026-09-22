//! Template clustering with a Drain-style streaming miner.
//!
//! Lines are normalised (numbers / ids masked), tokenised on whitespace and
//! routed through a fixed-depth prefix tree keyed by token count and the
//! first few tokens.  At each leaf the most similar existing template is
//! updated (differing positions become `<*>`) or a new template is created.

use std::collections::HashMap;

use crate::filter::Token;
use crate::format::Format;
use crate::level::Level;
use crate::view::Row;

/// A mined template.
#[derive(Debug, Clone)]
pub struct Template {
    pub id: usize,
    pub format: Format,
    pub tokens: Vec<Token>,
    pub count: u64,
    pub levels: [u64; Level::COUNT + 1],
    pub first: Row,
    pub last: Row,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub examples: Vec<Row>,
}

#[derive(Default)]
struct Node {
    children: HashMap<String, Node>,
    clusters: Vec<usize>,
}

/// The Drain miner.
pub struct Miner {
    pub sim_threshold: f64,
    pub depth: usize,
    pub max_children: usize,
    pub max_templates: usize,
    roots: HashMap<usize, Node>,
    templates: Vec<Template>,
}

impl Default for Miner {
    fn default() -> Self {
        Self::new()
    }
}

impl Miner {
    pub fn new() -> Self {
        Self {
            sim_threshold: 0.5,
            depth: 2,
            max_children: 100,
            max_templates: 20_000,
            roots: HashMap::new(),
            templates: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

