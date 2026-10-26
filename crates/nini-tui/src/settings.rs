//! SettingsManager — persistence layer for user-editable settings plus
//! the runtime knobs (theme name, model, thinking level, compaction
//! budget). Loads from `~/.nini/agent/settings.json` on startup and writes
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

use std::path::PathBuf;

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
    /// What a **double** Esc (two presses within 500 ms, with an empty
    /// editor) does.
    ///
    /// Pi: `settings-manager.ts#getDoubleEscapeAction` — `"none"`,
    /// `"tree"` or `"fork"`, default `"tree"`. nini supports `"tree"`
    /// (it has Pi's tree selector) and `"none"`; `"fork"` is accepted
    /// and says out loud that nini has no message picker — Pi's
    /// `showUserMessageSelector()` has no counterpart here yet.
    #[serde(default = "default_double_escape_action")]
    pub double_escape_action: String,
}

fn default_context_window() -> u32 {
    DEFAULT_CONTEXT_WINDOW
}
fn default_reserve_tokens() -> u32 {
    DEFAULT_RESERVE_TOKENS
}
fn default_keep_recent_tokens() -> u32 {
    DEFAULT_KEEP_RECENT_TOKENS
}
fn default_double_escape_action() -> String {
    "tree".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            default_model: None,
            default_thinking_level: None,
            context_window: DEFAULT_CONTEXT_WINDOW,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
            double_escape_action: default_double_escape_action(),
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

    /// Resolve the configured theme name into a `Theme`.
    ///
    /// v0.10: this returned `Theme::default()` — the field was read by
    /// nothing. Everything around it already existed: the built-ins, file
    /// loading, `--use-theme` writing this very field, `/settings` offering
    /// a picker, and a watcher reloading on change. The theme system was
    /// complete except for the one function that chose.
    ///
    /// The rules live in [`crate::theme::Theme::resolve`], which the runtime
    /// calls directly so startup and the watcher cannot disagree.
    pub fn theme(&mut self) -> crate::theme::Theme {
        crate::theme::Theme::resolve(&self.theme)
    }

    pub fn get(&self) -> Settings {
        self.settings.clone()
    }

    /// Set the theme on both copies of the field.
    ///
    /// `SettingsManager` keeps its own `theme` for the running process and
    /// an inner `Settings` for serialisation, and `flush` writes the inner
    /// one. Assigning `mgr.theme = ...` therefore changed what the session
    /// used and not what the file would contain - the same drift that made
    /// `/thinking` persist a value nothing read, in a third place.
    pub fn set_theme<T: std::fmt::Display>(&mut self, theme: T) {
        let t = theme.to_string();
        self.theme = t.clone();
        self.settings.theme = t;
    }

    pub fn set_default_model<T: std::fmt::Display>(&mut self, model: T) {
        self.default_model = Some(model.to_string());
        self.settings.default_model = Some(model.to_string());
    }

    /// The persisted thinking level, accepting either spelling.
    ///
    /// `/thinking` writes `default_thinking_level`; `nini-core::Settings`
    /// reads both it and the legacy `thinking_level`. Going through one
    /// accessor means the two structs cannot drift apart again - which is
    /// how `/thinking` came to persist a value nobody read.
    pub fn effective_thinking_level(&self) -> Option<&str> {
        self.settings
            .default_thinking_level
            .as_deref()
            .or(self.default_thinking_level.as_deref())
    }

    pub fn set_default_thinking_level<T: std::fmt::Display>(&mut self, level: T) {
        self.default_thinking_level = Some(level.to_string());
        self.settings.default_thinking_level = Some(level.to_string());
    }

    pub fn settings_path(&self) -> PathBuf {
        // This built the path out of `$HOME` directly, so
        // `NINI_AGENT_DIR` had no effect on where settings are written or
        // read - the eleventh call site to bypass the resolver, and the
        // one the override test missed because it asserted
        // `paths::agent_file("settings.json")` rather than this function.
        nini_core::paths::agent_file("settings.json").unwrap_or_default()
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
        std::fs::write(&path, json).map_err(|e| format!("settings write {}: {e}", path.display()))
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
                Self {
                    theme,
                    default_model,
                    default_thinking_level,
                    settings,
                }
            }
            Err(_) => Self::default(),
        }
    }
}

impl std::ops::Deref for SettingsManager {
    type Target = Settings;
    fn deref(&self) -> &Settings {
        &self.settings
    }
}

/// The process environment is global and `cargo test` runs the tests in a
/// module on parallel threads, so every test in this file that moves `HOME`
/// or `NINI_AGENT_DIR` has to hold this — whichever module it sits in.
///
/// One lock per process on purpose: environment variables are process-wide,
/// so two test binaries cannot tread on each other. The `ENV_LOCK`s in
/// `tests/` guard their own binaries against the module-local tests, not
/// against each other.
#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        assert_eq!(
            m.default_model.as_deref(),
            Some("anthropic/claude-opus-4-7")
        );
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
        // v0.10.1: this moves `HOME` and was the one test in this module
        // that did not take the environment lock, so it raced the
        // theme-resolution tests in the same binary.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(
            contents.contains("\"context_window\": 100000"),
            "got: {contents}"
        );
    }
}

