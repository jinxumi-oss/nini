//! v0.8.2: App factory + TUI bootstrap.
//!
//! The default `nini` (no subcommand) entry path. Owns:
//!   * first-run detection (`startup_ui::*`)
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
    /// v0.9.0: the `--api-key` flag. It was dropped in `main.rs`
    /// (`api_key: _`), which made the help text "API key (overrides env
    /// vars)" false — verified by hand: `--api-key wrong` succeeded when
    /// the env var was right, and `--api-key correct` failed with
    /// "OPENAI_API_KEY required" when it was not.
    pub api_key: Option<String>,
    /// v0.9.2 (P0-3 A-class): the `--no-skills` flag. It was also
    /// dropped in `main.rs` (`no_skills: _`), and `--help` advertises it
    /// as "Disable skill loading". Wired in the same change that started
    /// actually *injecting* skills, because a new always-on feature with
    /// no working opt-out is not shippable.
    pub no_skills: bool,
    /// v0.10 (P0-3 B-class): `--system-prompt`. Replaces the base prompt;
    /// skills and tool descriptions still land on top, as in Pi's
    /// `buildSystemPrompt` (`system-prompt.ts:46-68`).
    pub system_prompt: Option<String>,
    /// v0.10: `--append-system-prompt`. Added to whatever base is in force.
    pub append_system_prompt: Option<String>,
    /// v0.10: `--skill <paths>`. *Overrides* discovery, per its help text.
    pub skill_paths: Vec<std::path::PathBuf>,
    /// v0.10: `--thinking <level>`. Beats the settings file, as in Pi
    /// (`main.ts:487`: "Explicit --thinking still takes precedence").
    pub thinking: Option<String>,
    /// v0.10: `--verbose`. Consumed once by `init_logging` at the top of
    /// `main`, before any entry point runs; kept here so the flag that
    /// produced it and the config that carried it stay in one struct.
    #[allow(dead_code)]
    pub verbose: bool,
}

impl AppConfig {
    /// The subset of the config that tool assembly actually reads.
    ///
    /// v0.10 (fix): `nini -p` and `nini demo` need a config to pass to
    /// `build_run_tools`, but not the model/prompt fields. Naming the
    /// ignored ones here keeps the struct's growth from turning every
    /// call site into a place where a tool flag can be forgotten.
    pub(crate) fn for_tool_selection() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            fallback_keys: Vec::new(),
            fallback_base_urls: Vec::new(),
            tools: Vec::new(),
            exclude_tools: Vec::new(),
            no_tools: false,
            no_builtin_tools: false,
            use_theme: None,
            api_key: None,
            no_skills: false,
            system_prompt: None,
            append_system_prompt: None,
            skill_paths: Vec::new(),
            thinking: None,
            verbose: false,
        }
    }
}

/// Build the `RunConfig` (model + system prompt) for a TUI session,
/// plus the number of skills the prompt actually carries.
///
/// v0.9.2 (P0-1): extracted from `run_tui` purely so it is unit-testable.
/// The system-prompt assembly used to be inlined into an `async fn` that
/// requires a TTY, so **no test could reach it** — which is exactly how
/// the skills block shipped passing `""` while the status bar advertised
/// "loaded N skills" to the user.
///
/// Returning the count alongside the config is deliberate: the status bar
/// renders "loaded N skills", so the number the user sees and the number
/// the model receives must come from the *same* `load_skills` call.
/// Deriving them separately leaves two chances to disagree — the exact bug
/// class this release exists to remove.
///
/// `no_skills` mirrors `--no-skills`.
/// v0.10 (fix): the tool set a run will actually use.
///
/// Two things happen here, in this order, and the order is the point:
///
/// 1. the CLI filters (`--tools`, `--exclude-tools`, `--no-tools`,
///    `--no-builtin-tools`) decide *which* tools exist;
/// 2. the project-trust gate wraps *those* — installed after, so it cannot
///    guard a tool the run will not use, and installed here rather than
///    inside `build_tools` so an extension that registers its own tool
///    cannot arrive ungated.
///
/// Extracted as a function so the sequence can be asserted. The gate was
/// missing from production entirely while every one of its own unit tests
/// passed; an assembly that lives inline in `run_tui` cannot be tested,
/// which is the same seam problem v0.9.2 (P0-1) had to solve for skills.
pub(crate) fn build_run_tools(
    cfg: &AppConfig,
    cwd: &std::path::Path,
) -> nini_core::tool::ToolRegistry {
    nini_core::trust_gate::apply_project_trust(
        tool_registry::filter_tools(
            tool_registry::build_tools(),
            &cfg.tools,
            &cfg.exclude_tools,
            cfg.no_tools,
            cfg.no_builtin_tools,
        ),
        cwd,
    )
}

