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
        system: Some(prompt_setup::settings_to_system_prompt(
            settings,
            cwd,
            tools,
            &skills_prompt,
        )),
        // v0.10 (R1f): finally hand the persisted thinking level to the
        // provider. `settings.thinking_level` has been written and read
        // back since v0.8, and `/thinking` validates the exact vocabulary
        // Pi uses — but `Request` had no field for it, so nothing was ever
        // transmitted. `None` (unset / unparseable) leaves the field out of
        // the body entirely, which is what Pi does when `reasoning` is
        // absent.
        reasoning: settings
            .thinking_level
            .as_deref()
            .and_then(|l| l.parse::<nini_core::provider::ThinkingLevel>().ok()),
        ..RunConfig::new(cfg_model)
    };
    (config, skills_in_prompt)
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
        cfg.api_key.as_deref(),
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
        fn prepare_next_turn(
            &self,
            _messages: &[nini_core::provider::Message],
        ) -> nini_core::agent_hooks::TurnUpdate {
            nini_core::agent_hooks::TurnUpdate {
                model: self.sink.current_model(),
            }
        }
    }

    let agent_driver: AgentDriver = Arc::new(move |user_msg, sink, done| {
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
                        sink.push(nini_tui::runtime::AgentEventLite::Usage(
                            usage.input_tokens,
                            usage.output_tokens,
                            0.0,
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

    run_tui_bootstrap(agent_driver, &cfg, skills_in_prompt).await
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
            // Seed initial thinking level from settings.
            if let Some(t) = nini_core::settings::load_settings(&cwd).thinking_level {
                state.model_state.thinking_level = Some(t);
            }
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
mod p0_1_skills_tests {
    //! v0.9.2 (P0-1) — "the TUI told the user it loaded their skills, then
    //! built a system prompt containing none of them."
    //!
    //! Why this needed its own seam: the assembly lived inside `run_tui`,
    //! an `async fn` behind a TTY. 937 tests, 0 clippy warnings, and the
    //! user-visible status bar still said "loaded N skills" while the
    //! model had never heard of them.

    use super::*;
    use std::path::PathBuf;

    /// `load_skills` reads `$HOME/.pi/agent/skills`, so tests that touch
    /// it must not run concurrently. (Same reason
    /// `tests/slash_command_e2e.rs` has a HOME_LOCK.)
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        }
    }

    /// Create `<root>/.pi/skills/<name>/SKILL.md` and return `root`.
    fn make_project_with_skill(tmp: &std::path::Path, name: &str, desc: &str) -> PathBuf {
        let dir = tmp.join(".pi").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n\nbody text\n"),
        )
        .unwrap();
        tmp.to_path_buf()
    }

    /// Point `$HOME` at an empty temp dir so the developer's real
    /// `~/.pi/agent/skills` cannot leak into the test.
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
