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

use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::{Provider, Usage};
use nini_core::settings::load_settings;
use nini_core::skills::{format_skills_for_prompt, load_skills};
use nini_core::tool::Tool;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_core::tool::ToolRegistry;
use nini_tools::{BashTool, EditTool, FindTool, GrepTool, ReadTool, WriteTool};
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
            run_demo(&task, &provider, &model, &fallback_keys, &fallback_base_urls).await
        }
        Some(Cmd::Info) => run_info().await,
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
            run_print(
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
                .and_then(|raw| parse_scripted_turns(&raw))
                .unwrap_or_default();
            let shared_provider: Arc<dyn Provider> = build_provider(
                &provider_name,
                scripted_turns,
                &fallback_keys,
                &fallback_base_urls,
            )
                .unwrap_or_else(|_| {
                    std::sync::Arc::new(nini_ai::fixture::ProgrammedProvider::from_turns(
                        std::env::var("NINI_TUI_FIXTURE_TURNS")
                            .ok()
                            .and_then(|raw| parse_scripted_turns(&raw))
                            .unwrap_or_default(),
                    )) as Arc<dyn Provider>
                });
            let shared_tools = filter_tools(
                build_tools(),
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
                model_for_cfg(&settings)
            };
            let shared_cfg = RunConfig {
                model: cfg_model.clone(),
                system: Some(settings_to_system_prompt(&settings, "")),
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
                                 details: None}
                            }
                            Ok(AgentEvent::TurnEnd { usage, .. }) => {
                                sink.push(nini_tui::runtime::AgentEventLite::Usage(
                                    usage.input_tokens,
                                    usage.output_tokens,
                                ));
                                nini_tui::runtime::AgentEventLite::TurnEnd
                            }
                            Ok(AgentEvent::Error { message }) => {
                                nini_tui::runtime::AgentEventLite::Error(message)
                            }
                            Ok(AgentEvent::PhaseChanged(phase)) => {
                                nini_tui::runtime::AgentEventLite::PhaseChanged(
                                    format!("{phase:?}")
                                )
                            }
                            Err(_) => continue,
                            _ => continue,
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
                        if let Some(p) = s.provider {
                            state.model = p;
                        } else {
                            state.model = "test-model".to_string();
                        }
                        if let Some(m) = s.model {
                            state.model = m;
                        }
                        // Load models.json to populate the model cycle
                        // (Ctrl+P rotates through these).
                        let mj = nini_core::settings::load_models_json(dir);
                        if !mj.providers.is_empty() {
                            let runtime = nini_core::model_runtime::ModelRuntime::from_models_json(&mj);
                            state.models_cycle = runtime.all().iter().map(|m| m.id.clone()).collect();
                            // Pre-select current model in cycle.
                            state.models_cycle_idx = state
                                .models_cycle
                                .iter()
                                .position(|m| m == &state.model);
                        }
                        // Wire the live settings.json path so cycle_model /
                        // /model / /thinking actually persist.
                        if let Some(home) = std::env::var_os("HOME") {
                            let p = std::path::PathBuf::from(home)
                                .join(".pi")
                                .join("agent")
                                .join("settings.json");
                            if p.exists() {
                                state.settings_path = Some(p);
                            }
                        }
                        // Seed initial thinking level from settings so
                        // cycle_thinking reads a real value (not a string parse
                        // of state.status).
                        if let Some(t) = nini_core::settings::load_settings(dir)
                            .thinking_level
                        {
                            state.thinking_level = Some(t);
                        }
                        let skills = load_skills(dir);
                        if !skills.skills.is_empty() {
                            let count = skills.skills.len();
                            state.push_assistant(format!(
                                "loaded {count} skills; type to begin, Ctrl+C to quit, F1 for help"
                            ));
                            state.push_divider();
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
                            state.transcript.clear();
                            let session_id = state.session_id.clone().unwrap_or_default();
                            let session_arc = state.session.take();
                            if let Some(arc) = session_arc {
                                if let Ok(guard) = arc.try_lock() {
                                    for entry in &guard.entries {
                                        if let Some(msg) = cli_entry_legacy(entry) {
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
                                state.session = Some(arc);
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

async fn run_info() -> Result<()> {
    let cwd = std::env::current_dir()?;
    println!("nini info (cwd: {})", cwd.display());
    println!();
    let settings = load_settings(&cwd);
    println!("Settings (loaded):");
    println!("  provider:       {:?}", settings.provider);
    println!("  model:          {:?}", settings.model);
    println!("  thinking_level: {:?}", settings.thinking_level);
    println!();
    let skills_result = load_skills(&cwd);
    println!(
        "Skills ({} loaded, {} errors):",
        skills_result.skills.len(),
        skills_result.errors.len()
    );
    for s in &skills_result.skills {
        println!("  - {} ({}) -- {}", s.name, s.source.label(), s.description);
    }
    for e in &skills_result.errors {
        println!("  ! error: {e}");
    }
    Ok(())
}

fn make_anthropic(key: &str, base: Option<&str>) -> nini_ai::anthropic::AnthropicProvider {
    let mut p = nini_ai::anthropic::AnthropicProvider::new(key);
    if let Some(b) = base {
        if !b.trim().is_empty() {
            p = p.with_base_url(b);
        }
    }
    p
}

fn build_provider(
    provider: &str,
    turns: Vec<Vec<FixtureTurn>>,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<Arc<dyn Provider>> {
    match provider {
        "fixture" => Ok(Arc::new(ProgrammedProvider::from_turns(turns))),
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY required for anthropic")?;
            // Honor ANTHROPIC_BASE_URL so the same provider can target
            // Anthropic-compat gateways (e.g., https://m.aiio.chat).
            let base = std::env::var("ANTHROPIC_BASE_URL").ok();
            // Per-fallback-key base URLs (positional, aligned with
            // fallback_keys). Empty entries reuse the primary base URL.
            let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
            providers.push(Arc::new(make_anthropic(&key, base.as_deref())));
            for (i, fk) in fallback_keys.iter().enumerate() {
                let trimmed = fk.trim();
                if trimmed.is_empty() || trimmed == key {
                    continue;
                }
                let fbase = fallback_base_urls
                    .get(i)
                    .map(|s| s.as_str())
                    .filter(|s| !s.is_empty())
                    .or(base.as_deref());
                providers.push(Arc::new(make_anthropic(trimmed, fbase)));
            }
            if providers.len() == 1 {
                Ok(providers.remove(0))
            } else {
                Ok(Arc::new(nini_ai::fallback::FallbackProvider::new(providers)))
            }
        }
        "openai" => {
            let key =
                std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY required for openai")?;
            Ok(Arc::new(nini_ai::openai::OpenAiProvider::new(key)))
        }
        "openai-responses" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-responses")?;
            Ok(Arc::new(
                nini_ai::openai_responses::OpenAiResponsesProvider::new(key),
            ))
        }
        "openai-compat" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-compat")?;
            let base = std::env::var("OPENAI_BASE_URL")
                .context("OPENAI_BASE_URL required for openai-compat")?;
            Ok(Arc::new(nini_ai::openai_compat::OpenAiCompatProvider::new(
                base, key,
            )))
        }
        "google" => {
            // Auto-detect from GOOGLE_API_KEY or GEMINI_API_KEY.
            let key = std::env::var("GOOGLE_API_KEY")
                .or_else(|_| std::env::var("GEMINI_API_KEY"))
                .context("GOOGLE_API_KEY (or GEMINI_API_KEY) required for google")?;
            let base = std::env::var("GOOGLE_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::google::GoogleProvider::with_base_url(b, key),
                None => nini_ai::google::GoogleProvider::new(key),
            }))
        }
        "deepseek" => {
            let key = std::env::var("DEEPSEEK_API_KEY")
                .context("DEEPSEEK_API_KEY required for deepseek")?;
            let base = std::env::var("DEEPSEEK_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::deepseek::DeepSeekProvider::with_base_url(b, key),
                None => nini_ai::deepseek::DeepSeekProvider::new(key),
            }))
        }
        "groq" => {
            let key = std::env::var("GROQ_API_KEY")
                .context("GROQ_API_KEY required for groq")?;
            let base = std::env::var("GROQ_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::groq::GroqProvider::with_base_url(b, key),
                None => nini_ai::groq::GroqProvider::new(key),
            }))
        }
        "mistral" => {
            let key = std::env::var("MISTRAL_API_KEY")
                .context("MISTRAL_API_KEY required for mistral")?;
            let base = std::env::var("MISTRAL_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::mistral::MistralProvider::with_base_url(b, key),
                None => nini_ai::mistral::MistralProvider::new(key),
            }))
        }
        "cohere" => {
            let key = std::env::var("COHERE_API_KEY")
                .context("COHERE_API_KEY required for cohere")?;
            let base = std::env::var("COHERE_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::cohere::CohereProvider::with_base_url(b, key),
                None => nini_ai::cohere::CohereProvider::new(key),
            }))
        }
        other => {
            eprintln!("nini: unknown provider: {other}");
            std::process::exit(2);
        }
    }
}

