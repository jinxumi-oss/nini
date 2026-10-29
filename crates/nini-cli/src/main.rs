//! nini — Pi-compatible Rust coding agent (CLI entry point)
//!
//! Modes:
//! - `nini` launch interactive TUI (Phase 5)
//! - `nini -p "..."` single-shot print mode (no session)
//! - `nini demo [task]` scripted autonomous demo (no API key needed)
//! - `nini info` show loaded skills/settings

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

/// What the build was made of: profile, source, dirt, and when.
///
/// Four facts, because each one has a way of being wrong that only shows
/// up later:
///
/// - **profile** — `release` and `debug` are not the same program. The
///   release profile sets `panic = "abort"`, so the three `catch_unwind`
///   guards in `nini-tui` (clipboard, render-cache lock, signal handler)
///   are inert in the binary we ship and active in the one the 1428 tests
///   run. Without the profile in this line, both binaries claim the same
///   commit and the difference is invisible.
/// - **commit** — which source, because `~/.local/bin/nini` is a symlink
///   into `target/` and used to sit eight fixes behind the tree.
/// - **`+dirty`** — a commit id otherwise reads as "exactly this".
/// - **time** — `target/release/nini` outlives its own build by days, and
///   this is the only line that says so.
///
/// All three git-derived values come from `build.rs`; an explicit
/// environment variable still wins, for vendored builds.
fn build_identity() -> String {
    let commit = option_env!("NINI_BUILD_COMMIT").unwrap_or("unknown");
    let when = option_env!("NINI_BUILD_TIME").unwrap_or("unknown");
    let profile = option_env!("NINI_BUILD_PROFILE").unwrap_or("unknown");
    let dirty = option_env!("NINI_BUILD_DIRTY").unwrap_or("false");
    let suffix = if dirty == "true" { " +dirty" } else { "" };
    format!("{commit}{suffix} ({when}) [{profile}]")
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
        // first version of this line read `nini nini 0.9.1`. The profile
        // rides in `build_identity`, once, at the end — so the two
        // binaries differ in exactly one place rather than two.
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
    /// a 404. The default is now a real model:
    /// `MiniMax-M2.7-highspeed`.
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
    pub tui_mode: Option<String>,

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

    /// Turn cap for one agent run. 0 = no cap (default, Pi semantics).
    ///
    /// v0.11.4: nini used to stop at 50 turns and truncate long tasks
    /// before they delivered. The default is now uncapped; this flag is
    /// how a ceiling is put back.
    #[arg(long, value_name = "N")]
    max_iterations: Option<usize>,

    /// Ceiling on tool calls running at once. 0 = no ceiling (default).
    ///
    /// v0.14: Pi 0.85.1 runs a turn's tool calls all at once, so the
    /// default is no ceiling. This flag is how a ceiling is put back —
    /// nini's own knob, Pi has no counterpart.
    #[arg(long, value_name = "N")]
    max_parallel_tools: Option<usize>,

    /// Not supported. nini renders the export from the TUI's transcript,
    /// which takes no path; `/export` always writes to the exports
    /// directory (see `--help` output of `/export`).
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
/// v0.10.9: `MiniMax-M2.7-highspeed`. It is a real, fast model — and,
/// more to the point, it is the model the operator's `models.json`
/// declares (under the gateway provider that serves it), so the default
/// is a model that actually exists on the other side of the connection
/// instead of one nini has to hope for.
///
/// The id is sent to the provider as-is: nini does not qualify it with a
/// provider prefix, because the provider is chosen separately
/// (`--provider` / `NINI_PROVIDER`) and a prefix would make the id a
/// *different* id to every server that is not the one it was written for.
///
/// Override with `--model` or `/model`.
pub const DEFAULT_MODEL: &str = "MiniMax-M2.7-highspeed";

/// The built-in model catalogue for `nini --list-models` is **gone**.
///
/// It was dead code with a promise attached: the comment said it was the
/// fallback for `--list-models`, and nothing read it. The shipped catalog
/// (`nini-core`) is the real catalogue; the user's `models.json` adds to
/// it. Keeping a second, hand-written list of models next to the
/// generated one is how two answers to one question get written down —
/// so `--list-models` now prints what the installation can actually
/// reach, and only that.
///
/// The `AppConfig` for this invocation, assembled from the parsed flags.
///
/// v0.14: this function exists because the wiring it holds was untested
/// **end to end**. `--max-parallel-tools` was parsed, advertised in
/// `--help`, and could be dropped between `Args` and `AppConfig` without a
/// single test turning red — a contract test checked that the flag was
/// *accepted* (exit code 0), and nothing checked that the *value* went
/// anywhere. "Accepted" and "applied" are different properties, and only
/// the first one was pinned.
///
/// One function means one place where the flags become the config the run
/// actually uses, and a test that can reach that place.
fn app_config_from_args(args: &Args) -> app::AppConfig {
    // Start from the value every "flag not typed" case produces, then let
    // the flags that *were* typed overwrite it.
    let mut cfg = app::AppConfig::for_tool_selection();
    cfg.provider = args.provider.clone();
    cfg.model = args.model.clone();
    cfg.fallback_keys = args.fallback_keys.clone();
    cfg.fallback_base_urls = args.fallback_base_urls.clone();
    cfg.tools = args.tools.clone();
    cfg.exclude_tools = args.exclude_tools.clone();
    cfg.no_tools = args.no_tools;
    cfg.no_builtin_tools = args.no_builtin_tools;
    // v0.10: `--theme-file` and `--use-theme` are the same field.
    // Pi treats a theme value containing a slash as a path, so a file
    // argument and a name argument converge on one value. An explicit
    // file wins.
    cfg.use_theme = args.theme_file.clone().or_else(|| args.use_theme.clone());
    cfg.api_key = args.api_key.clone();
    cfg.no_skills = args.no_skills;
    cfg.system_prompt = args.system_prompt.clone();
    cfg.append_system_prompt =
        (!args.append_system_prompt.is_empty()).then(|| args.append_system_prompt.join("\n\n"));
    cfg.skill_paths = args.skill.iter().map(std::path::PathBuf::from).collect();
    cfg.thinking = args.thinking.clone();
    cfg.max_iterations = args.max_iterations;
    cfg.max_parallel_tools = args.max_parallel_tools;
    cfg.verbose = args.verbose;
    cfg
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Built before the destructure below: the config is a view of the
    // parsed flags, so it has to be taken while `args` is still whole.
    let cli_cfg = app_config_from_args(&args);
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
        tui_mode,
        list_models,
        approve,
        verbose,
        offline,
        export,
        prompt_template,
        no_prompt_templates,
        no_themes,
        extension,
        no_extensions,
        no_context_files,
        ..
    } = args;

    // v0.9.0 introduced the honest-refusal mechanism for flags nini does
    // not implement; v0.10 replaced the single blanket sentence with one
    // reason per flag.
    //
    // The blanket version said *"TUI session persistence is not implemented
    // yet"* about `--offline`, `--no-context-files` and `--theme-file`
    // among others — none of which has anything to do with sessions. A user
    // who typed `--theme-file x.json` and was told their session
    // persistence was unimplemented learns nothing except that the message
    // is not about their flag. Silence is worse than either; a wrong reason
    // is only slightly better than silence and no better than silence once
    // the user has learned to distrust it.
    //
    // Every refusal below states (a) what the flag would have done and
    // (b) what to do instead, or that the feature is not planned. Each is
    // a fact about this binary, not a guess.
    let mut refused: Vec<String> = Vec::new();
    let mut refuse = |flag: &str, why: String| refused.push(format!("{flag} — {why}"));

    // Session flags. These *do* belong to one gap: the CLI predates the
    // TUI's session layer, which exists now but is not wired to these.
    if continue_session {
        refuse(
            "--continue-session",
            "resume the previous session; use `nini --resume` inside the TUI".into(),
        );
    }
    if session_id.is_some() {
        refuse(
            "--session-id",
            "open one session by id; use `/resume` inside the TUI".into(),
        );
    }
    if no_session {
        // Harmless: sessions are always persisted, so this is the default.
        refuse(
            "--no-session",
            "already the default behaviour; nothing to do".into(),
        );
    }
    if name.is_some() {
        refuse(
            "--name",
            "name the session; use `/name` inside the TUI".into(),
        );
    }
    if fork.is_some() {
        refuse(
            "--fork",
            "fork the session; use `/fork` inside the TUI".into(),
        );
    }
    if session_dir.is_some() {
        refuse(
            "--session-dir",
            "choose where sessions are stored; set NINI_AGENT_DIR instead".into(),
        );
    }
    // `--export <path>` used to be destructured as `_` here: accepted,
    // advertised in `--help` as *"Export the session to HTML at this path
    // and exit"*, and dropped on the floor. Measured, not suspected:
    // `nini --export /tmp/x.html -p hello` exited 0, `/tmp/x.html` did not
    // exist, and the HTML landed in `~/.nini/agent/exports/` instead.
    //
    // The reason is structural, and worth stating rather than paraphrasing:
    // `cmd_export` renders `state.transcript_state.lines` and writes to
    // `paths::exports_dir()`. It never reads the path it was handed, so
    // there is nothing to forward the argument to. Exporting from rendered
    // UI state cannot honour a destination — or run headless at all.
    if export.is_some() {
        refuse(
            "--export",
            "export the transcript to a path you choose; `/export` inside the TUI \
             writes to the exports directory (set NINI_AGENT_DIR to move it)"
                .into(),
        );
    }
    // `--tui-mode` is a real Pi flag — `pi --help` lists
    // `--tui-mode <mode>  TUI mode: regular (default) or fullscreen` — and
    // nini advertised it without implementing it. Found by the coverage test
    // below, which asks a different question from the other tests here:
    // *is every advertised flag either used or refused?* rather than *did
    // anyone remember to list this one?*
    //
    // Note this is NOT `settings.fullscreenScrollbar`, which nini does
    // implement (`nini-tui/src/scrollbar.rs`); that is the scrollbar's mode
    // inside the regular layout, not a second layout.
    if tui_mode.is_some() {
        refuse(
            "--tui-mode",
            "choose Pi's fullscreen TUI layout; nini ships only the regular one \
             (the fullscreen scrollbar is a separate setting, and it is supported)"
                .into(),
        );
    }

    // Feature flags with no implementation behind them. Each says what is
    // actually missing, which is the thing a user could otherwise spend an
    // hour guessing at.
    if approve {
        refuse(
            "--approve",
            "trust project files for this run. Trust is enforced per session \
             via `/trust`; there is no command-line equivalent yet"
                .into(),
        );
    }
    if offline {
        refuse(
            "--offline",
            "no network calls. Every provider path makes them; use the `fixture` provider to run without a network".into(),
        );
    }
    if prompt_template.is_some() {
        refuse(
            "--prompt-template",
            "prompt templates are a Pi feature with no nini implementation".into(),
        );
    }
    if no_prompt_templates {
        refuse(
            "--no-prompt-templates",
            "there are no prompt templates to disable".into(),
        );
    }
    if no_themes {
        refuse(
            "--no-themes",
            "nini loads themes/<name>.json on demand but does not enumerate \
             them; pick one with --use-theme <name> instead"
                .into(),
        );
    }
    if !extension.is_empty() {
        refuse(
            "--extension",
            "the extension runtime was removed from scope: it was specified as a Rust cdylib and Pi loads TypeScript, so no amount of wiring would have connected them (docs/plans/v0.10 §26)".into(),
        );
    }
    if no_extensions {
        refuse(
            "--no-extensions",
            "no extensions are loaded, so there is nothing to disable".into(),
        );
    }
    if no_context_files {
        refuse(
            "--no-context-files",
            "AGENTS.md / CLAUDE.md context files are not read by nini; the `--help` text describes a feature that does not exist yet".into(),
        );
    }

    if !refused.is_empty() {
        eprintln!("[nini] {} flag(s) accepted but not applied:", refused.len());
        for line in &refused {
            eprintln!("[nini]   {line}");
        }
    }

    // Once, before any entry point: a subscriber installed after the first
    // request cannot explain the request that failed.
    app::init_logging(verbose);

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
                &cli_cfg,
            )
            .await
        }
        Some(Cmd::Info) => info::run_info().await,
        _ if list_models.is_some() => {
            // Build runtime from models.json or fall back to defaults.
            let cwd = std::env::current_dir().unwrap_or_default();
            let mj = nini_core::settings::load_models_json(&cwd);
            // v0.13: Pi warns about a `models.json` it could not use,
            // and so does nini — same heading, same sentences. Only the
            // **user-level** file is warned about: the project-level one
            // is a catalogue, and a provider in it is never composed.
            let (user_models, load_error) = nini_core::settings::load_user_models_json_reporting();
            let mut warnings = provider_factory::models_json_warnings(&user_models);
            // A file that could not be read or parsed is Pi's own error
            // (`model-config.js:226/233`), and it comes first: the
            // entries in it are not the problem, the file is.
            if let Some(load_error) = load_error {
                warnings.insert(0, load_error);
            }
            if !warnings.is_empty() {
                eprintln!(
                    "Warning: errors loading models.json:\n{}",
                    warnings.join("\n\n")
                );
            }
            // The shipped catalog carries the model metadata; `models.json`
            // is configuration and only overrides what it actually states.
            let mut runtime = nini_core::model_runtime::ModelRuntime::with_defaults();
            nini_core::model_runtime::merge_user_overrides(&mut runtime, &mj);
            // The models the user's `models.json` declares that the
            // shipped catalog does not have. They are models this
            // installation can reach, so a list of what can be reached
            // that omits them is not that list.
            for spec in nini_core::model_runtime::user_model_specs(&mj) {
                runtime.add(spec);
            }
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
                &cli_cfg,
            )
            .await
        }
        None => app::run_tui(cli_cfg).await,
    }
}

