//! Saved sessions: the files of a hem session plus each tab's filters,
//! pins, search, highlight and position, so `hem --session NAME` (or
//! `hem --resume`) picks up where the last run left off.
//!
//! Lines are referred to by file path and line number.  Each file is
//! fingerprinted (size and a hash of its first bytes) so a file that was
//! rotated or rewritten since does not get pins on unrelated lines.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use hem_core::filter::{key_of, FilterSpec, TemplateMatch, Token};
use hem_core::format::Format;
use serde::{Deserialize, Serialize};

/// The session every quit saves to, reopened by `hem --resume`.
pub const LAST: &str = "last";
/// Key of the merged tab in [`TabState::key`].
pub const MERGED: &str = "merged";
/// Bytes hashed from the start of each file.
const HEAD_BYTES: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub version: u32,
    /// When it was saved (UTC, informational).
    pub saved: String,
    /// The named session this state belongs to, so `hem --resume` keeps
    /// saving to it.  Empty for unnamed runs.
    pub name: String,
    pub merge: bool,
    /// Key of the active tab.
    pub active: String,
    pub files: Vec<FileEntry>,
    pub tabs: Vec<TabState>,
}

/// A file and its fingerprint at save time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    /// Bytes covered by `head`.
    pub head_len: u64,
    /// FNV-1a hash of the first `head_len` bytes, in hex.
    pub head: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    /// File path, or [`MERGED`].
    pub key: String,
    pub filter: FilterSpec,
    /// Template filter tokens as displayed (`<*>`, `key=<*>`, literals).
    pub template: Vec<String>,
    pub template_format: Option<Format>,
    pub collapse: bool,
    pub wrap: bool,
    pub columns: bool,
    pub follow: bool,
    pub search: String,
    pub highlight: String,
    pub cursor: Option<LineRef>,
    pub pins: Vec<LineRef>,
}

/// A line of a file (0-based line number).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRef {
    pub file: String,
    pub line: u64,
}

impl TabState {
    pub fn set_template(&mut self, t: Option<&TemplateMatch>) {
        self.template = t
            .map(|t| t.tokens.iter().map(|k| k.display().into_owned()).collect())
            .unwrap_or_default();
        self.template_format = t.map(|t| t.format);
    }

    pub fn template(&self) -> Option<TemplateMatch> {
        if self.template.is_empty() {
            return None;
        }
        let tokens = self
            .template
            .iter()
            .map(|s| {
                if s == "<*>" {
                    Token::Wild
                } else if let Some(k) = s
                    .strip_suffix("=<*>")
                    .filter(|k| key_of(s).is_some() && !k.is_empty())
                {
                    Token::Key(k.to_string())
                } else {
                    Token::Lit(s.clone())
                }
            })
            .collect();
        Some(TemplateMatch {
            tokens,
            format: self.template_format.unwrap_or_default(),
        })
    }
}

impl Session {
    pub fn tab(&self, key: &str) -> Option<&TabState> {
        self.tabs.iter().find(|t| t.key == key)
    }

    /// Paths whose file no longer matches its fingerprint (rotated,
    /// truncated or rewritten since the session was saved).
    pub fn changed_files(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| !f.still_matches())
            .map(|f| f.path.clone())
            .collect()
    }
}

impl FileEntry {
    pub fn new(path: &str) -> FileEntry {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let head_len = size.min(HEAD_BYTES as u64);
        let head = head_hash(Path::new(path), head_len as usize).unwrap_or_default();
        FileEntry {
            path: path.to_string(),
            size,
            head_len,
            head,
        }
    }

    /// Does the file still start the same and is it at least as long?
    pub fn still_matches(&self) -> bool {
        let Ok(m) = std::fs::metadata(&self.path) else {
            return false;
        };
        m.len() >= self.size
            && head_hash(Path::new(&self.path), self.head_len as usize).as_deref()
                == Some(self.head.as_str())
    }
}

fn head_hash(path: &Path, len: usize) -> Option<String> {
    let mut buf = vec![0u8; len];
    let mut f = std::fs::File::open(path).ok()?;
    f.read_exact(&mut buf).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in buf {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(format!("{h:016x}"))
}

/// Absolute form of a path as given, for use as a session key.  Not
/// canonicalised: on Windows that adds a `\\?\` prefix.
pub fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Where sessions are stored: `%LOCALAPPDATA%\hem\sessions` on Windows,
/// `$XDG_STATE_HOME/hem/sessions` (default `~/.local/state/hem/sessions`)
/// elsewhere.
pub fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HEM_SESSION_DIR") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(|a| PathBuf::from(a).join("hem").join("sessions"))
    } else {
        directories::BaseDirs::new().map(|d| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| d.home_dir().join(".local").join("state"))
                .join("hem")
                .join("sessions")
        })
    }
}