/// Parse scripted turns from `NINI_TUI_FIXTURE_TURNS`.
///
/// Format: a JSON array of arrays of objects. Each inner array is one
/// agent turn's events. Each object has a `kind` field:
///   - `{"kind":"text","text":"..."}`                 → FixtureTurn::Text
///   - `{"kind":"tool","name":"bash","args":{...}}`   → FixtureTurn::ToolCall
///   - `{"kind":"stop","stop_reason":"end_turn"}`     → FixtureTurn::Stop
/// Any other shape fails the parse and the env var is ignored.
fn parse_scripted_turns(raw: &str) -> Option<Vec<Vec<FixtureTurn>>> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let arr = parsed.as_array()?;
    let mut out: Vec<Vec<FixtureTurn>> = Vec::with_capacity(arr.len());
    for turn_value in arr {
        let turn_arr = turn_value.as_array()?;
        let mut turn: Vec<FixtureTurn> = Vec::with_capacity(turn_arr.len());
        for item in turn_arr {
            let obj = item.as_object()?;
            let kind = obj.get("kind")?.as_str()?;
            match kind {
                "text" => turn.push(FixtureTurn::Text(obj.get("text")?.as_str()?.to_string())),
                "tool" => {
                    let name = obj.get("name")?.as_str()?.to_string();
                    let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Null);
                    turn.push(FixtureTurn::ToolCall { name, args });
                }
                "stop" => {
                    let stop_reason = obj
                        .get("stop_reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("end_turn")
                        .to_string();
                    turn.push(FixtureTurn::Stop {
                        stop_reason,
                        usage: Usage::default(),
                    });
                }
                _ => return None,
            }
        }
        out.push(turn);
    }
    Some(out)
}

