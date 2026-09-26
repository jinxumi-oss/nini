//! nini — Pi-compatible Rust coding agent (CLI entry point)
//!
//! Modes:
//! - `nini`                  launch interactive TUI (Phase 5)
//! - `nini -p "..."`         single-shot print mode (no session)
//! - `nini demo [task]`      scripted autonomous demo (no API key needed)
//! - `nini info`             show loaded skills/settings

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;

pub mod startup_ui;

// v0.8.2: split from main.rs into focused submodules. The order of
// `mod` declarations matters because some types cross-reference
// (`provider_factory` uses `tool_registry`, `app` uses everything).
pub(crate) mod prompt_setup;
pub(crate) mod provider_factory;
pub(crate) mod tool_registry;
pub(crate) mod info;
pub(crate) mod demo;
pub(crate) mod app;

// FixtureTurn + ProgrammedProvider moved to demo.rs + provider_factory.rs
use nini_core::provider::{Provider, Usage};
use nini_core::settings::load_settings;
use nini_core::skills::{format_skills_for_prompt, load_skills};
use nini_core::tool::Tool;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_core::tool::ToolRegistry;
use nini_tui::run as run_tui;
use std::io::IsTerminal;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "nini", about = "Pi-compatible Rust coding agent", version)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Single-shot print mode (no session persistence).
    #[arg(short = 'p', long = "print")]
    print: Option<String>,

    /// Provider name (`fixture | anthropic | openai | openai-responses | openai-compat`).
    #[arg(long, env = "NINI_PROVIDER", default_value = "fixture")]
    provider: String,

    /// Model identifier.
    #[arg(long, default_value = "test-model")]
    model: String,

    /// Comma-separated secondary API key(s) for automatic fallback.
    /// When the primary provider returns a transient error (HTTP 5xx,
    /// 429, network), the request is retried against the next key.
    /// Currently only honored by the `anthropic` provider.
    #[arg(long, env = "NINI_FALLBACK_KEYS", value_delimiter = ',')]
    fallback_keys: Vec<String>,

    /// Comma-separated base URLs aligned with `--fallback-keys`. Empty
    /// entries reuse the primary `ANTHROPIC_BASE_URL`.
    #[arg(long, env = "NINI_FALLBACK_BASE_URLS", value_delimiter = ',')]
    fallback_base_urls: Vec<String>,

    /// Continue the most recent session. Finds the latest .jsonl file in
    /// ~/.pi/agent/sessions/<cwd>/ and loads it.
    #[arg(long, conflicts_with = "session_id")]
    continue_session: bool,

    /// Resume a specific session by filename (not full path).
    /// Example: `--session 20250915-143022-abc123.jsonl`
    #[arg(long, value_name = "SESSION_FILE")]
    session_id: Option<String>,

    /// Skip session persistence (ephemeral).
    #[arg(long)]
    no_session: bool,

    /// Set the session display name.
    #[arg(short = 'n', long)]
    name: Option<String>,

    /// Fork a specific session file (creates a new branch from a past point).
    #[arg(long, value_name = "SESSION_FILE")]
    fork: Option<String>,

    /// Override the directory used for session storage.
    #[arg(long)]
    session_dir: Option<String>,

    /// API key (overrides env vars).
    #[arg(long)]
    api_key: Option<String>,

    /// Override the system prompt.
    #[arg(long)]
    system_prompt: Option<String>,

    /// Append text (or file contents with @ prefix) to the system prompt.
    /// Can be used multiple times.
    #[arg(long, value_delimiter = '\n')]
    append_system_prompt: Vec<String>,

    /// Thinking level: off, minimal, low, medium, high, xhigh, max.
    #[arg(long)]
    thinking: Option<String>,

    /// TUI mode: regular or fullscreen.
    #[arg(long, value_parser = ["regular", "fullscreen"])]
    tui_mode: Option<String>,

    /// List available models (optionally filter by search term).
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    list_models: Option<String>,

    /// Comma-separated allowlist of tool names to enable.
    #[arg(short = 't', long, value_delimiter = ',')]
    tools: Vec<String>,

    /// Comma-separated denylist of tool names to disable.
    #[arg(long, value_delimiter = ',')]
    exclude_tools: Vec<String>,

    /// Disable all tools by default.
    #[arg(long)]
    no_tools: bool,

    /// Disable built-in tools (keep extension/custom tools enabled).
    #[arg(long)]
    no_builtin_tools: bool,

    /// Project trust: trust (auto-load AGENTS.md etc.) or no (skip).
    #[arg(short = 'a', long)]
    approve: bool,

    /// Disable network calls (force offline mode).
    #[arg(long)]
    offline: bool,

    /// Verbose startup (overrides quietStartup setting).
    #[arg(long)]
    verbose: bool,

    /// Export the session to HTML at this path and exit.
    #[arg(long)]
    export: Option<String>,

    /// Load a skill file or directory (can be used multiple times).
    /// Overrides automatic skill discovery.
    #[arg(long, value_delimiter = ',')]
    skill: Vec<String>,

    /// Disable skill discovery.
    #[arg(long)]
    no_skills: bool,

    /// Load a prompt template file or directory.
    #[arg(long)]
    prompt_template: Option<String>,

    /// Disable prompt template discovery.
    #[arg(long)]
    no_prompt_templates: bool,

    /// Load a theme file or directory.
    #[arg(long)]
    theme_file: Option<String>,

    /// Set the theme by name (must match a loaded theme).
    #[arg(long, value_name = "NAME")]
    use_theme: Option<String>,

    /// Disable theme discovery.
    #[arg(long)]
    no_themes: bool,

    /// Load an extension (path or name) (can be used multiple times).
    #[arg(long, value_delimiter = ',')]
    extension: Vec<String>,

    /// Disable extension discovery.
    #[arg(long)]
    no_extensions: bool,

    /// Disable AGENTS.md / CLAUDE.md context files.
    #[arg(long)]
    no_context_files: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run an autonomous demo task (no API key needed).
    Demo { task: Option<String> },
    /// Show loaded skills and settings.
    Info,
}

