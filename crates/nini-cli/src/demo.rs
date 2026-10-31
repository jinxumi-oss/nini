//! v0.8.2: Demo + print (single-shot) execution paths.
//!
//! Extracted from `main.rs`. Two public functions:
//!   * `run_demo()`  — scripted autonomous run (no API key needed)
//!   * `run_print()` — single-shot `-p "task"` invocation
//!
//! Plus 4 fixture builders + 2 helpers used only by the demo paths.

use anyhow::Result;
use futures_util::StreamExt;

use nini_ai::fixture::FixtureTurn;
use nini_core::settings::load_settings;
use nini_core::skills::{format_skills_for_prompt, load_skills};
use nini_core::{Agent, AgentEvent, RunConfig};

use crate::{prompt_setup, provider_factory};

/// The `RunConfig` of a single-shot entry point.
///
/// v0.14 (round 3): the two entries (`nini demo` and `nini -p`) used to
/// **inline** the same `RunConfig` literal, and nothing in the repository
/// tested either of them. A mutation that hard-wrote
/// `max_parallel_tools: 0` — dropping the command line's `--max-parallel-
/// tools` and the settings file's `maxParallelTools` — left every test in
/// the repository green, because the CLI's own wiring tests only walked
/// the TUI's assembly (`app::build_tui_run_config`).
///
/// The assembly now lives in one function with two named doors, and each
/// door has a test that can actually fail.
///
/// The precedence is the same as everywhere else: the command line beats
/// the settings file (it was typed for this invocation), and the fallback
/// is "no ceiling" — which is what Pi 0.85.1 does.
fn assemble_run_config(
    tool_cfg: &crate::app::AppConfig,
    settings: &nini_core::settings::Settings,
    model: &str,
    system: String,
) -> RunConfig {
    RunConfig {
        model: model.to_string(),
        system: Some(system),
        // v0.11.4 (P0/Pi parity): the same turn cap the TUI uses — a
        // restriction that holds in the TUI and evaporates in `-p` is not
        // a restriction. Default: no cap (Pi 0.85.1 has no turn limit).
        max_iterations: tool_cfg
            .max_iterations
            .or(settings.max_iterations)
            .unwrap_or(nini_core::agent::MAX_TOOL_ITERATIONS),
        // v0.14: the tool-concurrency ceiling. The default is "no
        // ceiling" — Pi 0.85.1 runs a turn's tool calls all at once.
        max_parallel_tools: tool_cfg
            .max_parallel_tools
            .or(settings.max_parallel_tools)
            .unwrap_or(nini_core::agent::DEFAULT_MAX_PARALLEL_TOOLS),
        // v0.15: the batch-level tool execution mode. Same shape as the
        // ceiling above — the command line beats the settings file, and
        // the fallback is `parallel`, which is how a turn runs when
        // nobody declared anything about it.
        tool_execution: tool_cfg
            .tool_execution
            .or(settings.tool_execution)
            .unwrap_or(nini_core::tool::ToolExecutionMode::default()),
        ..RunConfig::new(model.to_string())
    }
}

/// The `RunConfig` of the `nini demo` entry.
///
/// A named door, so a test can knock on **this** one and not only on the
/// TUI's.
pub(crate) fn demo_run_config(
    tool_cfg: &crate::app::AppConfig,
    settings: &nini_core::settings::Settings,
    model: &str,
    system: String,
) -> RunConfig {
    assemble_run_config(tool_cfg, settings, model, system)
}

/// The `RunConfig` of the `nini -p` entry.
///
/// `nini -p "..."` is the entry every CI job and every script uses, so a
/// value that dies here is a value that dies in everybody's CI.
pub(crate) fn print_run_config(
    tool_cfg: &crate::app::AppConfig,
    settings: &nini_core::settings::Settings,
    model: &str,
    system: String,
) -> RunConfig {
    assemble_run_config(tool_cfg, settings, model, system)
}

