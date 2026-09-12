//! Settings loader: parse `~/.pi/agent/settings.json` + `.pi/settings.json`.
//!
//! v1 reads only a subset of Pi's settings schema. Unknown keys are tolerated
//! and not written back. Provider mirrors spec

//! Settings loader: parse `~/.pi/agent/settings.json` + `.pi/settings.json`.
//!
//! v1 reads only a subset of Pi's settings schema. Unknown keys are tolerated
//! and not written back.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Settings shape (v1 subset).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub retry: Option<RetrySettings>,
    #[serde(default)]
    pub compaction: Option<CompactionSettings>,
    #[serde(default)]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetrySettings {
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub base_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CompactionSettings {
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub reserve_tokens: Option<u32>,
}

/// Load settings from user-level (`~/.pi/agent/settings.json`) and
/// project-level (`.pi/settings.json`). Project-level overrides user-level on
/// per-field collisions.
pub fn load_settings(cwd: &Path) -> Settings {
    let user = user_settings_path();
    let project = project_settings_path(cwd);

    let mut user_s: Settings = user
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let project_s: Settings = project
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // Project overrides user
    if project_s.provider.is_some() {
        user_s.provider = project_s.provider;
    }
    if project_s.model.is_some() {
        user_s.model = project_s.model;
    }
    if project_s.thinking_level.is_some() {
        user_s.thinking_level = project_s.thinking_level;
    }
    if project_s.retry.is_some() {
        user_s.retry = project_s.retry;
    }
    if project_s.compaction.is_some() {
        user_s.compaction = project_s.compaction;
    }
    if project_s.theme.is_some() {
        user_s.theme = project_s.theme;
    }
    user_s
}

pub fn user_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".pi").join("agent").join("settings.json"))
}

pub fn project_settings_path(cwd: &Path) -> Option<PathBuf> {
    Some(cwd.join(".pi").join("settings.json"))
}

/// Load models.json (`~/.pi/agent/models.json` + `.pi/models.json`).
///
/// v1: read-only; merge provider entries (project overrides user). The full
/// schema (with `models: Vec<...>` arrays) is intentionally not implemented
/// yet — providers' built-in catalogs are used instead.
pub fn load_models_json(cwd: &Path) -> ModelsJson {
    let user = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let candidates = vec![
        user.as_ref().map(|h| h.join(".pi").join("agent").join("models.json")),
        Some(cwd.join(".pi").join("models.json")),
    ];

    let mut out = ModelsJson::default();
    for path in candidates.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<ModelsJson>(&text) {
                out.providers.extend(parsed.providers);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelsJson {
    #[serde(default)]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::tempdir;

    #[test]
    fn load_missing_files_returns_default() {
        let dir = tempdir().unwrap();
        let s = load_settings(dir.path());
        assert!(s.provider.is_none());
    }

    #[test]
    fn load_project_overrides_user() {
        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".pi");
        std::fs::create_dir_all(&pi_dir).unwrap();

        let mut f = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(
            f,
            r#"{{"provider": "openai", "model": "gpt-5"}}"#
        )
        .unwrap();

        let s = load_settings(dir.path());
        assert_eq!(s.provider.as_deref(), Some("openai"));
        assert_eq!(s.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".pi");
        std::fs::create_dir_all(&pi_dir).unwrap();
        let mut f = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(
            f,
            r#"{{"provider": "anthropic", "futureField": {{"nested": true}}, "another": [1,2,3]}}"#
        )
        .unwrap();
        let s = load_settings(dir.path());
        assert_eq!(s.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn load_models_json_merges_providers() {
        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".pi");
        std::fs::create_dir_all(&pi_dir).unwrap();
        let mut f = std::fs::File::create(pi_dir.join("models.json")).unwrap();
        writeln!(
            f,
            r#"{{"providers": {{"custom": {{"api": "openai-completions", "baseUrl": "http://x"}}}}}}"#
        )
        .unwrap();
        let m = load_models_json(dir.path());
        assert!(m.providers.contains_key("custom"));
    }
}