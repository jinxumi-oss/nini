//! nini — Pi-compatible Rust coding agent (CLI entry point)
//!
//! Modes:
//! - `nini`                  launch interactive TUI (Phase 5)
//! - `nini -p "..."`         single-shot print mode (no session)
//! - `nini demo [task]`      scripted autonomous demo (no API key needed)
//! - `nini info`             show loaded skills/settings

use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod startup_ui;

// v0.8.2: split from main.rs into focused submodules. The order of
// `mod` declarations matters because some types cross-reference
// (`provider_factory` uses `tool_registry`, `app` uses everything).
pub(crate) mod app;
pub(crate) mod demo;
pub(crate) mod info;
pub(crate) mod prompt_setup;
pub(crate) mod provider_factory;
pub(crate) mod tool_registry;

/// v0.10: the build's identity, embedded by `build.rs`.
///
/// `nini 0.9.1` on its own cannot answer "am I running what I just built",
/// and `~/.local/bin/nini` is a symlink into `target/`, so the question is
/// worth asking. `+dirty` appears when the tree was modified at build time,
/// because a commit id otherwise reads as "exactly this".
fn build_identity() -> String {
    let commit = option_env!("NINI_BUILD_COMMIT").unwrap_or("unknown");
    let when = option_env!("NINI_BUILD_TIME").unwrap_or("unknown");
    let dirty = option_env!("NINI_BUILD_DIRTY").unwrap_or("false");
    let suffix = if dirty == "true" { " +dirty" } else { "" };
    format!("{commit}{suffix} ({when})")
}

/// What `nini --version` prints.
///
/// Pi's is `pi-coding-agent 0.84.2` — package name and version, nothing
/// more, and it ships from npm so there is no build to identify. nini is
/// built from a checkout and can go stale, so it says more.
///
/// Leaks one short string, once, when clap builds its command. `--version`
/// is the only caller.
fn version_line() -> &'static str {
    Box::leak(
        // No leading "nini": clap prints the program name itself, and the
        // first version of this line read `nini nini 0.9.1`.
        format!("{} - {}", env!("CARGO_PKG_VERSION"), build_identity()).into_boxed_str(),
    )
}

#[derive(Parser, Debug)]
#[command(
    name = "nini",
    about = "Pi-compatible Rust coding agent",
    version = version_line()
)]
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
    ///
    /// v0.9.0: was `test-model`, a placeholder that only makes sense
    /// for the fixture provider — so `nini --provider openai-compat`
    /// without `--model` sent a model id no server has heard of and got
    /// a 404. The default is now a real model.
    #[arg(long, default_value = DEFAULT_MODEL)]
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
    /// ~/.nini/agent/sessions/<cwd>/ and loads it.
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
/// v0.9.0: default model.
///
/// Was `test-model` — a placeholder that only means anything to the
/// `fixture` provider, so `nini --provider openai-compat` without an
/// explicit `--model` sent a model id no server has ever heard of and
/// came back 404.
///
/// MiniMax-M3.1-Flash-Preview is a real, fast, cheap model; it is the
/// one you want as a default because an agent's first token matters
/// more than its fifth. Override with `--model` or `/model`.
pub const DEFAULT_MODEL: &str = "MiniMax-M3.1-Flash-Preview";

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
        no_session,
        name,
        fork,
        session_dir,
        api_key,
        system_prompt: _,
        append_system_prompt: _,
        thinking: _,
        tui_mode: _,
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
        no_skills,
        prompt_template: _,
        no_prompt_templates: _,
        theme_file: _,
        use_theme,
        no_themes: _,
        extension: _,
        no_extensions: _,
        no_context_files: _,
    } = args;

    // v0.9.0: these were all `_: _` in the destructure above, i.e.
    // accepted, documented in `--help`, and then dropped on the floor.
    //
    // TUI session persistence is a known, README-declared gap, so the
    // honest thing is to say so at startup rather than let a user
    // believe `--continue-session` restored anything. Silently ignoring
    // a flag is worse than refusing it.
    let mut ignored: Vec<&str> = Vec::new();
    if continue_session {
        ignored.push("--continue-session");
    }
    if session_id.is_some() {
        ignored.push("--session-id");
    }
    if no_session {
        // Harmless: the TUI does not persist sessions anyway, so this
        // flag is already the default behaviour.
        ignored.push("--no-session (already the default)");
    }
    if name.is_some() {
        ignored.push("--name");
    }
    if fork.is_some() {
        ignored.push("--fork");
    }
    if session_dir.is_some() {
        ignored.push("--session-dir");
    }
    if !ignored.is_empty() {
        eprintln!(
            "[nini] ignoring {}: TUI session persistence is not implemented yet \
             (see README section 'Known gaps').",
            ignored.join(", ")
        );
    }

    match cmd {
        Some(Cmd::Demo { task }) => {
            let task = task.unwrap_or_else(|| "find TODOs and fix them".to_string());
            demo::run_demo(
                &task,
                &provider,
                &model,
                &fallback_keys,
                &fallback_base_urls,
                api_key.as_deref(),
            )
            .await
        }
        Some(Cmd::Info) => info::run_info().await,
        _ if list_models.is_some() => {
            // Build runtime from models.json or fall back to defaults.
            let cwd = std::env::current_dir().unwrap_or_default();
            let mj = nini_core::settings::load_models_json(&cwd);
            // The shipped catalog carries the model metadata; `models.json`
            // is configuration and only overrides what it actually states.
            let mut runtime = nini_core::model_runtime::ModelRuntime::with_defaults();
            nini_core::model_runtime::merge_user_overrides(&mut runtime, &mj);
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
                api_key.as_deref(),
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
                api_key: api_key.clone(),
                no_skills,
            })
            .await
        }
    }
}