pub(crate) fn build_tui_run_config(
    settings: &nini_core::settings::Settings,
    cwd: &std::path::Path,
    tools: &nini_core::tool::ToolRegistry,
    cfg: &AppConfig,
) -> (RunConfig, usize) {
    let cfg_model = if !cfg.model.is_empty() {
        cfg.model.clone()
    } else {
        prompt_setup::model_for_cfg(settings)
    };
    // v0.9.2 (P0-1): this used to be `let skills_prompt = "";`.
    let (skills_prompt, skills_in_prompt) = if cfg.no_skills {
        (String::new(), 0)
    } else if !cfg.skill_paths.is_empty() {
        // `--help`: "Overrides automatic skill discovery." A user naming one
        // directory gets that directory — appending it to the discovered set
        // would make the flag unable to narrow anything, which is the only
        // reason to reach for it.
        let skills = nini_core::skills::load_skills_from_paths(&cfg.skill_paths);
        for e in &skills.errors {
            eprintln!("[nini] {e}");
        }
        let n = skills.skills.len();
        (
            nini_core::skills::format_skills_for_prompt(&skills.skills),
            n,
        )
    } else {
        let skills = load_skills(cwd);
        let n = skills.skills.len();
        (
            nini_core::skills::format_skills_for_prompt(&skills.skills),
            n,
        )
    };
    let config = RunConfig {
        model: cfg_model.clone(),
        system: Some(prompt_setup::settings_to_system_prompt_with(
            settings,
            cwd,
            tools,
            &skills_prompt,
            cfg.system_prompt.as_deref(),
            cfg.append_system_prompt.as_deref(),
        )),
        // v0.10 (R1f): hand the persisted thinking level to the provider.
        // `Request` had no field for it, so nothing was transmitted.
        //
        // The comment here used to claim `settings.thinking_level` "has
        // been written and read back since v0.8". It had not: `/thinking`
        // wrote `default_thinking_level`, this read `thinking_level`, and
        // nothing in production ever assigned the latter. Both spellings
        // are accepted, since Pi's own file uses `defaultThinkingLevel`.
        // `None` (unset / unparseable) leaves the field out of the body
        // entirely, which is what Pi does when `reasoning` is absent.
        // The command line beats the file: it was typed for this
        // invocation. Pi orders it the same way (`main.ts:487`).
        reasoning: cfg
            .thinking
            .as_deref()
            .and_then(|l| l.parse::<nini_core::provider::ThinkingLevel>().ok())
            .or_else(|| {
                settings
                    .default_thinking_level
                    .as_deref()
                    .or(settings.thinking_level.as_deref())
                    .and_then(|l| l.parse::<nini_core::provider::ThinkingLevel>().ok())
            }),
        ..RunConfig::new(cfg_model)
    };
    (config, skills_in_prompt)
}

/// Construct an `App` from CLI config and launch the TUI.
///
/// Returns `Ok(())` on clean exit (Ctrl+C, Ctrl+D, /quit) or an
/// error if initialization fails (e.g., unknown provider).
/// Install logging.
///
/// v0.10: called once from `main` before any entry point runs. It used to
/// live inside `run_tui`, which meant `nini -p …` and `nini demo` — the
/// entry points CI uses — initialised no subscriber at all, so `--verbose`
/// produced nothing there. The same shape as the tool-policy gap: wired for
/// one entry point, invisible for the others.
pub(crate) fn init_logging(verbose: bool) {
    match nini_core::logging::log_path() {
        None => {
            if verbose {
                eprintln!(
                    "[nini] --verbose has nowhere to log: no agent directory \
                     (HOME and NINI_AGENT_DIR are both unset)"
                );
            }
        }
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let file = match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                Ok(f) => f,
                Err(e) => {
                    if verbose {
                        eprintln!("[nini] cannot open {}: {e}", path.display());
                    }
                    return;
                }
            };
            // The subscriber is installed once, statically, at DEBUG.
            // `/debug` does not rebuild it — it gates whether the *call
            // sites* emit the fine-grained events at all
            // (`if logging::is_debug() { tracing::debug!(…) }`).
            //
            // The alternative, a dynamic `EnvFilter`/`filter_fn` driven by
            // the same flag, would put the switch in the filter rather than
            // at the emit site: the flag would then decide what is
            // recordable, which is a different claim from "the user asked
            // for more detail", and it would need the filter-reload
            // machinery threaded through every construction site.
            //
            // `MakeWriter` wants `&mut File`, hence the lock inside a
            // wrapper. A poisoned lock discards the line: logging must never
            // be the thing that fails.
            struct Sink(std::sync::Mutex<std::fs::File>);
            // `MakeWriter` is implemented for `Arc<W>` when `&W: Write`, so
            // the impl belongs on the reference, not the value.
            impl std::io::Write for &Sink {
                fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    match self.0.lock() {
                        Ok(mut f) => f.write(buf),
                        Err(_) => Ok(buf.len()),
                    }
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    match self.0.lock() {
                        Ok(mut f) => f.flush(),
                        Err(_) => Ok(()),
                    }
                }
            }
            let _ = tracing_subscriber::fmt()
                .with_writer(std::sync::Arc::new(Sink(std::sync::Mutex::new(file))))
                .with_ansi(false)
                .with_max_level(tracing::Level::DEBUG)
                .try_init();
            if verbose {
                nini_core::logging::set_debug(true);
                tracing::debug!("--verbose: logging to {}", path.display());
            }
        }
    }
}

/// Build the provider, or fall back to the scripted fixture — and say
/// so.
///
/// The fallback exists so the TUI still starts (a user can still read
/// the screen, still use `/help`). What it must not do is *pretend*:
/// the error used to be discarded, and the status bar went on naming
/// the provider the user asked for while the app was talking to a
/// fixture with no scripted turns. The user's message went into the
/// transcript and nothing came back — no reply, no error, no way to
/// tell that the provider never started.
///
/// So the reason travels with the provider, and the TUI shows it.
fn build_provider_or_fallback(
    cfg: &AppConfig,
    scripted_turns: Vec<Vec<nini_ai::fixture::FixtureTurn>>,
) -> (Arc<dyn Provider>, Option<String>) {
    match provider_factory::build_provider(
        &cfg.provider,
        scripted_turns,
        &cfg.fallback_keys,
        &cfg.fallback_base_urls,
        cfg.api_key.as_deref(),
    ) {
        Ok(p) => (p, None),
        Err(e) => {
            let fallback = Arc::new(nini_ai::fixture::ProgrammedProvider::from_turns(
                std::env::var("NINI_TUI_FIXTURE_TURNS")
                    .ok()
                    .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
                    .unwrap_or_default(),
            )) as Arc<dyn Provider>;
            (fallback, Some(e.to_string()))
        }
    }
}

