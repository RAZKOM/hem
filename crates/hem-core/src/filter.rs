//! Composable filters: include / exclude patterns, level threshold, time
//! range, structured-field predicates and template matches.

use serde::{Deserialize, Serialize};

use crate::format::Format;
use crate::level::Level;

/// A compiled text pattern.
#[derive(Debug, Clone)]
pub struct Matcher {
    pub text: String,
    pub regex: regex::bytes::Regex,
    pub literal: bool,
}

/// Comparison operator in a field predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Regex match.
    Match,
    /// Regex non-match.
    NotMatch,
    /// Key exists.
    Exists,
}

/// `key op value` predicate over structured fields.
#[derive(Debug, Clone)]
pub struct FieldPredicate {
    pub key: String,
    pub op: Op,
    pub value: String,
    number: Option<f64>,
    regex: Option<regex::Regex>,
}

/// A template token: literal text or a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    Lit(String),
    Wild,
}

/// A mined template, usable as a filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateMatch {
    pub tokens: Vec<Token>,
    pub format: Format,
}

/// Serialisable description of a filter (what the user typed / configured).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilterSpec {
    /// Every include pattern must match (regex unless `literal`).
    pub include: Vec<String>,
    /// No exclude pattern may match.
    pub exclude: Vec<String>,
    /// Minimum level.
    pub level: Option<Level>,
    /// Lower time bound (any supported timestamp, or relative like `15m`).
    pub since: Option<String>,
    /// Upper time bound.
    pub until: Option<String>,
    /// Field predicates such as `status>=500`.
    pub fields: Vec<String>,
    /// Treat include/exclude patterns as literal text.
    pub literal: bool,
}

impl FilterSpec {
    pub fn is_empty(&self) -> bool {
        *self == FilterSpec::default()
    }
}

/// A compiled filter.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub include: Vec<Matcher>,
    pub exclude: Vec<Matcher>,
    pub min_level: Option<Level>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub fields: Vec<FieldPredicate>,
    pub template: Option<TemplateMatch>,
    /// Lines sharing this exact value (e.g. a request id), matched as a
    /// literal substring.
    pub highlight_value: Option<Matcher>,
    pub spec: FilterSpec,
}

