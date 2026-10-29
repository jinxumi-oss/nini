//! Settings loader: parse `~/.nini/agent/settings.json` + `.pi/settings.json`.
//!
//! v1 reads only a subset of Pi's settings schema. Unknown keys are tolerated
//! and not written back. Provider mirrors spec

//! Settings loader: parse `~/.nini/agent/settings.json` + `.pi/settings.json`.
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
    /// Pi and `nini-core` spell this `model`; `nini-tui::Settings` writes
    /// `defaultModel`. `/model` therefore persisted into a field nothing
    /// read - the same drift as `thinking_level`, one level up.
    #[serde(default, alias = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Pi persists this as `defaultThinkingLevel`
    /// (`pi-coding-agent/src/core/settings-manager.ts:753`).
    #[serde(default, alias = "defaultThinkingLevel")]
    pub default_thinking_level: Option<String>,
    /// nini's earlier spelling. Written by nothing - `/thinking` has always
    /// written `default_thinking_level` - and read here only so a settings
    /// file from an older nini is not silently ignored.
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub retry: Option<RetrySettings>,
    #[serde(default)]
    pub compaction: Option<CompactionSettings>,
    #[serde(default)]
    pub theme: Option<String>,
    /// When the transcript scrollbar is painted.
    ///
    /// Pi: `settings-manager.ts:139` —
    /// `fullscreenScrollbar?: ScrollViewScrollbar; // default: "auto"`.
    /// Feeds `ScrollView.isScrollbarVisible` (`scroll-view.ts:71-77`).
    /// Anything unrecognised coerces to `auto`, matching
    /// `getFullscreenScrollbar()` (`settings-manager.ts:1151-1154`).
    #[serde(default, alias = "fullscreenScrollbar")]
    pub fullscreen_scrollbar: Option<String>,
    /// Turn cap for one agent run. `0` / absent = no cap.
    ///
    /// v0.11.4 (P0/Pi parity): nini used to stop at 50 turns, a ceiling
    /// Pi 0.85.1 does not have, and it truncated long tasks before they
    /// delivered. The default is now "no cap"; this field is how a user
    /// puts a ceiling back. `maxIterations` is accepted as an alias
    /// because that is the camelCase spelling Pi's own settings use.
    #[serde(default, alias = "maxIterations")]
    pub max_iterations: Option<usize>,
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

/// Load settings from user-level (`~/.nini/agent/settings.json`) and
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
    if project_s.default_model.is_some() {
        user_s.default_model = project_s.default_model;
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
    if project_s.max_iterations.is_some() {
        user_s.max_iterations = project_s.max_iterations;
    }
    user_s
}

pub fn user_settings_path() -> Option<PathBuf> {
    crate::paths::agent_file("settings.json")
}

pub fn project_settings_path(cwd: &Path) -> Option<PathBuf> {
    Some(crate::paths::project_file(cwd, "settings.json"))
}

/// Load models.json (`~/.nini/agent/models.json` + `.pi/models.json`).
///
/// v1: read-only; merge provider entries (project overrides user). The full
/// schema (with `models: Vec<...>` arrays) is intentionally not implemented
/// yet — providers' built-in catalogs are used instead.
pub fn load_models_json(cwd: &Path) -> ModelsJson {
    let candidates = crate::paths::candidates(cwd, "models.json");

    let mut out = ModelsJson::default();
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<ModelsJson>(&text) {
                out.providers.extend(parsed.providers);
            }
        }
    }
    out
}

/// Load the **user-level** `models.json` — the file in the agent
/// directory, and nothing else.
///
/// This is the only `models.json` a provider may be **built** from.
/// The project-level file (`<cwd>/.nini/models.json`) is still read by
/// `load_models_json` for the model catalogue, but a provider entry in
/// it never reaches a request: Pi 0.85.1 has no project-level
/// `models.json` at all, and a directory a user merely cloned must not
/// be able to hand nini a base URL and a credential — least of all a
/// `!command` to run.
pub fn load_user_models_json() -> ModelsJson {
    let mut out = ModelsJson::default();
    if let Some(path) = crate::paths::agent_file("models.json") {
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
    /// Where the provider's requests go.
    ///
    /// Pi writes `baseUrl` (its `ProviderConfigSchema`), and that is the
    /// spelling every real `models.json` uses. The alias is not a
    /// convenience: without it serde dropped the key on the floor and
    /// nini had no idea where the user's provider lives.
    #[serde(default, alias = "baseUrl")]
    pub base_url: Option<String>,
    /// The credential the provider's requests carry.
    ///
    /// Pi writes `apiKey`; same story as `baseUrl` — the key was read
    /// from nobody, so a provider that declared one looked
    /// unauthenticated.
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,
    /// The provider's display name.
    ///
    /// Pi's `ProviderConfigSchema` has `name?`. It is carried into the
    /// struct so the file is not silently half-read, but it never
    /// appears in a rejection message: a refusal names the provider id
    /// and the missing item, nothing else.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub id: Option<String>,
    /// The context window, in nini's spelling.
    ///
    /// Pi writes `contextWindow` (its `ModelMetadataSchema`), and that is
    /// the spelling every real `models.json` on disk uses. The alias is not
    /// a convenience: without it nini parsed Pi's file and got **nothing**
    /// — `context_window` stayed `None`, so every model the user declared
    /// was silently ignored, and the window nini used was the 200 000
    /// fallback rather than the number the user had written down.
    #[serde(default, alias = "contextWindow")]
    pub context_window: Option<u32>,
    /// Output-token ceiling, in Pi's spelling.
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: Option<u32>,
    /// Whether the model does extended thinking.
    #[serde(default, alias = "reasoning")]
    pub reasoning: Option<bool>,
    /// Input modalities ("text", "image").
    #[serde(default)]
    pub input: Option<Vec<String>>,
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
        let pi_dir = dir.path().join(".nini");
        std::fs::create_dir_all(&pi_dir).unwrap();

        let mut f = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(f, r#"{{"provider": "openai", "model": "gpt-5"}}"#).unwrap();

        let s = load_settings(dir.path());
        assert_eq!(s.provider.as_deref(), Some("openai"));
        assert_eq!(s.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".nini");
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
        let pi_dir = dir.path().join(".nini");
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
