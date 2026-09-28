//! TOML configuration: theme, keymap overrides, custom formats, presets and
//! default behaviours.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
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

}
