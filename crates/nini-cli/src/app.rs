//! v0.8.2: App factory + TUI bootstrap.
//!
//! The default `nini` (no subcommand) entry path. Owns:
//!   * first-run setup detection (`startup_ui::*`)
//!   * Settings loading + --use-theme override
//!   * Provider construction + fallback to ProgrammedProvider
//!   * Tool registry construction + CLI filtering
//!   * RunConfig assembly
//!   * AgentDriver closure (the `Arc::new(move |user_msg, sink, done| {...})`
//!     that the TUI runtime invokes on each user input)
//!
//! Extracted from `main.rs` because:
//!   1. It contains the largest single closure in the codebase (~220 LOC)
//!   2. It's the only entry with a runtime-effect (TUI vs. demo vs. -p)
//!   3. Future entry modes (e.g., REPL without TUI) can reuse most of this

use std::sync::Arc;

use std::io::IsTerminal;

use anyhow::Result;
use futures_util::StreamExt;
use nini_core::provider::Provider;
use nini_core::settings::load_settings;
use nini_core::skills::load_skills;
use nini_core::{Agent, AgentEvent, RunConfig};

use crate::startup_ui;
use crate::{prompt_setup, provider_factory, tool_registry};

/// Configuration for the TUI bootstrap. Holds the CLI args that
/// `main()` would have inlined before extraction.
pub(crate) struct AppConfig {
    pub provider: String,
    pub model: String,
    pub fallback_keys: Vec<String>,
    pub fallback_base_urls: Vec<String>,
    pub tools: Vec<String>,
    pub exclude_tools: Vec<String>,
    pub no_tools: bool,
    pub no_builtin_tools: bool,
    pub use_theme: Option<String>,
}

/// Construct an `App` from CLI config and launch the TUI.
///
/// Returns `Ok(())` on clean exit (Ctrl+C, Ctrl+D, /quit) or an
/// error if initialization fails (e.g., unknown provider).
pub(crate) async fn run_tui(cfg: AppConfig) -> Result<()> {
    use nini_tui::runtime::AgentDriver;

    // First-run setup.
    if let Ok(home) = std::env::var("HOME") {
        let home_path = std::path::PathBuf::from(&home);
        if startup_ui::needs_first_time_setup(&home_path) {
            if let Err(e) = startup_ui::init_agent_dir(&home_path) {
                eprintln!("[nini] first-run setup: {e}");
            }
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut settings = load_settings(&cwd);
    if let Some(t) = cfg.use_theme.as_deref() {
        settings.theme = Some(t.to_string());
    }

    let _skills = load_skills(&cwd);
    let scripted_turns = std::env::var("NINI_TUI_FIXTURE_TURNS")
        .ok()
        .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
        .unwrap_or_default();

    // Provider — fallback to ProgrammedProvider on build error so the
    // TUI still has *something* to talk to.
    let shared_provider: Arc<dyn Provider> = provider_factory::build_provider(
        &cfg.provider,
        scripted_turns,
        &cfg.fallback_keys,
        &cfg.fallback_base_urls,
    )
    .unwrap_or_else(|_| {
        Arc::new(nini_ai::fixture::ProgrammedProvider::from_turns(
            std::env::var("NINI_TUI_FIXTURE_TURNS")
                .ok()
                .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
                .unwrap_or_default(),
        )) as Arc<dyn Provider>
    });

    let shared_tools = tool_registry::filter_tools(
        tool_registry::build_tools(),
        &cfg.tools,
        &cfg.exclude_tools,
        cfg.no_tools,
        cfg.no_builtin_tools,
    );

    let cfg_model = if !cfg.model.is_empty() {
        cfg.model.clone()
    } else {
        prompt_setup::model_for_cfg(&settings)
    };
    let shared_cfg = RunConfig {
        model: cfg_model.clone(),
        system: Some(prompt_setup::settings_to_system_prompt(&settings, "")),
        ..RunConfig::new(cfg_model)
    };

    let agent_driver: AgentDriver = Arc::new(move |user_msg, sink, done| {
        let provider = shared_provider.clone();
        let tools = shared_tools.clone();
        let cfg = shared_cfg.clone();
        let mut agent = Agent::new(provider, tools, cfg);
        tokio::spawn(async move {
            let mut stream =
                Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));
            while let Some(ev) = stream.next().await {
                let lite = match ev {
                    Ok(AgentEvent::TextDelta { text }) => {
                        nini_tui::runtime::AgentEventLite::TextDelta(text)
                    }
                    Ok(AgentEvent::ThinkingDelta { text }) => {
                        nini_tui::runtime::AgentEventLite::ThinkingDelta(text)
                    }
                    Ok(AgentEvent::ToolCallStart { name, .. }) => {
                        nini_tui::runtime::AgentEventLite::ToolCallStart { name }
                    }
                    Ok(AgentEvent::ToolCallStop { id, input_json }) => {
                        nini_tui::runtime::AgentEventLite::ToolCallStop {
                            id,
                            args: input_json.to_string(),
                        }
                    }
                    Ok(AgentEvent::ToolResult { output, .. }) => {
                        nini_tui::runtime::AgentEventLite::ToolResult {
                            ok: !output.is_error,
                            content: output.content,
                            details: None,
                            duration_ms: output.duration_ms,
                        }
                    }
                    Ok(AgentEvent::TurnEnd { usage, .. }) => {
                        sink.push(nini_tui::runtime::AgentEventLite::Usage(
                            usage.input_tokens,
                            usage.output_tokens,
                            0.0,
                        ));
                        nini_tui::runtime::AgentEventLite::TurnEnd
                    }
                    Ok(AgentEvent::Error { message }) => {
                        nini_tui::runtime::AgentEventLite::Error(message)
                    }
                    Ok(AgentEvent::PhaseChanged(phase)) => {
                        nini_tui::runtime::AgentEventLite::PhaseChanged(phase.to_string())
                    }
                    Ok(_) => continue,
                    Err(_) => continue,
                };
                sink.push(lite);
            }
            sink.push(nini_tui::runtime::AgentEventLite::Done);
            done.notify_waiters();
        })
    });

    run_tui_bootstrap(agent_driver, &cfg).await
}

