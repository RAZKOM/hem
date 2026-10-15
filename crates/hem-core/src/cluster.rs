//! Template clustering with a Drain-style streaming miner.
//!
//! Lines are normalised (numbers / ids masked), tokenised on whitespace and
//! routed through a fixed-depth prefix tree keyed by token count and the
//! first few tokens.  At each leaf the most similar existing template is
//! updated (differing positions become `<*>`) or a new template is created.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use std::borrow::Cow;

use crate::collapse::normalize;
use crate::fields;
use crate::filter::{key_of, TemplateMatch, Token};
use crate::format::Format;
use crate::level::Level;
use crate::parse::parse_line;
use crate::view::{Base, Row, CHUNK_ROWS};

const MAX_TOKENS: usize = 48;
const MAX_TOKEN_LEN: usize = 48;

/// Fields that carry the human-readable message in structured logs.
const MSG_KEYS: &[&str] = &["msg", "message", "event", "error", "err", "text", "log"];

/// Tokenise a line for clustering.  Structured lines (JSON / logfmt) are
/// clustered on their level + message field so that keys and quoted
/// values do not swallow the whole line.
pub fn tokenize(line: &[u8], format: Format) -> Vec<String> {
    let line = &line[..line.len().min(4096)];
    let format = format.for_line(line);
    let text: Cow<'_, [u8]> = match format {
        Format::Json | Format::Logfmt => {
            let msg = MSG_KEYS.iter().find_map(|k| match format {
                Format::Json => fields::json_get(line, k).map(|v| v.into_owned()),
                _ => fields::logfmt_get(line, k).map(|v| String::from_utf8_lossy(v).into_owned()),
            });
            match msg {
                Some(m) => {
                    let level = crate::parse::parse_line(line, format, 1970)
                        .level
                        .map(|l| l.as_str())
                        .unwrap_or("");
                    Cow::Owned(format!("{level} {m}").into_bytes())
                }
                None => Cow::Borrowed(line),
            }
        }
        _ => Cow::Borrowed(line),
    };
    tokenize_text(&text)
}

fn tokenize_text(text: &[u8]) -> Vec<String> {
    let norm = normalize(&text[..text.len().min(2048)]);
    let s = String::from_utf8_lossy(&norm);
    s.split_whitespace()
        .take(MAX_TOKENS)
        .map(|t| {
            if t.contains('#') {
                // Keep the key of a key=value pair whose value is variable.
                match key_of(t) {
                    Some(k) if !k.contains('#') => format!("{k}=<*>"),
                    _ => "<*>".to_string(),
                }
            } else if t.len() > MAX_TOKEN_LEN {
                let mut cut = MAX_TOKEN_LEN;
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t[..cut].to_string()
            } else {
                t.to_string()
            }
        })
        .collect()
}

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

impl Template {
    pub fn display(&self) -> String {
        self.matcher().display()
    }

    pub fn matcher(&self) -> TemplateMatch {
        TemplateMatch {
            tokens: self.tokens.clone(),
            format: self.format,
        }
    }

    /// Most common level.
    pub fn dominant_level(&self) -> Option<Level> {
        let (mut best, mut best_n) = (None, 0u64);
        for (i, n) in self.levels.iter().enumerate().take(Level::COUNT) {
            if *n > best_n {
                best_n = *n;
                best = Level::from_index(i);
            }
        }
        best
    }