/// Session names are file names: letters, digits, `.`, `_` and `-`.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(anyhow!(
            "bad session name {name:?}: use letters, digits, '.', '_' and '-'"
        ))
    }
}

fn path_for(name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let d = dir().ok_or_else(|| anyhow!("no directory to store sessions in"))?;
    Ok(d.join(format!("{name}.toml")))
}

/// Load a session; `Ok(None)` when it does not exist.
pub fn load(name: &str) -> Result<Option<Session>> {
    let path = path_for(name)?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading session {}", path.display()))?;
    let s: Session =
        toml::from_str(&text).map_err(|e| anyhow!("{}: {}", path.display(), e.message()))?;
    Ok(Some(s))
}

pub fn save(name: &str, session: &Session) -> Result<PathBuf> {
    let path = path_for(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut s = session.clone();
    s.version = 1;
    s.saved = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let text = toml::to_string(&s).map_err(|e| anyhow!("serialising session: {e}"))?;
    // Write then rename so a crash mid-write keeps the old session.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn delete(name: &str) -> Result<bool> {
    let path = path_for(name)?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    Ok(true)
}

/// Saved sessions as `(name, session)`, most recently saved first.
pub fn list() -> Result<Vec<(String, Session)>> {
    let Some(d) = dir() else {
        return Ok(Vec::new());
    };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return Ok(Vec::new());
    };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("toml") {
            continue;
        }
        let Some(name) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(Some(s)) = load(name) {
            v.push((name.to_string(), s));
        }
    }
    v.sort_by(|a, b| b.1.saved.cmp(&a.1.saved).then(a.0.cmp(&b.0)));
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_round_trips_through_text() {
        let t = TemplateMatch {
            tokens: vec![
                Token::Lit("GET".into()),
                Token::Wild,
                Token::Key("status".into()),
            ],
            format: Format::Logfmt,
        };
        let mut st = TabState::default();
        st.set_template(Some(&t));
        assert_eq!(st.template, ["GET", "<*>", "status=<*>"]);
        assert_eq!(st.template(), Some(t));
        st.set_template(None);
        assert_eq!(st.template(), None);
    }

    #[test]
    fn session_round_trips_through_toml() {
        let s = Session {
            version: 1,
            saved: "2026-10-15 10:00:00".into(),
            name: "incident".into(),
            merge: true,
            active: MERGED.into(),
            files: vec![FileEntry {
                path: "/var/log/a.log".into(),
                size: 10,
                head_len: 10,
                head: "00ff".into(),
            }],
            tabs: vec![TabState {
                key: MERGED.into(),
                filter: FilterSpec {
                    include: vec!["timeout".into()],
                    level: Some(hem_core::Level::Warn),
                    since: Some("2026-10-13 10:00:00.000".into()),
                    ..Default::default()
                },
                template: vec!["GET".into(), "<*>".into()],
                template_format: Some(Format::Plain),
                pins: vec![LineRef {
                    file: "/var/log/a.log".into(),
                    line: 3,
                }],
                cursor: Some(LineRef {
                    file: "/var/log/a.log".into(),
                    line: 7,
                }),
                search: "reset".into(),
                ..Default::default()
            }],
        };
        let text = toml::to_string(&s).unwrap();
        let back: Session = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn names_are_checked() {
        assert!(check_name("incident-42_v2.b").is_ok());
        for bad in ["", "../x", "a/b", ".hidden", "a b"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fingerprint_notices_rewrites_but_not_appends() {
        let dir = std::env::temp_dir().join(format!("hem-session-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.log");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let path = p.to_string_lossy().into_owned();
        let e = FileEntry::new(&path);
        assert!(e.still_matches());
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        assert!(e.still_matches(), "appending keeps line numbers valid");
        std::fs::write(&p, "uno\ndos\ntres\n").unwrap();
        assert!(!e.still_matches(), "a rewritten file does not match");
        std::fs::write(&p, "one\n").unwrap();
        assert!(!e.still_matches(), "a truncated file does not match");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
