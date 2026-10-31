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
    /// because it is nini's own camelCase spelling — **Pi 0.85.1 has no
    /// such key**; this is nini's knob, not a Pi setting.
    #[serde(default, alias = "maxIterations")]
    pub max_iterations: Option<usize>,
    /// v0.14: ceiling on tool calls running at once. `0` / absent = no
    /// ceiling.
    ///
    /// Pi 0.85.1 runs a turn's tool calls all at once
    /// (`references/pi-0.85.1/src/loop/agent-loop.ts:547-549`), so the
    /// default is "no ceiling"; this key is how a user puts one back.
    /// `maxParallelTools` is accepted as an alias because it is nini's
    /// own camelCase spelling, following `maxIterations` — **Pi 0.85.1
    /// has no such key** (nor a `maxIterations` one); this is nini's
    /// knob, not a Pi setting.
    ///
    /// Known limit: the **user-level** file's key is wiped when the TUI
    /// saves its settings (`crates/nini-tui/src/settings.rs` flushes
    /// only its own `Settings` struct) — the same thing that already
    /// happens to `maxIterations`. The project-level
    /// `.nini/settings.json` is not written back and is unaffected.
    #[serde(default, alias = "maxParallelTools")]
    pub max_parallel_tools: Option<usize>,
    /// v0.15: the batch-level tool execution mode.
    ///
    /// **This is nini's own key.** Pi 0.85.1's settings schema has **no**
    /// such key — nini is not reading a Pi setting here. What it does
    /// correspond to is Pi's **SDK** option `config.toolExecution`, whose
    /// decision site is
    /// `references/pi-0.85.1/src/loop/agent-loop.ts:420`
    /// (`config.toolExecution === "sequential"` → the whole batch runs one
    /// call at a time). The SDK's default is `"parallel"`
    /// (`node_modules/@earendil-works/pi-agent-core/dist/agent.js:134` —
    /// **not in the snapshot**: the snapshot only carries the
    /// `pi-coding-agent` package, so that coordinate points at the
    /// installed package). Pi's CLI never passes the option.
    /// Saying "this is Pi's camelCase spelling" would be false — it is
    /// nini's key, with Pi's semantics.
    ///
    /// **The canonical spelling is snake_case** (`tool_execution`), like
    /// every other key of this struct; `toolExecution` is accepted as an
    /// **alias**, the same way `maxIterations` / `maxParallelTools` /
    /// `fullscreenScrollbar` are. Both spellings are read, so a user who
    /// writes either one gets the mode they asked for.
    ///
    /// The values are `"parallel"` and `"sequential"`, lower case, the
    /// same two values Pi's `ToolExecutionMode` has
    /// (`references/spec-v0.85.1/session/types.ts:192`:
    /// `toolExecution: "sequential" | "parallel";`). The default is
    /// `parallel`, which is what both sides do without a declaration.
    ///
    /// Known limit: like `maxIterations`, one illegal value makes the
    /// **whole file** be ignored — the loader does not read half a file.
    ///
    /// Known limit: the **user-level** file's key is wiped when the TUI
    /// saves its settings (`crates/nini-tui/src/settings.rs` flushes
    /// only its own `Settings` struct, which has no `tool_execution`
    /// field) — the same thing that already happens to `maxIterations`.
    /// The project-level `.nini/settings.json` is not written back and is
    /// unaffected.
    #[serde(default, alias = "toolExecution")]
    pub tool_execution: Option<crate::tool::ToolExecutionMode>,
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
/// project-level (`.nini/settings.json`). Project-level overrides user-level on
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
    if project_s.max_parallel_tools.is_some() {
        user_s.max_parallel_tools = project_s.max_parallel_tools;
    }
    if project_s.tool_execution.is_some() {
        user_s.tool_execution = project_s.tool_execution;
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
            // The same parse the user-level file goes through: a file
            // with a comment in it is a file Pi reads, so it is a file
            // nini reads too — **in both** places nini reads one.
            if let Ok(parsed) = parse_models_json(&text) {
                out.providers.extend(parsed.providers);
            }
        }
    }
    out
}

