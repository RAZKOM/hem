//! TOML configuration: theme, keymap overrides, custom formats, presets and
//! default behaviours.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use hem_core::filter::FilterSpec;
use hem_core::timestamp::TsDisplay;
use serde::{Deserialize, Serialize};

use crate::keymap::KeySpec;

/// Default behaviours.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    /// Collapse repeated lines.
    pub collapse: bool,
    /// Wrap long lines.
    pub wrap: bool,
    /// Start in follow mode.
    pub follow: bool,
    /// Timestamp display: original, time, iso, local, relative.
    pub timestamp: TsDisplay,
    /// Use ASCII box drawing instead of Unicode.
    pub ascii: bool,
    /// Enable mouse support.
    pub mouse: bool,
    /// Show structured fields as columns for JSON / logfmt sources.
    pub columns: bool,
    /// Show the timeline minimap.
    pub minimap: bool,
    /// Lines sampled for template clustering (0 = unlimited).
    pub cluster_cap: usize,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            collapse: true,
            wrap: false,
            follow: false,
            timestamp: TsDisplay::Original,
            ascii: false,
            mouse: true,
            columns: false,
            minimap: true,
            cluster_cap: 2_000_000,
        }
    }
}

/// A user-defined named format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CustomFormat {
    /// Base format: plain, json, logfmt, syslog, nginx.
    pub base: Option<String>,
    /// Regex whose first capture group is the level.
    pub level_regex: Option<String>,
    /// Regex whose first capture group is the timestamp text.
    pub timestamp_regex: Option<String>,
}

/// The whole config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Theme name: dark or light.
    pub theme: Option<String>,
    /// Colour overrides by name (see README).
    pub colors: BTreeMap<String, String>,
    /// Key overrides: action = "key" | ["key", ...].
    pub keys: BTreeMap<String, KeySpec>,
    /// Default behaviours.
    pub defaults: Defaults,
    /// Saved filter presets.
    pub presets: BTreeMap<String, FilterSpec>,
    /// Custom named formats, selectable with `--format <name>`.
    pub formats: BTreeMap<String, CustomFormat>,
}

impl Config {
    /// Platform config path: `%APPDATA%\hem\config.toml` on Windows,
    /// `~/.config/hem/config.toml` elsewhere.
    pub fn default_path() -> Option<PathBuf> {
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("hem").join("config.toml"))
        } else {
            directories::BaseDirs::new().map(|d| {
                let base = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| d.home_dir().join(".config"));
                base.join("hem").join("config.toml")
            })
        }
    }

    /// Load from `path` (or the default path).  A missing file yields the
    /// defaults; a malformed file is an error naming the offending key.
    pub fn load(path: Option<&Path>) -> Result<(Config, Option<PathBuf>)> {
        let path = match path {
            Some(p) => Some(p.to_path_buf()),
            None => Self::default_path(),
        };
        let Some(path) = path else {
            return Ok((Config::default(), None));
        };
        if !path.exists() {
            return Ok((Config::default(), Some(path)));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg = Self::parse(&text).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        Ok((cfg, Some(path)))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            // toml's message already names the key / expected fields; add
            // the line for easy navigation.
            let loc = e
                .span()
                .map(|s| {
                    let line = text[..s.start.min(text.len())].matches('\n').count() + 1;
                    format!(" (line {line})")
                })
                .unwrap_or_default();
            anyhow!("invalid config{loc}: {}", e.message())
        })?;
        for (name, spec) in &cfg.presets {
            hem_core::filter::Filter::from_spec(spec)
                .map_err(|e| anyhow!("invalid config: presets.{name}: {e}"))?;
        }
        for (name, f) in &cfg.formats {
            if let Some(b) = &f.base {
                b.parse::<hem_core::format::Format>()
                    .map_err(|e| anyhow!("invalid config: formats.{name}.base: {e}"))?;
            }
            for (k, r) in [
                ("level_regex", &f.level_regex),
                ("timestamp_regex", &f.timestamp_regex),
            ] {
                if let Some(r) = r {
                    regex::bytes::Regex::new(r)
                        .map_err(|e| anyhow!("invalid config: formats.{name}.{k}: {e}"))?;
                }
            }
        }
        if let Some(t) = &cfg.theme {
            if crate::theme::Theme::by_name(t).is_none() {
                return Err(anyhow!(
                    "invalid config: theme = {t:?} (expected one of: {})",
                    crate::theme::Theme::names().join(", ")
                ));
            }
        }
        Ok(cfg)
    }

    /// Serialise and write to `path`, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialising config")?;
        let header = "# hem configuration. See README.md for every key.\n\n";
        std::fs::write(path, format!("{header}{text}"))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let text = r##"
theme = "light"

[colors]
error = "#ff0000"

[keys]
next_error = ["ctrl+n", "F8"]
quit = "ctrl+q"

[defaults]
collapse = false
timestamp = "time"

[presets.errors]
level = "warn"
include = ["timeout"]

[formats.myapp]
base = "plain"
level_regex = "\\[(\\w+)\\]"
"##;
        let cfg = Config::parse(text).unwrap();
        assert_eq!(cfg.theme.as_deref(), Some("light"));
        assert!(!cfg.defaults.collapse);
        assert_eq!(cfg.defaults.timestamp, TsDisplay::Time);
        assert_eq!(cfg.presets["errors"].level, Some(hem_core::Level::Warn));
        assert_eq!(
            cfg.keys["next_error"],
            KeySpec::Many(vec!["ctrl+n".into(), "F8".into()])
        );
    }

    #[test]
    fn unknown_key_is_named() {
        let err = Config::parse("[defaults]\ncollaps = true\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("collaps"), "{err}");
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn bad_values_are_named() {
        let err = Config::parse("[presets.x]\ninclude = [\"(\"]\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("presets.x"), "{err}");
        let err = Config::parse("theme = \"neon\"\n").unwrap_err().to_string();
        assert!(err.contains("theme"), "{err}");
        let err = Config::parse("[defaults]\ntimestamp = \"weird\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("timestamp") || err.contains("unknown variant"),
            "{err}"
        );
    }

    #[test]
    fn roundtrip_save() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("config.toml");
        let mut cfg = Config {
            theme: Some("light".into()),
            ..Default::default()
        };
        cfg.defaults.wrap = true;
        cfg.save(&p).unwrap();
        let (back, _) = Config::load(Some(&p)).unwrap();
        assert_eq!(back, cfg);
    }
}