pub(crate) async fn run_tui(cfg: AppConfig) -> Result<()> {
    use nini_tui::runtime::AgentDriver;

    // First run.
    //
    // This used to read `$HOME` itself and hand the *home* directory to
    // functions that wanted the agent directory, so it both ignored
    // `NINI_AGENT_DIR` and looked at the wrong level of the tree. It also
    // could not do anything: `needs_first_time_setup` returned `false` and
    // `init_agent_dir` returned `Ok(())`.
    let agent_dir = nini_core::paths::agent_dir();
    if startup_ui::needs_first_time_setup(agent_dir.as_deref()) {
        if let Some(dir) = agent_dir.as_deref() {
            if let Err(e) = startup_ui::init_agent_dir(dir) {
                eprintln!("[nini] first run: could not create {}: {e}", dir.display());
            }
        }
    }
    let overridden = std::env::var_os(nini_core::paths::ENV_AGENT_DIR).is_some();
    if let Some(notice) = startup_ui::first_run_notice(agent_dir.as_deref(), overridden) {
        eprintln!("[nini] {notice}");
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut settings = load_settings(&cwd);
    if let Some(t) = cfg.use_theme.as_deref() {
        settings.theme = Some(t.to_string());
    }

    let scripted_turns = std::env::var("NINI_TUI_FIXTURE_TURNS")
        .ok()
        .and_then(|raw| provider_factory::parse_scripted_turns(&raw))
        .unwrap_or_default();

    // Provider — fall back to ProgrammedProvider on build error so the
    // TUI still has *something* to talk to.
    //
    // The error used to be dropped (`unwrap_or_else(|_| …)`), and that
    // made the TUI lie: the status bar said `(openai-compat)
    // MiniMax-M3.1-Flash-Preview` while the app was talking to a
    // fixture provider with no scripted turns. The user's message went
    // into the transcript and **nothing came back** — no reply, no
    // error, no way to tell that the provider never started. Measured
    // end to end: `nini` without `OPENAI_API_KEY` is exactly that
    // silence.
    //
    // So the error is kept, and the TUI says so: a notice in the
    // transcript, and a status bar that names the fallback.
    let (shared_provider, provider_error): (Arc<dyn Provider>, Option<String>) =
        build_provider_or_fallback(&cfg, scripted_turns);

    let shared_tools = build_run_tools(&cfg, &cwd);

    // v0.9.2 (P0-1): `build_tui_run_config` injects the skills block and
    // returns how many it advertised, so the status bar and the prompt
    // cannot disagree. This directory used to be read three times per
    // startup (v0.5 loaded once and threw the result away); now once.
    let (shared_cfg, skills_in_prompt) = build_tui_run_config(&settings, &cwd, &shared_tools, &cfg);

    // v0.10 (R1e): a hook that re-syncs the live model into the loop.
    //
    // Pi does this from its session layer
    // (`agent-session.ts:534-554`, `_installAgentNextTurnRefresh`): before
    // every provider request it re-reads `agent.state.model` and pushes it
    // into the loop via `prepareNextTurn`. That is the mechanism behind a
    // model change taking effect *mid-run*.
    //
    // nini needed the same bridge because `RunConfig` is cloned once at
    // `run()` entry: the TUI updated `model_state.model` and the status bar
    // showed the new model, while every request kept going to the old one.
    // The sink is handed to the driver per run, so the hook reads the
    // current selection on each completed turn.
    struct LiveConfigHooks {
        sink: nini_tui::runtime::AgentSink,
    }

    impl nini_core::agent_hooks::AgentLoopHooks for LiveConfigHooks {
        /// v0.10.1 (M1): hand the agent whatever the user typed while a turn
        /// was running.
        ///
        /// v0.10's R1 work built the consumer — `agent.rs` polls this at the
        /// top of every iteration and injects what it returns before the next
        /// model call (`agent-loop.js:96-103`) — and never built a producer.
        /// `LiveConfigHooks` was the only hook implementation in the product
        /// and implemented only `prepare_next_turn`, so the poll always came
        /// back empty: a message submitted mid-run had nowhere to go.
        ///
        /// Pi: `await this.session.prompt(text, { streamingBehavior: "steer" })`
        /// (`interactive-mode.js:2491`).
        fn get_steering_messages(
            &self,
            _recent: &[nini_core::provider::Message],
        ) -> Vec<nini_core::provider::Message> {
            let shared = self.sink.state();
            let queued = {
                let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut g.run_state.pending_next_turn_messages)
            };
            queued
                .into_iter()
                .map(|text| nini_core::provider::Message {
                    role: nini_core::provider::Role::User,
                    content: vec![nini_core::provider::ContentBlock::Text { text }],
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })
                .collect()
        }

        fn prepare_next_turn(
            &self,
            _messages: &[nini_core::provider::Message],
        ) -> nini_core::agent_hooks::TurnUpdate {
            nini_core::agent_hooks::TurnUpdate {
                model: self.sink.current_model(),
            }
        }
    }

    let agent_driver: AgentDriver = Arc::new(move |user_msg, seed, sink, done| {
        let provider = shared_provider.clone();
        let tools = shared_tools.clone();
        let cfg = shared_cfg.clone();
        // v0.10 (R1e): the hooks travel with the sink the runtime just
        // handed us, so the running loop can see model changes.
        let mut agent = Agent::with_hooks(
            provider,
            tools,
            cfg,
            std::sync::Arc::new(LiveConfigHooks { sink: sink.clone() }),
        );
        // v0.10 (R2a/A3): start from the session's live context rather than
        // from nothing. The Agent is built fresh per run, so without this
        // the model saw only the current turn while the session held the
        // whole conversation - and a compaction recorded there had no effect
        // on the next request.
        if !seed.is_empty() {
            agent.seed(seed);
        }

        // v0.10 (R2a/A3b): overflow compaction goes through the session.
        //
        // Without this the Agent compacted its own memory on overflow and
        // recorded nothing, so that was the one case where the model's
        // context changed with no trace on disk. The sink is the only
        // handle the driver gets per run, and it carries the session.
        agent.set_compaction_hook({
            let shared = sink.state();
            nini_core::CompactionHook::new(move |_messages| {
                let keep = {
                    let g = shared.lock().ok()?;
                    g.model_state.settings_snapshot.keep_recent_tokens
                };
                let mut g = shared.lock().ok()?;
                let report = g.session_compact(keep)?;
                let messages = g.session_context_messages();
                drop(g);
                Some(nini_core::CompactionResult {
                    summary: report.summary,
                    tokens_before: report.tokens_before,
                    tokens_after: 0,
                    messages,
                })
            })
        });
        tokio::spawn(async move {
            let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));
            while let Some(ev) = stream.next().await {
                let lite = match ev {
                    Ok(AgentEvent::TextDelta { text }) => {
                        nini_tui::runtime::AgentEventLite::TextDelta(text)
                    }
                    Ok(AgentEvent::ThinkingDelta { text }) => {
                        nini_tui::runtime::AgentEventLite::ThinkingDelta(text)
                    }
                    Ok(AgentEvent::ToolCallStart { id, name }) => {
                        nini_tui::runtime::AgentEventLite::ToolCallStart { id, name }
                    }
                    Ok(AgentEvent::ToolCallStop { id, input_json }) => {
                        nini_tui::runtime::AgentEventLite::ToolCallStop {
                            id,
                            args: input_json.to_string(),
                        }
                    }
                    Ok(AgentEvent::ToolResult { id, name, output }) => {
                        nini_tui::runtime::AgentEventLite::ToolResult {
                            id,
                            name,
                            ok: !output.is_error,
                            content: output.content,
                            details: None,
                            duration_ms: output.duration_ms,
                        }
                    }
                    Ok(AgentEvent::TurnEnd { stop_reason, usage }) => {
                        // v0.10 (R2e): tokens only. The cost used to be
                        // computed here and sent as a third field, which
                        // made it a constant nobody could check; the TUI
                        // prices it against the selected model instead.
                        sink.push(nini_tui::runtime::AgentEventLite::Usage(
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.cache_read_tokens,
                            usage.cache_write_tokens,
                        ));
                        // v0.8.4 (ux-001, pi-parity): surface non-success
                        // stop reasons at the end of the assistant turn.
                        // Pi's `AssistantMessageComponent` shows "Response
                        // was truncated before completion." / "Operation
                        // aborted" / "Error: …" — same idea, one-line
                        // notice rendered in `error` colour so the user
                        // immediately sees the turn did not complete
                        // normally. `end_turn` / `tool_use` are the happy
                        // path so we don't emit anything.
                        match stop_reason.as_str() {
                            "length" => sink.push(nini_tui::runtime::AgentEventLite::StopReason(
                                "Response was truncated before completion.".to_string(),
                            )),
                            "aborted" => sink.push(nini_tui::runtime::AgentEventLite::StopReason(
                                "Operation aborted.".to_string(),
                            )),
                            "error" => sink.push(nini_tui::runtime::AgentEventLite::StopReason(
                                "Error during generation.".to_string(),
                            )),
                            _ => {}
                        }
                        nini_tui::runtime::AgentEventLite::TurnEnd
                    }
                    Ok(AgentEvent::Error { message }) => {
                        nini_tui::runtime::AgentEventLite::Error(message)
                    }
                    Ok(AgentEvent::PhaseChanged(phase)) => {
                        nini_tui::runtime::AgentEventLite::PhaseChanged(phase.to_string())
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        // v0.8.4 (bugfix): a provider error was
                        // previously swallowed by `Err(_) => continue`,
                        // which left the transcript stuck in
                        // Running with no visible feedback. Most often
                        // this is an upstream rate limit (HTTP 429)
                        // or a transport error during stream; either
                        // way the user needs to see *something*. Push
                        // the error to the sink so it lands in the
                        // transcript, then fall through so the loop
                        // exits after this single error (no point
                        // polling a stream that's already errored).
                        eprintln!("[nini] agent stream error: {e}");
                        nini_tui::runtime::AgentEventLite::Error(e.to_string())
                    }
                };
                sink.push(lite);
            }
            sink.push(nini_tui::runtime::AgentEventLite::Done);
            done.notify_waiters();
        })
    });

    run_tui_bootstrap(agent_driver, &cfg, skills_in_prompt, provider_error).await
}

