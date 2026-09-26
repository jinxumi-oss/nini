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
            app::run_tui(app::AppConfig {
                provider: provider.clone(),
                model: model.clone(),
                fallback_keys: fallback_keys.clone(),
                fallback_base_urls: fallback_base_urls.clone(),
                tools: tools.clone(),
                exclude_tools: exclude_tools.clone(),
                no_tools,
                no_builtin_tools,
                use_theme: use_theme.clone(),
            }).await
        }
    }
}