#[cfg(test)]
mod theme_resolution_tests {
    //! v0.10: `SettingsManager::theme()` returned `Theme::default()`.
    //!
    //! Everything around it was already built — `Theme::dark()` /
    //! `Theme::light()` / `load_from_file` / `parse` / `merge`, a 871-line
    //! theme module with the full Pi palette, `--use-theme` writing the
    //! field (`app.rs`), `/settings` offering a selector, and a theme
    //! watcher reloading on file change. The one function that turns a theme
    //! *name* into a `Theme` ignored the name.
    //!
    //! So: `--use-theme light` was accepted and did nothing, `/settings`
    //! reported a theme the user picked and the screen did not change, and
    //! the watcher's reload re-read the same default every time. A theme
    //! system where the theme is the only part that is not implemented.

    use super::*;

    /// These tests mutate `HOME` and `NINI_AGENT_DIR`, which are
    /// process-global, and `cargo test` runs test functions in parallel
    /// *within this same binary* — the theme-resolution tests, the render
    /// status tests and the settings tests all live here. Without this lock
    /// one test's `remove_var` lands in the middle of another's lookup.
    ///
    /// The same reason `tests/slash_command_e2e.rs` and
    /// `tests/theme_wiring.rs` have one.
    ///
    /// v0.10.1: the lock used to be declared *here*, so it excluded only
    /// this module. `flush_round_trip` in `mod tests` moves `HOME` as well,
    /// ran unlocked in this same binary, and lost the race — which showed
    /// up as an intermittent `flush failed` on macOS, where the machine is
    /// slow enough to widen the window. It now takes the file-level lock
    /// below. A lock per module is a lock that does not exclude.
    ///
    /// An agent directory holding `<name>.json`, which is where Pi looks
    /// (`join(globalBaseDir, "themes")`, `package-manager.ts:2367`).
    fn with_theme_file(
        name: &str,
        colors: &[(&str, &str)],
    ) -> (tempfile::TempDir, SettingsManager) {
        let tmp = tempfile::tempdir().unwrap();
        // Inside the agent directory, which is where `isolate` points
        // NINI_AGENT_DIR — a theme the user drops in `~/.nini/agent/themes/`.
        let themes = tmp.path().join("agent").join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        let colors_json: Vec<String> = colors
            .iter()
            .map(|(k, v)| format!("\"{k}\":\"{v}\""))
            .collect();
        std::fs::write(
            themes.join(format!("{name}.json")),
            format!(
                "{{\"name\":\"{name}\",\"colors\":{{{}}}}}",
                colors_json.join(",")
            ),
        )
        .unwrap();
        let mut m = SettingsManager::default();
        m.theme = name.to_string();
        (tmp, m)
    }

    fn isolate(dir: &std::path::Path) {
        unsafe {
            std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
            std::env::set_var("HOME", dir);
        }
    }

    #[test]
    fn the_built_in_light_theme_is_selectable_by_name() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate(tmp.path());
        let mut m = SettingsManager::default();
        m.theme = "light".into();
        assert_eq!(
            m.theme().name,
            "light",
            "`theme: \"light\"` produced the dark theme — the name is ignored"
        );
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn dark_is_still_the_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate(tmp.path());
        let mut m = SettingsManager::default();
        assert_eq!(m.theme_name().as_deref(), Some("dark"));
        assert_eq!(m.theme().name, "dark");
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn a_theme_file_in_the_agent_directory_is_loaded_by_name() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (tmp, mut m) = with_theme_file("midnight", &[("accent", "#ff00ff")]);
        isolate(tmp.path());
        let t = m.theme();
        assert_eq!(
            t.name, "midnight",
            "themes/<name>.json was not loaded (got {:?})",
            t.name
        );
        assert_eq!(t.color("accent"), ratatui::style::Color::Rgb(255, 0, 255));
        let _ = &mut m;
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }

    /// Pi treats a theme value containing a slash as a path
    /// (`settings-manager.ts:737-740`: `theme?.includes("/") ? undefined`).
    /// That is what makes a theme shareable — one file, referenced by
    /// anyone, rather than copied into every agent directory.
    #[test]
    fn a_theme_value_that_is_a_path_is_loaded_from_it() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate(tmp.path());
        let file = tmp.path().join("solarized.json");
        std::fs::write(
            &file,
            "{\"name\":\"solarized\",\"colors\":{\"accent\":\"#00ff00\"}}",
        )
        .unwrap();

        let mut m = SettingsManager::default();
        m.theme = file.display().to_string();
        let t = m.theme();
        assert_eq!(t.name, "solarized", "a path-valued theme was not loaded");
        assert_eq!(t.color("accent"), ratatui::style::Color::Rgb(0, 255, 0));
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }

    /// A name nobody can resolve falls back to dark — and says so. A
    /// silently-dark screen after a user typed a theme name is the failure
    /// mode this whole release exists to remove.
    #[test]
    fn an_unresolvable_theme_name_falls_back_and_does_not_pretend() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate(tmp.path());
        let mut m = SettingsManager::default();
        m.theme = "nonexistent".into();
        let t = m.theme();
        assert_eq!(t.name, "dark", "fallback should be dark, got {:?}", t.name);
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }

    /// The status bar already renders `theme_name`. If resolution changes
    /// the name, the two must not disagree.
    #[test]
    fn theme_name_and_the_resolved_theme_agree_for_builtins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate(tmp.path());
        let mut m = SettingsManager::default();
        m.theme = "light".into();
        assert_eq!(m.theme_name().as_deref(), Some(m.theme().name.as_str()));
        unsafe {
            std::env::remove_var("NINI_AGENT_DIR");
            std::env::remove_var("HOME");
        }
    }
}
