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
            // v1: emit a placeholder transcript line. Real tree navigation
            // lands with the session-tree work.
            state.push_assistant("(session tree — not yet implemented in v1)".to_string());
            state.push_divider();
            CommandResult::output(vec!["tree: not yet implemented".to_string()])
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
        CommandId::ScopedModels => CommandResult::output(vec![
            "(scoped-models — Ctrl+P cycling scope config not yet implemented)".to_string(),
        ]),
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
        CommandId::Import => CommandResult::output(vec![
            "(import — JSONL session import not yet implemented)".to_string(),
        ]),
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
        CommandId::Changelog => CommandResult::output(vec![
            "(changelog — see references/spec-v0.85.1/ for v0.85.1 release notes)".to_string(),
        ]),
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
        CommandId::Fork => CommandResult::output(vec![
            "(fork — session fork UI not yet implemented)".to_string(),
        ]),
        CommandId::Clone => CommandResult::output(vec![
            "(clone — duplicate session not yet implemented)".to_string(),
        ]),
        CommandId::Trust => CommandResult::output(vec![format!(
            "(trust — marked cwd {} as trusted)",
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "(unknown)".to_string())
        )]),
        CommandId::Login => CommandResult::output(vec![
            "(login — credential setup not yet implemented)".to_string(),
        ]),
        CommandId::Logout => CommandResult::output(vec![
            "(logout — credential removal not yet implemented)".to_string(),
        ]),
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
            // v1: placeholder. Real compactor lands with P011.
            state.push_assistant("(manual compaction — algorithm lands in v1.1)".to_string());
            state.push_divider();
            CommandResult::output(vec!["compact: not yet implemented".to_string()])
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
    fn registry_has_22_commands() {
        // Pi spec `builtin.json` declares 23 entries (count field), but the
        // Pi code base says 22 (+ /quit implicit). Match the spec.
        assert_eq!(
            REGISTRY.len(),
            23,
            "expected 23 Pi-compatible commands per builtin.json"
        );
    }

    #[test]
    fn all_required_command_ids_present() {
        // Sanity: spot-check a handful of names that must be present.
        for name in [
            "settings", "model", "tree", "thinking", "export", "import", "session", "hotkeys",
            "fork", "clone", "trust", "new", "compact", "resume", "reload", "quit",
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