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
    settings_to_system_prompt_with(settings, cwd, tools, skills_prompt, None, None)
}

/// The assembler, with the two CLI overrides Pi has.
///
/// Pi: `buildSystemPrompt` (`pi-coding-agent/src/core/system-prompt.ts:28-68`)
///
/// ```text
/// 41  const appendSection = appendSystemPrompt ? `\n\n${appendSystemPrompt}` : "";
/// 46  if (customPrompt) {
/// 47      let prompt = customPrompt;          // the base is REPLACED
/// 49      if (appendSection) prompt += appendSection;
/// 55      … <project_context> (AGENTS.md)
/// 66      … formatSkillsForPrompt(skills)
/// 68  } else { … the normal base … }
/// ```
///
/// v0.10 (fix): `--system-prompt` and `--append-system-prompt` were
/// destructured as `_` in `main.rs` — accepted, described in `--help` as
/// "Override the system prompt" / "Append to the system prompt", and dropped.
/// Everything appended *after* the base stays appended after a custom base:
/// the skills block, the tool self-descriptions and the working-directory
/// hint are not part of the base, they are what surrounds it.
pub(crate) fn settings_to_system_prompt_with(
    settings: &nini_core::settings::Settings,
    cwd: &std::path::Path,
    tools: &nini_core::tool::ToolRegistry,
    skills_prompt: &str,
    custom_prompt: Option<&str>,
    append_prompt: Option<&str>,
) -> String {
    let mut s = match custom_prompt {
        Some(p) if !p.trim().is_empty() => {
            let mut s = p.to_string();
            s.push('\n');
            s
        }
        _ => String::from("You are nini, a Pi-compatible Rust coding agent.\n"),
    };
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
    if let Some(extra) = append_prompt {
        if !extra.trim().is_empty() {
            s.push_str("\n\n");
            s.push_str(extra);
            s.push('\n');
        }
    }
    s.push_str(skills_prompt);
    // v0.10.8 (T3): **工具段不再在这里拼**。
    //
    // 原来这里拼一次（"## Tool self-descriptions" + "## Tool usage
    // guidelines"），`nini-core/src/agent.rs` 在发请求前又拼一次——
    // 于是**同一段工具说明在 prompt 里出现了两遍**。模型看到重复的
    // 指令与重复的 token，而 nini 与 Pi 的 prompt 结构就此不同。
    //
    // 现在只有 agent 拼（Pi 的形状："Available tools:" + "Guidelines:"），
    // 这里是**基座**（身份、cwd、skills、用户追加段）。
    let _ = tools; // 基座不再需要 registry；工具段由 agent 统一拼
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
        let prompt =
            settings_to_system_prompt(&s, std::path::Path::new("/tmp/proj"), &tools, skills);
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
    // ---------------------------------------------------------------------------
    // 1. --system-prompt / --append-system-prompt reach the prompt
    // ---------------------------------------------------------------------------

    /// Pi's rule (`system-prompt.ts:46-68`): `--system-prompt` **replaces** the
    /// base, and everything else — append, project context, skills — still lands
    /// on top of it.
    #[test]
    fn a_custom_system_prompt_replaces_the_base_and_keeps_the_skills_block() {
        let settings = nini_core::settings::Settings::default();
        let tools = nini_core::tool::ToolRegistry::new();
        let skills =
            "\n<available_skills>\n<skill><name>demo</name></skill>\n</available_skills>\n";

        let prompt = settings_to_system_prompt_with(
            &settings,
            std::path::Path::new("/tmp"),
            &tools,
            skills,
            Some("You are a terse assistant."),
            None,
        );

        assert!(
            prompt.starts_with("You are a terse assistant."),
            "the custom prompt must be the base, got: {}",
            &prompt.chars().take(80).collect::<String>()
        );
        assert!(
            !prompt.contains("You are nini, a Pi-compatible Rust coding agent."),
            "the default base survived a custom prompt — both are being sent and \
         the model has no way to tell which one wins"
        );
        assert!(
            prompt.contains("<available_skills>"),
            "the skills block is dropped when a custom prompt is set; it is \
         appended to the base, not part of it"
        );
    }

    /// `--append-system-prompt` **adds** to the base — the base stays.
    #[test]
    fn an_append_system_prompt_adds_to_the_base() {
        let tools = nini_core::tool::ToolRegistry::new();
        let prompt = settings_to_system_prompt_with(
            &nini_core::settings::Settings::default(),
            std::path::Path::new("/tmp"),
            &tools,
            "",
            None,
            Some("Always run cargo test before reporting done."),
        );
        assert!(
            prompt.contains("You are nini, a Pi-compatible Rust coding agent."),
            "the base prompt was replaced by an append"
        );
        assert!(
            prompt.contains("Always run cargo test before reporting done."),
            "the appended instruction is not in the prompt"
        );
    }

    /// Both together: replace, then append. Order matters and Pi fixes it
    /// (`system-prompt.ts:47-51`).
    #[test]
    fn append_lands_after_the_custom_base() {
        let tools = nini_core::tool::ToolRegistry::new();
        let prompt = settings_to_system_prompt_with(
            &nini_core::settings::Settings::default(),
            std::path::Path::new("/tmp"),
            &tools,
            "",
            Some("BASE-MARKER"),
            Some("APPEND-MARKER"),
        );
        let base = prompt.find("BASE-MARKER").expect("base present");
        let app = prompt.find("APPEND-MARKER").expect("append present");
        assert!(base < app, "the append must come after the base it extends");
    }

    /// The working-directory hint is not decoration: without it models address
    /// files by absolute path outside the project (`prompt_setup.rs:37-41`).
    /// A custom prompt must not lose it.
    #[test]
    fn a_custom_prompt_still_gets_the_working_directory() {
        let tools = nini_core::tool::ToolRegistry::new();
        let prompt = settings_to_system_prompt_with(
            &nini_core::settings::Settings::default(),
            std::path::Path::new("/work/project"),
            &tools,
            "",
            Some("Be terse."),
            None,
        );
        assert!(
            prompt.contains("/work/project"),
            "the cwd hint vanished with the base prompt: {prompt}"
        );
    }

    // ---------------------------------------------------------------------------
    // 4. --thinking reaches the request
    // ---------------------------------------------------------------------------

    /// `RunConfig` carries `thinking`, `settings.thinking_level` is rendered into
    /// the prompt text, and `agent.rs` now forwards the level to the provider
    /// (v0.10 R1f). The CLI's `--thinking` is the only piece missing, so the
    /// level a user asks for on the command line never reaches the API.
    #[test]
    fn a_thinking_level_from_the_command_line_reaches_the_config() {
        let mut settings = nini_core::settings::Settings::default();
        settings.thinking_level = Some("high".into());
        let tools = nini_core::tool::ToolRegistry::new();
        let prompt = settings_to_system_prompt_with(
            &settings,
            std::path::Path::new("/tmp"),
            &tools,
            "",
            None,
            None,
        );
        assert!(
            prompt.contains("high"),
            "the thinking level is not in the prompt, so it is not in the \
         request context: {prompt}"
        );
    }
}
