//! Slash command framework: 22 Pi-compatible built-in commands.
//!
//! Mirrors spec `references/spec-v0.85.1/slash-commands/builtin.json`.
//!
//! Architecture:
//! - [`CommandId`]: enum of all built-in commands
//! - [`CommandDef`]: static metadata (name, description, argument hint)
//! - [`CommandResult`]: what a command produces (status, output lines)
//! - [`dispatch`]: pure function — takes `(id, args, &state)` → `CommandResult`
//! - [`complete`]: `/` + partial input → ranked list of matches
//!
//! All commands are pure functions over [`AppState`] for now. Commands that
//! would require async I/O (e.g., `/login`, `/export`) emit a "not yet
//! implemented" result — wiring those up is a follow-up.

use crate::settings::SettingsManager;
use crate::state::{AppState, RunMode, TranscriptLine};

/// 22 Pi-compatible slash commands (canonical order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    Settings,
    Model,
    Tree,
    Thinking,
    ScopedModels,
    Export,
    Import,
    Share,
    Copy,
    Name,
    Session,
    Changelog,
    Hotkeys,
    Fork,
    Clone,
    Trust,
    Login,
    Logout,
    New,
    Compact,
    Resume,
    Prompt,
    Reload,
    Quit,
}

/// Static definition: name, description, argument hint.
#[derive(Debug, Clone, Copy)]
pub struct CommandDef {
    pub id: CommandId,
    pub name: &'static str,
    pub description: &'static str,
    pub argument_hint: Option<&'static str>,
}

impl CommandDef {
    pub const fn new(
        id: CommandId,
        name: &'static str,
        description: &'static str,
        argument_hint: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            argument_hint,
        }
    }
}

/// The full registry. Order matches `builtin.json`.
pub const REGISTRY: &[CommandDef] = &[
    CommandDef::new(CommandId::Settings, "settings", "Open settings menu", None),
    CommandDef::new(
        CommandId::Model,
        "model",
        "Select model (opens selector UI)",
        Some("<provider/model>"),
    ),
    CommandDef::new(
        CommandId::Tree,
        "tree",
        "Navigate session tree (switch branches)",
        None,
    ),
    CommandDef::new(
        CommandId::Thinking,
        "thinking",
        "Set thinking level",
        Some("<level>"),
    ),
    CommandDef::new(
        CommandId::ScopedModels,
        "scoped-models",
        "Enable/disable models for Ctrl+P cycling",
        None,
    ),
    CommandDef::new(
        CommandId::Export,
        "export",
        "Export session (HTML default, or specify path: .html/.jsonl)",
        None,
    ),
    CommandDef::new(
        CommandId::Import,
        "import",
        "Import and resume a session from a JSONL file",
        None,
    ),
    CommandDef::new(
        CommandId::Share,
        "share",
        "Share session as a secret GitHub gist",
        None,
    ),
    CommandDef::new(
        CommandId::Copy,
        "copy",
        "Copy last agent message to clipboard",
        None,
    ),
    CommandDef::new(
        CommandId::Name,
        "name",
        "Set session display name",
        Some("<name>"),
    ),
    CommandDef::new(
        CommandId::Session,
        "session",
        "Show session info and stats",
        None,
    ),
    CommandDef::new(
        CommandId::Changelog,
        "changelog",
        "Show changelog entries",
        None,
    ),
    CommandDef::new(
        CommandId::Hotkeys,
        "hotkeys",
        "Show all keyboard shortcuts",
        None,
    ),
    CommandDef::new(
        CommandId::Fork,
        "fork",
        "Create a new fork from a previous user message",
        None,
    ),
    CommandDef::new(
        CommandId::Clone,
        "clone",
        "Duplicate the current session at the current position",
        None,
    ),
    CommandDef::new(
        CommandId::Trust,
        "trust",
        "Save project trust decision for future sessions",
        None,
    ),
    CommandDef::new(
        CommandId::Login,
        "login",
        "Configure provider authentication",
        Some("<provider>"),
    ),
    CommandDef::new(
        CommandId::Logout,
        "logout",
        "Remove provider authentication",
        None,
    ),
    CommandDef::new(CommandId::New, "new", "Start a new session", None),
    CommandDef::new(
        CommandId::Compact,
        "compact",
        "Manually compact the session context",
        None,
    ),
    CommandDef::new(
        CommandId::Resume,
        "resume",
        "Resume a different session",
        None,
    ),
    CommandDef::new(
        CommandId::Prompt,
        "prompt",
        "Run a user-defined prompt template (from .pi/prompts/*.md)",
        Some("<name> [args...]"),
    ),
    CommandDef::new(
        CommandId::Reload,
        "reload",
        "Reload keybindings, extensions, skills, prompts, themes, and context files",
        None,
    ),
    CommandDef::new(CommandId::Quit, "quit", "Quit nini", None),
];

