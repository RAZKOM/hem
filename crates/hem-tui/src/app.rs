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
use hem_core::collapse::shape_hash;
use hem_core::filter::{Filter, Matcher};
use hem_core::format::Format;
use hem_core::merge::Merged;
use hem_core::parse::{parse_line, LineMeta, MetaCache};
use hem_core::search::Search;
use hem_core::source::Notice as SourceNotice;
use hem_core::timestamp::TsDisplay;
use hem_core::view::{Base, Row, View, ViewSpec};
use hem_core::{Level, Source};

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::keymap::{Action, KeyMap};
use crate::modal::{Modal, TemplateSort, TemplatesState};
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

    pub fn animating(&self) -> bool {
        false
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

    // --- View rebuilding ---------------------------------------------------

    // --- Scrolling ------------------------------------------------------

    // --- Actions -----------------------------------------------------------

    // --- Modals ---------------------------------------------------------------

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

    // --- Input ---------------------------------------------------------------

    // --- Mouse -----------------------------------------------------------------

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
