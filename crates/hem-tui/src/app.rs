//! Application state and input handling.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use anyhow::Result;

use hem_core::cluster::Clusters;
use hem_core::filter::{Filter, Matcher};
use hem_core::parse::MetaCache;
use hem_core::search::Search;
use hem_core::view::{Base, Row, View};

use crate::modal::TemplatesState;

/// One open source (or the merged stream).
pub struct Tab {
    pub name: String,
    pub base: Base,
    pub view: View,
    pub filter: Filter,
    pub collapse: bool,
    pub top: usize,
    pub cursor: usize,
    pub hscroll: usize,
    pub follow: bool,
    pub wrap: bool,
    pub columns: bool,
    pub search: Option<Search>,
    pub search_text: String,
    pub current_match: Option<usize>,
    pub pins: Vec<(u8, u64)>,
    pub highlight: Option<Matcher>,
    pub clusters: Option<Clusters>,
    pub templates: TemplatesState,
    pub expanded: HashSet<usize>,
    pub meta: MetaCache,
    /// Rows drawn in the last frame: (row index, y, height).
    pub last_visible: Vec<(usize, u16, u16)>,
    pub last_height: usize,
    pub last_len: usize,
    pub member_cache: HashMap<usize, Vec<Row>>,
    pub pending_anchor: Option<(u8, u64)>,
    pub view_gen: u32,
}

impl std::fmt::Debug for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab").field("name", &self.name).finish()
    }
}

impl Tab {
    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

}

