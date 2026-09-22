//! Composable filters: include / exclude patterns, level threshold, time
//! range, structured-field predicates and template matches.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
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

impl FieldPredicate {
    /// Parse expressions such as `status>=500`, `user=bob`, `msg~time.?out`,
    /// `err!=`, `trace_id?`.
    pub fn parse(expr: &str) -> Result<Self> {
        let expr = expr.trim();
        let bad = |m: &str| Error::FilterExpr {
            expr: expr.to_string(),
            message: m.to_string(),
        };
        // Longest operators first.
        const OPS: &[(&str, Op)] = &[
            ("!=", Op::Ne),
            ("!~", Op::NotMatch),
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("=", Op::Eq),
            ("<", Op::Lt),
            (">", Op::Gt),
            ("~", Op::Match),
        ];
        if let Some(key) = expr.strip_suffix('?') {
            if key.is_empty() {
                return Err(bad("missing key"));
            }
            return Ok(Self {
                key: key.to_string(),
                op: Op::Exists,
                value: String::new(),
                number: None,
                regex: None,
            });
        }
        let mut found: Option<(usize, &str, Op)> = None;
        for (tok, op) in OPS {
            if let Some(pos) = expr.find(tok) {
                if pos == 0 {
                    continue;
                }
                match found {
                    Some((p, _, _)) if p <= pos => {}
                    _ => found = Some((pos, tok, *op)),
                }
            }
        }
        let Some((pos, tok, op)) = found else {
            return Err(bad(
                "expected key=value, key!=value, key~regex, key<n, key>=n or key?",
            ));
        };
        let key = expr[..pos].trim().to_string();
        let value = expr[pos + tok.len()..].trim().to_string();
        if key.is_empty() {
            return Err(bad("missing key"));
        }
        let regex = match op {
            Op::Match | Op::NotMatch => Some(
                regex::RegexBuilder::new(&value)
                    .case_insensitive(!value.chars().any(|c| c.is_uppercase()))
                    .build()
                    .map_err(|e| Error::Regex {
                        pattern: value.clone(),
                        message: e.to_string(),
                    })?,
            ),
            _ => None,
        };
        let number = value.parse::<f64>().ok();
        Ok(Self {
            key,
            op,
            value,
            number,
            regex,
        })
    }

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

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty()
            && self.exclude.is_empty()
            && self.min_level.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.fields.is_empty()
            && self.template.is_none()
    }

    /// Whether evaluating this filter needs timestamp / level metadata.
    pub fn needs_meta(&self) -> bool {
        self.min_level.is_some() || self.since.is_some() || self.until.is_some()
    }

}
