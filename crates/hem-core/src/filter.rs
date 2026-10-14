//! Composable filters: include / exclude patterns, level threshold, time
//! range, structured-field predicates and template matches.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fields;
use crate::format::Format;
use crate::level::Level;
use crate::parse::LineMeta;
use crate::timestamp;

/// A compiled text pattern.
#[derive(Debug, Clone)]
pub struct Matcher {
    pub text: String,
    pub regex: regex::bytes::Regex,
    pub literal: bool,
}

impl Matcher {
    /// Compile a pattern.  `literal` escapes the text.  Case sensitivity is
    /// "smart": a pattern with no upper-case letters matches case-
    /// insensitively.
    pub fn new(text: &str, literal: bool) -> Result<Self> {
        let smart_ci = !text.chars().any(|c| c.is_uppercase());
        let body = if literal {
            regex::escape(text)
        } else {
            text.to_string()
        };
        let pattern = if smart_ci {
            format!("(?i){body}")
        } else {
            body
        };
        let regex = regex::bytes::RegexBuilder::new(&pattern)
            .size_limit(16 * 1024 * 1024)
            .dfa_size_limit(8 * 1024 * 1024)
            .build()
            .map_err(|e| Error::Regex {
                pattern: text.to_string(),
                message: e.to_string(),
            })?;
        Ok(Self {
            text: text.to_string(),
            regex,
            literal,
        })
    }

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
        let format = format.for_line(line);
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

/// A template token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    /// Literal text.
    Lit(String),
    /// A `key=value` pair whose value varies, shown as `key=<*>`.
    Key(String),
    /// Anything.
    Wild,
}

/// The key of a `key=value` token, if it is one.
pub fn key_of(tok: &str) -> Option<&str> {
    let p = tok.find('=')?;
    let key = &tok[..p];
    let ok = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@'));
    ok.then_some(key)
}

impl Token {
    /// Does a line token fit this template token?
    pub fn accepts(&self, tok: &str) -> bool {
        match self {
            Token::Wild => true,
            Token::Lit(s) => s == tok,
            Token::Key(k) => key_of(tok) == Some(k.as_str()),
        }
    }

    /// Does a line token match this one on something concrete (a literal
    /// or a key)? Wildcards don't count.
    pub fn matches_concretely(&self, tok: &str) -> bool {
        !matches!(self, Token::Wild) && self.accepts(tok)
    }

    /// Loosen this token just enough to also accept `tok`.
    pub fn generalise(&mut self, tok: &str) {
        if self.accepts(tok) {
            return;
        }
        *self = match self {
            Token::Lit(s) => match (key_of(s), key_of(tok)) {
                (Some(a), Some(b)) if a == b => Token::Key(a.to_string()),
                _ => Token::Wild,
            },
            _ => Token::Wild,
        };
    }

    pub fn display(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Token::Lit(s) => std::borrow::Cow::Borrowed(s),
            Token::Key(k) => std::borrow::Cow::Owned(format!("{k}=<*>")),
            Token::Wild => std::borrow::Cow::Borrowed("<*>"),
        }
    }
}

/// A mined template, usable as a filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateMatch {
    pub tokens: Vec<Token>,
    pub format: Format,
}

impl TemplateMatch {
    pub fn display(&self) -> String {
        let parts: Vec<std::borrow::Cow<'_, str>> =
            self.tokens.iter().map(|t| t.display()).collect();
        parts.join(" ")
    }