fn build_tools() -> ToolRegistry {
    let bash: Arc<dyn Tool> = Arc::new(BashTool::new());
    let read: Arc<dyn Tool> = Arc::new(ReadTool::new());
    let write: Arc<dyn Tool> = Arc::new(WriteTool::new());
    let edit: Arc<dyn Tool> = Arc::new(EditTool::new());
    let grep: Arc<dyn Tool> = Arc::new(GrepTool::new());
    let find: Arc<dyn Tool> = Arc::new(FindTool::new());
    let mut reg = ToolRegistry::new();
    reg.register_mut(bash);
    reg.register_mut(read);
    reg.register_mut(write);
    reg.register_mut(edit);
    reg.register_mut(grep);
    reg.register_mut(find);
    reg
}

/// Apply CLI tool filtering (`--tools`, `--exclude-tools`, `--no-tools`,
/// `--no-builtin-tools`) to a registry.
fn filter_tools(
    registry: ToolRegistry,
    allow: &[String],
    deny: &[String],
    no_tools: bool,
    no_builtin: bool,
) -> ToolRegistry {
    let builtin = ["bash", "read", "write", "edit", "grep", "find"];
    let tools: Vec<(String, Arc<dyn Tool>)> = registry
        .tools()
        .map(|t| (t.name().to_string(), t))
        .collect();
    let mut kept: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, tool) in tools {
        if no_tools {
            continue;
        }
        if no_builtin && builtin.contains(&name.as_str()) {
            continue;
        }
        if !allow.is_empty() && !allow.iter().any(|a| a == &name) {
            continue;
        }
        if deny.iter().any(|d| d == &name) {
            continue;
        }
        kept.push(tool);
    }
    let mut out = ToolRegistry::new();
    for tool in kept {
        out.register_mut(tool);
    }
    out
}

fn model_for_cfg(s: &nini_core::settings::Settings) -> String {
    s.model
        .clone()
        .or_else(|| s.provider.clone())
        .unwrap_or_else(|| "test-model".to_string())
}