/// Run the TUI itself with the given `AgentDriver`. This is the part
/// that requires a real TTY; if there isn't one, we print the
/// "no TTY detected" help and exit 2.
async fn run_tui_bootstrap(
    agent_driver: nini_tui::runtime::AgentDriver,
    cfg: &AppConfig,
    // v0.9.2 (P0-1): how many skills the system prompt actually carries.
    // Passed in rather than recomputed so the status bar cannot drift
    // from the prompt (see the comment at the use site).
    skills_in_prompt: usize,
    // Why the real provider could not be started, if it could not.
    // The TUI has to say it out loud: a silent fallback is a user who
    // waits for an answer that will never come.
    provider_error: Option<String>,
) -> Result<()> {
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
        eprintln!(
            "Provider: {} (override with NINI_PROVIDER env or --provider)",
            cfg.provider
        );
        std::process::exit(2);
    }

    // v0.8.8 (tool-identity): pass cfg.model as initial_model so the
    // status bar reflects the user's `--model` flag (was hardcoded
    // to "test-model" in v0.7.x). cli/cli.rs's bootstrap closure also
    // sets model_state.model which keeps things consistent.
    run_tui_inner(
        |state| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let dir = cwd.clone();
            state.set_status_bar_metadata(Some(dir.clone()), nini_core::git::git_branch(&dir));
            // v0.8.1: surface the provider name in the status bar
            // (Pi-style `(provider) model`). Helps users tell which
            // backend is active.
            if !cfg.provider.is_empty() {
                state.model_state.provider = Some(cfg.provider.clone());
            }
            // The provider could not be built: say so, in the
            // transcript and in the status bar. The status bar is the
            // part the user reads while waiting, so it must not claim
            // a backend that is not there.
            if let Some(error) = &provider_error {
                state.push_notice(format!(
                    "provider '{}' could not be started: {error}. Falling back to the \
                     scripted fixture provider — replies will not come from a model.",
                    cfg.provider
                ));
                state.model_state.provider = Some("fixture (fallback)".to_string());
                state.model_state.model = "no model".to_string();
            }
            // Seed initial thinking level from settings.
            // Reads the field `/thinking` actually writes. This read
            // `thinking_level`, which nothing wrote, so the status bar
            // started every session at "off" no matter what was saved.
            let saved = nini_core::settings::load_settings(&cwd);
            if let Some(t) = saved.default_thinking_level.or(saved.thinking_level) {
                state.model_state.thinking_level = Some(t);
            }
            // v0.10: `settings.fullscreenScrollbar`, matching Pi's
            // `getFullscreenScrollbar()` (`settings-manager.ts:1151`).
            // Seeded here for the same reason as the thinking level: the
            // setting exists, the renderer needs it, and nothing else is a
            // place where both are in scope. Unset means Pi's default,
            // `auto`.
            state.ui_state.fullscreen_scrollbar = nini_tui::scrollbar::ScrollbarMode::from_setting(
                saved.fullscreen_scrollbar.as_deref(),
            );
            // v0.9.2 (P0-1): this used to call `load_skills` a THIRD
            // time (the v0.5 version loaded once and threw the result
            // away; `build_tui_run_config` loads again). Three
            // independent reads of the same directory is three chances
            // for the status bar and the system prompt to disagree —
            // which is precisely the bug this release is about.
            //
            // Use the count computed once in `build_tui_run_config`'s
            // call site, and respect `--no-skills`. Reporting
            // "loaded N skills" while the prompt carries none is the
            // same lie in a new place.
            if skills_in_prompt > 0 {
                state.run_state.status =
                    format!("loaded {skills_in_prompt} skills — type to begin");
            }
        },
        agent_driver,
        &cfg.model,
    )
    .await
}