    pub fn matches(&self, line: &[u8]) -> bool {
        let toks = crate::cluster::tokenize(line, self.format);
        if toks.len() != self.tokens.len() {
            return false;
        }
        toks.iter().zip(&self.tokens).all(|(a, b)| b.accepts(a))
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
    pub fn from_spec(spec: &FilterSpec) -> Result<Self> {
        let mut f = Filter {
            spec: spec.clone(),
            ..Default::default()
        };
        for p in &spec.include {
            f.include.push(Matcher::new(p, spec.literal)?);
        }
        for p in &spec.exclude {
            f.exclude.push(Matcher::new(p, spec.literal)?);
        }
        f.min_level = spec.level;
        if let Some(s) = &spec.since {
            f.since = Some(timestamp::parse_user(s).ok_or_else(|| Error::BadTime(s.clone()))?);
        }
        if let Some(s) = &spec.until {
            f.until = Some(timestamp::parse_user(s).ok_or_else(|| Error::BadTime(s.clone()))?);
        }
        for e in &spec.fields {
            f.fields.push(FieldPredicate::parse(e)?);
        }
        Ok(f)
    }

    /// Rebuild the spec from the compiled parts (after programmatic edits).
    pub fn sync_spec(&mut self) {
        self.spec.include = self.include.iter().map(|m| m.text.clone()).collect();
        self.spec.exclude = self.exclude.iter().map(|m| m.text.clone()).collect();
        self.spec.level = self.min_level;
        self.spec.fields = self.fields.iter().map(|p| p.display()).collect();
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicates() {
        let p = FieldPredicate::parse("status>=500").unwrap();
        assert!(p.matches(br#"{"status":503}"#, Format::Json));
        assert!(!p.matches(br#"{"status":200}"#, Format::Json));
        assert!(!p.matches(br#"{"x":1}"#, Format::Json));
        let p = FieldPredicate::parse("user=bob").unwrap();
        assert!(p.matches(b"ts=1 user=bob", Format::Logfmt));
        assert!(p.matches(b"ts=1 user=Bob", Format::Logfmt));
        assert!(!p.matches(b"ts=1 user=alice", Format::Logfmt));
        let p = FieldPredicate::parse("msg~time.?out").unwrap();
        assert!(p.matches(br#"{"msg":"Timeout!"}"#, Format::Json));
        let p = FieldPredicate::parse("trace_id?").unwrap();
        assert!(p.matches(br#"{"trace_id":"x"}"#, Format::Json));
        assert!(!p.matches(br#"{"y":"x"}"#, Format::Json));
        let p = FieldPredicate::parse("user!=bob").unwrap();
        assert!(p.matches(br#"{"user":"al"}"#, Format::Json));
        assert!(
            p.matches(br#"{"x":1}"#, Format::Json),
            "missing field != value"
        );
        assert!(FieldPredicate::parse("nonsense").is_err());
        assert!(FieldPredicate::parse("=x").is_err());
    }

    #[test]
    fn composed_filter() {
        let spec = FilterSpec {
            include: vec!["timeout".into()],
            exclude: vec!["ignored".into()],
            level: Some(Level::Warn),
            ..Default::default()
        };
        let f = Filter::from_spec(&spec).unwrap();
        let warn = LineMeta {
            level: Some(Level::Warn),
            ..Default::default()
        };
        let info = LineMeta {
            level: Some(Level::Info),
            ..Default::default()
        };
        assert!(f.matches(b"WARN request Timeout", &warn, Format::Plain));
        assert!(!f.matches(b"WARN request Timeout ignored", &warn, Format::Plain));
        assert!(!f.matches(b"INFO request timeout", &info, Format::Plain));
        assert!(!f.matches(b"WARN ok", &warn, Format::Plain));
        assert!(!f.matches(b"x", &LineMeta::default(), Format::Plain));
        assert_eq!(f.count(), 3);
    }

    #[test]
    fn smart_case_and_literal() {
        let m = Matcher::new("error", false).unwrap();
        assert!(m.is_match(b"ERROR"));
        let m = Matcher::new("Error", false).unwrap();
        assert!(!m.is_match(b"ERROR"));
        let m = Matcher::new("a.b", true).unwrap();
        assert!(!m.is_match(b"axb"));
        assert!(m.is_match(b"A.B"));
        assert!(Matcher::new("(", false).is_err());
    }

    #[test]
    fn time_range() {
        let spec = FilterSpec {
            since: Some("2026-10-13T10:00:00Z".into()),
            until: Some("2026-10-13T11:00:00Z".into()),
            ..Default::default()
        };
        let f = Filter::from_spec(&spec).unwrap();
        let inside = LineMeta {
            ts: Some(timestamp::civil_to_ms(2026, 10, 13, 10, 30, 0, 0)),
            ..Default::default()
        };
        let outside = LineMeta {
            ts: Some(timestamp::civil_to_ms(2026, 10, 13, 12, 0, 0, 0)),
            ..Default::default()
        };
        assert!(f.matches(b"x", &inside, Format::Plain));
        assert!(!f.matches(b"x", &outside, Format::Plain));
        assert!(!f.matches(b"x", &LineMeta::default(), Format::Plain));
        assert!(Filter::from_spec(&FilterSpec {
            since: Some("never".into()),
            ..Default::default()
        })
        .is_err());
    }
}
