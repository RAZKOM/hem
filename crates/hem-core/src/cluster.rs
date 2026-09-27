//! Template clustering with a Drain-style streaming miner.
//!
//! Lines are normalised (numbers / ids masked), tokenised on whitespace and
//! routed through a fixed-depth prefix tree keyed by token count and the
//! first few tokens.  At each leaf the most similar existing template is
//! updated (differing positions become `<*>`) or a new template is created.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use std::borrow::Cow;

use crate::collapse::normalize;
use crate::fields;
use crate::filter::Token;
use crate::format::Format;
use crate::level::Level;
use crate::parse::parse_line;
use crate::view::{Base, Row};

const MAX_TOKENS: usize = 48;
const MAX_TOKEN_LEN: usize = 48;

/// Fields that carry the human-readable message in structured logs.
const MSG_KEYS: &[&str] = &["msg", "message", "event", "error", "err", "text", "log"];

/// Tokenise a line for clustering.  Structured lines (JSON / logfmt) are
/// clustered on their level + message field so that keys and quoted
/// values do not swallow the whole line.
pub fn tokenize(line: &[u8], format: Format) -> Vec<String> {
    let line = &line[..line.len().min(4096)];
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
                "<*>".to_string()
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
                .filter(|(a, b)| matches!(a, Token::Lit(s) if s == *b))
                .count();
            let sim = if n == 0 { 1.0 } else { same as f64 / n as f64 };
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
                    if matches!(a, Token::Lit(s) if s != b) {
                        *a = Token::Wild;
                    }
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
    /// Current templates (a cheap snapshot).
    pub fn templates(&self) -> Arc<Vec<Template>> {
        Arc::clone(&self.0.snapshot.read())
    }

    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::Acquire)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_with_wildcards() {
        let t = tokenize(
            b"User bob logged in from 10.0.0.1 (attempt 3)",
            Format::Plain,
        );
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

}