#[cfg(test)]
/// Shared by the skills tests and the flag-wiring tests.
///
/// `load_skills` reads `$HOME/.nini/agent/skills`, so any test that touches
/// it must serialise against every other test that does — two suites with
/// two locks would be two suites that could interleave.
pub(crate) mod p0_1_skills_tests_support {
    pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point HOME (and the agent directory) at a scratch directory.
    pub(crate) fn isolate_home(root: &std::path::Path) {
        unsafe {
            std::env::set_var("HOME", root);
            std::env::set_var("NINI_AGENT_DIR", root.join(".nini").join("agent"));
        }
    }

    /// Create `<root>/.nini/skills/<name>/SKILL.md` and return `<root>`.
    pub(crate) fn make_project_with_skill(
        root: &std::path::Path,
        name: &str,
        desc: &str,
    ) -> std::path::PathBuf {
        let dir = root.join(".nini").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\nbody\n"),
        )
        .unwrap();
        root.to_path_buf()
    }
}

#[cfg(test)]
mod p0_1_skills_tests {
    //! v0.9.2 (P0-1) — "the TUI told the user it loaded their skills, then
    //! built a system prompt containing none of them."
    //!
    //! Why this needed its own seam: the assembly lived inside `run_tui`,
    //! an `async fn` behind a TTY. 937 tests, 0 clippy warnings, and the
    //! user-visible status bar still said "loaded N skills" while the
    //! model had never heard of them.

    use super::p0_1_skills_tests_support::*;
    use super::*;
    use std::path::PathBuf;

    fn base_config() -> AppConfig {
        AppConfig {
            provider: String::new(),
            model: "test-model".to_string(),
            fallback_keys: vec![],
            fallback_base_urls: vec![],
            tools: vec![],
            exclude_tools: vec![],
            no_tools: false,
            no_builtin_tools: false,
            use_theme: None,
            api_key: None,
            no_skills: false,
            system_prompt: None,
            append_system_prompt: None,
            skill_paths: Vec::new(),
            thinking: None,
            verbose: false,
        }
    }