fn settings_to_system_prompt(
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

async fn run_print(
    user_input: &str,
    provider: &str,
    model: &str,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let settings = load_settings(&cwd);
    let skills = load_skills(&cwd);
    let skills_prompt = format_skills_for_prompt(&skills.skills);
    let cmd = format!("echo {user_input}");
    let turns = vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": cmd}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text(user_input.to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ];
    let provider_impl = build_provider(provider, turns, fallback_keys, fallback_base_urls)?;
    let system = settings_to_system_prompt(&settings, &skills_prompt);
    let tools = build_tools();
    let config = RunConfig {
        model: model.to_string(),
        system: Some(system),
        ..RunConfig::new(model.to_string())
    };
    let mut agent = Agent::new(provider_impl, tools, config);
    let mut stream = std::pin::pin!(agent.run(nini_core::AgentMessage::user(user_input)));
    let mut stdout_text = String::new();
    let mut any_error = false;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text }) => stdout_text.push_str(&text),
            Ok(AgentEvent::Error { message }) => {
                eprintln!("[agent error] {message}");
                any_error = true;
            }
            Ok(AgentEvent::ToolResult { output, .. }) => {
                if output.is_error {
                    eprintln!("[tool stderr]\n{}", output.content);
                    any_error = true;
                }
            }
            Err(e) => {
                eprintln!("[error] {e}");
                any_error = true;
            }
            _ => {}
        }
    }
    print!("{stdout_text}");
    if any_error {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_demo(
    task: &str,
    provider: &str,
    model: &str,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<()> {
    eprintln!("[demo] task: {task}");
    eprintln!("[demo] provider: {provider} | model: {model}");
    eprintln!();
    let cwd = std::env::current_dir()?;
    let settings = load_settings(&cwd);
    let skills = load_skills(&cwd);
    let skills_prompt = format_skills_for_prompt(&skills.skills);
    let task_lower = task.to_lowercase();
    let turns = if task_lower.contains("todo") {
        demo_fix_todos_turns(&cwd)
    } else {
        demo_simple_turns(task)
    };
    let provider_impl = build_provider(provider, turns, fallback_keys, fallback_base_urls)?;
    let system = settings_to_system_prompt(&settings, &skills_prompt);
    let tools = build_tools();
    let config = RunConfig {
        model: model.to_string(),
        system: Some(system),
        ..RunConfig::new(model.to_string())
    };
    let mut agent = Agent::new(provider_impl, tools, config);
    let mut stream = std::pin::pin!(agent.run(nini_core::AgentMessage::user(task)));
    let mut stdout_text = String::new();
    let mut tool_count = 0;
    let mut any_error = false;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text }) => stdout_text.push_str(&text),
            Ok(AgentEvent::ToolCallStart { name, .. }) => {
                eprintln!("[demo] -> tool call: {name}");
            }
            Ok(AgentEvent::ToolResult { output, .. }) => {
                tool_count += 1;
                let preview = output
                    .content
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ");
                eprintln!(
                    "[demo] <- tool result ({} bytes): {preview}",
                    output.content.len()
                );
            }
            Ok(AgentEvent::TurnEnd { .. }) => {
                eprintln!("[demo] -- turn end");
            }
            Ok(AgentEvent::Error { message }) => {
                eprintln!("[demo] !! error: {message}");
                any_error = true;
            }
            Err(e) => {
                eprintln!("[demo] !! agent error: {e}");
                any_error = true;
            }
            _ => {}
        }
    }
    eprintln!();
    eprintln!("[demo] === summary ===");
    eprintln!("[demo] tool calls executed: {tool_count}");
    eprintln!("[demo] assistant text: {stdout_text}");
    if any_error {
        std::process::exit(1);
    }
    Ok(())
}