    /// Highest level seen at least once.
    pub fn max_level(&self) -> Option<Level> {
        (0..Level::COUNT)
            .rev()
            .find(|&i| self.levels[i] > 0)
            .and_then(Level::from_index)
    }
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

impl std::fmt::Debug for Miner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Miner({} templates)", self.templates.len())
    }
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
            depth: 4,
            max_children: 100,
            max_templates: 20_000,
            roots: HashMap::new(),
            templates: Vec::new(),
        }
    }

    pub fn templates(&self) -> &[Template] {
        &self.templates
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Add a tokenised line; returns the template id.
    pub fn add(
        &mut self,
        tokens: &[String],
        format: Format,
        row: Row,
        level: Option<Level>,
        ts: Option<i64>,
    ) -> usize {
        let n = tokens.len();
        let mut node = self.roots.entry(n).or_default();
        // Route on the first `depth` non-wildcard tokens (timestamps at the
        // start of a line are wildcards and would otherwise be useless keys).
        let path: Vec<&String> = tokens
            .iter()
            .filter(|t| *t != "<*>")
            .take(self.depth)
            .collect();
        for tok in path {
            let key: &str = tok;
            if !node.children.contains_key(key) {
                if node.children.len() >= self.max_children && key != "<*>" {
                    node = node.children.entry("<*>".to_string()).or_default();
                    continue;
                }
                node.children.insert(key.to_string(), Node::default());
            }
            node = node.children.get_mut(key).expect("just inserted");
        }
        // Best matching template at this leaf.
        let mut best: Option<(usize, f64)> = None;
        for &id in &node.clusters {
            let t = &self.templates[id];
            let same = t
                .tokens
                .iter()
                .zip(tokens)
                .filter(|(a, b)| a.matches_concretely(b))
                .count();
            // Positions that are variable on both sides say nothing about
            // whether the two lines share a shape; leave them out.
            let both_wild = t
                .tokens
                .iter()
                .zip(tokens)
                .filter(|(a, b)| **a == Token::Wild && *b == "<*>")
                .count();
            let n = n - both_wild;
            // A line that fits the template exactly (its wildcards line up
            // with the template's) is a perfect match, even when it has few
            // literal tokens. Otherwise lines made mostly of wildcards
            // would each start a template of their own.
            let exact = t.tokens.iter().zip(tokens).all(|(a, b)| match a {
                Token::Wild => b == "<*>",
                other => other.accepts(b),
            });
            let sim = if n == 0 || exact {
                1.0
            } else {
                same as f64 / n as f64
            };
            if best.map(|(_, b)| sim > b).unwrap_or(true) {
                best = Some((id, sim));
            }
        }
        let force = self.templates.len() >= self.max_templates;
        let id = match best {
            Some((id, sim))
                if sim >= self.sim_threshold || (force && !node.clusters.is_empty()) =>
            {
                let t = &mut self.templates[id];
                for (a, b) in t.tokens.iter_mut().zip(tokens) {
                    a.generalise(b);
                }
                id
            }
            _ if force => {
                // Out of template budget: lump into a catch-all per length.
                let key = n;
                match self.templates.iter().position(|t| {
                    t.tokens.len() == key && t.tokens.iter().all(|t| *t == Token::Wild)
                }) {
                    Some(id) => id,
                    None => {
                        // Degrade the most recent template at this leaf.
                        match node.clusters.last().copied() {
                            Some(id) => {
                                for a in self.templates[id].tokens.iter_mut() {
                                    *a = Token::Wild;
                                }
                                id
                            }
                            None => {
                                let id = self.templates.len();
                                self.templates.push(Template {
                                    id,
                                    format,
                                    tokens: tokens.iter().map(|_| Token::Wild).collect(),
                                    count: 0,
                                    levels: [0; Level::COUNT + 1],
                                    first: row,
                                    last: row,
                                    first_ts: ts,
                                    last_ts: ts,
                                    examples: Vec::new(),
                                });
                                node.clusters.push(id);
                                id
                            }
                        }
                    }
                }
            }
            _ => {
                let id = self.templates.len();
                self.templates.push(Template {
                    id,
                    format,
                    tokens: tokens
                        .iter()
                        .map(|t| {
                            if t == "<*>" {
                                Token::Wild
                            } else {
                                Token::Lit(t.clone())
                            }
                        })
                        .collect(),
                    count: 0,
                    levels: [0; Level::COUNT + 1],
                    first: row,
                    last: row,
                    first_ts: ts,
                    last_ts: ts,
                    examples: Vec::new(),
                });
                node.clusters.push(id);
                id
            }
        };
        let t = &mut self.templates[id];
        t.count += 1;
        t.levels[level.map(|l| l.index()).unwrap_or(Level::COUNT)] += 1;
        t.last = row;
        if ts.is_some() {
            if t.first_ts.is_none() {
                t.first_ts = ts;
            }
            t.last_ts = ts;
        }
        if t.examples.len() < 3 {
            t.examples.push(row);
        }
        id
    }
}

struct Inner {
    base: Base,
    snapshot: RwLock<Arc<Vec<Template>>>,
    processed: AtomicU64,
    stride: AtomicU64,
    complete: AtomicBool,
    cancel: AtomicBool,
    wakers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    cap: usize,
}

/// Background clustering job over a base.
#[derive(Clone)]
pub struct Clusters(Arc<Inner>);

