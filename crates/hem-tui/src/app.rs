//! Application state and input handling.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::layout::Rect;

use hem_core::cluster::{Clusters, Template};
use hem_core::filter::{Filter, Matcher};
use hem_core::merge::Merged;
use hem_core::parse::MetaCache;
use hem_core::search::Search;
use hem_core::timestamp::TsDisplay;
use hem_core::view::{Base, Row, View, ViewSpec};
use hem_core::{Level, Source};

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::keymap::{Action, KeyMap};
use crate::modal::{Modal, TemplatesState};
use crate::theme::Theme;

/// Something clickable, recorded during render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Chip(usize),
    ClearChips,
    Row(usize),
    Minimap { cell: usize, cells: usize },
    Track { cell: usize, cells: usize },
    Scrollbar,
    Action(Action),
    ModalItem(usize),
    ModalClose,
    TemplateRow(usize),
    TemplateHeader(usize),
    Prompt,
}

/// A filter chip in the filter bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chip {
    Include(usize),
    Exclude(usize),
    Level,
    Time,
    Field(usize),
    Template,
    Search,
    Highlight,
}

/// Transient status-bar notice.
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub until: Instant,
    pub warning: bool,
}

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
    fn new(
        name: String,
        base: Base,
        filter: Filter,
        collapse: bool,
        follow: bool,
        wrap: bool,
        columns: bool,
    ) -> Tab {
        let view = View::start(
            base.clone(),
            ViewSpec {
                filter: filter.clone(),
                collapse,
            },
        );
        let view_gen = view.generation();
        Tab {
            name,
            base,
            view,
            filter,
            collapse,
            top: 0,
            cursor: 0,
            hscroll: 0,
            follow,
            wrap,
            columns,
            search: None,
            search_text: String::new(),
            current_match: None,
            pins: Vec::new(),
            highlight: None,
            clusters: None,
            templates: TemplatesState::default(),
            expanded: HashSet::new(),
            meta: MetaCache::default(),
            last_visible: Vec::new(),
            last_height: 20,
            last_len: 0,
            member_cache: HashMap::new(),
            pending_anchor: None,
            view_gen,
        }
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

}

/// Top-level application state.
pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub config: Config,
    pub config_path: Option<PathBuf>,
    pub theme: Theme,
    pub keymap: KeyMap,
    pub modal: Option<Modal>,
    pub templates_mode: bool,
    pub notices: VecDeque<Notice>,
    pub quit: bool,
    pub ascii: bool,
    pub mouse: bool,
    pub minimap: bool,
    pub ts_display: TsDisplay,
    pub hits: Vec<(Rect, Hit)>,
    pub chips: Vec<Chip>,
    pub clipboard: Clipboard,
    pub debug: Option<std::fs::File>,
    pub size: (u16, u16),
    dirty: Arc<AtomicBool>,
    osc52: Option<String>,
    minimap_drag: bool,
    last_progress: String,
    cluster_cap: usize,
    pub palette_items: Vec<PaletteItem>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("tabs", &self.tabs.len())
            .finish()
    }
}

/// An entry in the command palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    Action(Action),
    Preset(String),
    Theme(String),
    MergeAll,
    Tab(usize),
}

impl App {
    // --- Accessors ------------------------------------------------------

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    pub fn log_debug(&mut self, msg: &str) {
        if let Some(f) = self.debug.as_mut() {
            let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%H:%M:%S%.3f"));
        }
    }

    // --- Notices ---------------------------------------------------------

    pub fn notice_warn(&mut self, text: impl Into<String>) {
        self.push_notice(text.into(), true);
    }

    fn push_notice(&mut self, text: String, warning: bool) {
        self.log_debug(&format!("notice: {text}"));
        self.notices.push_back(Notice {
            text,
            until: Instant::now() + Duration::from_secs(4),
            warning,
        });
        while self.notices.len() > 3 {
            self.notices.pop_front();
        }
        self.mark_dirty();
    }

    // --- Periodic update ---------------------------------------------------

    // --- Tabs & sources -----------------------------------------------------

    fn add_merged_tab(
        &mut self,
        sources: Vec<Source>,
        filter: Filter,
        collapse: bool,
        follow: bool,
    ) {
        let merged = Merged::start(sources);
        merged.add_waker(Arc::clone(&self.wake));
        let tab = Tab::new(
            "merged".into(),
            Base::Merged(merged),
            filter,
            collapse,
            follow,
            self.config.defaults.wrap,
            self.config.defaults.columns,
        );
        tab.view.add_waker(Arc::clone(&self.wake));
        self.tabs.push(tab);
    }

    // --- View rebuilding ---------------------------------------------------

    // --- Scrolling ------------------------------------------------------

    // --- Actions -----------------------------------------------------------

    // --- Modals ---------------------------------------------------------------

    // --- Chips -------------------------------------------------------------

    // --- Input ---------------------------------------------------------------

    // --- Mouse -----------------------------------------------------------------

}