fn demo_fix_todos_turns(cwd: &std::path::Path) -> Vec<Vec<FixtureTurn>> {
    let target = find_first_file_with_todo(cwd).unwrap_or_else(|| "src/main.rs".to_string());
    vec![
        vec![
            FixtureTurn::ToolCall {
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": "TODO", "path": "."}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::ToolCall {
                name: "read".to_string(),
                args: serde_json::json!({"path": target.clone()}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::ToolCall {
                name: "edit".to_string(),
                args: serde_json::json!({
                    "path": target, "old_text": "// DONE: ", "new_text": "// DONE: "
                }),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo verified"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("Found and fixed TODOs in the codebase.".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]
}

fn demo_simple_turns(task: &str) -> Vec<Vec<FixtureTurn>> {
    vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": format!("echo {task}")}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text(task.to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]
}

fn find_first_file_with_todo(cwd: &std::path::Path) -> Option<String> {
    fn walk(dir: &std::path::Path) -> Option<String> {
        let entries = std::fs::read_dir(dir).ok()?;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(found) = walk(&p) {
                    return Some(found);
                }
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.to_lowercase().contains("todo") {
                        return Some(p.display().to_string());
                    }
                }
            }
        }
        None
    }
    let start = if cwd.join("src").is_dir() {
        cwd.join("src")
    } else {
        cwd.to_path_buf()
    };
    walk(&start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fake_registry() -> ToolRegistry {
        use nini_core::tool::Tool;
        // Build a tiny registry with two fake tools for filter testing.
        // We use real BashTool (so names are realistic) and a placeholder.
        let mut reg = ToolRegistry::new();
        reg.register_mut(Arc::new(BashTool::new()) as Arc<dyn Tool>);
        // ReadTool as a second builtin to verify builtin detection.
        reg.register_mut(Arc::new(ReadTool::new()) as Arc<dyn Tool>);
        reg
    }

    #[test]
    fn filter_tools_no_tools_flag_disables_all() {
        let reg = make_fake_registry();
        let filtered = filter_tools(reg, &[], &[], true, false);
        assert_eq!(filtered.names().len(), 0);
    }

    #[test]
    fn filter_tools_no_builtin_disables_only_builtins() {
        let reg = make_fake_registry();
        let filtered = filter_tools(reg, &[], &[], false, true);
        // bash + read are both builtins, so filtered is empty.
        assert_eq!(filtered.names().len(), 0);
    }

    #[test]
    fn filter_tools_allowlist_keeps_only_listed() {
        let reg = make_fake_registry();
        let filtered = filter_tools(reg, &["bash".to_string()], &[], false, false);
        let names = filtered.names();
        assert_eq!(names, vec!["bash".to_string()]);
    }

    #[test]
    fn filter_tools_denylist_removes_listed() {
        let reg = make_fake_registry();
        let filtered = filter_tools(reg, &[], &["bash".to_string()], false, false);
        let names = filtered.names();
        assert_eq!(names, vec!["read".to_string()]);
    }

    #[test]
    fn filter_tools_no_filters_keeps_all() {
        let reg = make_fake_registry();
        let filtered = filter_tools(reg, &[], &[], false, false);
        assert_eq!(filtered.names().len(), 2);
    }

    #[test]
    fn filter_tools_allowlist_and_denylist_combined() {
        let reg = make_fake_registry();
        // Allow only "bash", but also deny "bash" → result is empty.
        let filtered = filter_tools(reg, &["bash".to_string()], &["bash".to_string()], false, false);
        assert_eq!(filtered.names().len(), 0);
    }
}


fn cli_entry_legacy(entry: &nini_core::SessionEntry) -> Option<nini_core::AgentMessage> {
    use nini_core::entries::{AgentMessage as PiMsg, ContentBlock as PiContentBlock};
    match entry {
        nini_core::SessionEntry::Message(m) => match &m.message {
            PiMsg::User(u) => {
                let blocks: Vec<nini_core::ContentBlock> = match &u.content {
                    nini_core::entries::StringOrContentBlocks::String(s) => vec![nini_core::ContentBlock::Text { text: s.clone() }],
                    nini_core::entries::StringOrContentBlocks::Blocks(bs) => bs.iter().map(|b| match b {
                        PiContentBlock::Text { text } => nini_core::ContentBlock::Text { text: text.clone() },
                        PiContentBlock::ToolCall { id, name, arguments } => nini_core::ContentBlock::ToolUse { id: id.clone(), name: name.clone(), input: arguments.clone() },
                        _ => nini_core::ContentBlock::Text { text: String::new() },
                    }).collect(),
                };
                Some(nini_core::AgentMessage { role: nini_core::Role::User, content: blocks, timestamp: u.timestamp })
            }
            PiMsg::Assistant(a) => {
                let blocks: Vec<nini_core::ContentBlock> = a.content.iter().map(|b| match b {
                    PiContentBlock::Text { text } => nini_core::ContentBlock::Text { text: text.clone() },
                    PiContentBlock::ToolCall { id, name, arguments } => nini_core::ContentBlock::ToolUse { id: id.clone(), name: name.clone(), input: arguments.clone() },
                    _ => nini_core::ContentBlock::Text { text: String::new() },
                }).collect();
                Some(nini_core::AgentMessage { role: nini_core::Role::Assistant, content: blocks, timestamp: a.timestamp })
            }
            _ => None,
        },
        _ => None,
    }
}