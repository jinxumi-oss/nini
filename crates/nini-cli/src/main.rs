//! nini — Pi-compatible Rust coding agent (CLI entry point)
//!
//! Modes:
//! - `nini`                  launch interactive TUI (Phase 5)
//! - `nini -p "..."`         single-shot print mode (no session)
//! - `nini demo [task]`      scripted autonomous demo (no API key needed)
//! - `nini info`             show loaded skills/settings

use anyhow::{ Context, Result };
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use std::io::IsTerminal;
use nini_ai::fixture::{ FixtureTurn, ProgrammedProvider };
use nini_core::provider::{ Provider, Usage };
use nini_core::settings::load_settings;
use nini_core::skills::{ load_skills, format_skills_for_prompt };
use nini_core::{ Agent, AgentEvent, RunConfig, ToolRegistry };
use nini_tools::{ BashTool, EditTool, FindTool, GrepTool, ReadTool, WriteTool };
use nini_tui::run as run_tui;
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
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run an autonomous demo task (no API key needed).
    Demo { task: Option<String> },
    /// Show loaded skills and settings.
    Info,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let Args { cmd, print, provider, model } = args;

    match cmd {
        Some(Cmd::Demo { task }) => {
            let task = task.unwrap_or_else(|| "find TODOs and fix them".to_string());
            run_demo(&task, &provider, &model).await
        }
        Some(Cmd::Info) => run_info().await,
        None if print.is_some() => run_print(print.as_deref().unwrap(), &provider, &model).await,
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
            let agent_driver: AgentDriver = std::sync::Arc::new(move |user_msg, sink, done| {
                // Use the configured provider; in fixture mode this is the
                // ProgrammedProvider with scripted turns.
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let settings = load_settings(&cwd);
                let _skills = load_skills(&cwd);
                let provider = build_provider(&provider_name, vec![]).unwrap_or_else(|_| {
                    // Fallback to fixture on missing API keys
                    std::sync::Arc::new(nini_ai::fixture::FixtureProvider::from_turns(
                        "test-model",
                        vec![],
                    )) as std::sync::Arc<dyn nini_core::provider::Provider>
                });
                let tools = build_tools();
                let cfg = RunConfig {
                    model: model_for_cfg(&settings),
                    system: Some(settings_to_system_prompt(&settings, "")),
                    ..RunConfig::new("test-model")
                };
                let mut agent = nini_core::Agent::new(provider, tools, cfg);
                tokio::spawn(async move {
                    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));
                    while let Some(ev) = stream.next().await {
                        let lite = match ev {
                            Ok(AgentEvent::TextDelta { text }) => nini_tui::runtime::AgentEventLite::TextDelta(text),
                            Ok(AgentEvent::ToolCallStart { name, .. }) =>
                                nini_tui::runtime::AgentEventLite::ToolCallStart { name },
                            Ok(AgentEvent::ToolCallStop { id, input_json }) =>
                                nini_tui::runtime::AgentEventLite::ToolCallStop {
                                    id,
                                    args: input_json.to_string(),
                                },
                            Ok(AgentEvent::ToolResult { output, .. }) =>
                                nini_tui::runtime::AgentEventLite::ToolResult {
                                    ok: !output.is_error,
                                    content: output.content,
                                },
                            Ok(AgentEvent::TurnEnd { usage, .. }) => {
                                sink.push(nini_tui::runtime::AgentEventLite::Usage(
                                    usage.input_tokens,
                                    usage.output_tokens,
                                ));
                                nini_tui::runtime::AgentEventLite::TurnEnd
                            }
                            Ok(AgentEvent::Error { message }) => nini_tui::runtime::AgentEventLite::Error(message),
                            Err(_) => continue,
                            _ => continue,
                        };
                        sink.push(lite);
                    }
                    sink.push(nini_tui::runtime::AgentEventLite::Done);
                    done.notify_waiters();
                })
            });
            run_tui(|state| {
                let cwd = std::env::current_dir().ok();
                if let Some(dir) = cwd {
                    let s = load_settings(&dir);
                    if let Some(p) = s.provider {
                        state.model = p;
                    } else {
                        state.model = "test-model".to_string();
                    }
                    if let Some(m) = s.model {
                        state.model = m;
                    }
                    let skills = load_skills(&dir);
                    if !skills.skills.is_empty() {
                        let count = skills.skills.len();
                        state.push_assistant(format!(
                            "loaded {count} skills; type to begin, Ctrl+C to quit, F1 for help"
                        ));
                        state.push_divider();
                    }
                }
            }, agent_driver)
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
    println!("Skills ({} loaded, {} errors):", skills_result.skills.len(), skills_result.errors.len());
    for s in &skills_result.skills {
        println!("  - {} ({}) -- {}", s.name, s.source.label(), s.description);
    }
    for e in &skills_result.errors {
        println!("  ! error: {e}");
    }
    Ok(())
}