/// Parse the text of a `models.json`, the way Pi parses it.
///
/// Pi: `JSON.parse(stripJsonComments(stripBom(content)))`
/// (`model-config.js:230`). A user who writes a comment in their
/// `models.json` has a working setup in Pi; the same file must not be
/// "Failed to parse models.json" here.
fn parse_models_json(text: &str) -> Result<ModelsJson, serde_json::Error> {
    serde_json::from_str(&strip_json_comments(strip_bom(text)))
}

/// Strip a UTF-8 byte order mark.
///
/// Pi: `stripBom` (`dist/utils/text.js`), applied before the JSON is
/// parsed (`model-config.js:230`).
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Strip `//` line comments and trailing commas, leaving string
/// literals untouched.
///
/// Pi: `stripJsonComments` (`dist/utils/json.js`):
///
/// ```text
/// input.replace(/"(?:\\.|[^"\\])*"|\/\/[^\n]*/g, m => m[0] === '"' ? m : "")
///      .replace(/"(?:\\.|[^"\\])*"|,(\s*[}\]])/g, (m, tail) => tail ?? (m[0] === '"' ? m : ""))
/// ```
///
/// A file Pi accepts must be a file nini accepts: a user who writes a
/// comment in their `models.json` gets a working setup in Pi and a
/// "Failed to parse models.json" in nini otherwise.
///
/// The pattern is Pi's, translated: a string literal (with escapes) is
/// matched first and kept whole, and only what is **not** inside one is
/// treated as a comment or as a trailing comma.
fn strip_json_comments(text: &str) -> String {
    // Step 1: drop `//` line comments.
    let bytes = text.as_bytes();
    let mut without_line_comments = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let end = string_literal_end(bytes, index);
            without_line_comments.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let ch = text[index..].chars().next().expect("a char");
        without_line_comments.push(ch);
        index += ch.len_utf8();
    }

    // Step 2: drop a comma that is only there because of how the file was
    // written — a comma directly before `}` or `]`.
    let bytes = without_line_comments.as_bytes();
    let mut out = String::with_capacity(without_line_comments.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let end = string_literal_end(bytes, index);
            out.push_str(&without_line_comments[index..end]);
            index = end;
            continue;
        }
        if bytes[index] == b',' {
            let mut next = index + 1;
            while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                next += 1;
            }
            if next < bytes.len() && (bytes[next] == b'}' || bytes[next] == b']') {
                // The comma goes; what follows it stays.
                index += 1;
                continue;
            }
        }
        let ch = without_line_comments[index..]
            .chars()
            .next()
            .expect("a char");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// The index just past the string literal that starts at `start`.
///
/// A backslash escapes the next byte, so `"a\\"` ends at the second
/// quote, not the first.
fn string_literal_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
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
    load_user_models_json_reporting().0
}

