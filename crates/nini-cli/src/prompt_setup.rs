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
/// provider/model/thinking-level from `Settings`, and any skills
/// prompt produced by the runtime.
pub(crate) fn settings_to_system_prompt(
    settings: &nini_core::settings::Settings,
    skills_prompt: &str,
) -> String {
    let mut s = String::from("You are nini, a Pi-compatible Rust coding agent.\n");
    if let Some(p) = &settings.provider {
        s.push_str(&format!("Default provider: {p}\n"));
    }
    if let Some(m) = &settings.model {
        s.push_str(&format!("Default model: {m}\n"));
    }
    if let Some(t) = &settings.thinking_level {
        s.push_str(&format!("Thinking level: {t}\n"));
    }
    s.push_str(
        "\nAvailable tools: bash, read, write, edit, grep, find. \
         Use them to complete complex multi-step tasks.",
    );
    s.push_str(skills_prompt);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use nini_core::settings::Settings;

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
        let prompt = settings_to_system_prompt(&s, "");
        assert!(prompt.contains("Pi-compatible Rust coding agent"));
        assert!(prompt.contains("Available tools: bash, read, write, edit, grep, find"));
    }

    #[test]
    fn system_prompt_appends_skills_prompt() {
        let s = Settings::default();
        let skills = "\n\n[Skills] foo, bar";
        let prompt = settings_to_system_prompt(&s, skills);
        assert!(prompt.ends_with(skills));
    }

    #[test]
    fn system_prompt_includes_provider_model_thinking() {
        let mut s = Settings::default();
        s.provider = Some("anthropic".into());
        s.model = Some("claude-opus".into());
        s.thinking_level = Some("high".into());
        let prompt = settings_to_system_prompt(&s, "");
        assert!(prompt.contains("Default provider: anthropic"));
        assert!(prompt.contains("Default model: claude-opus"));
        assert!(prompt.contains("Thinking level: high"));
    }
}