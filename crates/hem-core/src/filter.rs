//! Composable filters: include / exclude patterns, level threshold, time
//! range, structured-field predicates and template matches.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fields;
use crate::format::Format;
use crate::level::Level;
use crate::parse::LineMeta;

/// A compiled text pattern.
#[derive(Debug, Clone)]
pub struct Matcher {
    pub text: String,
    pub regex: regex::bytes::Regex,
    pub literal: bool,
}

impl Matcher {
    #[inline]
    pub fn is_match(&self, line: &[u8]) -> bool {
        self.regex.is_match(line)
    }
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

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Match => "~",
            Op::NotMatch => "!~",
            Op::Exists => "?",
        }
    }
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

    pub fn matches(&self, line: &[u8], format: Format) -> bool {
        let Some(v) = fields::get(line, format, &self.key) else {
            return matches!(self.op, Op::Ne | Op::NotMatch) && !self.value.is_empty();
        };
        let v: &str = &v;
        match self.op {
            Op::Exists => true,
            Op::Eq => cmp_eq(v, &self.value, self.number),
            Op::Ne => !cmp_eq(v, &self.value, self.number),
            Op::Match => self.regex.as_ref().map(|r| r.is_match(v)).unwrap_or(false),
            Op::NotMatch => self.regex.as_ref().map(|r| !r.is_match(v)).unwrap_or(false),
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let ord = match (v.parse::<f64>().ok(), self.number) {
                    (Some(a), Some(b)) => a.partial_cmp(&b),
                    _ => Some(v.cmp(self.value.as_str())),
                };
                let Some(ord) = ord else { return false };
                match self.op {
                    Op::Lt => ord.is_lt(),
                    Op::Le => ord.is_le(),
                    Op::Gt => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
        }
    }

    pub fn display(&self) -> String {
        if self.op == Op::Exists {
            format!("{}?", self.key)
        } else {
            format!("{}{}{}", self.key, self.op.as_str(), self.value)
        }
    }
}

fn cmp_eq(v: &str, want: &str, want_num: Option<f64>) -> bool {
    if v == want {
        return true;
    }
    if let (Some(n), Ok(m)) = (want_num, v.parse::<f64>()) {
        return n == m;
    }
    v.eq_ignore_ascii_case(want)
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

impl TemplateMatch {
    pub fn display(&self) -> String {
        let parts: Vec<&str> = self
            .tokens
            .iter()
            .map(|t| match t {
                Token::Lit(s) => s.as_str(),
                Token::Wild => "<*>",
            })
            .collect();
        parts.join(" ")
    }

    pub fn matches(&self, line: &[u8]) -> bool {
        let toks = crate::cluster::tokenize(line, self.format);
        if toks.len() != self.tokens.len() {
            return false;
        }
        toks.iter().zip(&self.tokens).all(|(a, b)| match b {
            Token::Wild => true,
            Token::Lit(s) => a == s,
        })
    }
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

    /// Number of active criteria (for the status bar).
    pub fn count(&self) -> usize {
        self.include.len()
            + self.exclude.len()
            + usize::from(self.min_level.is_some())
            + usize::from(self.since.is_some() || self.until.is_some())
            + self.fields.len()
            + usize::from(self.template.is_some())
    }

    /// Evaluate against a line.  `meta` must already have inherited values
    /// from preceding lines (see [`crate::view`]).
    pub fn matches(&self, line: &[u8], meta: &LineMeta, format: Format) -> bool {
        if let Some(min) = self.min_level {
            match meta.level {
                Some(l) if l >= min => {}
                _ => return false,
            }
        }
        if self.since.is_some() || self.until.is_some() {
            let Some(ts) = meta.ts else { return false };
            if let Some(s) = self.since {
                if ts < s {
                    return false;
                }
            }
            if let Some(u) = self.until {
                if ts > u {
                    return false;
                }
            }
        }
        for m in &self.exclude {
            if m.is_match(line) {
                return false;
            }
        }
        for m in &self.include {
            if !m.is_match(line) {
                return false;
            }
        }
        for p in &self.fields {
            if !p.matches(line, format) {
                return false;
            }
        }
        if let Some(t) = &self.template {
            if !t.matches(line) {
                return false;
            }
        }
        true
    }
}