/// v0.14: the flag's **effect**, not just its existence.
///
/// `--max-parallel-tools` was parsed, advertised in `--help`, and could be
/// dropped on the way to `RunConfig` without a single test turning red: the
/// contract test checked that the flag was *accepted* (exit code 0), and the
/// unit test checked `AppConfig → RunConfig` with a hand-built `AppConfig`.
/// The hop in between — clap → `Args` → `AppConfig` — was untested, and it
/// is exactly the hop where a value goes to die.
///
/// These tests walk the whole way: the command line, through clap, into the
/// config, into the `RunConfig`, and — where it matters — into the
/// **measured** peak concurrency of a real run.
#[cfg(test)]
mod cli_wiring_tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
    use nini_core::{AgentMessage, Provider, RunConfig};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// How many tool calls the scripted turn asks for.
    const CALLS: usize = 8;

    /// A tool that reports how many of its own bodies were in flight at the
    /// same time. "All N calls finished" cannot tell "N ran at once" from
    /// "3 ran at once and 5 queued".
    struct ProbeTool {
        live: Arc<AtomicU32>,
        peak: Arc<AtomicU32>,
    }

    impl ProbeTool {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                live: Arc::new(AtomicU32::new(0)),
                peak: Arc::new(AtomicU32::new(0)),
            })
        }
    }

    #[async_trait]
    impl Tool for ProbeTool {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "probe".to_string(),
                description: "Records peak concurrency (test only).".to_string(),
                input_schema: json!({"type": "object"}),
            }
        }

        async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            let mut peak = self.peak.load(Ordering::SeqCst);
            while now > peak {
                match self
                    .peak
                    .compare_exchange(peak, now, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => break,
                    Err(actual) => peak = actual,
                }
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("probe done".to_string()))
        }
    }

    /// Parse a command line the way the binary does.
    fn args_from(argv: &[&str]) -> Args {
        let mut full = vec!["nini"];
        full.extend_from_slice(argv);
        Args::try_parse_from(full).expect("these flags parse")
    }

    /// A provider whose first turn asks for `calls` tool calls.
    fn provider_with(calls: usize) -> Arc<dyn Provider> {
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(calls + 1);
        for i in 0..calls {
            turn0.push(FixtureTurn::ToolCall {
                name: "probe".to_string(),
                args: json!({"label": format!("c{i}")}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: nini_core::ProviderUsage::default(),
        });
        let turn1 = vec![FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: nini_core::ProviderUsage::default(),
        }];
        Arc::new(ProgrammedProvider::from_turns(vec![turn0, turn1]))
    }

    /// The `RunConfig` the command line and the settings file produce,
    /// through the same assembly the binary uses.
    fn run_config_for(argv: &[&str], settings: &nini_core::settings::Settings) -> RunConfig {
        let args = args_from(argv);
        let cfg = app_config_from_args(&args);
        let (run_cfg, _) = app::build_tui_run_config(
            settings,
            std::path::Path::new("/tmp"),
            &ToolRegistry::new(),
            &cfg,
        );
        run_cfg
    }

    /// Run one scripted turn and report the tool's own peak concurrency.
    async fn peak_with(run_cfg: RunConfig) -> u32 {
        let probe = ProbeTool::new();
        let peak = probe.peak.clone();
        let tools = ToolRegistry::new().register(probe as Arc<dyn Tool>);
        let mut agent = nini_core::Agent::new(provider_with(CALLS), tools, run_cfg);
        let mut stream = Box::pin(agent.run(AgentMessage::user("go")));
        while let Some(ev) = stream.next().await {
            let _ = ev.expect("agent stream error");
        }
        peak.load(Ordering::SeqCst)
    }

    /// v0.14: the value the user typed has to arrive.
    ///
    /// Three hops, three assertions: clap parsed it, the config carries it,
    /// the `RunConfig` carries it. The middle hop is the one that was
    /// missing — a value that clap parsed and the config then dropped
    /// passes every other test in the repository.
    #[test]
    fn the_max_parallel_tools_flag_reaches_the_run_config() {
        let args = args_from(&["--max-parallel-tools", "3"]);
        assert_eq!(
            args.max_parallel_tools,
            Some(3),
            "clap did not parse the flag's value"
        );
        let cfg = app_config_from_args(&args);
        assert_eq!(
            cfg.max_parallel_tools,
            Some(3),
            "the flag's value did not survive `Args` → `AppConfig`"
        );
        let settings = nini_core::settings::Settings::default();
        let (run_cfg, _) = app::build_tui_run_config(
            &settings,
            std::path::Path::new("/tmp"),
            &ToolRegistry::new(),
            &cfg,
        );
        assert_eq!(
            run_cfg.max_parallel_tools, 3,
            "the flag's value did not reach the `RunConfig`"
        );
    }

    /// v0.14: a ceiling of 3 must actually cap the concurrency at 3.
    ///
    /// Not "the number is 3 somewhere in the config" — the tool measures
    /// its own in-flight bodies, so this is the number of tool calls that
    /// really ran at the same time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_max_parallel_tools_flag_really_caps_the_concurrency() {
        let run_cfg = run_config_for(
            &["--max-parallel-tools", "3"],
            &nini_core::settings::Settings::default(),
        );
        assert_eq!(run_cfg.max_parallel_tools, 3);
        let peak = peak_with(run_cfg).await;
        assert_eq!(
            peak, 3,
            "the ceiling of 3 did not hold — peak concurrency was {peak}"
        );
    }

    /// v0.14: `0` means "no ceiling", the same spelling `--max-iterations`
    /// uses. It must not be read as "a ceiling of zero" (nothing would run)
    /// nor as "unset" (the default is 0 anyway, but the point is that the
    /// flag is *applied*).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_max_parallel_tools_flag_zero_means_no_ceiling() {
        let run_cfg = run_config_for(
            &["--max-parallel-tools", "0"],
            &nini_core::settings::Settings::default(),
        );
        assert_eq!(run_cfg.max_parallel_tools, 0);
        let peak = peak_with(run_cfg).await;
        assert_eq!(
            peak, CALLS as u32,
            "0 must mean 'no ceiling' — all {CALLS} calls should run at once, \
             but peak concurrency was {peak}"
        );
    }

    /// v0.14: the two ceilings are independent (owner ruling: no coupling).
    /// A turn cap says nothing about how many tools one turn may run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_two_ceilings_do_not_interfere() {
        let run_cfg = run_config_for(
            &["--max-parallel-tools", "3", "--max-iterations", "5"],
            &nini_core::settings::Settings::default(),
        );
        assert_eq!(
            run_cfg.max_parallel_tools, 3,
            "--max-iterations changed the tool ceiling"
        );
        assert_eq!(run_cfg.max_iterations, 5);
        let peak = peak_with(run_cfg).await;
        assert_eq!(
            peak, 3,
            "--max-iterations changed the observed concurrency — peak was {peak}"
        );
    }

    /// v0.14: the settings file may say `0` explicitly, and that means
    /// "no ceiling" — it must not be read as "the key is absent" (which
    /// would fall back to the default) and it must not be confused with a
    /// missing value.
    #[test]
    fn a_settings_file_may_say_zero_explicitly() {
        let mut settings = nini_core::settings::Settings::default();
        settings.max_parallel_tools = Some(0);

        // No flag: the file speaks.
        let run_cfg = run_config_for(&[], &settings);
        assert_eq!(
            run_cfg.max_parallel_tools, 0,
            "an explicit `0` in the settings file was not applied"
        );

        // A flag still wins over the file.
        let run_cfg = run_config_for(&["--max-parallel-tools", "4"], &settings);
        assert_eq!(
            run_cfg.max_parallel_tools, 4,
            "the flag did not beat the settings file"
        );
    }
}