/// Run the TUI itself with the given `AgentDriver`. This is the part
/// that requires a real TTY; if there isn't one, we print the
/// "no TTY detected" help and exit 2.
async fn run_tui_bootstrap(agent_driver: nini_tui::runtime::AgentDriver, cfg: &AppConfig) -> Result<()> {
    use nini_tui::run as run_tui_inner;

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("nini v0.4.0 — Pi-compatible Rust coding agent");
        eprintln!();
        eprintln!("(no TTY detected — interactive TUI unavailable in this env)");
        eprintln!();
        eprintln!("Usage:");
        eprintln!("  nini -p \"<task>\"              single-shot (no session)");
        eprintln!("  nini demo [task]             scripted autonomous demo");
        eprintln!("  nini info                    show loaded skills/settings");
        eprintln!();
        eprintln!("Provider: {} (override with NINI_PROVIDER env or --provider)", cfg.provider);
        std::process::exit(2);
    }

    run_tui_inner(
        |state| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let dir = cwd.clone();
            state.set_status_bar_metadata(
                Some(dir.clone()),
                nini_core::git::git_branch(&dir),
            );
            // v0.8.1: surface the provider name in the status bar
            // (Pi-style `(provider) model`). Helps users tell which
            // backend is active.
            if !cfg.provider.is_empty() {
                state.model_state.provider = Some(cfg.provider.clone());
            }
            // Seed initial thinking level from settings.
            if let Some(t) = nini_core::settings::load_settings(&cwd)
                .thinking_level
            {
                state.model_state.thinking_level = Some(t);
            }
            let skills = nini_core::skills::load_skills(&cwd);
            if !skills.skills.is_empty() {
                let count = skills.skills.len();
                state.run_state.status =
                    format!("loaded {count} skills — type to begin");
            }
        },
        agent_driver,
    )
    .await
}
