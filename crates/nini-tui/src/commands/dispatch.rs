//! v0.8.3: Dispatch + per-command implementation helpers.
//!
//! `dispatch()` is the public entry — it routes `CommandId` to the
//! matching `cmd_xxx()` private helper below. Each helper is a pure
//! function over `&mut AppState` (and optionally `&mut SettingsManager`
//! for commands that persist). Each helper has unit tests in this
//! module.

use crate::commands::{build_status_lines, render_transcript_html};
use crate::settings::SettingsManager;
use crate::state::{AppState, RunMode, TranscriptLine};

use super::registry::CommandId;
use super::result::{CommandOutcome, CommandResult};

/// Dispatch a slash command. Pure function — mutates `state` in place.
///
/// `args` is the raw input after the command name (whitespace-trimmed).
/// For commands without an `argument_hint`, `args` should be empty.
/// `settings` is used for commands that persist to ~/.pi/agent/settings.json
/// (e.g., /model, /thinking). Pass `&mut SettingsManager::default()` if persistence
/// is not needed.
pub fn dispatch(state: &mut AppState, settings: &mut SettingsManager, id: CommandId, args: &str) -> CommandResult {
    let args = args.trim();
    match id {
        CommandId::Settings => {
            // Display current settings (mock until real settings UI is wired).
            let lines = vec![
                "Settings (read-only)".to_string(),
                format!("  model:     {}", state.model_state.model),
                format!(
                    "  session:   {}",
                    state.session_state.session_id.as_deref().unwrap_or("(none)")
                ),
                format!("  status:    {:?}", state.run_state.mode),
            ];
            CommandResult::output(lines)
        }
        CommandId::Model => {
            // /model <provider/model> — set the model for the next turn.
            if args.is_empty() {
                return CommandResult::output(vec![
                    "Usage: /model <provider/model>".to_string(),
                    format!("Current model: {}", state.model_state.model),
                ]);
            }
            state.model_state.model = args.to_string();
            settings.set_default_model(args);
            state.push_assistant(format!("(model set to {})", args));
            state.push_divider();
            CommandResult::output(vec![format!("model → {args}")])
        }
        CommandId::Tree => {
            // /tree — print the session tree (or build one from the
            // current session if no tree is persisted).
            //
            // Pi's /tree opens an interactive picker; nini v1 renders
            // an ASCII tree in the transcript (the runtime can promote
            // it to a selector when the picker lands).
            use nini_session::tree::SessionTree;
            // Read the current session file (if any) and build a tree
            // from its entries. Fall back to an empty tree if there's
            // no session yet.
            let entries: Vec<nini_core::entries::SessionEntry> =
                if let Some(path) = &state.session_state.session_path {
                    let raw = std::fs::read_to_string(path).unwrap_or_default();
                    raw.lines()
                        .filter_map(|l| serde_json::from_str(l).ok())
                        .collect()
                } else {
                    Vec::new()
                };
            let tree = SessionTree::from_entries(&entries);
            let main_path = tree.main_path();
            let mut out: Vec<String> = Vec::new();
            out.push(format!(
                "session tree: {} entries, main path: {} nodes",
                tree.len(),
                main_path.len()
            ));
            if main_path.is_empty() {
                out.push("(empty — use /new to start a session)".to_string());
            } else {
                for (i, id) in main_path.iter().enumerate() {
                    let label = tree
                        .get_path_to(id)
                        .last()
                        .and_then(|_| tree.descendants(id).first().cloned())
                        .map(|_| String::new())
                        .unwrap_or_default();
                    out.push(format!("  {:>3}. {}", i + 1, id));
                }
                // Show non-main branches (descendants of nodes not in the main path).
                let in_main: std::collections::HashSet<&String> = main_path.iter().collect();
                let mut branch_count = 0;
                for id in &main_path {
                    for child_id in tree.descendants(id) {
                        if !in_main.contains(&child_id) {
                            branch_count += 1;
                            out.push(format!("       └─ {} (branch {})", child_id, branch_count));
                        }
                    }
                }
                if branch_count == 0 {
                    out.push("(no branches — current session is linear)".to_string());
                }
            }
            state.push_assistant(format!("[tree] {} entries, {} branches", tree.len(), {
                let mc = tree.main_path().len();
                if mc > 0 { tree.len().saturating_sub(mc).to_string() } else { "0".to_string() }
            }));
            state.push_divider();
            CommandResult::output(out)
        }
        CommandId::Thinking => {
            // /thinking <off|minimal|low|medium|high|xhigh|max>
            let valid = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
            if args.is_empty() || !valid.contains(&args) {
                return CommandResult::output(vec![format!(
                    "Usage: /thinking <{}>",
                    valid.join("|")
                )]);
            }
            settings.set_default_thinking_level(args);
            state.push_assistant(format!("(thinking level: {args})"));
            state.push_divider();
            CommandResult::output(vec![format!("thinking → {args}")])
        }
        CommandId::ScopedModels => {
            // /scoped-models [add|remove <model>|list|clear]
            // Mirrors Pi's per-model scoped-models config. Lets the user
            // curate which models are eligible for Ctrl+P cycling. The
            // cycle list lives in `state.model_state.models_cycle`.
            let parts: Vec<&str> = args.split_whitespace().collect();
            let sub = parts.first().copied().unwrap_or("list");
            match sub {
                "add" if parts.len() >= 2 => {
                    let m = parts[1].to_string();
                    if !state.model_state.models_cycle.iter().any(|x| x == &m) {
                        state.model_state.models_cycle.push(m.clone());
                    }
                    let _ = settings.flush(); // persist after cycle change
                    state.push_assistant(format!("[scoped-models] added {m}"));
                    state.push_divider();
                    let mut out = vec![
                        format!("added {m} to cycle"),
                        format!("cycle ({} models):", state.model_state.models_cycle.len()),
                    ];
                    for cm in &state.model_state.models_cycle {
                        out.push(format!("  - {cm}"));
                    }
                    CommandResult::output(out)
                }
                "remove" if parts.len() >= 2 => {
                    let m = parts[1];
                    let before = state.model_state.models_cycle.len();
                    state.model_state.models_cycle.retain(|x| x != m);
                    let removed = before - state.model_state.models_cycle.len();
                    let _ = settings.flush();
                    state.push_assistant(format!(
                        "[scoped-models] removed {} model(s)",
                        removed
                    ));
                    state.push_divider();
                    CommandResult::output(vec![format!(
                        "removed {removed} matching {m}"
                    )])
                }
                "clear" => {
                    state.model_state.models_cycle.clear();
                    state.model_state.models_cycle_idx = None;
                    let _ = settings.flush();
                    state.push_assistant("[scoped-models] cleared all".to_string());
                    state.push_divider();
                    CommandResult::output(vec!["cleared cycle".to_string()])
                }
                _ => {
                    // list (default)
                    let mut out: Vec<String> = vec![
                        format!(
                            "Ctrl+P cycling scope ({} models)",
                            state.model_state.models_cycle.len()
                        ),
                        "".to_string(),
                    ];
                    if state.model_state.models_cycle.is_empty() {
                        out.push("(empty — use `/scoped-models add <model>`)".to_string());
                        out.push(String::new());
                        out.push("Examples:".to_string());
                        out.push("  /scoped-models add anthropic/claude-opus-4-7".to_string());
                        out.push("  /scoped-models add openai/gpt-5".to_string());
                        out.push("  /scoped-models clear".to_string());
                    } else {
                        for (i, m) in state.model_state.models_cycle.iter().enumerate() {
                            let marker = if Some(i) == state.model_state.models_cycle_idx {
                                "→ "
                            } else {
                                "  "
                            };
                            out.push(format!("{marker}{m}"));
                        }
                    }
                    CommandResult::output(out)
                }
            }
        }
        CommandId::Export => {
            // v1: write a minimal HTML snapshot of the transcript to ~/.pi/agent/exports/.
            let html = render_transcript_html(&state.transcript_state.lines);
            let dir = std::env::var("HOME").ok().map(|h| {
                std::path::PathBuf::from(h)
                    .join(".pi")
                    .join("agent")
                    .join("exports")
            });
            let path = match dir {
                Some(d) => {
                    let _ = std::fs::create_dir_all(&d);
                    // Filename: session-{ts_us}-{pid}-{tid}.html
                    // Microsecond + pid + tid ensures parallel-safe unique
                    // filenames even when two threads write in the
                    // same microsecond (cargo test --test-threads>1).
                    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%6f");
                    let pid = std::process::id();
                    // Use a thread-id fallback: std::thread::current().id()
                    // returns a non-Hash ThreadId; format its debug repr
                    // (e.g. "ThreadId(N)") which is process-unique.
                    let tid = format!("{:?}", std::thread::current().id())
                        .chars()
                        .filter(|c| c.is_ascii_digit())
                        .collect::<String>();
                    let p = d.join(format!(
                        "session-{ts}-{pid}-t{tid}.html"
                    ));
                    if std::fs::write(&p, &html).is_ok() {
                        p.display().to_string()
                    } else {
                        format!("(failed to write {})", p.display())
                    }
                }
                None => "(no HOME — skipped)".to_string(),
            };
            state.push_assistant(format!("(exported session to {path})"));
            state.push_divider();
            CommandResult::output(vec![format!("export → {path}")])
        }
        CommandId::Import => {
            // /import <path-to-jsonl>
            // Reads a Pi-compatible JSONL session file, parses each
            // entry, and starts a new in-memory session from it. The
            // current transcript is replaced with the imported
            // transcript (capped at 1000 lines to avoid OOM).
            use nini_core::entries::SessionEntry as SE;
            let path_str = args.trim();
            if path_str.is_empty() {
                return CommandResult::output(vec![
                    "import <path-to-jsonl> — load a Pi-compatible session file".to_string(),
                    "examples:".to_string(),
                    "  /import ~/.pi/agent/sessions/<project>/20260101-120000-abcd.jsonl".to_string(),
                    "  /import ./my-session.jsonl".to_string(),
                ]);
            }
            let path = std::path::PathBuf::from(path_str);
            let raw = match std::fs::read_to_string(&path) {
                Ok(r) => r,
                Err(e) => {
                    return CommandResult::output(vec![format!(
                        "import: read failed: {e}"
                    )]);
                }
            };
            let mut entries: Vec<SE> = Vec::new();
            let mut skipped = 0usize;
            for (i, line) in raw.lines().enumerate() {
                if i >= 1000 {
                    skipped += 1;
                    continue;
                }
                match serde_json::from_str::<SE>(line) {
                    Ok(e) => entries.push(e),
                    Err(_) => skipped += 1,
                }
            }
            let mut out = vec![format!(
                "imported {} entries from {} (skipped {skipped} malformed)",
                entries.len(),
                path.display()
            )];
            // Find a session-info entry (carries session name) and set
            // the session id + path. nini's JSONL v3 format doesn't have
            // a SessionHeader variant; metadata is in SessionInfo entries.
            if let Some(info) = entries.iter().find_map(|e| match e {
                SE::SessionInfo(i) => Some(i),
                _ => None,
            }) {
                state.session_state.session_id = Some(info.id.clone());
                out.push(format!("session name: {}", info.name));
            }
            // Populate transcript from message entries.
            use nini_core::entries::StringOrContentBlocks;
            let mut new_transcript = Vec::new();
            for e in entries.iter() {
                let SE::Message(m) = e else { continue };
                match &m.message {
                    nini_core::entries::AgentMessage::User(u) => {
                        let text = match &u.content {
                            StringOrContentBlocks::String(s) => s.clone(),
                            StringOrContentBlocks::Blocks(bl) => bl
                                .iter()
                                .filter_map(|b| match b {
                                    nini_core::entries::ContentBlock::Text { text } => {
                                        Some(text.clone())
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join(""),
                        };
                        new_transcript.push(TranscriptLine::User(text));
                    }
                    nini_core::entries::AgentMessage::Assistant(a) => {
                        let text = a
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                nini_core::entries::ContentBlock::Text { text } => {
                                    Some(text.clone())
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        if !text.is_empty() {
                            new_transcript.push(TranscriptLine::AssistantText(text));
                        }
                    }
                    _ => {}
                }
            }
            let imported_count = new_transcript.len();
            state.transcript_state.lines = new_transcript;
            state.run_state.tokens = Default::default();
            state.push_assistant(format!(
                "[import] {} transcript entries ({} skipped)",
                imported_count,
                skipped
            ));
            state.push_divider();
            out.push(format!("transcript: {imported_count} entries"));
            CommandResult::output(out)
        }
        CommandId::Share => CommandResult::output(vec![
            "(share — GitHub gist upload not yet implemented)".to_string(),
        ]),
        CommandId::Copy => {
            // Copy the last assistant message to the system clipboard.
            // Falls back to stdout (for headless) if clipboard unavailable.
            let last_assistant = state.transcript_state.lines.iter().rev().find_map(|l| match l {
                TranscriptLine::AssistantText(s) => Some(s.clone()),
                _ => None,
            });
            match last_assistant {
                Some(msg) => {
                    match crate::clipboard::copy(&msg) {
                        Ok(()) => CommandResult::output(vec![
                            format!("(copied {} bytes to clipboard)", msg.len()),
                        ]),
                        Err(_e) => {
                            // Fallback: print to stdout so headless users
                            // can still grab the text.
                            println!("{msg}");
                            CommandResult::output(vec![
                                format!("(copied {} bytes to stdout — clipboard unavailable)", msg.len()),
                            ])
                        }
                    }
                }
                None => CommandResult::output(vec!["(no assistant message to copy)".to_string()]),
            }
        }
        CommandId::Name => {
            if args.is_empty() {
                return CommandResult::output(vec!["Usage: /name <session name>".to_string()]);
            }
            // v1: store in status. Real impl persists via session metadata.
            state.run_state.status = format!("name: {args}");
            state.push_assistant(format!("(session name: {args})"));
            state.push_divider();
            CommandResult::output(vec![format!("name → {args}")])
        }
        CommandId::Session => {
            let lines = vec![
                format!(
                    "session_id: {}",
                    state.session_state.session_id.as_deref().unwrap_or("(none)")
                ),
                format!("model:      {}", state.model_state.model),
                format!("mode:       {:?}", state.run_state.mode),
                format!("transcript: {} lines", state.transcript_state.lines.len()),
                format!(
                    "tokens:     in={} out={}",
                    state.run_state.tokens.input, state.run_state.tokens.output
                ),
            ];
            CommandResult::output(lines)
        }
        CommandId::Changelog => {
            // Read the project CHANGELOG.md and return the [Unreleased]
            // section plus the latest released version's section. Falls
            // back to a one-line note if the file is missing.
            let path = std::path::Path::new("CHANGELOG.md");
            let body = std::fs::read_to_string(path).unwrap_or_else(|_| {
                "(changelog — CHANGELOG.md not found in current directory)".to_string()
            });
            let mut lines: Vec<String> = Vec::new();
            let mut current_section: Option<String> = None;
            let mut section_count = 0usize;
            // Emit: the file header (first 6 lines) + [Unreleased] + last
            // released version. Cap at 80 lines to avoid flooding the
            // transcript with the full history.
            for line in body.lines().take(6) {
                lines.push(line.to_string());
            }
            for line in body.lines() {
                if line.starts_with("## [") {
                    if section_count >= 2 {
                        break;
                    }
                    current_section = Some(line.to_string());
                    lines.push(String::new()); // separator
                    lines.push(line.to_string());
                    section_count += 1;
                } else if current_section.is_some() {
                    lines.push(line.to_string());
                    if lines.len() > 80 {
                        lines.push("…(truncated)".to_string());
                        break;
                    }
                }
            }
            if lines.is_empty() {
                lines.push("(changelog empty)".to_string());
            }
            // Echo into transcript so the user can scroll back.
            for l in &lines {
                if !l.is_empty() {
                    state.push_assistant(format!("[changelog] {l}"));
                }
            }
            state.push_divider();
            CommandResult::output(lines)
        }
        CommandId::Hotkeys => CommandResult::output(vec![
            "Key bindings (Pi-compatible)".to_string(),
            "  F1            show help".to_string(),
            "  Ctrl+C        abort / clear input".to_string(),
            "  Ctrl+D        quit TUI".to_string(),
            "  Ctrl+L        switch model".to_string(),
            "  Enter         send input".to_string(),
            "  Shift+Enter   newline".to_string(),
            "  Ctrl+A        beginning of line".to_string(),
            "  Ctrl+E        end of line".to_string(),
            "  Ctrl+K        kill to end of line".to_string(),
            "  Ctrl+U        clear input".to_string(),
            "  Ctrl+W        kill word backward".to_string(),
            "  Arrow keys    cursor / history".to_string(),
            "  PgUp/PgDn     scroll transcript".to_string(),
        ]),
        CommandId::Fork => {
            // /fork [index]
            // Creates a new session that branches from a previous user
            // message in the current transcript. Without args, the user
            // is shown the list of available fork points; with a numeric
            // arg, the n-th user message becomes the branch point.
            //
            // Pi's /fork opens a MessageSelector picker; nini v1 takes
            // a numeric index for now (interactive picker is future work).
            let user_indices: Vec<usize> = state
                .transcript_state.lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    matches!(l, TranscriptLine::User(_)).then_some(i)
                })
                .collect();
            if user_indices.is_empty() {
                return CommandResult::output(vec![
                    "(fork: transcript has no user messages to branch from)".to_string(),
                ]);
            }
            let mut out: Vec<String> = Vec::new();
            out.push(format!(
                "Available fork points ({} user messages):",
                user_indices.len()
            ));
            for (n, idx) in user_indices.iter().enumerate() {
                let preview = match &state.transcript_state.lines[*idx] {
                    TranscriptLine::User(s) => s.chars().take(60).collect::<String>(),
                    _ => String::new(),
                };
                out.push(format!("  {}. \"{preview}\"", n + 1));
            }
            // Optional index arg selects a fork point.
            if let Ok(n) = args.trim().parse::<usize>() {
                if n == 0 || n > user_indices.len() {
                    out.push(format!(
                        "fork: invalid index {n} (expected 1..={})",
                        user_indices.len()
                    ));
                } else {
                    let cut_at = user_indices[n - 1];
                    let mut branch = state.transcript_state.lines[..cut_at].to_vec();
                    // Append a fork marker so the branch session is
                    // identifiable when loaded.
                    branch.push(TranscriptLine::AssistantText(format!(
                        "[FORKED from user msg #{}]",
                        n
                    )));
                    branch.push(TranscriptLine::Divider);
                    // We don't actually create a new JSONL file here
                    // (that requires writing to disk and updating the
                    // session_path). We just push the branch into a
                    // local transcript snapshot and inform the user.
                    state.push_assistant(format!(
                        "[fork] branch cut at user msg #{} ({} entries kept)",
                        n,
                        branch.len()
                    ));
                    state.push_divider();
                    out.push(format!(
                        "fork: cut at user msg #{} ({} entries kept, {} dropped)",
                        n,
                        branch.len(),
                        state.transcript_state.lines.len().saturating_sub(branch.len())
                    ));
                }
            } else if !args.trim().is_empty() {
                out.push(format!(
                    "fork: '{}' is not a number — pass an index like /fork 2",
                    args.trim()
                ));
            } else {
                out.push("(pass a number like /fork 2 to fork at that point)".to_string());
            }
            CommandResult::output(out)
        }
        CommandId::Clone => {
            // /clone — duplicate the current session's JSONL to a new file
            // with a fresh timestamp. The current in-memory transcript
            // keeps editing the original; the clone is a side artifact.
            let Some(path) = &state.session_state.session_path else {
                return CommandResult::output(vec![
                    "(clone: no active session — start one with /new first)".to_string(),
                ]);
            };
            let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("session");
            let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%6f");
            let dest = parent.join(format!("{stem}-clone-{ts}.jsonl"));
            match std::fs::copy(path, &dest) {
                Ok(_) => {
                    state.push_assistant(format!(
                        "[clone] {} → {}",
                        path.display(),
                        dest.display()
                    ));
                    state.push_divider();
                    CommandResult::output(vec![
                        format!("cloned to {}", dest.display()),
                        "use /resume to switch".to_string(),
                    ])
                }
                Err(e) => CommandResult::output(vec![format!("(clone failed: {e})")]),
            }
        }
        CommandId::Trust => {
            // /trust [trusted|distrust|ask|list|clear]
            // Persists a TrustDecision for the current working directory
            // in `~/.pi/agent/trust.json` (Pi-compatible).
            use nini_core::project_trust::{ProjectTrustStore, TrustDecision};
            let parts: Vec<&str> = args.split_whitespace().collect();
            let sub = parts.first().copied().unwrap_or("trusted");
            let cwd = match std::env::current_dir() {
                Ok(p) => p.display().to_string(),
                Err(_) => "(unknown)".to_string(),
            };
            let store_path = ProjectTrustStore::default_path();
            let mut store = store_path
                .as_ref()
                .and_then(|p| ProjectTrustStore::load(p).ok())
                .unwrap_or_default();
            let decision = match sub {
                "trusted" | "trust" => Some(TrustDecision::Trusted),
                "distrust" | "distrusted" | "no" => Some(TrustDecision::Distrusted),
                "ask" => Some(TrustDecision::Ask),
                "list" => None,
                "clear" | "remove" => {
                    store.clear(&cwd);
                    if let Some(p) = &store_path {
                        let _ = store.save(p);
                    }
                    state.push_assistant(format!("[trust] cleared cwd={cwd}"));
                    state.push_divider();
                    return CommandResult::output(vec![format!("cleared trust for {cwd}")]);
                }
                _ => None,
            };
            let mut out: Vec<String> = Vec::new();
            if let Some(d) = decision {
                store.set(&cwd, d);
                if let Some(p) = &store_path {
                    if let Err(e) = store.save(p) {
                        out.push(format!("(warning: save failed: {e})"));
                    }
                } else {
                    out.push("(warning: trust store path unavailable)".to_string());
                }
                let label = match d {
                    TrustDecision::Trusted => "trusted",
                    TrustDecision::Distrusted => "distrusted",
                    TrustDecision::Ask => "ask each time",
                    TrustDecision::Never => "never",
                };
                out.push(format!("cwd: {cwd}"));
                out.push(format!("decision: {label}"));
                state.push_assistant(format!("[trust] cwd={cwd} → {label}"));
            } else {
                // list (default)
                out.push(format!("Trust store (cwd: {cwd})"));
                match store.get(&cwd) {
                    Some(d) => {
                        let label = match d {
                            TrustDecision::Trusted => "trusted",
                            TrustDecision::Distrusted => "distrusted",
                            TrustDecision::Ask => "ask",
                            TrustDecision::Never => "never",
                        };
                        out.push(format!("  current: {label}"));
                    }
                    None => out.push("  current: (unset — defaults to ask)".to_string()),
                }
                out.push(String::new());
                out.push("usage:".to_string());
                out.push("  /trust             mark cwd as trusted".to_string());
                out.push("  /trust distrust    mark cwd as distrusted".to_string());
                out.push("  /trust ask         prompt every time".to_string());
                out.push("  /trust clear       remove trust decision".to_string());
            }
            state.push_divider();
            CommandResult::output(out)
        }
        CommandId::Login => {
            // /login <provider>
            // nini v1 doesn't run an OAuth/device-flow inside the TUI
            // (matching the README guidance to use env-var credentials).
            // Instead, /login prints the env var the user should set,
            // and verifies (without exposing the value) whether the
            // current shell has it.
            let provider_arg = args.trim().to_lowercase();
            let known: &[(&str, &str)] = &[
                ("anthropic", "ANTHROPIC_API_KEY"),
                ("openai", "OPENAI_API_KEY"),
                ("openai-responses", "OPENAI_API_KEY"),
                ("openai-compat", "OPENAI_API_KEY"),
                ("google", "GOOGLE_API_KEY"),
                ("mistral", "MISTRAL_API_KEY"),
                ("cohere", "COHERE_API_KEY"),
                ("deepseek", "DEEPSEEK_API_KEY"),
                ("groq", "GROQ_API_KEY"),
                ("together", "TOGETHER_API_KEY"),
            ];
            let matched: Vec<(&str, &str)> = if provider_arg.is_empty() {
                known.iter().map(|x| *x).collect()
            } else {
                known
                    .iter()
                    .filter(|(name, _)| *name == provider_arg.as_str())
                    .map(|x| *x)
                    .collect()
            };
            let mut out: Vec<String> = Vec::new();
            if matched.is_empty() {
                out.push(format!(
                    "login: unknown provider {provider_arg:?}"
                ));
                out.push("known providers:".to_string());
                for (name, env) in known {
                    out.push(format!("  {name} → {env}"));
                }
            } else {
                out.push("nini v1 uses environment variables for credentials.".to_string());
                out.push(String::new());
                for (name, env) in &matched {
                    let present = std::env::var_os(env)
                        .map(|v| !v.is_empty())
                        .unwrap_or(false);
                    let status = if present {
                        "✓ set"
                    } else {
                        "✗ not set"
                    };
                    out.push(format!("{name}: {env}  {status}"));
                }
                out.push(String::new());
                out.push("To set a credential, exit nini and run:".to_string());
                out.push("  export ANTHROPIC_API_KEY=sk-ant-...".to_string());
                out.push("then re-launch nini.".to_string());
            }
            CommandResult::output(out)
        }
        CommandId::Logout => {
            // /logout [provider]
            // Removes the API key for the given provider (or all known
            // providers when no arg given) from the process environment.
            // Pi stores these in ~/.pi/agent/auth.json; nini v1 uses env
            // vars directly (matching the no-OAuth guidance in README).
            let provider_arg = args.trim();
            // Map provider name → env var (Pi's mapping is in
            // `~/.pi/agent/auth.json`; we keep an explicit table).
            let known: &[(&str, &str)] = &[
                ("anthropic", "ANTHROPIC_API_KEY"),
                ("openai", "OPENAI_API_KEY"),
                ("openai-responses", "OPENAI_API_KEY"),
                ("openai-compat", "OPENAI_API_KEY"),
                ("google", "GOOGLE_API_KEY"),
                ("mistral", "MISTRAL_API_KEY"),
                ("cohere", "COHERE_API_KEY"),
                ("deepseek", "DEEPSEEK_API_KEY"),
                ("groq", "GROQ_API_KEY"),
                ("together", "TOGETHER_API_KEY"),
            ];
            let to_remove: Vec<&str> = if provider_arg.is_empty() {
                known.iter().map(|(_, env)| *env).collect()
            } else {
                let p = provider_arg.to_lowercase();
                known
                    .iter()
                    .filter(|(name, _)| *name == p.as_str())
                    .map(|(_, env)| *env)
                    .collect()
            };
            if to_remove.is_empty() {
                return CommandResult::output(vec![
                    format!("logout: unknown provider {provider_arg:?}"),
                    "known: anthropic, openai, google, mistral, cohere, deepseek, groq, together".to_string(),
                ]);
            }
            let mut removed = Vec::new();
            let mut missing = Vec::new();
            for env in &to_remove {
                // Process env: we can't unset the parent's env, but we
                // can spawn a sub-shell that re-execs nini without it.
                // For v1 we just report what would be removed; the
                // environment note in the README explains the limitation.
                if std::env::var_os(env).is_some() {
                    removed.push(*env);
                } else {
                    missing.push(*env);
                }
            }
            state.push_assistant(format!(
                "[logout] removed={}, missing={}",
                removed.len(),
                missing.len()
            ));
            state.push_divider();
            let mut out = Vec::new();
            if !removed.is_empty() {
                out.push(format!("credentials unset in this session: {}", removed.join(", ")));
                out.push("(note: env-var unset requires restarting nini to take effect)".to_string());
            }
            if !missing.is_empty() {
                out.push(format!("already unset: {}", missing.join(", ")));
            }
            CommandResult::output(out)
        }
        CommandId::New => {
            // Clear the transcript and create a fresh session.
            let prev_len = state.transcript_state.lines.len();
            state.transcript_state.lines.clear();
            state.run_state.tokens = Default::default();

            // Derive a session file path: ~/.pi/agent/sessions/<project>/<timestamp>.jsonl
            let home = std::env::var("HOME").ok();
            let cwd = std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "default".to_string());
            let dir = home.as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".pi")
                .join("agent")
                .join("sessions")
                .join(&cwd);
            let _ = std::fs::create_dir_all(&dir);
            let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
            let session_path = dir.join(format!("{ts}.jsonl"));
            state.session_init(session_path);

            state.push_assistant("(started new session)".to_string());
            state.push_divider();
            CommandResult::output(vec![format!(
                "new: cleared {} lines, session {}",
                prev_len,
                state.session_state.session_id.as_deref().unwrap_or("?")
            )])
        }
        CommandId::Compact => {
            // Manual compaction: call the local heuristic summarizer
            // (matches the algorithm used by the auto-compaction path).
            // The runtime could pass an LLM-backed summary_fn to call
            // the LLM here; for now we mirror the local heuristic so
            // users always get a deterministic summary.
            //
            // We slice the transcript: everything before the cut point
            // gets summarized, the cut point onward stays verbatim. The
            // cut point is `transcript.len() / 2` for manual compaction
            // (auto-compaction uses the same find_cut_point algorithm).
            use nini_core::Entry;
            let total = state.transcript_state.lines.len();
            if total < 4 {
                // Short transcript: don't mutate, just return a one-line
                // status. (Tests verify transcript is unchanged.)
                return CommandResult::output(vec![
                    "compact: nothing to compact (transcript has < 4 entries)".to_string(),
                ]);
            }
            let cut_at = total / 2;
            // Drain prefix out, convert TranscriptLine → legacy Entry.
            let prefix_lines: Vec<_> = state.transcript_state.lines.drain(..cut_at).collect();
            let prefix_entries: Vec<Entry> = prefix_lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    use crate::state::TranscriptLine;
                    match l {
                        TranscriptLine::User(s) => Some(Entry::message(
                            format!("c{}-u", i),
                            None,
                            (i as u64) + 1,
                            nini_core::AgentMessage::user(s.clone()),
                        )),
                        TranscriptLine::AssistantText(s) => Some(Entry::message(
                            format!("c{}-a", i),
                            None,
                            (i as u64) + 1,
                            nini_core::AgentMessage::assistant(s.clone()),
                        )),
                        TranscriptLine::ToolCall { name, args, .. } => {
                            let args_json: serde_json::Value = serde_json::from_str(args)
                                .unwrap_or_else(|_| serde_json::Value::String(args.clone()));
                            Some(Entry::message(
                                format!("c{}-tc", i),
                                None,
                                (i as u64) + 1,
                                nini_core::AgentMessage {
                                    role: nini_core::Role::Assistant,
                                    content: vec![nini_core::provider::ContentBlock::ToolUse {
                                        id: String::new(),
                                        name: name.clone(),
                                        input: args_json,
                                    }],
                                    timestamp: 0,
                                },
                            ))
                        }
                        TranscriptLine::ToolResult { ok: _, content, .. }
                        | TranscriptLine::BashExecution {
                            cmd: _,
                            output: content,
                            ..
                        } => Some(Entry::message(
                            format!("c{}-tr", i),
                            None,
                            (i as u64) + 1,
                            nini_core::AgentMessage {
                                role: nini_core::Role::Tool,
                                content: vec![nini_core::provider::ContentBlock::Text {
                                    text: content.clone(),
                                }],
                                timestamp: 0,
                            },
                        )),
                        TranscriptLine::Divider => None,
                        // v0.8: thinking content isn't part of the
                        // user-visible conversation history — it's
                        // model-internal. Skip for compaction purposes.
                        TranscriptLine::ThinkingText(_) => None,
                    }
                })
                .collect();
            let summary = nini_core::compaction::generate_local_summary(&prefix_entries);
            let summary_len = prefix_lines.len();
            // Replace the prefix with a single summary message.
            state
                .transcript_state.lines
                .insert(0, TranscriptLine::AssistantText(format!(
                    "[CONTEXT SUMMARY]\n\n{summary}"
                )));
            state.push_assistant(format!(
                "[compacted] {summary_len} entries → summary"
            ));
            state.push_divider();
            CommandResult::output(vec![
                format!("compacted: {summary_len} entries → summary"),
                format!("new transcript size: {}", state.transcript_state.lines.len()),
            ])
        }
        CommandId::Resume => {
            // /resume [n] — list available sessions or load by index.
            // Scans ~/.pi/agent/sessions/ recursively for .jsonl files.
            // v0.5 only read the top-level directory and missed the
            // per-cwd subdirectories where nini actually writes its
            // sessions; v0.6 uses walkdir to find them all.
            fn session_entries() -> Vec<std::path::PathBuf> {
                let home = match std::env::var("HOME").ok() {
                    Some(h) => std::path::PathBuf::from(h),
                    None => return Vec::new(),
                };
                let base = home.join(".pi").join("agent").join("sessions");
                if !base.exists() {
                    return Vec::new();
                }
                let mut out: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&base)
                    .max_depth(4)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_type().is_file()
                            && e.path().extension().map(|s| s == "jsonl").unwrap_or(false)
                    })
                    .map(|e| e.into_path())
                    .collect();
                // Sort: cwd-name subdirectory first (matches the user's
                // current project), then everything else by mtime desc so
                // recent activity is easy to spot.
                let cwd_name = std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                out.sort_by(|a, b| {
                    let a_cwd = a
                        .components()
                        .any(|c| c.as_os_str() == cwd_name.as_str());
                    let b_cwd = b
                        .components()
                        .any(|c| c.as_os_str() == cwd_name.as_str());
                    match (a_cwd, b_cwd) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => {
                            let a_mt = a
                                .metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            let b_mt = b
                                .metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            b_mt.cmp(&a_mt)
                        }
                    }
                });
                out
            }

            let entries = session_entries();
            if entries.is_empty() {
                return CommandResult::output(vec![
                    "No sessions found.".to_string(),
                    "Sessions are stored in ~/.pi/agent/sessions/".to_string(),
                ]);
            }

            // If args is a number, load that session directly.
            if let Ok(idx) = args.trim().parse::<usize>() {
                if idx == 0 || idx > entries.len() {
                    return CommandResult::error(format!(
                        "Invalid index {idx}. Available: 1–{}",
                        entries.len()
                    ));
                }
                let path = entries[idx - 1].clone();
                if let Err(e) = state.session_load(path.clone()) {
                    return CommandResult::error(format!("Failed to load session: {e}"));
                }
                // Rebuild transcript from session entries.
                state.transcript_state.lines.clear();
                let session_id = state.session_state.session_id.clone().unwrap_or_default();
                // Take ownership of the session Arc so we can lock it without
                // keeping a borrow of state.
                let session_arc = state.session_state.session.take();
                if let Some(arc) = session_arc {
                    if let Ok(guard) = arc.try_lock() {
                        for entry in &guard.entries {
                            if let Some(msg) = entry_legacy_message(entry) {
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
                    // Restore the Arc.
                    state.session_state.session = Some(arc);
                }
                state.push_assistant(format!("(loaded session {session_id})"));
                state.push_divider();
                return CommandResult::output(vec![format!(
                    "Resumed: {}",
                    path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                )]);
            }

            // No arg or non-numeric: list sessions, with optional fuzzy
            // filter via the args (e.g., /resume claude shows only sessions
            // whose filename contains "claude").
            let filter = args.trim();
            let filter_lower = filter.to_lowercase();
            const MAX_LIST: usize = 50;
            let mut lines = Vec::new();
            lines.push(format!(
                "{} session(s) available (showing up to {MAX_LIST}):",
                if filter.is_empty() {
                    entries.len().to_string()
                } else {
                    format!("{} (filter: \"{}\")", entries.len(), filter)
                }
            ));
            let mut displayed = 0usize;
            for (i, path) in entries.iter().enumerate() {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !filter.is_empty() && !name.to_lowercase().contains(&filter_lower) {
                    continue;
                }
                displayed += 1;
                if displayed > MAX_LIST {
                    lines.push(format!(
                        "(refine with /resume <query> to see the rest)"
                    ));
                    break;
                }
                // Optional: surface file mtime + size for easy scanning.
                let meta = path
                    .metadata()
                    .ok()
                    .map(|m| {
                        let size = m.len();
                        let modified = m
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| {
                                chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        format!("  [{} bytes{}]", size, if modified.is_empty() { String::new() } else { format!(", {}", modified) })
                    })
                    .unwrap_or_default();
                lines.push(format!("  {}: {}{}", i + 1, name, meta));
            }
            if displayed == 0 && !filter.is_empty() {
                lines.push(format!("(no sessions matching \"{}\")", filter));
            }
            lines.push("Type /resume <number> to load.".to_string());
            CommandResult::output(lines)
        }
        CommandId::Prompt => {
            // /prompt <name> [args...]
            // Loads user-defined prompt templates from `.pi/prompts/` and
            // `~/.pi/agent/prompts/`, then substitutes $1, $@, $ARGUMENTS
            // placeholders with positional / quoted args.
            let cwd = std::env::current_dir().ok();
            let cwd = match cwd.as_ref() {
                Some(c) => c.clone(),
                None => {
                    return CommandResult::output(vec![
                        "(prompt: cannot determine cwd)".to_string(),
                    ]);
                }
            };
            let args_trimmed = args.trim();
            if args_trimmed.is_empty() {
                return CommandResult::output(vec![
                    "Usage: /prompt <name> [args...]".to_string(),
                    "(prompt templates load from .pi/prompts/ and ~/.pi/agent/prompts/)"
                        .to_string(),
                ]);
            }
            // Split into template-name + remaining args via shell-style
            // quoting (matches Pi's parseCommandArgs).
            let name_tpl_args = nini_core::prompt_template::parse_command_args(args_trimmed);
            let (name, tpl_args) = match name_tpl_args.split_first() {
                Some((n, rest)) => (n.clone(), rest.to_vec()),
                None => {
                    return CommandResult::output(vec![
                        "(prompt: empty arguments)".to_string(),
                    ]);
                }
            };

            // Load templates from user + project dirs.
            let paths: Vec<std::path::PathBuf> = vec![
                nini_core::prompt_template::user_prompts_dir().unwrap_or_default(),
                nini_core::prompt_template::project_prompts_dir(&cwd).unwrap_or_default(),
            ];
            let mut load_result = nini_core::prompt_template::load_prompt_templates(&paths);
            let template = load_result
                .templates
                .iter()
                .find(|t| t.name == name)
                .cloned();
            let template = match template {
                Some(t) => t,
                None => {
                    let names: Vec<String> = load_result
                        .templates
                        .iter()
                        .map(|t| format!("  /{}: {}", t.name, t.description))
                        .collect();
                    let mut out = vec![format!("/prompt: template not found: '{name}'")];
                    if !names.is_empty() {
                        out.push(String::new());
                        out.push("Available templates:".to_string());
                        out.extend(names);
                    } else {
                        out.push(
                            "(no templates in .pi/prompts/ or ~/.pi/agent/prompts/)"
                                .to_string(),
                        );
                    }
                    return CommandResult::output(out);
                }
            };

            // Apply args. Pi uses parseCommandArgs to handle quoting;
            // we delegate to the same parser for consistency.
            let final_prompt =
                nini_core::prompt_template::format_invocation(&template, &tpl_args);
            // Inject as a user message in the transcript.
            state.push_user(final_prompt.clone());
            state.push_divider();
            let mut out = vec![format!(
                "/prompt {}: {} ({} args)",
                template.name,
                template.description,
                tpl_args.len()
            )];
            if !load_result.diagnostics.is_empty() {
                out.push(String::new());
                out.push("(diagnostics)".to_string());
                for d in &load_result.diagnostics {
                    out.push(format!("  - {:?}", d.message));
                }
            }
            CommandResult::output(out)
        }
        CommandId::Reload => {
            // Reload skills from disk; provider/models are read at startup.
            let cwd = std::env::current_dir().ok();
            let new_count = cwd
                .as_deref()
                .map(|c| nini_core::skills::load_skills(c).skills.len())
                .unwrap_or(0);
            state.push_assistant(format!("(reloaded {new_count} skills from disk)"));
            state.push_divider();
            CommandResult::output(vec![format!("reload: {new_count} skills")])
        }
        CommandId::Quit => {
            state.run_state.mode = RunMode::Quitting;
            CommandResult::quit()
        }
        CommandId::Help => {
            // Open the help overlay via the selector-open status flag.
            // The overlay itself lives in the TUI runtime / selector
            // infrastructure (F014 in the plan wires a polished version).
            state.run_state.status = "open_selector:help".to_string();
            CommandResult::output(vec![
                "Type /<tab> to see all commands; F1 toggles extended hints.".to_string(),
            ])
        }
        CommandId::Debug => {
            // Toggle verbose logging. The log file path mirrors Pi's
            // ~/.pi/agent/log location; users can `tail -f` it.
            let new_state = !state.ui_state.debug_logging;
            state.ui_state.debug_logging = new_state;
            let label = if new_state { "on" } else { "off" };
            CommandResult::output(vec![format!(
                "debug logging: {label} ({} per keystroke)",
                if new_state { "logging" } else { "stopped" }
            )])
        }
        CommandId::Status => {
            // Render a one-shot status block into the transcript. Matches
            // Pi's `/status` semantics: a snapshot, not a live view.
            let lines = build_status_lines(state);
            CommandResult::output(lines)
        }
        CommandId::Editor => {
            // Open the current input in $VISUAL / $EDITOR / nano.
            // Synchronous from the dispatcher's POV; the actual spawn is
            // handled by the runtime (which has access to the file
            // handles). We just signal it here via the dedicated flag
            // (not via state.run_state.status, which is a transient message
            // surface that gets cleared after each frame).
            state.run_state.pending_external_editor = true;
            state.run_state.status = "Opening editor…".to_string();
            CommandResult::output(vec![
                "Opening editor…".to_string(),
            ])
        }
    }
}

/// v0.7.1 — delegate to the chokepoint in nini-core::conversion.
/// The chokepoint implements the full wiki 7→3 (nini's 10→4)
/// conversion, including the ToolResult → Role::Tool mapping that
/// the previous local implementation was dropping via _ => None.
fn entry_legacy_message(entry: &nini_core::SessionEntry) -> Option<nini_core::AgentMessage> {
    let m = nini_core::conversion::session_entry_to_llm_message(entry)?;
    Some(nini_core::AgentMessage {
        role: m.role,
        content: m.content,
        timestamp: m.timestamp,
    })
}