/// Load the user-level `models.json`, and say why it could not be read.
///
/// Pi does not swallow a broken file: `model-config.js:226/233/240` turns
/// a read error, a parse error and a schema error into three different
/// messages, and the user is shown them. nini used to drop the file on
/// the floor, which left a user staring at a provider that silently was
/// not there.
///
/// The message is Pi's, minus the `File: <path>` tail (nini prints the
/// path in its own header).
pub fn load_user_models_json_reporting() -> (ModelsJson, Option<String>) {
    let mut out = ModelsJson::default();
    let Some(path) = crate::paths::agent_file("models.json") else {
        // Pi: a file that is not there is an empty configuration, not an
        // error (`model-config.js:215-224`).
        return (out, None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            // Pi: a file that is **not there** is an empty
            // configuration, not an error (`model-config.js:215-224`).
            // A file that is there and cannot be read is an error
            // (`model-config.js:226`).
            if error.kind() == std::io::ErrorKind::NotFound {
                return (out, None);
            }
            return (out, Some(format!("Failed to load models.json: {error}")));
        }
    };
    match parse_models_json(&text) {
        Ok(parsed) => {
            out.providers.extend(parsed.providers);
            (out, None)
        }
        Err(error) => (out, Some(format!("Failed to parse models.json: {error}"))),
    }
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

    /// v0.14: the tool-concurrency ceiling is readable from the settings
    /// file under nini's camelCase spelling, and a project-level file
    /// overrides the user-level one — exactly like `maxIterations`.
    ///
    /// One test, because `NINI_AGENT_DIR` is process-wide: two tests
    /// redirecting it would see each other's files.
    #[test]
    fn max_parallel_tools_is_read_from_the_camel_case_key() {
        let guard = crate::paths::agent_lock();

        // The user-level file alone.
        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"maxParallelTools": 3}}"#).unwrap();

        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".nini");
        std::fs::create_dir_all(&pi_dir).unwrap();

        let s = load_settings(dir.path());
        assert_eq!(
            s.max_parallel_tools,
            Some(3),
            "the user-level `maxParallelTools` key was not read"
        );

        // A project-level file overrides it.
        let mut pf = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"maxParallelTools": 5}}"#).unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.max_parallel_tools,
            Some(5),
            "the project-level file must override the user-level one"
        );

        // The key is absent: no ceiling from the file, so the default
        // (no ceiling) stands.
        std::fs::remove_file(pi_dir.join("settings.json")).unwrap();
        std::fs::remove_file(guard.path().join("settings.json")).unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.max_parallel_tools, None,
            "a settings file without the key must not invent a ceiling"
        );
    }

    /// v0.14: an explicit `0` in the settings file means "no ceiling".
    ///
    /// It must not be read as "the key is absent" (which falls back to the
    /// default) — the file said what it meant, and `0` is a value.
    #[test]
    fn max_parallel_tools_zero_in_the_file_is_read_as_zero() {
        let guard = crate::paths::agent_lock();

        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"maxParallelTools": 0}}"#).unwrap();

        let dir = tempdir().unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.max_parallel_tools,
            Some(0),
            "an explicit `0` in the settings file was not read as `0`"
        );
    }

    /// v0.15: the batch-level execution mode is readable from the
    /// settings file under the key `toolExecution`, at the user level and
    /// at the project level, the project level winning.
    ///
    /// One test, because `NINI_AGENT_DIR` is process-wide: two tests
    /// redirecting it would see each other's files.
    #[test]
    fn tool_execution_is_read_from_the_tool_execution_key() {
        let guard = crate::paths::agent_lock();

        // The user-level file alone.
        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"toolExecution": "sequential"}}"#).unwrap();

        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".nini");
        std::fs::create_dir_all(&pi_dir).unwrap();

        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution,
            Some(crate::tool::ToolExecutionMode::Sequential),
            "the user-level `toolExecution` key was not read"
        );

        // A project-level file overrides it.
        let mut pf = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"toolExecution": "parallel"}}"#).unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution,
            Some(crate::tool::ToolExecutionMode::Parallel),
            "the project-level file must override the user-level one"
        );

        // The key is absent: no mode from the file, so the default
        // (parallel) stands.
        std::fs::remove_file(pi_dir.join("settings.json")).unwrap();
        std::fs::remove_file(guard.path().join("settings.json")).unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution, None,
            "a settings file without the key must not invent a mode"
        );
    }

    /// v0.15 (审查修正): the **canonical** spelling of the key is
    /// snake_case, like every other key of this struct; the camelCase
    /// spelling is an **alias**.
    ///
    /// The key used to be `rename = "toolExecution"`, i.e. the camelCase
    /// spelling was the *only* one that worked. A user who followed the
    /// repository's own convention (snake_case, like `max_parallel_tools`
    /// / `fullscreen_scrollbar`) wrote `"tool_execution": "sequential"`
    /// and got **silently ignored** — no error, no warning, the mode
    /// quietly fell back to `parallel`.
    ///
    /// Both spellings must work; the canonical one is the snake_case.
    #[test]
    fn both_spellings_of_the_tool_execution_key_are_read() {
        let snake: Settings =
            serde_json::from_str(r#"{"tool_execution": "sequential"}"#).unwrap();
        assert_eq!(
            snake.tool_execution,
            Some(crate::tool::ToolExecutionMode::Sequential),
            "the canonical snake_case spelling must be read"
        );

        let camel: Settings =
            serde_json::from_str(r#"{"toolExecution": "sequential"}"#).unwrap();
        assert_eq!(
            camel.tool_execution,
            Some(crate::tool::ToolExecutionMode::Sequential),
            "the camelCase alias must be read too"
        );
    }

    /// v0.15 (审查修正): the canonical spelling must work through the
    /// **whole** loader, not only through serde directly.
    ///
    /// The file-level path is the one a user actually goes through, and it
    /// is a different function (`load_settings`) than the one the previous
    /// test calls.
    #[test]
    fn the_snake_case_spelling_is_read_from_the_settings_file() {
        let guard = crate::paths::agent_lock();

        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"tool_execution": "sequential"}}"#).unwrap();

        let dir = tempdir().unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution,
            Some(crate::tool::ToolExecutionMode::Sequential),
            "the canonical snake_case key was not read from the settings file"
        );
    }

    /// v0.15 (审查修正): an illegal value in the **project-level** file
    /// leaves the **user-level** value in force.
    ///
    /// The loader ignores a file it cannot parse — that is the existing
    /// behaviour (the same one `maxIterations` has). The consequence for
    /// the project level is that the user-level value is *not* replaced by
    /// nothing: it keeps applying. This is the semantics nobody had
    /// pinned down, so it is pinned down here.
    #[test]
    fn an_illegal_project_level_value_leaves_the_user_level_value_in_force() {
        let guard = crate::paths::agent_lock();

        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"tool_execution": "sequential"}}"#).unwrap();

        let dir = tempdir().unwrap();
        let pi_dir = dir.path().join(".nini");
        std::fs::create_dir_all(&pi_dir).unwrap();
        let mut pf = std::fs::File::create(pi_dir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"tool_execution": "bogus"}}"#).unwrap();

        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution,
            Some(crate::tool::ToolExecutionMode::Sequential),
            "an illegal project-level value must not wipe the user-level value"
        );
    }

    /// v0.15 (审查修正): the known limit — "the TUI's settings flush wipes
    /// this key" — must be **written down** next to the key.
    ///
    /// `crates/nini-tui/src/settings.rs` persists a `Settings` struct that
    /// has no `tool_execution` field, so every TUI settings write drops
    /// the key from the user-level file. The neighbouring
    /// `max_parallel_tools` records the very same limit; the new key
    /// shipped without it, which is how a documented knob disappears
    /// without anybody noticing.
    ///
    /// A comment cannot be asserted at runtime, so this asserts the
    /// **source text**: the doc comment of the field must name the TUI
    /// flush and the wiping.
    #[test]
    fn the_tool_execution_key_records_the_tui_flush_limit() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/settings.rs"))
            .expect("read settings.rs");
        let field_at = source
            .find("pub tool_execution")
            .expect("the tool_execution field exists");
        let doc_start = source[..field_at]
            .rfind("/// v0.15")
            .expect("the field's doc comment starts with its version");
        let doc = &source[doc_start..field_at];
        assert!(
            doc.contains("TUI") && doc.contains("flush") && doc.contains("wiped"),
            "the tool_execution doc comment must record the known limit that the \
             TUI's settings flush wipes the key (crates/nini-tui/src/settings.rs) — \
             found: {doc}"
        );
    }

    /// v0.15: an illegal value makes the **whole file** be ignored.
    ///
    /// That is the existing behaviour of the settings loader (the same
    /// one `maxIterations` has): a file that does not parse is a file
    /// nini does not read, and it does not read half of it either. This
    /// round does not make an exception for the new key.
    ///
    /// The witness is `theme`: the very same file carries a legal
    /// `theme` value, and it must not be read either.
    #[test]
    fn an_illegal_tool_execution_value_ignores_the_whole_file() {
        let guard = crate::paths::agent_lock();

        let mut uf = std::fs::File::create(guard.path().join("settings.json")).unwrap();
        writeln!(uf, r#"{{"theme": "solarized", "toolExecution": "bogus"}}"#).unwrap();

        let dir = tempdir().unwrap();
        let s = load_settings(dir.path());
        assert_eq!(
            s.tool_execution, None,
            "an illegal `toolExecution` value must not be read"
        );
        assert_eq!(
            s.theme.as_deref(),
            None,
            "an illegal value must not make the loader read half of the file"
        );
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
