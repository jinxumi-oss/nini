//! v0.8.2: Demo + print (single-shot) execution paths.
//!
//! Extracted from `main.rs`. Two public functions:
//!   * `run_demo()`  — scripted autonomous run (no API key needed)
//!   * `run_print()` — single-shot `-p "task"` invocation
//!
//! Plus 4 fixture builders + 2 helpers used only by the demo paths.

use std::path::Path;

use anyhow::Result;
use futures_util::StreamExt;

use nini_ai::fixture::FixtureTurn;
use nini_core::settings::load_settings;
use nini_core::skills::{format_skills_for_prompt, load_skills};
use nini_core::{Agent, AgentEvent, RunConfig};

use crate::{prompt_setup, provider_factory, tool_registry};

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
    let provider_impl = provider_factory::build_provider(provider, turns, fallback_keys, fallback_base_urls)?;
    let system = prompt_setup::settings_to_system_prompt(&settings, &skills_prompt);
    let tools = tool_registry::build_tools();
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

/// Single-shot invocation: `nini -p "task"`.
///
/// Streams all events to stdout + stderr; the assistant's final text
/// goes to stdout (printable), tool calls/results go to stderr.
pub(crate) async fn run_print(
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
    let turns = vec![vec![
        FixtureTurn::Text(user_input.to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: nini_core::provider::Usage::default(),
        },
    ]];
    let provider_impl = provider_factory::build_provider(provider, turns, fallback_keys, fallback_base_urls)?;
    let system = prompt_setup::settings_to_system_prompt(&settings, &skills_prompt);
    let tools = tool_registry::build_tools();
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

/// Scripted fixture for the "fix TODOs" demo.
///
/// Discovers the first source file containing `TODO`, then scripts
/// a multi-turn agent run: grep → read → edit → verify.
pub(crate) fn demo_fix_todos_turns(cwd: &std::path::Path) -> Vec<Vec<FixtureTurn>> {
    use nini_ai::fixture::FixtureTurn as F;
    use nini_core::provider::Usage;
    let _target = find_first_file_with_todo(cwd);
    // Single-turn demo: agent issues one grep + emits a final text.
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

/// Walk `cwd` looking for the first regular file containing the
/// word `TODO` in its content. Returns the relative path if found.
pub(crate) fn find_first_file_with_todo(cwd: &Path) -> Option<String> {
    for entry in std::fs::read_dir(cwd).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.contains("TODO") {
            return path
                .strip_prefix(cwd)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                    .or_else(|| Some(path.to_string_lossy().into_owned()));
        }
    }
    None
}

/// v0.7.1 — delegate to the chokepoint in nini-core::conversion.
/// See crates/nini-tui/src/commands.rs::entry_legacy_message for the
/// parallel refactor.
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
    fn find_first_file_with_todo_finds_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "// TODO: hello").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn main() {}").unwrap();
        let found = find_first_file_with_todo(dir.path());
        assert!(found.is_some(), "should find a.txt with TODO");
    }

    #[test]
    fn find_first_file_with_todo_returns_none_if_no_todo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "no markers here").unwrap();
        let found = find_first_file_with_todo(dir.path());
        assert_eq!(found, None);
    }

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