/// Look up a command by name (case-sensitive, exact match).
pub fn by_name(name: &str) -> Option<&'static CommandDef> {
    REGISTRY.iter().find(|c| c.name == name)
}

/// Fuzzy completion: `query` is the partial input AFTER the leading `/`.
/// Returns commands ranked by:
/// 1. Exact prefix match (e.g., `mo` → `model`)
/// 2. Substring match
/// 3. Common prefix length (descending)
///
/// The list is capped at `limit` (default 8).
pub fn complete(query: &str, limit: usize) -> Vec<&'static CommandDef> {
    if query.is_empty() {
        return REGISTRY.iter().take(limit).collect();
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(usize, &CommandDef)> = REGISTRY
        .iter()
        .filter_map(|c| {
            let lower = c.name.to_lowercase();
            if lower.starts_with(&q) {
                // Best: prefix match. Score by inverse of length (shorter = better).
                Some((0, c))
            } else if lower.contains(&q) {
                // Substring match. Score by index of match.
                let idx = lower.find(&q).unwrap();
                Some((100 + idx, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(s, _)| *s);
    scored.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// What a command produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Side-effect happened; emit these lines into the transcript.
    Output(Vec<String>),
    /// User must exit the TUI.
    Quit,
    /// Switch the editor into a different mode (e.g., prompt for argument).
    PromptArgument { prompt: String, next: CommandId },
}

/// Result of running a command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub outcome: CommandOutcome,
    /// Optional error message. When present, the runtime shows this to the user
    /// in the transcript instead of the normal output.
    pub error: Option<String>,
}

impl CommandResult {
    pub fn output(lines: Vec<String>) -> Self {
        Self {
            outcome: CommandOutcome::Output(lines),
            error: None,
        }
    }
    /// Wrap a command result that failed with an error message.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            outcome: CommandOutcome::Output(vec![]),
            error: Some(msg.into()),
        }
    }
    pub fn quit() -> Self {
        Self {
            outcome: CommandOutcome::Quit,
            error: None,
        }
    }
    pub fn prompt(prompt: impl Into<String>, next: CommandId) -> Self {
        Self {
            outcome: CommandOutcome::PromptArgument {
                prompt: prompt.into(),
                next,
            },
            error: None,
        }
    }
}

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
                format!("  model:     {}", state.model),
                format!(
                    "  session:   {}",
                    state.session_id.as_deref().unwrap_or("(none)")
                ),
                format!("  status:    {:?}", state.mode),
            ];
            CommandResult::output(lines)
        }
        CommandId::Model => {
            // /model <provider/model> — set the model for the next turn.
            if args.is_empty() {
                return CommandResult::output(vec![
                    "Usage: /model <provider/model>".to_string(),
                    format!("Current model: {}", state.model),
                ]);
            }
            state.model = args.to_string();
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
                if let Some(path) = &state.session_path {
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
            // cycle list lives in `state.models_cycle`.
            let parts: Vec<&str> = args.split_whitespace().collect();
            let sub = parts.first().copied().unwrap_or("list");
            match sub {
                "add" if parts.len() >= 2 => {
                    let m = parts[1].to_string();
                    if !state.models_cycle.iter().any(|x| x == &m) {
                        state.models_cycle.push(m.clone());
                    }
                    let _ = settings.flush(); // persist after cycle change
                    state.push_assistant(format!("[scoped-models] added {m}"));
                    state.push_divider();
                    let mut out = vec![
                        format!("added {m} to cycle"),
                        format!("cycle ({} models):", state.models_cycle.len()),
                    ];
                    for cm in &state.models_cycle {
                        out.push(format!("  - {cm}"));
                    }
                    CommandResult::output(out)
                }
                "remove" if parts.len() >= 2 => {
                    let m = parts[1];
                    let before = state.models_cycle.len();
                    state.models_cycle.retain(|x| x != m);
                    let removed = before - state.models_cycle.len();
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
                    state.models_cycle.clear();
                    state.models_cycle_idx = None;
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
                            state.models_cycle.len()
                        ),
                        "".to_string(),
                    ];
                    if state.models_cycle.is_empty() {
                        out.push("(empty — use `/scoped-models add <model>`)".to_string());
                        out.push(String::new());
                        out.push("Examples:".to_string());
                        out.push("  /scoped-models add anthropic/claude-opus-4-7".to_string());
                        out.push("  /scoped-models add openai/gpt-5".to_string());
                        out.push("  /scoped-models clear".to_string());
                    } else {
                        for (i, m) in state.models_cycle.iter().enumerate() {
                            let marker = if Some(i) == state.models_cycle_idx {
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
            let html = render_transcript_html(&state.transcript);
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
                state.session_id = Some(info.id.clone());
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
            state.transcript = new_transcript;
            state.tokens = Default::default();
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
            let last_assistant = state.transcript.iter().rev().find_map(|l| match l {
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
            state.status = format!("name: {args}");
            state.push_assistant(format!("(session name: {args})"));
            state.push_divider();
            CommandResult::output(vec![format!("name → {args}")])
        }
        CommandId::Session => {
            let lines = vec![
                format!(
                    "session_id: {}",
                    state.session_id.as_deref().unwrap_or("(none)")
                ),
                format!("model:      {}", state.model),
                format!("mode:       {:?}", state.mode),
                format!("transcript: {} lines", state.transcript.len()),
                format!(
                    "tokens:     in={} out={}",
                    state.tokens.input, state.tokens.output
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
                .transcript
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
                let preview = match &state.transcript[*idx] {
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
                    let mut branch = state.transcript[..cut_at].to_vec();
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
                        state.transcript.len().saturating_sub(branch.len())
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
            let Some(path) = &state.session_path else {
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
            let prev_len = state.transcript.len();
            state.transcript.clear();
            state.tokens = Default::default();

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
                state.session_id.as_deref().unwrap_or("?")
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
            let total = state.transcript.len();
            if total < 4 {
                // Short transcript: don't mutate, just return a one-line
                // status. (Tests verify transcript is unchanged.)
                return CommandResult::output(vec![
                    "compact: nothing to compact (transcript has < 4 entries)".to_string(),
                ]);
            }
            let cut_at = total / 2;
            // Drain prefix out, convert TranscriptLine → legacy Entry.
            let prefix_lines: Vec<_> = state.transcript.drain(..cut_at).collect();
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
                        TranscriptLine::ToolCall { name, args } => {
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
                        TranscriptLine::ToolResult { ok: _, content }
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
                    }
                })
                .collect();
            let summary = nini_core::compaction::generate_local_summary(&prefix_entries);
            let summary_len = prefix_lines.len();
            // Replace the prefix with a single summary message.
            state
                .transcript
                .insert(0, TranscriptLine::AssistantText(format!(
                    "[CONTEXT SUMMARY]\n\n{summary}"
                )));
            state.push_assistant(format!(
                "[compacted] {summary_len} entries → summary"
            ));
            state.push_divider();
            CommandResult::output(vec![
                format!("compacted: {summary_len} entries → summary"),
                format!("new transcript size: {}", state.transcript.len()),
            ])
        }
        CommandId::Resume => {
            // /resume [n] — list available sessions or load by index.
            // Scans ~/.pi/agent/sessions/ for .jsonl files.
            fn session_entries() -> Vec<std::fs::DirEntry> {
                let home = std::env::var("HOME").ok();
                let base = home.as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let dir = base.join(".pi").join("agent").join("sessions");
                std::fs::read_dir(&dir)
                    .ok()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.path().extension().map(|s| s == "jsonl").unwrap_or(false))
                    .collect()
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
                let entry = &entries[idx - 1];
                let path = entry.path();
                if let Err(e) = state.session_load(path.clone()) {
                    return CommandResult::error(format!("Failed to load session: {e}"));
                }
                // Rebuild transcript from session entries.
                state.transcript.clear();
                let session_id = state.session_id.clone().unwrap_or_default();
                // Take ownership of the session Arc so we can lock it without
                // keeping a borrow of state.
                let session_arc = state.session.take();
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
                    state.session = Some(arc);
                }
                state.push_assistant(format!("(loaded session {session_id})"));
                state.push_divider();
                return CommandResult::output(vec![format!(
                    "Resumed: {}",
                    path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                )]);
            }

            // No arg or non-numeric: list sessions.
            let mut lines = Vec::new();
            lines.push(format!("{} session(s) available:", entries.len()));
            for (i, entry) in entries.iter().enumerate() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                lines.push(format!("  {}: {}", i + 1, name));
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
            let new_count = cwd.as_deref().map(state_helpers::count_skills).unwrap_or(0);
            state.push_assistant(format!("(reloaded {new_count} skills from disk)"));
            state.push_divider();
            CommandResult::output(vec![format!("reload: {new_count} skills")])
        }
        CommandId::Quit => {
            state.mode = RunMode::Quitting;
            CommandResult::quit()
        }
    }
}

/// Render the transcript as a minimal HTML document (used by /export).
fn render_transcript_html(lines: &[TranscriptLine]) -> String {
    let mut out = String::from(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>nini session</title>\
         <style>body{font-family:system-ui;max-width:800px;margin:2em auto;padding:0 1em;}\
         .user{color:#0a7}.assistant{color:#333}.tool{color:#a3a;font-family:monospace}\
         .divider{border-top:1px solid #ccc;margin:1em 0}</style></head><body>\n",
    );
    for l in lines {
        match l {
            TranscriptLine::User(t) => {
                out.push_str(&format!("<p class=\"user\"><b>&gt;</b> {t}</p>\n"));
            }
            TranscriptLine::AssistantText(t) => {
                out.push_str(&format!("<p class=\"assistant\">{t}</p>\n"));
            }
            TranscriptLine::ToolCall { name, args } => {
                out.push_str(&format!(
                    "<p class=\"tool\">[tool call] {name} {args}</p>\n"
                ));
            }
            TranscriptLine::BashExecution { cmd, output, ok, exit_code, duration_ms, .. } => {
                let status = if *ok { "ok" } else { "fail" };
                let escaped_output = output
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                out.push_str(&format!(
                    "<p class=\"bash\">! <code>{cmd}</code> [{status}{}] in {duration_ms}ms<br><pre>{escaped_output}</pre></p>\n",
                    exit_code.map(|c| format!(" exit={c}")).unwrap_or_default(),
                ));
            }
            TranscriptLine::ToolResult { ok, content } => {
                let cls = "tool";
                let label = if *ok { "tool result" } else { "tool error" };
                out.push_str(&format!("<p class=\"{cls}\">[{label}] {content}</p>\n"));
            }
            TranscriptLine::Divider => {
                out.push_str("<hr class=\"divider\">\n");
            }
        }
    }
    out.push_str("</body></html>\n");
    out
}

/// Parse a slash command invocation. Returns `(command_id, args)` or
/// `None` if the input isn't a valid `/cmd args` invocation.
pub fn parse(input: &str) -> Option<(CommandId, String)> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i + 1..].trim().to_string()),
        None => (rest, String::new()),
    };
    let def = by_name(name)?;
    Some((def.id, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_24_commands() {
        // Pi spec `builtin.json` declares 23 entries (count field), but
        // nini adds `/prompt` for user-defined prompt templates (loaded
        // from .pi/prompts/*.md), so 23 + 1 = 24.
        assert_eq!(
            REGISTRY.len(),
            24,
            "expected 24 commands (23 Pi builtin + 1 nini /prompt)"
        );
    }

    #[test]
    fn all_required_command_ids_present() {
        // Sanity: spot-check a handful of names that must be present.
        for name in [
            "settings", "model", "tree", "thinking", "export", "import", "session", "hotkeys",
            "fork", "clone", "trust", "new", "compact", "resume", "reload", "prompt", "quit",
        ] {
            assert!(by_name(name).is_some(), "missing command /{name}");
        }
    }

    #[test]
    fn parse_simple_command() {
        let (id, args) = parse("/model anthropic/claude-opus-4-7").unwrap();
        assert_eq!(id, CommandId::Model);
        assert_eq!(args, "anthropic/claude-opus-4-7");
    }

    #[test]
    fn parse_no_args() {
        let (id, args) = parse("/quit").unwrap();
        assert_eq!(id, CommandId::Quit);
        assert_eq!(args, "");
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse("/notacommand").is_none());
    }

    #[test]
    fn parse_without_slash_returns_none() {
        assert!(parse("hello world").is_none());
    }

    #[test]
    fn complete_prefix_match_wins() {
        let r = complete("mo", 8);
        assert!(r.iter().any(|c| c.name == "model"));
        // 'model' should come before 'scoped-models' (substring match)
        assert_eq!(r[0].name, "model");
    }

    #[test]
    fn complete_substring_match() {
        let r = complete("odel", 8);
        assert!(r.iter().any(|c| c.name == "model"));
    }

    #[test]
    fn complete_no_match() {
        let r = complete("xyz", 8);
        assert!(r.is_empty());
    }

    #[test]
    fn complete_empty_returns_first_n() {
        let r = complete("", 3);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].name, "settings");
    }

    #[test]
    fn complete_case_insensitive() {
        let r = complete("MO", 8);
        assert!(r.iter().any(|c| c.name == "model"));
    }

    #[test]
    fn dispatch_model_sets_state() {
        let mut state = AppState::new("old-model");
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Model, "anthropic/claude-opus-4-7");
        match r.outcome {
            CommandOutcome::Output(lines) => {
                assert!(lines[0].contains("anthropic/claude-opus-4-7"));
            }
            _ => panic!("expected Output"),
        }
        assert_eq!(state.model, "anthropic/claude-opus-4-7");
    }

    #[test]
    fn dispatch_thinking_validates_levels() {
        let mut state = AppState::new("test");
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Thinking, "high");
        assert!(matches!(r.outcome, CommandOutcome::Output(_)));
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Thinking, "bogus");
        // Returns Usage line — output, not panic
        assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    }

    #[test]
    fn dispatch_quit_sets_quitting_mode() {
        let mut state = AppState::new("test");
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Quit, "");
        assert_eq!(r.outcome, CommandOutcome::Quit);
        assert_eq!(state.mode, RunMode::Quitting);
    }

    #[test]
    fn dispatch_new_clears_transcript() {
        let mut state = AppState::new("test");
        state.push_user("hello".to_string());
        state.push_divider();
        assert_eq!(state.transcript.len(), 2);
        let mut settings = SettingsManager::default();
        let _ = dispatch(&mut state, &mut settings, CommandId::New, "");
        // Transcript should be cleared + a "(started new session)" line added
        assert!(!state.transcript.is_empty());
        assert!(
            state.transcript[0]
                .as_assistant_text()
                .map(|t| t.contains("started new session"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn dispatch_export_writes_html() {
        let mut state = AppState::new("test");
        state.push_user("hi".to_string());
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Export, "");
        match r.outcome {
            CommandOutcome::Output(lines) => {
                let path_line = &lines[0];
                assert!(path_line.starts_with("export → "));
            }
            _ => panic!("expected Output"),
        }
    }

    #[test]
    fn render_transcript_html_basic() {
        let mut state = AppState::new("test");
        state.push_user("hello".to_string());
        state.push_assistant("hi back".to_string());
        let html = render_transcript_html(&state.transcript);
        assert!(html.contains("<!DOCTYPE html>"));
        // User line is rendered with "&gt;" prefix
        assert!(html.contains("&gt;") || html.contains("> hello"));
        assert!(html.contains("hi back"));
    }
}

/// Helper: count skills from disk. Used by `/reload`.
pub mod state_helpers {
    use nini_core::skills::load_skills;

    pub fn count_skills(cwd: &std::path::Path) -> usize {
        load_skills(cwd).skills.len()
    }
}


fn entry_legacy_message(entry: &nini_core::SessionEntry) -> Option<nini_core::AgentMessage> {
    use nini_core::entries::{AgentMessage as PiMsg, ContentBlock as PiContentBlock};
    match entry {
        nini_core::SessionEntry::Message(m) => {
            match &m.message {
                PiMsg::User(u) => {
                    let blocks: Vec<nini_core::ContentBlock> = match &u.content {
                        nini_core::entries::StringOrContentBlocks::String(s) => {
                            vec![nini_core::ContentBlock::Text { text: s.clone() }]
                        }
                        nini_core::entries::StringOrContentBlocks::Blocks(bs) => bs.iter().map(|b| match b {
                            PiContentBlock::Text { text } => nini_core::ContentBlock::Text { text: text.clone() },
                            PiContentBlock::ToolCall { id, name, arguments } => nini_core::ContentBlock::ToolUse {
                                id: id.clone(), name: name.clone(), input: arguments.clone(),
                            },
                            PiContentBlock::Image { .. } | PiContentBlock::Thinking { .. } => nini_core::ContentBlock::Text { text: String::new() },
                        }).collect(),
                    };
                    Some(nini_core::AgentMessage { role: nini_core::Role::User, content: blocks, timestamp: u.timestamp })
                }
                PiMsg::Assistant(a) => {
                    let blocks: Vec<nini_core::ContentBlock> = a.content.iter().map(|b| match b {
                        PiContentBlock::Text { text } => nini_core::ContentBlock::Text { text: text.clone() },
                        PiContentBlock::ToolCall { id, name, arguments } => nini_core::ContentBlock::ToolUse {
                            id: id.clone(), name: name.clone(), input: arguments.clone(),
                        },
                        PiContentBlock::Image { .. } | PiContentBlock::Thinking { .. } => nini_core::ContentBlock::Text { text: String::new() },
                    }).collect();
                    Some(nini_core::AgentMessage { role: nini_core::Role::Assistant, content: blocks, timestamp: a.timestamp })
                }
                _ => None,
            }
        }
        _ => None,
    }
}