/// Scripted autonomous demo run (no API key needed).
///
/// `task` is parsed loosely: "TODO" substring triggers
/// `demo_fix_todos_turns()` fixture, anything else triggers
/// `demo_simple_turns()`. Output is streamed to stderr with
/// `[demo] -> tool call`, `[demo] <- tool result`, etc.
pub(crate) async fn run_demo(
    task: &str,
    provider: &str,
    model: &str,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
    api_key: Option<&str>,
    tool_cfg: &crate::app::AppConfig,
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
    // v0.13: only the **user-level** `models.json` configures a
    // provider. The project-level file is a catalogue, not a
    // configuration.
    let user_models = nini_core::settings::load_user_models_json();
    let provider_impl = provider_factory::build_provider(
        provider,
        turns,
        fallback_keys,
        fallback_base_urls,
        api_key,
        Some(&user_models),
    )?;
    // v0.10 (fix): the same assembly the TUI uses. This called
    // `build_tools()` directly, which meant `nini -p "..."` — the entry
    // point every CI job and script uses — silently ignored
    // `--tools`/`--exclude-tools`/`--no-tools` *and* the project trust
    // decision. A restriction that holds in the TUI and evaporates in
    // `-p` is not a restriction.
    let tools = crate::app::build_run_tools(tool_cfg, &cwd);
    let system = prompt_setup::settings_to_system_prompt(&settings, &cwd, &tools, &skills_prompt);
    let config = demo_run_config(tool_cfg, &settings, model, system);
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

/// Single-shot invocation: `nini -p "task"`.
///
/// Streams all events to stdout + stderr; the assistant's final text
/// goes to stdout (printable), tool calls/results go to stderr.
/// Wrap `text` so that no line is wider than the terminal, breaking at
/// character boundaries.
///
/// The terminal's last column is a hard edge for a wide character: it
/// needs two columns, and with one left the terminal folds the line
/// *before* the character. A fold in the middle of an answer is a broken
/// answer, so the wrap happens here, where the width is known.
fn wrap_to_terminal_width(text: &str) -> String {
    let cols = nini_tui::width::terminal_cols();
    wrap_to_width(text, cols)
}

/// The same wrap, with the width given — so it can be tested without a
/// terminal.
fn wrap_to_width(text: &str, cols: usize) -> String {
    if cols == 0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut width = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            out.push(ch);
            width = 0;
            continue;
        }
        let cw = nini_tui::width::display_width(&ch.to_string());
        if width + cw > cols {
            out.push('\n');
            width = 0;
        }
        out.push(ch);
        width += cw;
    }
    out
}

pub(crate) async fn run_print(
    user_input: &str,
    provider: &str,
    model: &str,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
    api_key: Option<&str>,
    tool_cfg: &crate::app::AppConfig,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let settings = load_settings(&cwd);
    let skills = load_skills(&cwd);
    let skills_prompt = format_skills_for_prompt(&skills.skills);
    let turns = vec![vec![
        FixtureTurn::Text(user_input.to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: nini_core::provider::Usage::default(),
        },
    ]];
    // v0.13: only the **user-level** `models.json` configures a
    // provider. The project-level file is a catalogue, not a
    // configuration.
    let user_models = nini_core::settings::load_user_models_json();
    let provider_impl = provider_factory::build_provider(
        provider,
        turns,
        fallback_keys,
        fallback_base_urls,
        api_key,
        Some(&user_models),
    )?;
    // v0.10 (fix): the same assembly the TUI uses. This called
    // `build_tools()` directly, which meant `nini -p "..."` — the entry
    // point every CI job and script uses — silently ignored
    // `--tools`/`--exclude-tools`/`--no-tools` *and* the project trust
    // decision. A restriction that holds in the TUI and evaporates in
    // `-p` is not a restriction.
    let tools = crate::app::build_run_tools(tool_cfg, &cwd);
    let system = prompt_setup::settings_to_system_prompt(&settings, &cwd, &tools, &skills_prompt);
    let config = print_run_config(tool_cfg, &settings, model, system);
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
    // The answer is printed raw, and a raw print does not know where the
    // terminal's last column is: a **wide** character (Chinese, emoji)
    // that lands in the last column does not fit, and the terminal folds
    // the line *before* it — so the answer arrives broken in half, one
    // character early, and the rest of the line lands on the next row.
    //
    // Wrap on cell boundaries first, so the character never reaches the
    // edge it cannot fit in.
    print!("{}", wrap_to_terminal_width(&stdout_text));
    if any_error {
        std::process::exit(1);
    }
    Ok(())
}