    /// Create `<root>/.pi/skills/<name>/SKILL.md` and return `root`.
    fn make_project_with_skill(tmp: &std::path::Path, name: &str, desc: &str) -> PathBuf {
        let dir = tmp.join(".nini").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\nbody text\n"),
        )
        .unwrap();
        tmp.to_path_buf()
    }

    /// Point `$HOME` at an empty temp dir so the developer's real
    /// `~/.nini/agent/skills` cannot leak into the test.
    fn isolate_home(tmp: &std::path::Path) {
        let home = tmp.join("fake-home");
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);
    }

    fn system_of(t: (RunConfig, usize)) -> String {
        t.0.system.unwrap_or_default()
    }

    /// The count the status bar will render, as reported by the same
    /// call that built the prompt.
    fn advertised_count(t: (RunConfig, usize)) -> usize {
        t.1
    }

    // ---------------------------------------------------------------
    // The regression itself.
    // ---------------------------------------------------------------

    #[test]
    fn tui_system_prompt_actually_contains_the_skills_block() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "p0-one-skill", "does a p0 one thing");

        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let cfg = build_tui_run_config(&settings, &cwd, &tools, &base_config());
        let system = system_of(cfg);

        assert!(
            system.contains("<available_skills>"),
            "system prompt has no skills block at all.\n--- got ---\n{system}"
        );
        assert!(
            system.contains("p0-one-skill"),
            "the skill name the status bar advertised is absent from the system prompt.\n--- got ---\n{system}"
        );
        assert!(
            system.contains("does a p0 one thing"),
            "skill description missing from the system prompt.\n--- got ---\n{system}"
        );
        // The model needs the path so it can `read` the skill.
        assert!(
            system.contains("SKILL.md"),
            "skills are advertised without a <location>, so the model cannot load them.\n--- got ---\n{system}"
        );
    }

    /// The status bar says "loaded N skills". If this count and the
    /// prompt disagree, we are lying to the user in one place while
    /// lying to the model in another.
    #[test]
    fn skills_visible_to_user_match_skills_in_prompt() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "alpha-skill", "first");
        make_project_with_skill(tmp.path(), "beta-skill", "second");

        let loaded = load_skills(&cwd);
        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let system = system_of(build_tui_run_config(
            &settings,
            &cwd,
            &tools,
            &base_config(),
        ));

        for s in &loaded.skills {
            assert!(
                system.contains(&s.name),
                "status bar advertises skill {:?} but the prompt omits it",
                s.name
            );
        }
        assert!(
            loaded.skills.len() >= 2,
            "fixture should have produced >=2 skills, got {:?}",
            loaded.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    // ---------------------------------------------------------------
    // The opt-out (--no-skills, P0-3 A-class) must actually work.
    // ---------------------------------------------------------------

    #[test]
    fn no_skills_flag_suppresses_the_block() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "suppressed-skill", "should not appear");

        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let mut cfg = base_config();
        cfg.no_skills = true;
        let system = system_of(build_tui_run_config(&settings, &cwd, &tools, &cfg));

        assert!(
            !system.contains("<available_skills>"),
            "--no-skills did not suppress the skills block.\n--- got ---\n{system}"
        );
        assert!(
            !system.contains("suppressed-skill"),
            "--no-skills leaked a skill name into the prompt.\n--- got ---\n{system}"
        );
    }

    // ---------------------------------------------------------------
    // Guard: do not regress the rest of the prompt while fixing this.
    // ---------------------------------------------------------------

    #[test]
    fn skills_fix_preserves_the_working_directory_hint() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "cwd-hint-skill", "x");

        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let system = system_of(build_tui_run_config(
            &settings,
            &cwd,
            &tools,
            &base_config(),
        ));

        // v0.8.5 added this because models otherwise guessed absolute
        // paths and the explicit-path tools failed.
        assert!(
            system.contains(&cwd.display().to_string()),
            "the working-directory hint disappeared.\n--- got ---\n{system}"
        );
    }

    #[test]
    fn empty_project_yields_no_skills_block_and_no_panic() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = tmp.path().join("empty");
        std::fs::create_dir_all(&cwd).unwrap();

        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let system = system_of(build_tui_run_config(
            &settings,
            &cwd,
            &tools,
            &base_config(),
        ));

        assert!(!system.contains("<available_skills>"));
        assert!(system.contains("You are nini"));
    }

    /// v0.9.2 (P0-1) follow-up, found while implementing the fix.
    ///
    /// The v0.9.1 fix would have introduced a *new* instance of the very
    /// bug it was fixing: `--no-skills` suppressed the prompt, but the
    /// status bar still computed its own count and announced
    /// "loaded N skills". Two sources of truth, one of them lying.
    ///
    /// This asserts the invariant directly: the number the status bar
    /// reports and the number of skills the prompt carries must be the
    /// same number, in both flag states.
    #[test]
    fn status_bar_count_and_prompt_contents_agree_under_both_flag_states() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "agree-a", "a");
        make_project_with_skill(tmp.path(), "agree-b", "b");
        make_project_with_skill(tmp.path(), "agree-c", "c");

        let settings = load_settings(&cwd);
        let tools = tool_registry::build_tools();
        let on_disk = load_skills(&cwd).skills.len();
        assert!(on_disk >= 3);

        // --- flag OFF: status bar count == prompt count == on-disk count
        let t_on = build_tui_run_config(&settings, &cwd, &tools, &base_config());
        let in_prompt = system_of(t_on.clone()).matches("<skill>").count();
        assert_eq!(
            in_prompt, on_disk,
            "prompt advertises {in_prompt} skills, {on_disk} exist on disk"
        );
        // `advertised_count` is the exact value run_tui hands to the
        // status bar, so this compares the two real production outputs
        // rather than two literals. (An earlier draft of this test wrote
        // `let status_count = on_disk;` and asserted it equalled
        // `in_prompt` — a tautology that would stay green no matter how
        // broken the code under test became.)
        assert_eq!(
            advertised_count(t_on.clone()),
            in_prompt,
            "status bar count and prompt contents disagree with skills enabled"
        );
        assert_eq!(advertised_count(t_on), on_disk);

        // --- flag ON: BOTH must be zero. Reporting "loaded 3 skills"
        //     while the prompt carries none is the same lie, relocated.
        let mut cfg = base_config();
        cfg.no_skills = true;
        let t_off = build_tui_run_config(&settings, &cwd, &tools, &cfg);
        assert_eq!(
            system_of(t_off.clone()).matches("<skill>").count(),
            0,
            "--no-skills still advertised skills"
        );
        // Because the count is 0, the status-bar branch
        // `if skills_in_prompt > 0` is skipped and the message never
        // renders at all.
        assert_eq!(advertised_count(t_off.clone()), 0);
        assert_eq!(
            advertised_count(t_off),
            system_of(build_tui_run_config(&settings, &cwd, &tools, &cfg))
                .matches("<skill>")
                .count(),
            "status bar count and prompt contents disagree under --no-skills"
        );
    }
}

#[cfg(test)]
mod trust_wiring_tests {
    //! v0.10 (fix) — the trust gate existed, `/trust` existed, and nothing
    //! in a real run ever consulted the gate.
    //!
    //! This module asserts the *assembly*, not the gate. The gate itself is
    //! covered in `nini-core/tests/trust_gate.rs`; what only exists here is
    //! the fact that the run's tools are the gated ones. A test that built
    //! its own registry would have passed against a binary that never called
    //! `apply_project_trust` at all.