impl std::fmt::Debug for Clusters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clusters({} templates)", self.templates().len())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Clusters {
    /// Start mining.  At most `cap` lines are processed; larger inputs are
    /// sampled with a fixed stride (reported by [`Clusters::stride`]).
    pub fn start(base: Base, cap: usize) -> Clusters {
        let inner = Arc::new(Inner {
            base,
            snapshot: RwLock::new(Arc::new(Vec::new())),
            processed: AtomicU64::new(0),
            stride: AtomicU64::new(1),
            complete: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
            cap: cap.max(1000),
        });
        let c = Clusters(inner);
        let c2 = c.clone();
        let _ = std::thread::Builder::new()
            .name("hem-cluster".into())
            .spawn(move || cluster_main(c2));
        c
    }

    /// Current templates (a cheap snapshot).
    pub fn templates(&self) -> Arc<Vec<Template>> {
        Arc::clone(&self.0.snapshot.read())
    }

    /// Lines processed so far.
    pub fn processed(&self) -> u64 {
        self.0.processed.load(Ordering::Acquire)
    }

    /// Sampling stride (1 = every line).
    pub fn stride(&self) -> u64 {
        self.0.stride.load(Ordering::Acquire)
    }

    pub fn is_sampled(&self) -> bool {
        self.stride() > 1
    }

    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> f64 {
        let total = self.0.base.len() as f64;
        if total == 0.0 {
            return 1.0;
        }
        ((self.processed() * self.stride()) as f64 / total).clamp(0.0, 1.0)
    }

    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::Release);
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        self.0.wakers.lock().push(w);
    }

    fn wake(&self) {
        for w in self.0.wakers.lock().iter() {
            w();
        }
    }
}