/// Scripted fixture for the "fix TODOs" demo.
///
/// v0.10 (R2f): this doc used to promise "discovers the first source file
/// containing TODO, then scripts a multi-turn agent run: grep -> read ->
/// edit -> verify". It does neither half. `find_first_file_with_todo` was
/// called and its result dropped on the floor - `let _target = ...` - and
/// the body has always been a single turn.
///
/// The doc is corrected rather than the program extended: a demo is a
/// scripted fixture, and quietly making it four turns to satisfy a comment
/// would be a bigger change than the comment deserved. What it does is now
/// what it says.
pub(crate) fn demo_fix_todos_turns(_cwd: &std::path::Path) -> Vec<Vec<FixtureTurn>> {
    use nini_ai::fixture::FixtureTurn as F;
    use nini_core::provider::Usage;
    // Single-turn demo: the agent issues one grep and emits a final text.
    // The CLI's exit code is 0 because we don't simulate a 50-iter
    // runaway.
    vec![
        // Turn 1: model emits a grep tool call.
        vec![
            F::ToolCall {
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": "TODO", "path": "."}),
            },
            F::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        // Turn 2: model emits final text + ends the run.
        vec![
            F::Text("Found TODOs. Demo complete.".to_string()),
            F::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]
}

/// Trivial fixture for any non-TODO demo: emit a single text response.
pub(crate) fn demo_simple_turns(task: &str) -> Vec<Vec<FixtureTurn>> {
    use nini_ai::fixture::FixtureTurn as F;
    use nini_core::provider::Usage;
    vec![vec![
        F::Text(format!("Demo response for: {task}")),
        F::Stop {
            stop_reason: "end_turn".into(),
            usage: Usage::default(),
        },
    ]]
}

/// v0.7.1 — delegate to the chokepoint in nini-core::conversion.
/// See crates/nini-tui/src/commands.rs::entry_legacy_message for the
/// parallel refactor.
///
/// Currently only exercised by `cli_entry_legacy_handles_real_entry`
/// below; the `let _ =` form doesn't count as a use for `dead_code`,
/// and `pub(crate)` widens visibility so rustc flags it. The function
/// is kept for symmetry with the TUI's `entry_legacy_message` and may
/// be needed again when the CLI gets a `--dump-legacy` flag.
#[allow(dead_code)]
pub(crate) fn cli_entry_legacy(entry: &nini_core::SessionEntry) -> Option<nini_core::AgentMessage> {
    let m = nini_core::conversion::session_entry_to_llm_message(entry)?;
    Some(nini_core::AgentMessage {
        role: m.role,
        content: m.content,
        timestamp: m.timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_entry_legacy_handles_real_entry() {
        // Just verify the function doesn't panic on any valid input.
        // The chokepoint's return value depends on the entry type; we
        // only assert "no panic" here.
        use nini_core::SessionEntry;
        let entry = SessionEntry::SessionInfo(nini_core::SessionInfoEntry {
            id: "test".into(),
            parent_id: None,
            timestamp: "0".into(),
            name: "test".into(),
        });
        let _ = cli_entry_legacy(&entry);
    }
}

/// v0.14 (round 3): the two single-shot entries carry the tool-concurrency
/// ceiling.
///
/// The gap this closes: `nini demo` and `nini -p` do **not** go through
/// `app::build_tui_run_config`, so the CLI's wiring tests never touched
/// them. Hard-writing `max_parallel_tools: 0` in `demo.rs` — dropping
/// `--max-parallel-tools` and `maxParallelTools` on the floor — left every
/// test in the repository green.
///
/// Each entry gets its own test, and each test walks the **whole** way:
/// the command line, through clap, into the `AppConfig`, into the
/// `RunConfig` the entry actually runs with.
#[cfg(test)]
mod entry_ceiling_tests {
    use super::*;
    use nini_core::settings::Settings;

    /// The `AppConfig` the binary's own assembly produces for a command
    /// line — the same `app_config_from_args` the binary calls, with the
    /// same clap parse.
    fn cfg_from_argv(argv: &[&str]) -> crate::app::AppConfig {
        use clap::Parser;
        let mut full = vec!["nini"];
        full.extend_from_slice(argv);
        let args = crate::Args::try_parse_from(full).expect("these flags parse");
        crate::app_config_from_args(&args)
    }

    fn cfg_with_ceiling(ceiling: Option<usize>) -> crate::app::AppConfig {
        let mut cfg = crate::app::AppConfig::for_tool_selection();
        cfg.max_parallel_tools = ceiling;
        cfg
    }

    /// The `nini demo` entry must carry the ceiling.
    #[test]
    fn the_demo_entry_carries_the_tool_concurrency_ceiling() {
        let quiet = Settings::default();

        // The command line speaks.
        let from_flag = cfg_from_argv(&["--max-parallel-tools", "3"]);
        assert_eq!(
            demo_run_config(&from_flag, &quiet, "test-model", String::new()).max_parallel_tools,
            3,
            "the demo entry dropped the command line's ceiling"
        );

        // The settings file speaks when the command line is silent.
        let mut from_file = Settings::default();
        from_file.max_parallel_tools = Some(4);
        let silent = cfg_from_argv(&[]);
        assert_eq!(
            demo_run_config(&silent, &from_file, "test-model", String::new()).max_parallel_tools,
            4,
            "the demo entry dropped the settings file's ceiling"
        );

        // The command line beats the settings file.
        assert_eq!(
            demo_run_config(&from_flag, &from_file, "test-model", String::new()).max_parallel_tools,
            3,
            "the settings file beat the command line in the demo entry"
        );

        // `0` means "no ceiling", not "a ceiling of zero".
        let zero = cfg_from_argv(&["--max-parallel-tools", "0"]);
        assert_eq!(
            demo_run_config(&zero, &quiet, "test-model", String::new()).max_parallel_tools,
            0,
            "an explicit 0 in the demo entry was not applied"
        );

        // Nothing says anything → no ceiling (Pi 0.85.1's behaviour).
        assert_eq!(
            demo_run_config(&silent, &quiet, "test-model", String::new()).max_parallel_tools,
            0,
            "the demo entry invented a ceiling nobody asked for"
        );
    }

    /// The `nini -p` entry must carry the ceiling.
    ///
    /// `nini -p "..."` is the entry every CI job and every script uses, so
    /// a value that dies here dies in everybody's CI.
    #[test]
    fn the_print_entry_carries_the_tool_concurrency_ceiling() {
        let quiet = Settings::default();

        let from_flag = cfg_from_argv(&["--max-parallel-tools", "3"]);
        assert_eq!(
            print_run_config(&from_flag, &quiet, "test-model", String::new()).max_parallel_tools,
            3,
            "the print entry dropped the command line's ceiling"
        );

        let mut from_file = Settings::default();
        from_file.max_parallel_tools = Some(4);
        let silent = cfg_from_argv(&[]);
        assert_eq!(
            print_run_config(&silent, &from_file, "test-model", String::new()).max_parallel_tools,
            4,
            "the print entry dropped the settings file's ceiling"
        );

        assert_eq!(
            print_run_config(&from_flag, &from_file, "test-model", String::new())
                .max_parallel_tools,
            3,
            "the settings file beat the command line in the print entry"
        );

        let zero = cfg_from_argv(&["--max-parallel-tools", "0"]);
        assert_eq!(
            print_run_config(&zero, &quiet, "test-model", String::new()).max_parallel_tools,
            0,
            "an explicit 0 in the print entry was not applied"
        );

        assert_eq!(
            print_run_config(&silent, &quiet, "test-model", String::new()).max_parallel_tools,
            0,
            "the print entry invented a ceiling nobody asked for"
        );
    }

    /// The `nini demo` entry must carry the execution mode.
    ///
    /// The mode says how a turn's tool calls run: `parallel` (the
    /// default) or `sequential`. A mode that reaches the TUI and dies
    /// here is a mode that does not exist for half the binary.
    #[test]
    fn the_demo_entry_carries_the_tool_execution_mode() {
        let quiet = Settings::default();

        // The command line speaks.
        let from_flag = cfg_from_argv(&["--tool-execution", "sequential"]);
        assert_eq!(
            demo_run_config(&from_flag, &quiet, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Sequential,
            "the demo entry dropped the command line's execution mode"
        );

        // The settings file speaks when the command line is silent.
        let mut from_file = Settings::default();
        from_file.tool_execution = Some(nini_core::tool::ToolExecutionMode::Sequential);
        let silent = cfg_from_argv(&[]);
        assert_eq!(
            demo_run_config(&silent, &from_file, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Sequential,
            "the demo entry dropped the settings file's execution mode"
        );

        // The command line beats the settings file.
        let from_other_flag = cfg_from_argv(&["--tool-execution", "parallel"]);
        assert_eq!(
            demo_run_config(&from_other_flag, &from_file, "test-model", String::new())
                .tool_execution,
            nini_core::tool::ToolExecutionMode::Parallel,
            "the settings file beat the command line in the demo entry"
        );

        // Nothing says anything → `parallel`, the way a turn runs by
        // itself.
        assert_eq!(
            demo_run_config(&silent, &quiet, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Parallel,
            "the demo entry invented an execution mode nobody asked for"
        );
    }

    /// The `nini -p` entry must carry the execution mode.
    ///
    /// `nini -p "..."` is the entry every CI job and every script uses,
    /// so a mode that dies here dies in everybody's CI.
    #[test]
    fn the_print_entry_carries_the_tool_execution_mode() {
        let quiet = Settings::default();

        let from_flag = cfg_from_argv(&["--tool-execution", "sequential"]);
        assert_eq!(
            print_run_config(&from_flag, &quiet, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Sequential,
            "the print entry dropped the command line's execution mode"
        );

        let mut from_file = Settings::default();
        from_file.tool_execution = Some(nini_core::tool::ToolExecutionMode::Sequential);
        let silent = cfg_from_argv(&[]);
        assert_eq!(
            print_run_config(&silent, &from_file, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Sequential,
            "the print entry dropped the settings file's execution mode"
        );

        assert_eq!(
            print_run_config(&from_flag, &from_file, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Sequential,
            "the settings file beat the command line in the print entry"
        );

        assert_eq!(
            print_run_config(&silent, &quiet, "test-model", String::new()).tool_execution,
            nini_core::tool::ToolExecutionMode::Parallel,
            "the print entry invented an execution mode nobody asked for"
        );
    }

    /// The hand-built config must not be the only way in: a mode that
    /// arrives through the config struct has to survive the assembly too.
    #[test]
    fn a_ceiling_in_the_config_survives_both_entries() {
        let cfg = cfg_with_ceiling(Some(6));
        let settings = Settings::default();
        assert_eq!(
            demo_run_config(&cfg, &settings, "test-model", String::new()).max_parallel_tools,
            6
        );
        assert_eq!(
            print_run_config(&cfg, &settings, "test-model", String::new()).max_parallel_tools,
            6
        );
    }
}

#[cfg(test)]
mod print_wrap_tests {
    use super::wrap_to_width;

    /// No line may be wider than the terminal: a line that is too wide
    /// is folded by the terminal, and the fold lands in the middle of
    /// the answer.
    ///
    /// A wide character (Chinese, emoji) is the case that matters: it
    /// needs two columns, so it must be moved to the next line rather
    /// than left in the last column, where the terminal would fold
    /// *before* it.
    #[test]
    fn no_line_is_wider_than_the_terminal() {
        for cols in [4usize, 5, 8, 11] {
            let text = "中".repeat(cols);
            let wrapped = wrap_to_width(&text, cols);
            for line in wrapped.split('\n') {
                let w = nini_tui::width::display_width(line);
                assert!(
                    w <= cols,
                    "a line is {w} cells wide, the terminal has {cols}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_text_survives_the_wrap() {
        let text = "统计结果：crates 下共有二百一十三个 Rust 源文件。";
        for cols in [4usize, 7, 16, 40] {
            let wrapped = wrap_to_width(text, cols);
            let joined: String = wrapped.chars().filter(|c| *c != '\n').collect();
            assert_eq!(joined, text, "the wrap lost or changed text");
        }
    }
}
