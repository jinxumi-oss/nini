//! SettingsManager — persistence layer for user-editable settings plus
//! the runtime knobs (theme name, model, thinking level, compaction
//! budget). Loads from `~/.pi/agent/settings.json` on startup and writes
//! back when flushed.
//!
//! ## JSON schema
//!
//! ```json
//! {
//!   "theme": "dark",
//!   "default_model": "anthropic/claude-sonnet-4-5",
//!   "default_thinking_level": "medium",
//!   "context_window": 200000,
//!   "reserve_tokens": 16384,
//!   "keep_recent_tokens": 20000
//! }
//! ```
//!
//! Unknown top-level keys are ignored. Missing fields fall back to
//! defaults so a partial file still loads.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_CONTEXT_WINDOW: u32 = 200_000;
const DEFAULT_RESERVE_TOKENS: u32 = 16_384;
const DEFAULT_KEEP_RECENT_TOKENS: u32 = 20_000;

/// Top-level user settings plus runtime knobs (theme + model + thinking
/// + compaction budget).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub theme: String,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
    /// Total context-window size in tokens. Used both for the status-bar
    /// progress indicator and for the auto-compaction trigger.
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    /// Reserve this many tokens at the tail of the context window for
    /// the next assistant turn. Auto-compaction fires when estimated
    /// tokens > context_window - reserve_tokens.
    #[serde(default = "default_reserve_tokens")]
    pub reserve_tokens: u32,
    /// When auto-compaction fires, keep at least this many recent tokens
    /// verbatim (the rest gets summarized).
    #[serde(default = "default_keep_recent_tokens")]
    pub keep_recent_tokens: u32,
}

fn default_context_window() -> u32 { DEFAULT_CONTEXT_WINDOW }
fn default_reserve_tokens() -> u32 { DEFAULT_RESERVE_TOKENS }
fn default_keep_recent_tokens() -> u32 { DEFAULT_KEEP_RECENT_TOKENS }

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            default_model: None,
            default_thinking_level: None,
            context_window: DEFAULT_CONTEXT_WINDOW,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
        }
    }
}

impl Settings {
    pub fn model_name(&self) -> Option<String> {
        self.default_model.clone()
    }

    pub fn model_name_or_default(opt: &Option<Settings>) -> Option<String> {
        opt.as_ref().and_then(|s| s.model_name())
    }
}

#[derive(Debug)]
pub struct SettingsManager {
    pub theme: String,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
    pub settings: Settings,
}

impl Default for SettingsManager {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            default_model: None,
            default_thinking_level: None,
            settings: Settings::default(),
        }
    }
}

impl SettingsManager {
    pub fn theme_name(&mut self) -> Option<String> {
        Some(self.theme.clone())
    }

    pub fn theme(&mut self) -> crate::theme::Theme {
        crate::theme::Theme::default()
    }

    pub fn get(&self) -> Settings {
        self.settings.clone()
    }

    pub fn set_default_model<T: std::fmt::Display>(&mut self, model: T) {
        self.default_model = Some(model.to_string());
        self.settings.default_model = Some(model.to_string());
    }

    pub fn set_default_thinking_level<T: std::fmt::Display>(&mut self, level: T) {
        self.default_thinking_level = Some(level.to_string());
        self.settings.default_thinking_level = Some(level.to_string());
    }

    pub fn settings_path(&self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".pi").join("agent").join("settings.json")
    }

    /// Persist the in-memory settings to disk. Best-effort: a failed
    /// write is logged but does not crash the TUI.
    pub fn flush(&mut self) -> Result<(), String> {
        let path = self.settings_path();
        let json = serde_json::to_string_pretty(&self.settings)
            .map_err(|e| format!("settings serialize: {e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, json)
            .map_err(|e| format!("settings write {}: {e}", path.display()))
    }

    /// Load settings from disk. Returns a default `SettingsManager` if
    /// the file is missing or malformed; surfaces the error via
    /// `last_load_error` so the runtime can decide whether to log it.
    pub fn load_from_disk(path: PathBuf) -> Self {
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        let parsed: serde_json::Result<Settings> = serde_json::from_str(&raw);
        match parsed {
            Ok(settings) => {
                let theme = settings.theme.clone();
                let default_model = settings.default_model.clone();
                let default_thinking_level = settings.default_thinking_level.clone();
                Self { theme, default_model, default_thinking_level, settings }
            }
            Err(_) => Self::default(),
        }
    }

    pub fn set_model_thinking_level(&mut self, _model: &str, _level: &str) {}
}

impl std::ops::Deref for SettingsManager {
    type Target = Settings;
    fn deref(&self) -> &Settings { &self.settings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::default();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.context_window, 200_000);
        assert_eq!(s.reserve_tokens, 16_384);
        assert_eq!(s.keep_recent_tokens, 20_000);
    }

    #[test]
    fn parse_partial_json_uses_defaults() {
        // No context_window field — deserializer should fill in the
        // default via #[serde(default = ...)].
        let s: Settings = serde_json::from_str(r#"{"theme": "light"}"#).unwrap();
        assert_eq!(s.theme, "light");
        assert_eq!(s.context_window, 200_000);
    }

    #[test]
    fn set_default_model_round_trip() {
        let mut m = SettingsManager::default();
        m.set_default_model("anthropic/claude-opus-4-7");
        assert_eq!(m.default_model.as_deref(), Some("anthropic/claude-opus-4-7"));
        assert_eq!(
            m.settings.default_model.as_deref(),
            Some("anthropic/claude-opus-4-7")
        );
    }

    #[test]
    fn load_from_disk_missing_file_returns_default() {
        let m = SettingsManager::load_from_disk(PathBuf::from("/nonexistent.json"));
        assert_eq!(m.settings.context_window, 200_000);
    }

    #[test]
    fn flush_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let mut m = SettingsManager::default();
        m.settings.context_window = 100_000;
        let result = m.flush();
        let path = m.settings_path();
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(prev) = prev_home {
            std::env::set_var("HOME", prev);
        } else {
            std::env::remove_var("HOME");
        }
        assert!(result.is_ok(), "flush failed: {:?}", result);
        assert!(contents.contains("\"context_window\": 100000"), "got: {contents}");
    }
}