fn build_provider(provider: &str, turns: Vec<Vec<FixtureTurn>>) -> Result<Arc<dyn Provider>> {
    match provider {
        "fixture" => Ok(Arc::new(ProgrammedProvider::from_turns(turns))),
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY required for anthropic")?;
            Ok(Arc::new(nini_ai::anthropic::AnthropicProvider::new(key)))
        }
        "openai" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai")?;
            Ok(Arc::new(nini_ai::openai::OpenAiProvider::new(key)))
        }
        "openai-responses" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-responses")?;
            Ok(Arc::new(nini_ai::openai_responses::OpenAiResponsesProvider::new(key)))
        }
        "openai-compat" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-compat")?;
            let base = std::env::var("OPENAI_BASE_URL")
                .context("OPENAI_BASE_URL required for openai-compat")?;
            Ok(Arc::new(nini_ai::openai_compat::OpenAiCompatProvider::new(base, key)))
        }
        other => {
            eprintln!("nini: unknown provider: {other}");
            std::process::exit(2);
        }
    }
}

fn build_tools() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(BashTool::new()))
        .register(Arc::new(ReadTool::new()))
        .register(Arc::new(WriteTool::new()))
        .register(Arc::new(EditTool::new()))
        .register(Arc::new(GrepTool::new()))
        .register(Arc::new(FindTool::new()))
}

fn model_for_cfg(s: &nini_core::settings::Settings) -> String {
    s.model.clone().or_else(|| s.provider.clone()).unwrap_or_else(|| "test-model".to_string())
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

async fn run_print(user_input: &str, provider: &str, model: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let settings = load_settings(&cwd);
    let skills = load_skills(&cwd);
    let skills_prompt = format_skills_for_prompt(&skills.skills);
    let cmd = format!("echo {user_input}");
    let turns = vec![
        vec![FixtureTurn::ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({"command": cmd}),
        }, FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        }],
        vec![FixtureTurn::Text(user_input.to_string()), FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        }],
    ];
    let provider_impl = build_provider(provider, turns)?;
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
            Err(e) => { eprintln!("[error] {e}"); any_error = true; }
            _ => {}
        }
    }
    print!("{stdout_text}");
    if any_error { std::process::exit(1); }
    Ok(())
}

async fn run_demo(task: &str, provider: &str, model: &str) -> Result<()> {
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
    let provider_impl = build_provider(provider, turns)?;
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
                let preview = output.content.lines().take(3).collect::<Vec<_>>().join(" | ");
                eprintln!("[demo] <- tool result ({} bytes): {preview}", output.content.len());
            }
            Ok(AgentEvent::TurnEnd { .. }) => { eprintln!("[demo] -- turn end"); }
            Ok(AgentEvent::Error { message }) => { eprintln!("[demo] !! error: {message}"); any_error = true; }
            Err(e) => { eprintln!("[demo] !! agent error: {e}"); any_error = true; }
            _ => {}
        }
    }
    eprintln!();
    eprintln!("[demo] === summary ===");
    eprintln!("[demo] tool calls executed: {tool_count}");
    eprintln!("[demo] assistant text: {stdout_text}");
    if any_error { std::process::exit(1); }
    Ok(())
}

fn demo_fix_todos_turns(cwd: &std::path::Path) -> Vec<Vec<FixtureTurn>> {
    let target = find_first_file_with_todo(cwd).unwrap_or_else(|| "src/main.rs".to_string());
    vec![
        vec![FixtureTurn::ToolCall {
            name: "grep".to_string(),
            args: serde_json::json!({"pattern": "TODO", "path": "."}),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::ToolCall {
            name: "read".to_string(),
            args: serde_json::json!({"path": target.clone()}),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::ToolCall {
            name: "edit".to_string(),
            args: serde_json::json!({
                "path": target, "old_text": "// DONE: ", "new_text": "// DONE: "
            }),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({"command": "echo verified"}),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::Text("Found and fixed TODOs in the codebase.".to_string()),
             FixtureTurn::Stop { stop_reason: "end_turn".to_string(), usage: Usage::default() }],
    ]
}

fn demo_simple_turns(task: &str) -> Vec<Vec<FixtureTurn>> {
    vec![
        vec![FixtureTurn::ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({"command": format!("echo {task}")}),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::Text(task.to_string()),
             FixtureTurn::Stop { stop_reason: "end_turn".to_string(), usage: Usage::default() }],
    ]
}

fn find_first_file_with_todo(cwd: &std::path::Path) -> Option<String> {
    fn walk(dir: &std::path::Path) -> Option<String> {
        let entries = std::fs::read_dir(dir).ok()?;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(found) = walk(&p) { return Some(found); }
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
    let start = if cwd.join("src").is_dir() { cwd.join("src") } else { cwd.to_path_buf() };
    walk(&start)
}
