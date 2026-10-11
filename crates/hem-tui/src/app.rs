//! Application state and input handling.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use hem_core::cluster::{Clusters, Template};
use hem_core::collapse::shape_hash;
use hem_core::filter::{FieldPredicate, Filter, FilterSpec, Matcher, TemplateMatch};
use hem_core::format::Format;
use hem_core::merge::Merged;
use hem_core::parse::{parse_line, LineMeta, MetaCache};
use hem_core::search::Search;
use hem_core::source::{Notice as SourceNotice, SourceOptions};
use hem_core::timestamp::{self, TsDisplay};
use hem_core::view::{Base, Row, View, ViewSpec};
use hem_core::{Level, Source};

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::keymap::{Action, KeyMap};
use crate::modal::{
    FilterEditorState, Input, Modal, PromptKind, SettingItem, TemplateSort, TemplatesState,
};
use crate::theme::{Depth, Theme};
use crate::RunOptions;

/// Maximum lines expanded inline for a collapsed run.
pub const MAX_EXPAND: usize = 500;

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

    pub fn format_for(&self, source: u8) -> Format {
        self.view.format_for(source)
    }

    pub fn is_pinned(&self, row: Row) -> bool {
        self.pins.contains(&(row.source(), row.line()))
    }

    pub fn line_bytes(&self, row: Row) -> Vec<u8> {
        self.base.line_bytes(row).unwrap_or_default()
    }

    pub fn meta_for(&self, row: Row, line: &[u8]) -> LineMeta {
        let gen = self.view.generation();
        let key = ((row.source() as u64) << 48) | row.line();
        self.meta.get_or_parse(
            gen,
            key,
            line,
            self.format_for(row.source()),
            self.view.default_year(),
        )
    }

    /// Member lines of a collapsed run (bounded).
    pub fn members(&mut self, row_idx: usize) -> Vec<Row> {
        if let Some(m) = self.member_cache.get(&row_idx) {
            return m.clone();
        }
        let Some(row) = self.view.row(row_idx) else {
            return Vec::new();
        };
        let want = row.count() as usize;
        let mut out = vec![row.with_count(1)];
        if want <= 1 {
            return out;
        }
        if let Some(src) = self.base.source(row.source()) {
            let g = src.read();
            let fmt = self.format_for(row.source());
            let year = self.view.default_year();
            let rep = g.line(row.line() as usize).unwrap_or(&[]).to_vec();
            let key = shape_hash(&rep);
            let mut l = row.line() as usize + 1;
            let mut scanned = 0usize;
            let n = g.line_count();
            while out.len() < want.min(MAX_EXPAND) && l < n && scanned < 200_000 {
                if let Some(line) = g.line(l) {
                    let meta = parse_line(line, fmt, year);
                    if self.filter.matches(line, &meta, fmt) {
                        if shape_hash(line) == key {
                            out.push(Row::new(l as u64, row.source(), 1));
                        } else if !self.base.is_merged() {
                            // Consecutive (in filtered order) means the run ended.
                            break;
                        }
                    }
                }
                l += 1;
                scanned += 1;
            }
        }
        if self.member_cache.len() > 256 {
            self.member_cache.clear();
        }
        self.member_cache.insert(row_idx, out.clone());
        out
    }

    /// Sparkline heights (0..=8) for a collapsed run: where in time (or
    /// position) its repeats occurred.
    pub fn sparkline(&mut self, row_idx: usize, cells: usize) -> Vec<u8> {
        let members = self.members(row_idx);
        if members.len() < 2 || cells == 0 {
            return Vec::new();
        }
        let fmt = self.format_for(members[0].source());
        let year = self.view.default_year();
        let mut keys: Vec<i64> = Vec::with_capacity(members.len());
        let mut carry = None;
        if let Some(src) = self.base.source(members[0].source()) {
            let g = src.read();
            for m in &members {
                let line = g.line(m.line() as usize).unwrap_or(&[]);
                let meta = parse_line(line, fmt, year);
                if meta.ts.is_some() {
                    carry = meta.ts;
                }
                keys.push(carry.unwrap_or(m.line() as i64));
            }
        }
        let (min, max) = (
            keys.iter().copied().min().unwrap_or(0),
            keys.iter().copied().max().unwrap_or(0),
        );
        let mut buckets = vec![0u32; cells];
        for k in &keys {
            let i = if max > min {
                (((k - min) as u128 * cells as u128) / (max - min + 1) as u128) as usize
            } else {
                0
            };
            buckets[i.min(cells - 1)] += 1;
        }
        let peak = buckets.iter().copied().max().unwrap_or(1).max(1);
        buckets
            .iter()
            .map(|b| {
                if *b == 0 {
                    0
                } else {
                    (1 + (*b as u64 * 7) / peak as u64) as u8
                }
            })
            .collect()
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

impl PaletteItem {
    pub fn label(&self, app: &App) -> String {
        match self {
            PaletteItem::Action(a) => a.label().to_string(),
            PaletteItem::Preset(p) => format!("Preset: {p}"),
            PaletteItem::Theme(t) => format!("Theme: {t}"),
            PaletteItem::MergeAll => "Merge all sources by timestamp".into(),
            PaletteItem::Tab(i) => format!(
                "Switch to: {}",
                app.tabs.get(*i).map(|t| t.name.as_str()).unwrap_or("?")
            ),
        }
    }
}

impl App {
    pub fn new(opts: RunOptions) -> Result<App> {
        let depth = Depth::detect();
        let theme_name = opts
            .theme
            .clone()
            .or_else(|| opts.config.theme.clone())
            .unwrap_or_else(|| "dark".into());
        let mut theme = Theme::by_name(&theme_name).unwrap_or_else(Theme::dark);
        let bad = theme.apply_overrides(&opts.config.colors);
        theme = theme.adapt(depth);
        let mut keymap = KeyMap::default();
        let key_errors = keymap.apply_overrides(&opts.config.keys);

        let defaults = &opts.config.defaults;
        let collapse = opts.collapse.unwrap_or(defaults.collapse);
        let follow = opts.follow || defaults.follow;
        let filter = Filter::from_spec(&opts.initial_filter)?;
        let dirty = Arc::new(AtomicBool::new(true));
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let d = Arc::clone(&dirty);
            Arc::new(move || d.store(true, Ordering::Release))
        };

        let mut tabs = Vec::new();
        for src in &opts.sources {
            src.add_waker(Arc::clone(&wake));
            let tab = Tab::new(
                src.name().to_string(),
                Base::Single(src.clone()),
                filter.clone(),
                collapse,
                follow,
                defaults.wrap,
                defaults.columns,
            );
            tab.view.add_waker(Arc::clone(&wake));
            tabs.push(tab);
        }
        let mut app = App {
            tabs,
            active: 0,
            config: opts.config.clone(),
            config_path: opts.config_path.clone(),
            theme,
            keymap,
            modal: None,
            templates_mode: false,
            notices: VecDeque::new(),
            quit: false,
            ascii: opts.ascii || defaults.ascii,
            mouse: opts.mouse && defaults.mouse,
            minimap: defaults.minimap,
            ts_display: defaults.timestamp,
            hits: Vec::new(),
            chips: Vec::new(),
            clipboard: Clipboard::new(),
            debug: None,
            size: (80, 24),
            dirty,
            osc52: None,
            minimap_drag: false,
            last_progress: String::new(),
            cluster_cap: if defaults.cluster_cap == 0 {
                usize::MAX / 2
            } else {
                defaults.cluster_cap
            },
            palette_items: Vec::new(),
            wake,
        };
        if opts.merge && opts.sources.len() > 1 {
            app.add_merged_tab(opts.sources.clone(), filter.clone(), collapse, follow);
            app.active = app.tabs.len() - 1;
        }
        for b in bad {
            app.notice_warn(format!("config: unknown colour {b}"));
        }
        for e in key_errors {
            app.notice_warn(format!("config: {e}"));
        }
        if app.tabs.is_empty() {
            anyhow::bail!("no sources");
        }
        Ok(app)
    }

    // --- Accessors ------------------------------------------------------

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let i = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }

    pub fn take_dirty(&mut self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    pub fn take_osc52(&mut self) -> Option<String> {
        self.osc52.take()
    }

    pub fn animating(&self) -> bool {
        false
    }

    /// Any background work still running?
    pub fn busy(&self) -> bool {
        let t = self.tab();
        let src_busy = t.base.sources().iter().any(|s| !s.is_complete());
        src_busy
            || !t.view.is_complete()
            || t.search.as_ref().map(|s| !s.is_complete()).unwrap_or(false)
            || t.clusters
                .as_ref()
                .map(|c| !c.is_complete())
                .unwrap_or(false)
            || t.pending_anchor.is_some()
    }

    pub fn log_debug(&mut self, msg: &str) {
        if let Some(f) = self.debug.as_mut() {
            let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%H:%M:%S%.3f"));
        }
    }

    // --- Notices ---------------------------------------------------------

    pub fn notice(&mut self, text: impl Into<String>) {
        self.push_notice(text.into(), false);
    }

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

    pub fn tick(&mut self) {
        let now = Instant::now();
        let before = self.notices.len();
        self.notices.retain(|n| n.until > now);
        if self.notices.len() != before {
            self.mark_dirty();
        }
        // Source notices.
        let mut msgs = Vec::new();
        for t in &self.tabs {
            for s in t.base.sources() {
                for n in s.take_notices() {
                    msgs.push(match n {
                        SourceNotice::Truncated => {
                            (format!("{}: file truncated, re-indexed", s.name()), true)
                        }
                        SourceNotice::Rotated => {
                            (format!("{}: file rotated, re-opened", s.name()), true)
                        }
                        SourceNotice::Removed => (
                            format!("{}: file removed (still showing last contents)", s.name()),
                            true,
                        ),
                        SourceNotice::Error(e) => (format!("{}: {e}", s.name()), true),
                    });
                }
            }
        }
        for (m, w) in msgs {
            self.push_notice(m, w);
        }
        // View generation changes (rotation) reset positions.
        for t in &mut self.tabs {
            let g = t.view.generation();
            if g != t.view_gen {
                t.view_gen = g;
                t.top = 0;
                t.cursor = 0;
                t.expanded.clear();
                t.member_cache.clear();
                t.meta.clear();
            }
            // Pending anchor after a view rebuild.
            if let Some((src, line)) = t.pending_anchor {
                let n = t.len();
                if n > 0 {
                    let pos = t.view.row_for_line(src, line);
                    if t.view.is_complete() || pos + 1 < n {
                        t.cursor = pos.min(n - 1);
                        t.top = t.cursor.saturating_sub(t.last_height / 3);
                        t.pending_anchor = None;
                    }
                } else if t.view.is_complete() && t.base.is_complete() {
                    t.pending_anchor = None;
                }
            }
            // Follow.
            let n = t.len();
            if t.follow && n != t.last_len {
                t.cursor = n.saturating_sub(1);
                t.top = n.saturating_sub(t.last_height.max(1));
            }
            if n != t.last_len {
                t.last_len = n;
            }
        }
        // Progress text changes should redraw.
        let p = self.progress_text();
        if p != self.last_progress {
            self.last_progress = p;
            self.mark_dirty();
        }
    }

    /// Short indexing / search progress summary for the status bar.
    pub fn progress_text(&self) -> String {
        let t = self.tab();
        let mut parts = Vec::new();
        for s in t.base.sources() {
            if !s.is_complete() {
                parts.push(format!(
                    "indexing {} {:.0}%",
                    s.name(),
                    s.progress() * 100.0
                ));
            }
        }
        if !t.view.is_complete() && t.base.is_complete() {
            parts.push(format!("filtering {:.0}%", t.view.progress() * 100.0));
        }
        if let Some(s) = &t.search {
            if !s.is_complete() {
                parts.push(format!("searching {:.0}%", s.progress() * 100.0));
            }
        }
        if let Some(c) = &t.clusters {
            if !c.is_complete() {
                parts.push(format!("clustering {:.0}%", c.progress() * 100.0));
            }
        }
        parts.join(" · ")
    }

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

    pub fn open_path(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        let opts = SourceOptions {
            follow: true,
            ..Default::default()
        };
        match Source::open(path, opts) {
            Ok(src) => {
                src.add_waker(Arc::clone(&self.wake));
                let t = self.tab();
                let (filter, collapse, follow, wrap, columns) =
                    (t.filter.clone(), t.collapse, t.follow, t.wrap, t.columns);
                let tab = Tab::new(
                    src.name().to_string(),
                    Base::Single(src),
                    filter,
                    collapse,
                    follow,
                    wrap,
                    columns,
                );
                tab.view.add_waker(Arc::clone(&self.wake));
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.notice(format!("opened {path}"));
            }
            Err(e) => self.notice_warn(e.to_string()),
        }
    }

    pub fn merge_all(&mut self) {
        let sources: Vec<Source> = self
            .tabs
            .iter()
            .filter(|t| !t.base.is_merged())
            .flat_map(|t| t.base.sources())
            .collect();
        if sources.len() < 2 {
            self.notice_warn("need at least two sources to merge");
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.base.is_merged()) {
            self.active = i;
            return;
        }
        let t = self.tab();
        let (filter, collapse, follow) = (t.filter.clone(), t.collapse, t.follow);
        self.add_merged_tab(sources, filter, collapse, follow);
        self.active = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, i: usize) {
        if self.tabs.len() <= 1 {
            self.quit = true;
            return;
        }
        if i < self.tabs.len() {
            let t = self.tabs.remove(i);
            t.view.cancel();
            if let Some(s) = &t.search {
                s.cancel();
            }
            if let Some(c) = &t.clusters {
                c.cancel();
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
        }
    }

    // --- View rebuilding ---------------------------------------------------

    /// Rebuild the active tab's view after a filter / collapse change.
    pub fn rebuild_view(&mut self) {
        let wake = Arc::clone(&self.wake);
        let t = self.tab_mut();
        let anchor = t.view.row(t.cursor).map(|r| (r.source(), r.line()));
        t.view.cancel();
        if let Some(s) = t.search.take() {
            s.cancel();
        }
        t.view = View::start(
            t.base.clone(),
            ViewSpec {
                filter: t.filter.clone(),
                collapse: t.collapse,
            },
        );
        t.view.add_waker(wake);
        t.view_gen = t.view.generation();
        t.expanded.clear();
        t.member_cache.clear();
        t.current_match = None;
        t.last_len = 0;
        t.pending_anchor = anchor;
        if !t.search_text.is_empty() {
            let pat = t.search_text.clone();
            self.start_search(&pat);
        }
        self.mark_dirty();
    }

    pub fn start_search(&mut self, pattern: &str) {
        let wake = Arc::clone(&self.wake);
        let t = self.tab_mut();
        if let Some(s) = t.search.take() {
            s.cancel();
        }
        t.current_match = None;
        if pattern.is_empty() {
            t.search_text.clear();
            return;
        }
        match Matcher::new(pattern, false) {
            Ok(m) => {
                t.search_text = pattern.to_string();
                let s = Search::start(t.view.clone(), m);
                s.add_waker(wake);
                t.search = Some(s);
            }
            Err(e) => {
                let msg = e.to_string();
                self.notice_warn(msg);
            }
        }
    }

    // --- Scrolling ------------------------------------------------------

    fn page_size(&self) -> usize {
        self.tab().last_height.max(1)
    }

    pub fn set_cursor(&mut self, row: usize) {
        let h = self.page_size();
        let t = self.tab_mut();
        let n = t.len();
        if n == 0 {
            t.cursor = 0;
            t.top = 0;
            return;
        }
        t.cursor = row.min(n - 1);
        if t.cursor < t.top {
            t.top = t.cursor;
        } else if t.cursor >= t.top + h {
            t.top = t.cursor + 1 - h;
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let n = self.tab().len();
        let cur = self.tab().cursor as isize;
        let target = (cur + delta).clamp(0, n.saturating_sub(1) as isize) as usize;
        let live = self.tab().base.is_live();
        self.set_cursor(target);
        let t = self.tab_mut();
        if delta < 0 {
            t.follow = false;
        } else if live && t.cursor + 1 >= n && n > 0 && delta > 0 {
            t.follow = true;
        }
    }

    pub fn center_on(&mut self, row: usize) {
        let h = self.page_size();
        let t = self.tab_mut();
        let n = t.len();
        if n == 0 {
            return;
        }
        t.cursor = row.min(n - 1);
        t.top = t.cursor.saturating_sub(h / 2);
        t.follow = false;
    }

    pub fn go_bottom(&mut self) {
        let n = self.tab().len();
        let live = self.tab().base.is_live();
        self.set_cursor(n.saturating_sub(1));
        if live {
            self.tab_mut().follow = true;
        }
    }

    // --- Actions -----------------------------------------------------------

    fn jump_match(&mut self, forward: bool) {
        let t = self.tab();
        let Some(s) = &t.search else {
            self.notice("no search · press / to search");
            return;
        };
        let cur = t.cursor;
        let found = if forward {
            s.next_after(cur)
        } else {
            s.prev_before(cur)
        };
        let total = s.count();
        match found {
            Some((i, row)) => {
                self.center_on(row);
                self.tab_mut().current_match = Some(i);
                self.notice(format!("match {}/{total}", i + 1));
            }
            None => self.notice("no matches"),
        }
    }

    fn jump_error(&mut self, forward: bool) {
        let t = self.tab();
        let from = if forward {
            t.cursor + 1
        } else {
            t.cursor.saturating_sub(1)
        };
        if (forward && from >= t.len()) || (!forward && t.cursor == 0) {
            self.notice("no more errors");
            return;
        }
        match t.view.find_level(from, Level::Error, forward) {
            Some(row) => self.center_on(row),
            None => self.notice(if forward {
                "no more errors"
            } else {
                "no earlier errors"
            }),
        }
    }

    pub fn jump_to_line(&mut self, line: u64) {
        let row = self.tab().view.row_for_line(0, line.saturating_sub(1));
        self.center_on(row);
    }

    pub fn jump_to_fraction(&mut self, pct: f64) {
        let n = self.tab().len();
        let row = ((n as f64) * pct.clamp(0.0, 1.0)) as usize;
        self.center_on(row.min(n.saturating_sub(1)));
    }

    pub fn jump_to_timestamp(&mut self, ts: i64) {
        let row = self.tab().view.row_for_timestamp(ts);
        self.center_on(row);
    }

    // --- Modals ---------------------------------------------------------------

    fn open_prompt(&mut self, kind: PromptKind, initial: &str) {
        self.modal = Some(Modal::Prompt {
            kind,
            input: Input::with(initial),
        });
    }

    fn open_palette(&mut self) {
        self.palette_items = self.build_palette_items();
        self.modal = Some(Modal::Palette {
            input: Input::default(),
            selected: 0,
        });
    }

    fn build_palette_items(&self) -> Vec<PaletteItem> {
        let mut items: Vec<PaletteItem> = Action::ALL
            .iter()
            .map(|a| PaletteItem::Action(*a))
            .collect();
        for p in self.config.presets.keys() {
            items.push(PaletteItem::Preset(p.clone()));
        }
        for t in Theme::names() {
            items.push(PaletteItem::Theme(t.to_string()));
        }
        if self.tabs.len() > 1 {
            items.push(PaletteItem::MergeAll);
            for i in 0..self.tabs.len() {
                items.push(PaletteItem::Tab(i));
            }
        }
        items
    }

    /// Palette entries matching the query, best first.
    pub fn palette_matches(&self, query: &str) -> Vec<(usize, i32)> {
        let mut v: Vec<(usize, i32)> = self
            .palette_items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy_score(query, &item.label(self)).map(|s| (i, s)))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    pub fn apply_preset(&mut self, name: &str) {
        let Some(spec) = self.config.presets.get(name).cloned() else {
            self.notice_warn(format!("no preset {name}"));
            return;
        };
        match Filter::from_spec(&spec) {
            Ok(f) => {
                self.tab_mut().filter = f;
                self.rebuild_view();
                self.notice(format!("preset {name}"));
            }
            Err(e) => self.notice_warn(e.to_string()),
        }
    }

    pub fn set_theme(&mut self, name: &str) {
        let Some(mut t) = Theme::by_name(name) else {
            self.notice_warn(format!("unknown theme {name}"));
            return;
        };
        t.apply_overrides(&self.config.colors);
        let depth = self.theme.depth;
        self.theme = t.adapt(depth);
        self.config.theme = Some(name.to_string());
        self.save_config();
    }

    pub fn save_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            self.notice_warn("no config path available");
            return;
        };
        match self.config.save(&path) {
            Ok(()) => self.notice(format!("saved {}", path.display())),
            Err(e) => self.notice_warn(format!("save failed: {e}")),
        }
    }

    fn open_filter_editor(&mut self) {
        let f = &self.tab().filter;
        let st = FilterEditorState {
            include: Input::with(
                &f.include
                    .iter()
                    .map(|m| m.text.clone())
                    .collect::<Vec<_>>()
                    .join(" && "),
            ),
            exclude: Input::with(
                &f.exclude
                    .iter()
                    .map(|m| m.text.clone())
                    .collect::<Vec<_>>()
                    .join(" && "),
            ),
            fields: Input::with(
                &f.fields
                    .iter()
                    .map(|p| p.display())
                    .collect::<Vec<_>>()
                    .join(" && "),
            ),
            since: Input::with(f.spec.since.as_deref().unwrap_or("")),
            until: Input::with(f.spec.until.as_deref().unwrap_or("")),
            level: f.min_level,
            literal: f.spec.literal,
            focus: 0,
            error: None,
        };
        self.modal = Some(Modal::FilterEditor(Box::new(st)));
    }

    fn apply_filter_editor(&mut self, st: &FilterEditorState) -> Result<(), String> {
        let split = |s: &str| -> Vec<String> {
            s.split("&&")
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };
        let spec = FilterSpec {
            include: split(&st.include.text),
            exclude: split(&st.exclude.text),
            level: st.level,
            since: Some(st.since.text.trim().to_string()).filter(|s| !s.is_empty()),
            until: Some(st.until.text.trim().to_string()).filter(|s| !s.is_empty()),
            fields: split(&st.fields.text),
            literal: st.literal,
        };
        let mut f = Filter::from_spec(&spec).map_err(|e| e.to_string())?;
        f.template = self.tab().filter.template.clone();
        self.tab_mut().filter = f;
        self.rebuild_view();
        Ok(())
    }

    fn open_detail(&mut self) {
        let t = self.tab();
        let Some(row) = t.view.row(t.cursor) else {
            return;
        };
        self.modal = Some(Modal::Detail {
            row_idx: t.cursor,
            row,
            scroll: 0,
            selected_field: 0,
        });
    }

    fn toggle_templates(&mut self) {
        if self.templates_mode {
            self.templates_mode = false;
            return;
        }
        let wake = Arc::clone(&self.wake);
        let cap = self.cluster_cap;
        let t = self.tab_mut();
        if t.clusters.is_none() {
            let c = Clusters::start(t.base.clone(), cap);
            c.add_waker(wake);
            t.clusters = Some(c);
        }
        self.templates_mode = true;
    }

    /// Templates sorted for display.
    pub fn sorted_templates(&self) -> Vec<Template> {
        let t = self.tab();
        let Some(c) = &t.clusters else {
            return Vec::new();
        };
        let mut v: Vec<Template> = c.templates().iter().cloned().collect();
        let q = t.templates.filter.text.to_lowercase();
        if !q.is_empty() {
            v.retain(|t| t.display().to_lowercase().contains(&q));
        }
        let st = &t.templates;
        v.sort_by(|a, b| {
            let ord = match st.sort {
                TemplateSort::Count => a.count.cmp(&b.count),
                TemplateSort::Level => a.max_level().cmp(&b.max_level()),
                TemplateSort::First => a.first_ts.cmp(&b.first_ts).then(a.first.cmp(&b.first)),
                TemplateSort::Last => a.last_ts.cmp(&b.last_ts).then(a.last.cmp(&b.last)),
                TemplateSort::Template => a.display().cmp(&b.display()),
            };
            if st.ascending {
                ord
            } else {
                ord.reverse()
            }
        });
        v
    }

    fn apply_selected_template(&mut self) {
        let list = self.sorted_templates();
        let sel = self.tab().templates.selected;
        let Some(tpl) = list.get(sel) else { return };
        let m: TemplateMatch = tpl.matcher();
        let label = m.display();
        let first = tpl.first;
        let t = self.tab_mut();
        t.filter.template = Some(m);
        self.rebuild_view();
        self.templates_mode = false;
        self.tab_mut().pending_anchor = Some((first.source(), first.line()));
        self.notice(format!("template: {}", truncate(&label, 60)));
    }

    // --- Chips -------------------------------------------------------------

    /// Rebuild the chip list from the active filter (called by the renderer).
    pub fn compute_chips(&mut self) -> Vec<(Chip, String)> {
        let t = self.tab();
        let f = &t.filter;
        let mut v = Vec::new();
        for (i, m) in f.include.iter().enumerate() {
            v.push((Chip::Include(i), format!("filter: {}", m.text)));
        }
        for (i, m) in f.exclude.iter().enumerate() {
            v.push((Chip::Exclude(i), format!("exclude: {}", m.text)));
        }
        if let Some(l) = f.min_level {
            v.push((Chip::Level, format!("level ≥ {l}")));
        }
        if f.since.is_some() || f.until.is_some() {
            let s = f.spec.since.clone().unwrap_or_else(|| "…".into());
            let u = f.spec.until.clone().unwrap_or_else(|| "…".into());
            v.push((Chip::Time, format!("{s} → {u}")));
        }
        for (i, p) in f.fields.iter().enumerate() {
            v.push((Chip::Field(i), p.display()));
        }
        if let Some(tm) = &f.template {
            v.push((
                Chip::Template,
                format!("template: {}", truncate(&tm.display(), 40)),
            ));
        }
        if !t.search_text.is_empty() {
            v.push((Chip::Search, format!("/{}", t.search_text)));
        }
        if let Some(h) = &t.highlight {
            v.push((Chip::Highlight, format!("highlight: {}", h.text)));
        }
        self.chips = v.iter().map(|(c, _)| c.clone()).collect();
        v
    }

    pub fn remove_chip(&mut self, chip: Chip) {
        let t = self.tab_mut();
        let mut rebuild = true;
        match chip {
            Chip::Include(i) => {
                if i < t.filter.include.len() {
                    t.filter.include.remove(i);
                }
            }
            Chip::Exclude(i) => {
                if i < t.filter.exclude.len() {
                    t.filter.exclude.remove(i);
                }
            }
            Chip::Level => t.filter.min_level = None,
            Chip::Time => {
                t.filter.since = None;
                t.filter.until = None;
                t.filter.spec.since = None;
                t.filter.spec.until = None;
            }
            Chip::Field(i) => {
                if i < t.filter.fields.len() {
                    t.filter.fields.remove(i);
                }
            }
            Chip::Template => t.filter.template = None,
            Chip::Search => {
                rebuild = false;
                if let Some(s) = t.search.take() {
                    s.cancel();
                }
                t.search_text.clear();
                t.current_match = None;
            }
            Chip::Highlight => {
                rebuild = false;
                t.highlight = None;
            }
        }
        t.filter.sync_spec();
        if rebuild {
            self.rebuild_view();
        }
        self.mark_dirty();
    }

    // --- Input ---------------------------------------------------------------

    fn source_pick(&mut self, s: usize) {
        if s < self.tabs.len() {
            self.active = s;
        } else if s == self.tabs.len() {
            self.merge_all();
        } else {
            self.open_prompt(PromptKind::OpenFile, "");
        }
    }

    fn toggle_setting(&mut self, item: SettingItem, forward: bool) {
        match item {
            SettingItem::Theme => {
                let names = Theme::names();
                let cur = names
                    .iter()
                    .position(|n| *n == self.theme.name)
                    .unwrap_or(0);
                let next = if forward {
                    (cur + 1) % names.len()
                } else {
                    (cur + names.len() - 1) % names.len()
                };
                self.set_theme(names[next]);
                return;
            }
            SettingItem::Collapse => {
                self.config.defaults.collapse = !self.config.defaults.collapse;
                let v = self.config.defaults.collapse;
                if self.tab().collapse != v {
                    self.tab_mut().collapse = v;
                    self.rebuild_view();
                }
            }
            SettingItem::Timestamp => {
                let order = [
                    TsDisplay::Original,
                    TsDisplay::Time,
                    TsDisplay::Iso,
                    TsDisplay::Local,
                    TsDisplay::Relative,
                ];
                let cur = order
                    .iter()
                    .position(|o| *o == self.ts_display)
                    .unwrap_or(0);
                let next = if forward {
                    (cur + 1) % order.len()
                } else {
                    (cur + order.len() - 1) % order.len()
                };
                self.ts_display = order[next];
                self.config.defaults.timestamp = self.ts_display;
            }
            SettingItem::Wrap => {
                self.config.defaults.wrap = !self.config.defaults.wrap;
                let v = self.config.defaults.wrap;
                self.tab_mut().wrap = v;
            }
            SettingItem::Follow => {
                self.config.defaults.follow = !self.config.defaults.follow;
            }
            SettingItem::Columns => {
                self.config.defaults.columns = !self.config.defaults.columns;
                let v = self.config.defaults.columns;
                self.tab_mut().columns = v;
            }
            SettingItem::Minimap => {
                self.config.defaults.minimap = !self.config.defaults.minimap;
                self.minimap = self.config.defaults.minimap;
            }
            SettingItem::Mouse => {
                self.config.defaults.mouse = !self.config.defaults.mouse;
                self.notice("mouse setting applies on next start");
            }
            SettingItem::Ascii => {
                self.config.defaults.ascii = !self.config.defaults.ascii;
                self.ascii = self.config.defaults.ascii;
            }
        }
        self.save_config();
    }

    fn submit_prompt(&mut self, kind: PromptKind, text: &str) {
        let text = text.trim();
        match kind {
            PromptKind::Search => {
                self.start_search(text);
                if !text.is_empty() {
                    // Jump to the first match once results arrive: try now.
                    self.jump_match(true);
                }
            }
            PromptKind::Include | PromptKind::Exclude => {
                if text.is_empty() {
                    return;
                }
                match Matcher::new(text, false) {
                    Ok(m) => {
                        let t = self.tab_mut();
                        if kind == PromptKind::Include {
                            t.filter.include.push(m);
                        } else {
                            t.filter.exclude.push(m);
                        }
                        t.filter.sync_spec();
                        self.rebuild_view();
                    }
                    Err(e) => self.notice_warn(e.to_string()),
                }
            }
            PromptKind::Field => {
                if text.is_empty() {
                    return;
                }
                match FieldPredicate::parse(text) {
                    Ok(p) => {
                        let t = self.tab_mut();
                        t.filter.fields.push(p);
                        t.filter.sync_spec();
                        self.rebuild_view();
                    }
                    Err(e) => self.notice_warn(e.to_string()),
                }
            }
            PromptKind::OpenFile => self.open_path(text),
            PromptKind::Highlight => {
                if text.is_empty() {
                    self.tab_mut().highlight = None;
                } else {
                    match Matcher::new(text, true) {
                        Ok(m) => self.tab_mut().highlight = Some(m),
                        Err(e) => self.notice_warn(e.to_string()),
                    }
                }
            }
            PromptKind::Goto => self.goto(text),
        }
    }

    pub fn goto(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(p) = text.strip_suffix('%') {
            if let Ok(v) = p.trim().parse::<f64>() {
                self.jump_to_fraction(v / 100.0);
                return;
            }
        }
        if let Ok(n) = text.parse::<u64>() {
            self.jump_to_line(n);
            return;
        }
        if let Some(ts) = timestamp::parse_user(text) {
            self.jump_to_timestamp(ts);
            return;
        }
        self.notice_warn(format!(
            "cannot interpret {text:?} as a line, percentage or time"
        ));
    }

    // --- Mouse -----------------------------------------------------------------

    fn hit_at(&self, x: u16, y: u16) -> Option<Hit> {
        // Later entries are drawn on top; search from the end.
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, h)| h.clone())
    }

}

/// Apply a key event to a text input (shared by prompts and editors).
pub fn edit_input(input: &mut Input, ev: &KeyEvent) {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    match ev.code {
        KeyCode::Char('u') if ctrl => input.kill_to_start(),
        KeyCode::Char('w') if ctrl => input.delete_word(),
        KeyCode::Char('a') if ctrl => input.home(),
        KeyCode::Char('e') if ctrl => input.end(),
        KeyCode::Char(c) if !ctrl => input.insert(c),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left => input.left(),
        KeyCode::Right => input.right(),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        _ => {}
    }
}

/// Simple subsequence fuzzy score: higher is better, `None` if no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0i32;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            score += 10;
            if let Some(l) = last {
                if l + 1 == i {
                    score += 15;
                }
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                score += 8;
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    if let Some(pos) = text.to_lowercase().find(&query.to_lowercase()) {
        score += 40 - (pos as i32).min(30);
    }
    Some(score - t.len() as i32 / 8)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(
            fuzzy_score("ne", "Next error").unwrap()
                > fuzzy_score("ne", "Toggle line wrap").unwrap_or(-999)
        );
        assert_eq!(fuzzy_score("zzz", "Next error"), None);
        assert!(fuzzy_score("", "anything").is_some());
    }
}