    use super::*;

    fn cfg() -> AppConfig {
        AppConfig {
            provider: "fixture".into(),
            model: "test-model".into(),
            fallback_keys: Vec::new(),
            fallback_base_urls: Vec::new(),
            tools: Vec::new(),
            exclude_tools: Vec::new(),
            no_tools: false,
            no_builtin_tools: false,
            use_theme: None,
            api_key: None,
            no_skills: false,
            system_prompt: None,
            append_system_prompt: None,
            skill_paths: Vec::new(),
            thinking: None,
            verbose: false,
        }
    }

    #[test]
    fn the_run_tools_carry_a_before_hook() {
        let tools = build_run_tools(&cfg(), std::path::Path::new("/tmp"));
        let mut gated = 0;
        let mut total = 0;
        for t in tools.tools() {
            total += 1;
            if t.before().is_some() {
                gated += 1;
            }
        }
        assert_eq!(total, 6, "the six built-ins should survive the filters");
        assert_eq!(
            gated, total,
            "{gated}/{total} tools carry the trust gate — a run built from \
             these tools would not enforce `/trust`"
        );
    }

    /// The filters run first, so the gate is installed on exactly the tools
    /// the run will use — no more (guarding tools that never run) and no
    /// fewer.
    #[test]
    fn the_gate_follows_the_cli_filters() {
        let mut c = cfg();
        c.exclude_tools = vec!["bash".into(), "write".into()];
        let tools = build_run_tools(&c, std::path::Path::new("/tmp"));
        let names: Vec<String> = tools.tools().map(|t| t.name().to_string()).collect();
        assert_eq!(names.len(), 4, "exclusions were not applied: {names:?}");
        for n in &names {
            assert!(
                !n.contains("bash") && !n.contains("write"),
                "excluded tool survived: {names:?}"
            );
        }
        for t in tools.tools() {
            assert!(t.before().is_some(), "{} is ungated", t.name());
        }
    }

    /// `apply_project_trust` returns the registry untouched when there is no
    /// agent directory at all (a tarball install, a CI sandbox). That path
    /// must not panic and must not drop tools.
    #[test]
    fn a_missing_agent_directory_still_yields_working_tools() {
        let tools = build_run_tools(&cfg(), std::path::Path::new("/tmp"));
        assert_eq!(tools.tools().count(), 6);
    }
}

#[cfg(test)]
mod print_mode_tool_tests {
    //! v0.10 (fix) — `nini -p` and `nini demo` called
    //! `tool_registry::build_tools()` directly.
    //!
    //! Two consequences, both invisible from the TUI:
    //!
    //! 1. `--tools`, `--exclude-tools`, `--no-tools` and `--no-builtin-tools`
    //!    were accepted, documented in `--help`, and applied to nothing.
    //!    `nini --no-tools -p "…"` still gave the model six tools.
    //! 2. The project trust gate, installed in the TUI path, was absent —
    //!    so a directory the user marked untrusted could still be written by
    //!    every scripted and CI invocation.
    //!
    //! These assert the *shared* assembly, which is the point: one function
    //! builds the tool set for every entry point, so the next entry point
    //! cannot quietly get a different one.

    use super::*;

    fn cfg_with(exclude: &[&str]) -> AppConfig {
        let mut c = AppConfig::for_tool_selection();
        c.exclude_tools = exclude.iter().map(|s| s.to_string()).collect();
        c
    }

    #[test]
    fn print_mode_honours_exclude_tools() {
        let tools = build_run_tools(&cfg_with(&["bash"]), std::path::Path::new("/tmp"));
        let names: Vec<String> = tools.tools().map(|t| t.name().to_string()).collect();
        assert!(
            !names.iter().any(|n| n == "bash"),
            "`nini --exclude-tools bash -p …` still had bash: {names:?}"
        );
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn print_mode_honours_no_tools() {
        let mut c = AppConfig::for_tool_selection();
        c.no_tools = true;
        let tools = build_run_tools(&c, std::path::Path::new("/tmp"));
        assert_eq!(tools.tools().count(), 0, "`--no-tools` still had tools");
    }

    #[test]
    fn print_mode_carries_the_trust_gate_like_the_tui() {
        let tools = build_run_tools(&cfg_with(&[]), std::path::Path::new("/tmp"));
        for t in tools.tools() {
            assert!(
                t.before().is_some(),
                "{} is ungated in the -p path",
                t.name()
            );
        }
    }

    /// The constructor exists so the two callers cannot drift apart; this is
    /// the property that makes it worth having.
    #[test]
    fn the_default_tool_config_excludes_nothing() {
        let c = AppConfig::for_tool_selection();
        assert!(c.tools.is_empty() && c.exclude_tools.is_empty());
        assert!(!c.no_tools && !c.no_builtin_tools);
        assert_eq!(
            build_run_tools(&c, std::path::Path::new("/tmp"))
                .tools()
                .count(),
            6
        );
    }
}

#[cfg(test)]
mod cli_flag_wiring_tests {
    //! v0.10: five flags were accepted, documented in `--help`, and dropped at
    //! the CLI boundary.
    //!
    //! `prompt_setup.rs` has unit tests for the assembler's behaviour — that
    //! a custom prompt replaces the base, that an append lands after it. All
    //! of them passed while the flags were discarded, because the assembler
    //! was never handed anything to do. These tests are about the wiring: the
    //! values reach the assembler at all.

    use super::super::app::p0_1_skills_tests_support::*;
    use super::*;

    fn cfg_with_system(custom: Option<&str>, append: Option<&str>) -> AppConfig {
        let mut c = AppConfig::for_tool_selection();
        c.system_prompt = custom.map(str::to_string);
        c.append_system_prompt = append.map(str::to_string);
        c
    }