/// Built-in model catalog for `nini --list-models` (fallback when
/// models.json is missing).
#[allow(dead_code)]
const BUILTIN_CATALOG: &[(&str, &str)] = &[
    ("anthropic/claude-opus-4-7", "Anthropic Claude Opus 4.7"),
    ("anthropic/claude-sonnet-4-5", "Anthropic Claude Sonnet 4.5"),
    ("anthropic/claude-haiku-4-5", "Anthropic Claude Haiku 4.5"),
    ("openai/gpt-5", "OpenAI GPT-5"),
    ("openai/gpt-4o", "OpenAI GPT-4o"),
    ("openai/o1", "OpenAI o1"),
    ("google/gemini-2.5-pro", "Google Gemini 2.5 Pro"),
    ("deepseek/deepseek-chat", "DeepSeek Chat"),
    ("groq/llama-3.3-70b", "Groq Llama 3.3 70B"),
    ("mistral/mistral-large-latest", "Mistral Large"),
    ("mistral/codestral-latest", "Mistral Codestral"),
    ("cohere/command-r-plus", "Cohere Command R+"),
    ("cohere/command-r", "Cohere Command R"),
    ("minimax/MiniMax-M3", "MiniMax M3"),
];

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let Args {
        cmd,
        print,
        provider,
        model,
        fallback_keys,
        fallback_base_urls,
        continue_session,
        session_id,
        no_session: _,
        name: _,
        fork: _,
        session_dir: _,
        api_key: _,
        system_prompt: _,
        append_system_prompt: _,
        thinking: _,
        tui_mode,
        list_models,
        tools,
        exclude_tools,
        no_tools,
        no_builtin_tools,
        approve: _,
        offline: _,
        verbose: _,
        export: _,
        skill: _,
        no_skills: _,
        prompt_template: _,
        no_prompt_templates: _,
        theme_file: _,
        use_theme,
        no_themes: _,
        extension: _,
        no_extensions: _,
        no_context_files: _,
    } = args;

    match cmd {
        Some(Cmd::Demo { task }) => {
            let task = task.unwrap_or_else(|| "find TODOs and fix them".to_string());
            demo::run_demo(&task, &provider, &model, &fallback_keys, &fallback_base_urls).await
        }
        Some(Cmd::Info) => info::run_info().await,
        _ if list_models.is_some() => {
            // Build runtime from models.json or fall back to defaults.
            let cwd = std::env::current_dir().unwrap_or_default();
            let mj = nini_core::settings::load_models_json(&cwd);
            let runtime = if mj.providers.is_empty() {
                nini_core::model_runtime::ModelRuntime::with_defaults()
            } else {
                nini_core::model_runtime::ModelRuntime::from_models_json(&mj)
            };
            let filter = list_models.as_deref().unwrap_or("");
            for m in nini_core::model_runtime::resolve_pattern(&runtime, filter) {
                println!("{}", m.id);
            }
            Ok(())
        }
        None if print.is_some() => {
            demo::run_print(
                print.as_deref().unwrap(),
                &provider,
                &model,
                &fallback_keys,
                &fallback_base_urls,
            )
            .await
        }
        None => {
            // Default: launch interactive TUI. Fall back to help text if
            // stdin/stdout aren't TTYs (e.g., piped, CI, or this headless env).
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
                eprintln!("Provider: {provider} (override with NINI_PROVIDER env or --provider)");
                std::process::exit(2);
            }

            // Default: launch interactive TUI.
            use nini_tui::runtime::AgentDriver;
            let provider_name = provider.clone();

            // First-run setup: detect missing agent dir and write defaults.
            // Interactive wizard UI is deferred to v2 — for now we just
            // ensure the directory structure and a settings.json exist.
            if let Ok(home) = std::env::var("HOME") {
                let home_path = std::path::PathBuf::from(&home);
                if crate::startup_ui::needs_first_time_setup(&home_path) {
                    if let Err(e) = crate::startup_ui::init_agent_dir(&home_path) {
                        eprintln!("[nini] first-run setup: {e}");
                    }
                }
            }

            // Build the provider (and its scripted turns) ONCE so the cycling
            // cursor survives across user messages. Without this, every
            // submit would reset to turn 0 and multi-turn demos would loop
            // on the same first response.
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mut settings = load_settings(&cwd);
            // --use-theme overrides settings.theme for this run.
            if let Some(t) = use_theme.as_deref() {
                settings.theme = Some(t.to_string());
            }
            // TUI mode is parsed and printed but not yet wired through to
            // the runtime (the runtime::run signature doesn't take it).
            if let Some(_mode) = tui_mode.as_deref() {
                // v2: plumb into the TUI via a new field in AppState.
            }
            let _skills = load_skills(&cwd);
            let scripted_turns = std::env::var("NINI_TUI_FIXTURE_TURNS")
                .ok()
                .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
                .unwrap_or_default();
            let shared_provider: Arc<dyn Provider> = provider_factory::build_provider(
                &provider_name,
                scripted_turns,
                &fallback_keys,
                &fallback_base_urls,
            )
                .unwrap_or_else(|_| {
                    std::sync::Arc::new(nini_ai::fixture::ProgrammedProvider::from_turns(
                        std::env::var("NINI_TUI_FIXTURE_TURNS")
                            .ok()
                            .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
                            .unwrap_or_default(),
                    )) as Arc<dyn Provider>
                });
            let shared_tools = tool_registry::filter_tools(
                tool_registry::build_tools(),
                &tools,
                &exclude_tools,
                no_tools,
                no_builtin_tools,
            );
            // Honor --model (CLI) over settings; this is what gets sent in
            // every provider request body.
            let cfg_model = if !model.is_empty() {
                model.clone()
            } else {
                prompt_setup::model_for_cfg(&settings)
            };
            let shared_cfg = RunConfig {
                model: cfg_model.clone(),
                system: Some(prompt_setup::settings_to_system_prompt(&settings, "")),
                ..RunConfig::new(cfg_model)
            };
            let agent_driver: AgentDriver = std::sync::Arc::new(move |user_msg, sink, done| {
                let provider = shared_provider.clone();
                let tools = shared_tools.clone();
                let cfg = shared_cfg.clone();
                let mut agent = nini_core::Agent::new(provider, tools, cfg);
                tokio::spawn(async move {
                    let mut stream =
                        Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));
                    while let Some(ev) = stream.next().await {
                        let lite = match ev {
                            Ok(AgentEvent::TextDelta { text }) => {
                                nini_tui::runtime::AgentEventLite::TextDelta(text)
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
                                // `ProviderUsage` doesn't carry cost today;
                                // pass 0.0 so cost_usd only updates when a
                                // future Usage variant surfaces it.
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
                                nini_tui::runtime::AgentEventLite::PhaseChanged(
                                    phase.to_string()
                                )
                            }
                            Err(e) => {
                                // CRITICAL: surface the error to the TUI
                                // instead of silently dropping it. The
                                // previous `_ => continue` swallowed every
                                // Err (network failure, JSON parse error,
                                // provider auth error, etc.), leaving the
                                // user staring at an unresponsive TUI with
                                // zero indication of what went wrong.
                                nini_tui::runtime::AgentEventLite::Error(
                                    format!("agent stream error: {e}")
                                )
                            }
                            Ok(other) => {
                                // Forward any AgentEvent variants we
                                // haven't explicitly handled (AgentStart,
                                // AgentEnd, Aborted) as a PhaseChanged
                                // event so the user at least sees the
                                // agent transitioned through them. Without
                                // this, matching future event variants
                                // becomes a silent default.
                                nini_tui::runtime::AgentEventLite::PhaseChanged(
                                    format!("agent: {other:?}")
                                )
                            }
                        };
                        sink.push(lite);
                    }
                    sink.push(nini_tui::runtime::AgentEventLite::Done);
                    done.notify_waiters();
                })
            });
            run_tui(
                |state| {
                    let cwd = std::env::current_dir().ok();
                    if let Some(ref dir) = cwd {
                        let s = load_settings(dir);
                        // v0.7.4 (UX fix) — priority order:
                        //   1. CLI --model flag (passed via `model` capture)
                        //   2. settings.json `model` field
                        //   3. settings.json `provider` field (fallback name)
                        //   4. "test-model" (last resort)
                        // The original v0.7.2 code read `s.model_state.provider` first,
                        // which meant the status bar showed "openai-compat"
                        // (the provider name) instead of the actual model.
                        let picked = if !model.is_empty() {
                            model.clone()
                        } else if let Some(m) = s.model.clone() {
                            m
                        } else if let Some(p) = s.provider.clone() {
                            p
                        } else {
                            "test-model".to_string()
                        };
                        state.model_state.model = picked;
                        // v0.8: surface the provider name in the status bar
                        // (Pi-style `(provider) model`). Helps users
                        // disambiguate `MiniMax-M3` from gateway vs local,
                        // or `gpt-4o` from openai vs openai-compat.
                        if !provider.is_empty() {
                            state.model_state.provider = Some(provider.clone());
                        }
                        // Seed state.session_state.cwd / state.session_state.git_branch so the status
                        // bar can render them (previously only declared in
                        // AppState, never populated).
                        state.set_status_bar_metadata(
                            Some(dir.clone()),
                            nini_core::git::git_branch(dir),
                        );
                        // Set context window from the active model so the
                        // status bar can show ctx% from turn 1.
                        let mj = nini_core::settings::load_models_json(dir);
                        if !mj.providers.is_empty() {
                            let runtime = nini_core::model_runtime::ModelRuntime::from_models_json(&mj);
                            if let Some(m) = runtime.find_by_id(&state.model_state.model) {
                                state.run_state.context_window = m.context_window;
                            }
                        }
                        // Load models.json to populate the model cycle
                        // (Ctrl+P rotates through these).
                        let mj = nini_core::settings::load_models_json(dir);
                        if !mj.providers.is_empty() {
                            let runtime = nini_core::model_runtime::ModelRuntime::from_models_json(&mj);
                            state.model_state.models_cycle = runtime.all().iter().map(|m| m.id.clone()).collect();
                            // Pre-select current model in cycle.
                            state.model_state.models_cycle_idx = state
                                .model_state.models_cycle
                                .iter()
                                .position(|m| m == &state.model_state.model);
                        }
                        // Wire the live settings.json path so cycle_model /
                        // /model / /thinking actually persist.
                        if let Some(home) = std::env::var_os("HOME") {
                            let p = std::path::PathBuf::from(home)
                                .join(".pi")
                                .join("agent")
                                .join("settings.json");
                            if p.exists() {
                                state.model_state.settings_path = Some(p);
                            }
                        }
                        // Seed initial thinking level from settings so
                        // cycle_thinking reads a real value (not a string parse
                        // of state.run_state.status).
                        if let Some(t) = nini_core::settings::load_settings(dir)
                            .thinking_level
                        {
                            state.model_state.thinking_level = Some(t);
                        }
                        let skills = load_skills(dir);
                        if !skills.skills.is_empty() {
                            let count = skills.skills.len();
                            // Toast in the status bar instead of a
                            // transcript banner — the banner was a
                            // permanent eyesore (and worse, overlapped
                            // with the selector panel).
                            state.run_state.status = format!("loaded {count} skills — type to begin");
                        }
                    }

                    // Session initialization from CLI flags.
                    let session_result: Result<(), String> = (|| {
                        let home = std::env::var("HOME")
                            .map_err(|e| format!("HOME not set: {e}"))?;
                        let cwd_name = cwd
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "default".to_string());
                        let sessions_base = std::path::PathBuf::from(&home)
                            .join(".pi")
                            .join("agent")
                            .join("sessions")
                            .join(&cwd_name);

                        if let Some(ref session_file) = session_id {
                            // --session <file> — load that specific file.
                            let path = sessions_base.join(session_file);
                            state.session_load(path.clone())
                                .map_err(|e| format!("Failed to load session: {e}"))?;
                        } else if continue_session {
                            // --continue — find the latest .jsonl file.
                            let entries: Vec<_> = std::fs::read_dir(&sessions_base)
                                .ok()
                                .into_iter()
                                .flatten()
                                .flatten()
                                .filter(|e| e.path().extension().map(|s| s == "jsonl").unwrap_or(false))
                                .collect();
                            if entries.is_empty() {
                                return Err("No sessions found to continue.".to_string());
                            }
                            // Pick the newest by mtime.
                            let mut latest = entries.into_iter();
                            let first = latest.next().unwrap();
                            let path = latest.fold(first.path(), |best, e| {
                                let best_time = best.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                let e_time = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                if e_time > best_time {
                                    e.path()
                                } else {
                                    best
                                }
                            });
                            state.session_load(path.clone())
                                .map_err(|e| format!("Failed to load session: {e}"))?;
                            // Rebuild transcript from loaded session entries.
                            // Take the Arc out of state so we can lock it without borrowing state.
                            state.transcript_state.lines.clear();
                            let session_id = state.session_state.session_id.clone().unwrap_or_default();
                            let session_arc = state.session_state.session.take();
                            if let Some(arc) = session_arc {
                                if let Ok(guard) = arc.try_lock() {
                                    for entry in &guard.entries {
                                        if let Some(msg) = demo::cli_entry_legacy(entry) {
                                            for block in &msg.content {
                                                if let nini_core::ContentBlock::Text { text } = block {
                                                    match msg.role {
                                                        nini_core::Role::User => state.push_user(text.clone()),
                                                        nini_core::Role::Assistant => state.push_assistant(text.clone()),
                                                        nini_core::Role::System | nini_core::Role::Tool => {}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                state.session_state.session = Some(arc);
                            }
                            state.push_assistant(format!("(continued session {session_id})"));
                            state.push_divider();
                        }
                        // If neither flag: create a new session.
                        if session_id.is_none() && !continue_session {
                            let path = sessions_base.join(format!(
                                "{}.jsonl",
                                chrono::Utc::now().format("%Y%m%d-%H%M%S")
                            ));
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            state.session_init(path);
                        }
                        Ok(())
                    })();

                    if let Err(msg) = session_result {
                        state.push_assistant(format!("[session init] {msg}"));
                        state.push_divider();
                    }
                },
                agent_driver,
            )
            .await
        }
    }
}








