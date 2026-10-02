//! v0.8.2: System-prompt assembly.
//!
//! Two pure functions that turn a `Settings` snapshot into strings
//! the agent uses for routing (model) and identity (system prompt).
//!
//! Extracted from `main.rs` so the logic can be unit-tested without
//! spinning up the full CLI. Both functions are pure — same input,
//! same output — so they're trivially testable.

/// Pick the model ID to put in `RunConfig.model`. Honors the user's
/// `settings.model`, falling back to `settings.provider`, then
/// `"test-model"`.
pub(crate) fn model_for_cfg(s: &nini_core::settings::Settings) -> String {
    s.model
        .clone()
        .or_else(|| s.provider.clone())
        .unwrap_or_else(|| "test-model".to_string())
}

/// Build the system prompt for an agent run. Combines the agent
/// identity (Pi-compatible Rust coding agent), the user's default
/// provider/model/thinking-level from `Settings`, the current working
/// directory (so the model uses relative paths for `read`/`grep`/
/// `find`/`write`/`edit` instead of guessing absolute paths), the
/// tool snippets+guidelines (v0.8.6 — was hardcoded before), and
/// any skills prompt produced by the runtime.
pub(crate) fn settings_to_system_prompt(
    settings: &nini_core::settings::Settings,
    cwd: &std::path::Path,
    tools: &nini_core::tool::ToolRegistry,
    skills_prompt: &str,
) -> String {
    let mut s = String::from("You are nini, a Pi-compatible Rust coding agent.\n");
    // v0.8.5: surface the working directory so the model uses relative
    // paths for read/grep/find/write/edit. Without this, models often
    // guess `/home/<user>/<file>` for project files — which fails
    // when the project lives at `/home/<user>/<project>/<file>`.
    // bash already runs in cwd, but the explicit-path tools need this
    // hint to compose correct relative paths.
    s.push_str(&format!("\nWorking directory: {}\n", cwd.display()));
    s.push_str("All relative paths in tool calls are resolved against this directory.\n");
    if let Some(p) = &settings.provider {
        s.push_str(&format!("Default provider: {p}\n"));
    }
    if let Some(m) = &settings.model {
        s.push_str(&format!("Default model: {m}\n"));
    }
    if let Some(t) = &settings.thinking_level {
        s.push_str(&format!("Thinking level: {t}\n"));
    }
    // v0.8.6: v0.7.1's system_prompt_contribution() was never wired up.
    // Append each registered tool's snippet + guidelines (sorted by name
    // for stable token-cache hits). The hardcoded
    // "Available tools: bash, read, write, edit, grep, find" line is
    // replaced with Pi-style "## Tool self-descriptions" + "## Tool usage
    // guidelines" sections built from the live registry.
    let mut s = nini_core::tool::build_system_prompt_with_contributions(Some(&s), tools)
        .unwrap_or(s);
    s.push_str(skills_prompt);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use nini_core::settings::Settings;
use nini_core::tool::ToolRegistry;

    #[test]
    fn model_for_cfg_prefers_model_over_provider() {
        let mut s = Settings::default();
        s.model = Some("claude-opus".into());
        s.provider = Some("anthropic".into());
        assert_eq!(model_for_cfg(&s), "claude-opus");
    }

    #[test]
    fn model_for_cfg_falls_back_to_provider() {
        let mut s = Settings::default();
        s.model = None;
        s.provider = Some("openai".into());
        assert_eq!(model_for_cfg(&s), "openai");
    }

    #[test]
    fn model_for_cfg_defaults_to_test_model() {
        let s = Settings::default();
        assert_eq!(model_for_cfg(&s), "test-model");
    }

    #[test]
    fn system_prompt_includes_identity_and_tools() {
        let s = Settings::default();
        let tools = ToolRegistry::new();
        let prompt = settings_to_system_prompt(&s, std::path::Path::new("/tmp/proj"), &tools, "");
        assert!(prompt.contains("Pi-compatible Rust coding agent"));
        // v0.8.6: hardcoded "Available tools: ..." line replaced with
        // the snippet+guidelines sections from the live registry.
        // With an empty registry there's no snippet section.
        assert!(!prompt.contains("Available tools: bash, read, write, edit, grep, find"));
    }

    #[test]
    fn system_prompt_appends_skills_prompt() {
        let s = Settings::default();
        let skills = "\n\n[Skills] foo, bar";
        let tools = ToolRegistry::new();
        let prompt = settings_to_system_prompt(&s, std::path::Path::new("/tmp/proj"), &tools, skills);
        assert!(prompt.ends_with(skills));
    }

    #[test]
    fn system_prompt_includes_provider_model_thinking() {
        let mut s = Settings::default();
        s.provider = Some("anthropic".into());
        s.model = Some("claude-opus".into());
        s.thinking_level = Some("high".into());
        let tools = ToolRegistry::new();
        let prompt = settings_to_system_prompt(&s, std::path::Path::new("/tmp/proj"), &tools, "");
        assert!(prompt.contains("Default provider: anthropic"));
        assert!(prompt.contains("Default model: claude-opus"));
        assert!(prompt.contains("Thinking level: high"));
    }

    // v0.8.5 regression: cwd must appear in system prompt so the
    // model uses relative paths for read/grep/find/write/edit.
    // Before this fix, models would guess `/home/<user>/<file>` for
    // project files, which fails when the project lives at
    // `/home/<user>/<project>/<file>`.
    #[test]
    fn system_prompt_includes_cwd() {
        let s = Settings::default();
        let cwd = std::path::Path::new("/home/jin/nini");
        let tools = ToolRegistry::new();
        let prompt = settings_to_system_prompt(&s, cwd, &tools, "");
        assert!(
            prompt.contains("Working directory: /home/jin/nini"),
            "system prompt must surface cwd; got: {prompt}"
        );
        assert!(
            prompt.contains("relative paths"),
            "system prompt must hint that relative paths are resolved against cwd"
        );
    }
}