    fn system_of(cfg: &AppConfig) -> String {
        let settings = nini_core::settings::Settings::default();
        let (run_cfg, _) = build_tui_run_config(
            &settings,
            std::path::Path::new("/tmp"),
            &nini_core::tool::ToolRegistry::new(),
            cfg,
        );
        run_cfg.system.unwrap_or_default()
    }

    #[test]
    fn system_prompt_reaches_the_run_config() {
        let system = system_of(&cfg_with_system(Some("CUSTOM-BASE"), None));
        assert!(
            system.contains("CUSTOM-BASE"),
            "--system-prompt never reached the prompt: {system}"
        );
        assert!(
            !system.contains("You are nini, a Pi-compatible Rust coding agent."),
            "the default base is still being sent alongside the custom one"
        );
    }

    #[test]
    fn append_system_prompt_reaches_the_run_config() {
        let system = system_of(&cfg_with_system(None, Some("APPEND-ONLY")));
        assert!(
            system.contains("APPEND-ONLY"),
            "--append-system-prompt never reached the prompt: {system}"
        );
        assert!(
            system.contains("You are nini, a Pi-compatible Rust coding agent."),
            "an append must not replace the base"
        );
    }

    /// `--thinking` beats the settings file. Pi orders it the same way
    /// (`main.ts:487`: "Explicit --thinking still takes precedence"), and a
    /// user who typed it wants it for this run.
    #[test]
    fn thinking_on_the_command_line_beats_the_settings_file() {
        let mut settings = nini_core::settings::Settings::default();
        settings.default_thinking_level = Some("low".into());

        let mut cfg = AppConfig::for_tool_selection();
        cfg.thinking = Some("high".into());
        let (run_cfg, _) = build_tui_run_config(
            &settings,
            std::path::Path::new("/tmp"),
            &nini_core::tool::ToolRegistry::new(),
            &cfg,
        );
        assert_eq!(
            run_cfg.reasoning,
            Some(nini_core::provider::ThinkingLevel::High),
            "the settings file won over the flag"
        );

        // With no flag the file still applies.
        let (from_file, _) = build_tui_run_config(
            &settings,
            std::path::Path::new("/tmp"),
            &nini_core::tool::ToolRegistry::new(),
            &AppConfig::for_tool_selection(),
        );
        assert_eq!(
            from_file.reasoning,
            Some(nini_core::provider::ThinkingLevel::Low)
        );
    }

    /// `--skill` *overrides* discovery, as its help text says. This is the
    /// only reason to reach for the flag: narrowing.
    #[test]
    fn skill_paths_override_automatic_discovery() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        isolate_home(tmp.path());
        let cwd = make_project_with_skill(tmp.path(), "discovered", "found automatically");

        // Discovery alone finds the project skill.
        let (auto_cfg, auto_n) = build_tui_run_config(
            &nini_core::settings::Settings::default(),
            &cwd,
            &nini_core::tool::ToolRegistry::new(),
            &AppConfig::for_tool_selection(),
        );
        assert_eq!(auto_n, 1);
        assert!(auto_cfg.system.unwrap().contains("discovered"));

        // Naming a different directory replaces it.
        let other = tmp.path().join("other-skills");
        let one = other.join("named");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::write(
            one.join("SKILL.md"),
            "---\nname: named\ndescription: chosen by flag\n---\n\nbody\n",
        )
        .unwrap();

        let mut cfg = AppConfig::for_tool_selection();
        cfg.skill_paths = vec![other];
        let (flag_cfg, flag_n) = build_tui_run_config(
            &nini_core::settings::Settings::default(),
            &cwd,
            &nini_core::tool::ToolRegistry::new(),
            &cfg,
        );
        assert_eq!(flag_n, 1, "--skill did not override discovery");
        let system = flag_cfg.system.unwrap();
        assert!(system.contains("named"), "{system}");
        assert!(
            !system.contains("discovered"),
            "the discovered skill survived an explicit --skill: {system}"
        );
    }
}

#[cfg(test)]
mod provider_fallback_tests {
    use super::*;

    fn cfg_with(provider: &str) -> AppConfig {
        AppConfig {
            provider: provider.to_string(),
            model: "MiniMax-M3.1-Flash-Preview".to_string(),
            fallback_keys: Vec::new(),
            fallback_base_urls: Vec::new(),
            tools: Vec::new(),
            exclude_tools: Vec::new(),
            no_tools: false,
            no_builtin_tools: false,
            use_theme: None,
            api_key: None,
            no_skills: false,
            system_prompt: None,
            append_system_prompt: None,
            skill_paths: Vec::new(),
            thinking: None,
            verbose: false,
        }
    }

    /// A provider that cannot be started must say so.
    ///
    /// The failure used to be dropped, and the TUI went on claiming a
    /// backend that was not there: the user typed a message, the
    /// transcript showed it, and nothing ever came back. Silence is
    /// the worst possible error report, because the user cannot tell
    /// "the model is slow" from "the model is not there".
    #[test]
    fn a_provider_that_cannot_be_started_is_reported() {
        let mut cfg = cfg_with("openai-compat");
        // An empty key never resolves: the provider cannot be built.
        cfg.api_key = Some(String::new());
        let (_provider, error) = build_provider_or_fallback(&cfg, Vec::new());
        let error = error.expect("the reason must not be swallowed");
        assert!(
            !error.trim().is_empty(),
            "the reason must say what went wrong, not just that something did"
        );
    }

    /// The other half: a provider that *can* be started reports no
    /// error, so the TUI does not cry wolf.
    #[test]
    fn a_provider_that_starts_reports_no_error() {
        let cfg = cfg_with("fixture");
        let (_provider, error) = build_provider_or_fallback(&cfg, Vec::new());
        assert!(
            error.is_none(),
            "a working provider must not produce a fallback notice: {error:?}"
        );
    }
}