fn cluster_main(c: Clusters) {
    let inner = &c.0;
    let (tx, rx) = mpsc::channel::<()>();
    {
        let tx = tx.clone();
        inner.base.add_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
    }
    let default_year = crate::timestamp::current_year();
    let mut miner = Miner::new();
    let mut gen = inner.base.generation();
    let mut next: usize = 0;
    let mut stride: usize = 1;
    let mut last_publish = Instant::now();
    let mut dirty = false;

    loop {
        if inner.cancel.load(Ordering::Acquire) {
            break;
        }
        let g = inner.base.generation();
        if g != gen {
            gen = g;
            next = 0;
            miner = Miner::new();
            *inner.snapshot.write() = Arc::new(Vec::new());
            inner.processed.store(0, Ordering::Release);
        }
        // "Complete" before length; see `view::builder_main`.
        let base_done = inner.base.is_complete();
        let avail = inner.base.len();
        // Decide the stride from the first sizeable estimate of the total.
        if stride == 1 {
            let total_est = match &inner.base {
                Base::Single(s) => {
                    let indexed = s.indexed_bytes().max(1);
                    let lines = s.line_count() as u64;
                    if s.is_complete() {
                        lines
                    } else {
                        lines.saturating_mul(s.total_bytes()) / indexed
                    }
                }
                Base::Merged(_) => avail as u64,
            } as usize;
            if total_est > inner.cap {
                stride = total_est.div_ceil(inner.cap);
                inner.stride.store(stride as u64, Ordering::Release);
            }
        }
        if avail > next {
            inner.complete.store(false, Ordering::Release);
            let b = (next + CHUNK_ROWS * stride).min(avail);
            let chunk = inner.base.chunk(next, b);
            let formats = inner.base.formats();
            let mut i = 0;
            while i < chunk.len() {
                let line = chunk.line(i);
                let row = chunk.row(i);
                let fmt = formats
                    .get(row.source() as usize)
                    .copied()
                    .unwrap_or_default();
                let meta = parse_line(line, fmt, default_year);
                let toks = tokenize(line, fmt);
                if !toks.is_empty() {
                    miner.add(&toks, fmt, row, meta.level, meta.ts);
                }
                i += stride;
            }
            next = b;
            inner
                .processed
                .fetch_add((chunk.len() / stride.max(1)) as u64, Ordering::Release);
            dirty = true;
            if last_publish.elapsed() > Duration::from_millis(150) {
                *inner.snapshot.write() = Arc::new(miner.templates().to_vec());
                last_publish = Instant::now();
                dirty = false;
                c.wake();
            }
            continue;
        }
        if dirty {
            *inner.snapshot.write() = Arc::new(miner.templates().to_vec());
            dirty = false;
            c.wake();
        }
        if base_done {
            inner.complete.store(true, Ordering::Release);
            c.wake();
            if !inner.base.is_live() {
                break;
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while rx.try_recv().is_ok() {}
    }
    *inner.snapshot.write() = Arc::new(miner.templates().to_vec());
    inner.complete.store(true, Ordering::Release);
    c.wake();
}

/// Block until clustering is complete (tests / CLI).
pub fn wait_complete(c: &Clusters, timeout: Duration) -> bool {
    let start = Instant::now();
    while !c.is_complete() {
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Source;

    #[test]
    fn tokenizes_with_wildcards() {
        let t = tokenize(
            b"User bob logged in from 10.0.0.1 (attempt 3)",
            Format::Plain,
        );
        // A JSON line inside a plain file is clustered on its message.
        let t2 = tokenize(
            br#"{"level":"warn","msg":"slow query 12ms"}"#,
            Format::Plain,
        );
        assert_eq!(t2, vec!["WARN", "slow", "query", "<*>"]);
        assert_eq!(
            t,
            vec!["User", "bob", "logged", "in", "from", "<*>", "(attempt", "<*>"]
        );
        let t = tokenize(
            br#"{"level":"warn","user":"bob","msg":"slow query took 12ms"}"#,
            Format::Json,
        );
        assert_eq!(t, vec!["WARN", "slow", "query", "took", "<*>"]);
    }

    #[test]
    fn mines_templates() {
        let mut m = Miner::new();
        let r = Row::new(0, 0, 1);
        for (i, name) in ["bob", "alice", "carol", "dave"].iter().enumerate() {
            let line = format!("2026-10-13 10:00:0{i} INFO login ok: user {name} from 10.0.0.{i}");
            m.add(
                &tokenize(line.as_bytes(), Format::Plain),
                Format::Plain,
                Row::new(i as u64, 0, 1),
                Some(Level::Info),
                None,
            );
        }
        m.add(
            &tokenize(b"Disk full on /dev/sda1", Format::Plain),
            Format::Plain,
            r,
            Some(Level::Error),
            None,
        );
        assert_eq!(m.len(), 2);
        let t = &m.templates()[0];
        assert_eq!(t.display(), "<*> <*> INFO login ok: user <*> from <*>");
        assert_eq!(t.count, 4);
        assert_eq!(t.levels[Level::Info.index()], 4);
        assert!(t
            .matcher()
            .matches(b"2026-10-13 10:00:09 INFO login ok: user zed from 1.2.3.4"));
        assert!(!t
            .matcher()
            .matches(b"2026-10-13 10:00:09 INFO login failed: user zed from 1.2.3.4"));
        assert_eq!(m.templates()[1].max_level(), Some(Level::Error));
    }

    #[test]
    fn variable_words_merge_but_different_messages_do_not() {
        let mut m = Miner::new();
        let lines = [
            "2026-10-13 10:00:01.1 FATAL [search] upstream timeout user=frank duration=3ms id=req-1a",
            "2026-10-13 10:00:02.2 FATAL [search] upstream timeout user=dave duration=9ms id=req-2b",
            "2026-10-13 10:00:03.3 FATAL [search] cache hit user=frank duration=1ms id=req-3c",
        ];
        for (i, l) in lines.iter().enumerate() {
            m.add(
                &tokenize(l.as_bytes(), Format::Plain),
                Format::Plain,
                Row::new(i as u64, 0, 1),
                Some(Level::Fatal),
                None,
            );
        }
        let shown: Vec<String> = m.templates().iter().map(|t| t.display()).collect();
        assert_eq!(shown.len(), 2, "{shown:?}");
        assert!(shown[0].contains("upstream timeout user=<*>"), "{shown:?}");
        assert!(shown[1].contains("cache hit user=frank"), "{shown:?}");
    }

    #[test]
    fn all_wildcard_lines_share_one_template() {
        let mut m = Miner::new();
        for i in 0..50u64 {
            let line = format!("{} {}", 1000 + i, 0x5000 + i);
            m.add(
                &tokenize(line.as_bytes(), Format::Plain),
                Format::Plain,
                Row::new(i, 0, 1),
                None,
                None,
            );
        }
        assert_eq!(m.len(), 1);
        assert_eq!(m.templates()[0].count, 50);
    }

    #[test]
    fn background_job_and_sampling() {
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("INFO request {i} served in {}ms\n", i % 7));
        }
        for _ in 0..5 {
            text.push_str("ERROR disk full\n");
        }
        let src = Source::from_bytes("t", text.into_bytes());
        src.stop();
        let c = Clusters::start(Base::Single(src), 5000);
        assert!(wait_complete(&c, Duration::from_secs(10)));
        assert!(c.is_sampled());
        assert_eq!(c.stride(), 5);
        let t = c.templates();
        assert!(!t.is_empty());
        let top = t.iter().max_by_key(|t| t.count).unwrap();
        assert_eq!(top.display(), "INFO request <*> served in <*>");
        assert!(top.count >= 3900 && top.count <= 4001, "{}", top.count);
    }
}
