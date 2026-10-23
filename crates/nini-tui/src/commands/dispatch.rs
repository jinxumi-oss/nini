//! v0.8.3: Dispatch + per-command implementation helpers.
//!
//! invariant: fe56543d —— 声明本文件守住了哪几条历史不变量。
//! invariant: cc2e35ff —— 同一批修复里的「不许谎报字节数」。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，
//! 真正的证据是 docs/invariant-verification.md 里的注入-观察记录。
//!
//! `dispatch()` is the public entry — it routes `CommandId` to the
//! matching `cmd_xxx()` private helper below. Each helper is a pure
//! function over `&mut AppState` (and optionally `&mut SettingsManager`
//! for commands that persist). Each helper has unit tests in this
//! module.

use crate::commands::{build_status_lines, render_transcript_html};
use crate::settings::SettingsManager;
use crate::state::{AppState, RunMode, TranscriptLine};
use std::sync::Arc;

use super::registry::CommandId;
use super::result::CommandResult;

/// Dispatch a slash command. Pure function — mutates `state` in place.
///
/// `args` is the raw input after the command name (whitespace-trimmed).
/// For commands without an `argument_hint`, `args` should be empty.
/// `settings` is used for commands that persist to ~/.nini/agent/settings.json
/// (e.g., /model, /thinking). Pass `&mut SettingsManager::default()` if persistence
/// is not needed.
pub fn dispatch(
    state: &mut AppState,
    settings: &mut SettingsManager,
    id: CommandId,
    args: &str,
) -> CommandResult {
    let args = args.trim();
    match id {
        CommandId::Settings => cmd_settings(state, settings, args),
        CommandId::Model => cmd_model(state, settings, args),
        CommandId::Tree => cmd_tree(state, settings, args),
        CommandId::Thinking => cmd_thinking(state, settings, args),
        CommandId::ScopedModels => cmd_scoped_models(state, settings, args),
        CommandId::Export => cmd_export(state, settings, args),
        CommandId::Import => cmd_import(state, settings, args),
        CommandId::Copy => cmd_copy(state, settings, args),
        CommandId::Name => cmd_name(state, settings, args),
        CommandId::Session => cmd_session(state, settings, args),
        CommandId::Changelog => cmd_changelog(state, settings, args),
        CommandId::Fork => cmd_fork(state, settings, args),
        CommandId::Clone => cmd_clone(state, settings, args),
        CommandId::Trust => cmd_trust(state, settings, args),
        CommandId::Login => cmd_login(state, settings, args),
        CommandId::Logout => cmd_logout(state, settings, args),
        CommandId::New => cmd_new(state, settings, args),
        CommandId::Compact => cmd_compact(state, settings, args),
        CommandId::Resume => cmd_resume(state, settings, args),
        CommandId::Prompt => cmd_prompt(state, settings, args),
        CommandId::Reload => cmd_reload(state, settings, args),
        CommandId::Quit => cmd_quit(state, settings, args),
        CommandId::Help => cmd_help(state, settings, args),
        CommandId::Debug => cmd_debug(state, settings, args),
        CommandId::Status => cmd_status(state, settings, args),
        CommandId::Editor => cmd_editor(state, settings, args),
        CommandId::Hotkeys => cmd_hotkeys(state, settings, args),
        CommandId::Share => cmd_share(state, settings, args),
    }
}

// =================================================================
// Per-command implementations (one fn per CommandId arm)
// =================================================================

fn cmd_settings(
    state: &mut AppState,
    _settings: &mut SettingsManager,
    _args: &str,
) -> CommandResult {
    // Display current settings (mock until real settings UI is wired).
    let lines = vec![
        "Settings (read-only)".to_string(),
        format!("  model:     {}", state.model_state.model),
        format!(
            "  session:   {}",
            state
                .session_state
                .session_id
                .as_deref()
                .unwrap_or("(none)")
        ),
        format!("  status:    {:?}", state.run_state.mode),
    ];
    CommandResult::output(lines)
}

fn cmd_model(state: &mut AppState, settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /model <provider/model> — set the model for the next turn.
    if args.is_empty() {
        return CommandResult::output(vec![
            "Usage: /model <provider/model>".to_string(),
            format!("Current model: {}", state.model_state.model),
        ]);
    }
    // `set_model` (not a direct assignment) so that the provider
    // fallback stays visible: with no provider, the model name is a
    // lie, and `/model` must not make the status bar look normal
    // again.
    state.set_model(args);
    settings.set_default_model(args);
    state.push_assistant(format!("(model set to {})", args));
    state.push_divider();
    CommandResult::output(vec![format!("model → {args}")])
}

fn cmd_tree(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
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
            // This asked the tree two questions - the node's path to its
            // root, and whether it has descendants - and then answered
            // both with `map(|_| String::new())`. The label came out empty
            // every time and was discarded on the next line regardless.
            // What is worth showing beside a spine node is whether the
            // conversation forked there.
            let branches = tree.descendants(id).len();
            let label = match branches {
                0 => String::new(),
                1 => "  (1 branch)".to_string(),
                n => format!("  ({n} branches)"),
            };
            out.push(format!("  {:>3}. {id}{label}", i + 1));
        }
        // Show non-main branches (descendants of nodes not in the main path).
        let in_main: std::collections::HashSet<&String> = main_path.iter().collect();
        let mut branch_count = 0;
        // Walk the whole tree once from the root of the spine and list only
        // the nodes that leave it. Iterating `descendants` of every spine
        // node separately re-visits each node once per ancestor, so the
        // same branch was counted as many times as it was deep.
        let mut printed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let spine_roots: Vec<String> = tree
            .roots()
            .iter()
            .filter(|r| in_main.contains(*r))
            .cloned()
            .collect();
        for root in &spine_roots {
            for node in tree.descendants(root) {
                if in_main.contains(&node) {
                    // On the spine: already listed above, with its branch
                    // count.
                    continue;
                }
                if printed.insert(node.clone()) {
                    branch_count += 1;
                    out.push(format!("       └─ {node} (branch {branch_count})"));
                }
            }
        }
        if branch_count == 0 {
            out.push("(no branches — current session is linear)".to_string());
        }
    }
    state.push_assistant(format!("[tree] {} entries, {} branches", tree.len(), {
        let mc = tree.main_path().len();
        if mc > 0 {
            tree.len().saturating_sub(mc).to_string()
        } else {
            "0".to_string()
        }
    }));
    state.push_divider();
    CommandResult::output(out)
}

fn cmd_thinking(state: &mut AppState, settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /thinking <off|minimal|low|medium|high|xhigh|max>
    let valid = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if args.is_empty() || !valid.contains(&args) {
        return CommandResult::output(vec![format!("Usage: /thinking <{}>", valid.join("|"))]);
    }
    settings.set_default_thinking_level(args);
    // Written but never flushed, so `/thinking high` was gone the moment
    // nini exited. `settings` here is the manager the runtime loaded from
    // the real file, so flushing keeps the user's other settings.
    if let Err(e) = settings.flush() {
        state.push_assistant(format!("(thinking level set for this session only: {e})"));
        state.push_divider();
        return CommandResult::output(vec![format!("thinking → {args} (not saved)")]);
    }
    state.push_assistant(format!("(thinking level: {args})"));
    state.push_divider();
    CommandResult::output(vec![format!("thinking → {args}")])
}

fn cmd_scoped_models(
    state: &mut AppState,
    settings: &mut SettingsManager,
    args: &str,
) -> CommandResult {
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
            state.push_assistant(format!("[scoped-models] removed {} model(s)", removed));
            state.push_divider();
            CommandResult::output(vec![format!("removed {removed} matching {m}")])
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

fn cmd_export(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // A minimal HTML snapshot of the transcript, into the agent directory.
    let html = render_transcript_html(&state.transcript_state.lines);
    let dir = nini_core::paths::exports_dir();
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
            let p = d.join(format!("session-{ts}-{pid}-t{tid}.html"));
            if std::fs::write(&p, &html).is_ok() {
                p.display().to_string()
            } else {
                format!("(failed to write {})", p.display())
            }
        }
        None => format!(
            "(no session directory — set {} or HOME — skipped)",
            nini_core::paths::ENV_AGENT_DIR
        ),
    };
    state.push_assistant(format!("(exported session to {path})"));
    state.push_divider();
    let mut out = vec![format!("export → {path}")];
    // `/export <path>` silently dropped its path argument. That is the same
    // defect as `--export <path>` in the CLI, and the fix is the same: say so.
    // A user who typed a path and got no file will look in the wrong place,
    // then in their shell history, then conclude nini lost the export.
    if !args.trim().is_empty() {
        let dest = nini_core::paths::exports_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "(no session directory)".to_string());
        out.push(format!(
            "note: the path argument `{}` is not honoured — /export renders the \
             transcript and always writes to {dest} (the CLI's --export is refused \
             for the same reason)",
            args.trim()
        ));
    }
    CommandResult::output(out)
}

fn cmd_import(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
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
            // The second example is the one that matters. The first was
            // changed from `~/.pi/...` to `~/.nini/...` by a blanket
            // string replace in R3, which made it self-referential:
            // importing a session from the directory it is already in.
            // Reading a Pi-written session is exactly what the unchanged
            // JSONL schema is for, and this line was its only in-product
            // documentation.
            "  /import ~/.pi/agent/sessions/<project>/20260101-120000-abcd.jsonl".to_string(),
            "  /import ./my-session.jsonl".to_string(),
        ]);
    }
    let path = std::path::PathBuf::from(path_str);
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            return CommandResult::output(vec![format!("import: read failed: {e}")]);
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
                            nini_core::entries::ContentBlock::Text { text } => Some(text.clone()),
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
                        nini_core::entries::ContentBlock::Text { text } => Some(text.clone()),
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
    Arc::make_mut(&mut state.transcript_state).lines = new_transcript;
    state.run_state.tokens = Default::default();
    state.push_assistant(format!(
        "[import] {} transcript entries ({} skipped)",
        imported_count, skipped
    ));
    state.push_divider();
    out.push(format!("transcript: {imported_count} entries"));
    CommandResult::output(out)
}

/// What a `/copy` attempt actually achieved.
///
/// v0.9.2 (P0-4): extracted so both branches are unit-testable without a
/// real clipboard. CI has none, which is exactly why the old unconditional
/// `Ok(())` stub survived 937 green tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The text really is on the system clipboard.
    Clipboard { bytes: usize },
    /// No clipboard was reachable, so the text went into the transcript
    /// instead — the only place a reader can see it that the TUI owns.
    Transcript { bytes: usize, reason: String },
}

/// Map a clipboard attempt onto what we can honestly claim.
fn classify_copy(msg: &str, res: Result<(), String>) -> CopyOutcome {
    match res {
        Ok(()) => CopyOutcome::Clipboard { bytes: msg.len() },
        Err(reason) => CopyOutcome::Transcript {
            bytes: msg.len(),
            reason,
        },
    }
}

/// How much of a fallback message the transcript gets.
///
/// A notice is a row, not a document: the reader can see the beginning
/// of the answer and is told where the rest is.
const NOTICE_MAX_CHARS: usize = 400;

/// The message as a notice may show it: the head, cut at a character
/// boundary, with a pointer to the rest.
fn truncate_for_notice(msg: &str) -> String {
    if msg.chars().count() <= NOTICE_MAX_CHARS {
        return msg.to_string();
    }
    let cut: String = msg.chars().take(NOTICE_MAX_CHARS).collect();
    format!(
        "{cut}\n… ({} more characters — the full text is in /export)",
        msg.chars().count() - NOTICE_MAX_CHARS
    )
}

/// What to say when the clipboard could not be reached.
///
/// Every word of it has to be true. It used to say "copied 3482 bytes to
/// the transcript" — 3482 being the length of the *source* message,
/// while the transcript received a truncated copy of it — and to point
/// at `/export` for the full text, which is only true because the
/// assistant's own turn is in the transcript anyway.
fn copy_fallback_message(reason: &str) -> String {
    format!(
        "(clipboard unavailable: {reason}) — the message is in the transcript above; \
         the full text is in /export"
    )
}

fn cmd_copy(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Copy the last assistant message to the system clipboard.
    // If no clipboard is reachable (headless, no $DISPLAY), print to
    // stdout instead and SAY SO — v0.9.2 (P0-4) fixed this reporting a
    // copy that never happened.
    let last_assistant = state
        .transcript_state
        .lines
        .iter()
        .rev()
        .find_map(|l| match l {
            TranscriptLine::AssistantText(s) => Some(s.clone()),
            _ => None,
        });
    match last_assistant {
        Some(msg) => match classify_copy(&msg, crate::clipboard::copy(&msg)) {
            CopyOutcome::Clipboard { bytes } => {
                CommandResult::output(vec![format!("(copied {bytes} bytes to clipboard)")])
            }
            CopyOutcome::Transcript { bytes, reason } => {
                // Fallback: put the text where the reader can see it.
                // It used to be `println!`, which writes to stdout —
                // the alternate screen the TUI is painting — so the
                // message landed on top of the frame and read as
                // garbage. A transcript notice says what it is.
                //
                // The reason is surfaced verbatim so a Wayland-permissions
                // failure is distinguishable from a headless run.
                //
                // The text is **truncated**: a 200 KB answer would be
                // 200 KB of transcript — every line rendered, indexed
                // for search, and written into the HTML export, for a
                // fallback whose whole purpose is to show the user
                // something they can copy from somewhere better.
                state.push_notice(truncate_for_notice(&msg));
                // What is true, and nothing more: the clipboard was not
                // reachable, the head of the message is in the notice
                // above, and the whole message is in the transcript (the
                // assistant's own turn) and in `/export`.
                //
                // It used to say "copied N bytes to the transcript" — N
                // being the length of the *source* message, while the
                // transcript got a truncated copy of it. A byte count
                // that describes something else is a lie in a
                // diagnostic.
                let _ = bytes;
                CommandResult::output(vec![copy_fallback_message(&reason)])
            }
        },
        None => CommandResult::output(vec!["(no assistant message to copy)".to_string()]),
    }
}

fn cmd_name(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    if args.is_empty() {
        return CommandResult::output(vec!["Usage: /name <session name>".to_string()]);
    }
    // v1: store in status. Real impl persists via session metadata.
    state.run_state.status = format!("name: {args}");
    state.push_assistant(format!("(session name: {args})"));
    state.push_divider();
    CommandResult::output(vec![format!("name → {args}")])
}

fn cmd_session(
    state: &mut AppState,
    _settings: &mut SettingsManager,
    _args: &str,
) -> CommandResult {
    let lines = vec![
        format!(
            "session_id: {}",
            state
                .session_state
                .session_id
                .as_deref()
                .unwrap_or("(none)")
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

fn cmd_changelog(
    state: &mut AppState,
    _settings: &mut SettingsManager,
    _args: &str,
) -> CommandResult {
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

fn cmd_fork(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /fork [n]
    //
    // v0.10 (fix): this used to copy `transcript_state.lines[..cut_at]`
    // into a local variable and print how many entries it kept. No session
    // was created, no entry tree was touched, and the "branch" existed only
    // as the text of one line in the current transcript. Consequences:
    //
    //   * the branch could not be resumed — there was no file;
    //   * `/tree` had nothing to draw, because nothing branched;
    //   * the 585 lines in `branch_summary.rs` had no caller.
    //
    // Pi: `agent-session-runtime.ts:262` → `createBranchedSession(leafId)`
    // (`session-manager.ts:1412`), which writes a *new session file* whose
    // entries are the retained path, re-chained, under a header naming its
    // parent — and then replaces the live runtime with it.
    //
    // The fork points are read from the **session**, not the transcript:
    // the transcript is a rendering and can hold lines the session does not
    // (notices, dividers) or miss ones it does.
    let Some(session) = state.session_state.session.clone() else {
        return CommandResult::output(vec![
            "(fork: no active session — start one with /new first)".to_string(),
        ]);
    };

    let points = {
        // v0.10 (fix): was `try_lock` with a "(fork: the session is busy)"
        // message. The lock is never busy for longer than one append now
        // that the flush thread does its IO outside the critical section,
        // so the message could only ever have been a lie told to a user
        // mid-command.
        crate::state::lock_session(&session).fork_points()
    };

    // `/fork at N` keeps the N-th message (Pi's `position: "at"`);
    // plain `/fork N` branches before it and hands the text back, which is
    // Pi's default (`agent-session-runtime.ts:277-287`).
    // `at 2` is the inclusive form; the exact string `at` is the prefix of
    // a malformed invocation (`/fork at` with no index) and must fall
    // through to the listing rather than silently branching at nothing.
    let inclusive = args.trim() == "at 0" || args.trim().starts_with("at ");
    let selector = args
        .trim()
        .strip_prefix("at")
        .map(str::trim)
        .unwrap_or_else(|| args.trim());

    if selector.is_empty() {
        let mut out = vec![format!(
            "Fork points ({} user messages{}):",
            points.len(),
            if inclusive {
                ", keeping the chosen one"
            } else {
                ""
            }
        )];
        for p in &points {
            out.push(format!("  {}. \"{}\"", p.ordinal, p.preview));
        }
        out.push("(pass an index like /fork 2, or /fork at 2 to keep it)".to_string());
        return CommandResult::output(out);
    }

    let Ok(n) = selector.parse::<usize>() else {
        return CommandResult::output(vec![format!(
            "fork: '{}' is not a number — pass an index like /fork 2",
            selector
        )]);
    };
    // `n` is the number in the list the user just read, so it selects by
    // ordinal; the path position it maps to is what actually gets forked.
    let Some(point) = points.iter().find(|p| p.ordinal == n) else {
        return CommandResult::output(vec![format!(
            "fork: invalid index {n} (expected 1..={})",
            points.len()
        )]);
    };

    let forked = {
        let guard = crate::state::lock_session(&session);
        if inclusive {
            guard.fork_at_inclusive(point.position)
        } else {
            guard.fork_at(point.position)
        }
    };
    let forked = match forked {
        Ok(f) => f,
        Err(e) => {
            return CommandResult::output(vec![format!("(fork failed: {e})")]);
        }
    };

    // Pi writes the fork immediately when the retained path already contains
    // an assistant message, and defers otherwise (`session-manager.ts:1482`)
    // so a branch with no model output yet does not leave a header-only
    // file behind.
    let has_assistant = forked.entries.iter().any(|e| {
        matches!(
            e,
            nini_core::SessionEntry::Message(m)
                if matches!(m.message, nini_core::entries::AgentMessage::Assistant(_))
        )
    });
    let parent_path = state.session_state.session_path.clone();
    let dir = nini_core::paths::sessions_dir().or_else(|| {
        parent_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    });
    let Some(dir) = dir else {
        return CommandResult::output(vec![
            "(fork: no session directory — this session was never persisted)".to_string(),
        ]);
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return CommandResult::output(vec![format!("(fork failed: {e})")]);
    }
    let path = nini_session::fork_path_for(&dir, &forked.header);
    if let Err(e) = forked.write_to_file(&path) {
        return CommandResult::output(vec![format!("(fork failed: {e})")]);
    }
    // Without an assistant turn there is nothing yet worth listing, and
    // `session_load` on an empty branch would show an empty transcript.
    let _ = has_assistant;

    match state.session_load(path.clone()) {
        Ok(()) => {
            let mut out = vec![format!(
                "forked at user msg #{n} (\"{}\") — {} entries kept",
                point.preview,
                forked.entries.len()
            )];
            out.push(format!("new session: {}", path.display()));
            if !inclusive {
                out.push(format!(
                    "the message is back in the transcript history of the new \
                     branch; /resume lists both sessions"
                ));
            }
            CommandResult::output(out)
        }
        Err(e) => CommandResult::output(vec![format!("(fork failed to load: {e})")]),
    }
}

fn cmd_clone(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // /clone — duplicate the current session's JSONL to a new file
    // with a fresh timestamp. The current in-memory transcript
    // keeps editing the original; the clone is a side artifact.
    let Some(path) = &state.session_state.session_path else {
        return CommandResult::output(vec![
            "(clone: no active session — start one with /new first)".to_string(),
        ]);
    };
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%6f");
    let dest = parent.join(format!("{stem}-clone-{ts}.jsonl"));
    match std::fs::copy(path, &dest) {
        Ok(_) => {
            state.push_assistant(format!("[clone] {} → {}", path.display(), dest.display()));
            state.push_divider();
            CommandResult::output(vec![
                format!("cloned to {}", dest.display()),
                "use /resume to switch".to_string(),
            ])
        }
        Err(e) => CommandResult::output(vec![format!("(clone failed: {e})")]),
    }
}

fn cmd_trust(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /trust [trusted|distrust|ask|list|clear]
    // Persists a TrustDecision for the current working directory
    // in `~/.nini/agent/trust.json` (Pi-compatible).
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

fn cmd_login(_state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
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
        out.push(format!("login: unknown provider {provider_arg:?}"));
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
            let status = if present { "✓ set" } else { "✗ not set" };
            out.push(format!("{name}: {env}  {status}"));
        }
        out.push(String::new());
        out.push("To set a credential, exit nini and run:".to_string());
        out.push("  export ANTHROPIC_API_KEY=sk-ant-...".to_string());
        out.push("then re-launch nini.".to_string());
    }
    CommandResult::output(out)
}

fn cmd_logout(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /logout [provider]
    //
    // v0.9.2 (P0-5): this USED TO claim to remove credentials. It only
    // ever called `std::env::var_os(env).is_some()` and then reported
    // "credentials unset in this session" — nothing was ever unset.
    //
    // The tempting one-liner here is `std::env::remove_var`. Do not do
    // it: the TUI is a multi-threaded tokio runtime, and `remove_var`
    // racing another thread's `var_os` is undefined behaviour, not just
    // a style problem. (On edition 2024 the compiler rejects it outright,
    // which is why the temptation is easy to fall for when the code is
    // moved to a newer edition.)
    //
    // What nini CAN do honestly: detect which keys are set and hand the
    // user the exact shell command to unset them. That is what this
    // does now.
    let provider_arg = args.trim();
    // Map provider name → env var (Pi's mapping is in
    // `~/.nini/agent/auth.json`; we keep an explicit table).
    const KNOWN: &[(&str, &str)] = &[
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
    // `openai` / `openai-responses` / `openai-compat` all map to the same
    // variable. v0.9.2 (P0-5): dedupe, or a bare `/logout` printed
    // "OPENAI_API_KEY" three times.
    let to_check: Vec<&str> = if provider_arg.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        KNOWN
            .iter()
            .map(|(_, env)| *env)
            .filter(|env| seen.insert(*env))
            .collect()
    } else {
        let p = provider_arg.to_lowercase();
        let mut seen = std::collections::BTreeSet::new();
        KNOWN
            .iter()
            .filter(|(name, _)| *name == p.as_str())
            .map(|(_, env)| *env)
            .filter(|env| seen.insert(*env))
            .collect()
    };
    if to_check.is_empty() {
        return CommandResult::output(vec![
            format!("logout: unknown provider {provider_arg:?}"),
            "known: anthropic, openai, openai-responses, openai-compat, google, mistral, cohere, deepseek, groq, together"
                .to_string(),
        ]);
    }
    let mut present: Vec<&str> = Vec::new();
    let mut already: Vec<&str> = Vec::new();
    for env in &to_check {
        // Presence check only. Note this reads nini's OWN environment,
        // which is the environment it was launched with.
        if std::env::var_os(env).is_some() {
            present.push(env);
        } else {
            already.push(env);
        }
    }

    let mut out = Vec::new();
    if present.is_empty() {
        out.push("no credentials set in nini's environment".to_string());
        out.push(String::new());
        out.push("nini reads API keys from environment variables and cannot".to_string());
        out.push("modify the environment of the shell that started it.".to_string());
    } else {
        out.push(format!(
            "nini cannot unset credentials in your shell ({} found).",
            present.len()
        ));
        out.push("Run this in the shell that launched nini, then restart nini:".to_string());
        out.push(String::new());
        for env in &present {
            out.push(format!("  unset {env}"));
        }
        out.push(String::new());
        out.push("(if you exported it in a profile, remove it there too)".to_string());
    }
    if !already.is_empty() {
        out.push(String::new());
        out.push(format!("already unset: {}", already.join(", ")));
    }
    state.push_assistant(format!(
        "[logout] present={}, already-unset={}",
        present.len(),
        already.len()
    ));
    state.push_divider();
    CommandResult::output(out)
}

fn cmd_new(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Clear the transcript and create a fresh session.
    let prev_len = state.transcript_state.lines.len();
    // v0.8.9 (P1-5): reset the viewport along with the content — a
    // stale `scroll_offset` renders the fresh transcript as an empty
    // box until the user scrolls all the way back down.
    let ts = Arc::make_mut(&mut state.transcript_state);
    ts.lines.clear();
    ts.reset_view();
    // v0.8.9 (perf): the new session reuses the same line indices.
    state.invalidate_item_counts();
    state.run_state.tokens = Default::default();

    // Session file path: <agent dir>/sessions/<project>/<timestamp>_<suffix>.jsonl
    //
    // v0.10.8 (A5)：与**启动时**用的是同一个 helper（`paths::new_session_path`），
    // 否则 `/new` 与启动会造出两种命名的文件，`/resume` 的排序就乱了。
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match nini_core::paths::new_session_path(&cwd) {
        Some(path) => state.session_init(path),
        // No `HOME` and no `NINI_AGENT_DIR`: run the conversation, record
        // nothing, and say so. The previous `unwrap_or_else(|| ".")` wrote
        // `./.nini/agent/sessions/...` into the user's project.
        None => {
            state.session_init_ephemeral();
            state.push_assistant(format!(
                "(not recorded — set {} to give nini a place to keep sessions)",
                nini_core::paths::ENV_AGENT_DIR
            ));
        }
    }

    state.push_assistant("(started new session)".to_string());
    state.push_divider();
    CommandResult::output(vec![format!(
        "new: cleared {} lines, session {}",
        prev_len,
        state.session_state.session_id.as_deref().unwrap_or("?")
    )])
}

fn cmd_compact(state: &mut AppState, settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // A2: this used to drain the first half of the *transcript lines* at
    // `total / 2`, synthesise a numbered list of line previews, and put
    // that back into the transcript. No session write, no token budget, and
    // nothing the model would ever see - and its comment claimed it
    // "matches the algorithm used by the auto-compaction path", which it
    // did not. The same comment described the cut as `find_cut_point`,
    // which was never called from here.
    let keep = settings.get().keep_recent_tokens;
    match state.session_compact(keep) {
        Some(r) => {
            state.session_flush();
            CommandResult::output(vec![format!(
                "compacted: folded {} entries into a summary \
                 ({} tokens before); context now starts at {}",
                r.folded, r.tokens_before, r.first_kept_entry_id
            )])
        }
        None => CommandResult::output(vec![
            "compact: nothing to compact (session too small)".to_string()
        ]),
    }
}

fn cmd_resume(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /resume [n] — list available sessions or load by index.
    // Scans ~/.nini/agent/sessions/ recursively for .jsonl files.
    // v0.5 only read the top-level directory and missed the
    // per-cwd subdirectories where nini actually writes its
    // sessions; v0.6 uses walkdir to find them all.
    fn session_entries() -> Vec<std::path::PathBuf> {
        // The previous version fell back to `$HOME/fallback/sessions` when
        // the agent directory could not be resolved. That path exists
        // nowhere and was never written to; it only ever made the listing
        // look like it was looking somewhere.
        let Some(base) = nini_core::paths::sessions_dir() else {
            return Vec::new();
        };
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
            let a_cwd = a.components().any(|c| c.as_os_str() == cwd_name.as_str());
            let b_cwd = b.components().any(|c| c.as_os_str() == cwd_name.as_str());
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
            "Sessions are stored in ~/.nini/agent/sessions/".to_string(),
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
        // v0.8.9 (P1-5): same as `/new` — the new session can have far
        // fewer lines than the one we were scrolled into.
        let ts = Arc::make_mut(&mut state.transcript_state);
        ts.lines.clear();
        ts.reset_view();
        // v0.8.9 (perf): same — the rebuilt transcript reuses indices.
        state.invalidate_item_counts();
        let session_id = state.session_state.session_id.clone().unwrap_or_default();
        // Take ownership of the session Arc so we can lock it without
        // keeping a borrow of state.
        let session_arc = state.session_state.session.take();
        if let Some(arc) = session_arc {
            let guard = crate::state::lock_session(&arc);
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
            drop(guard);
            // Restore the Arc.
            state.session_state.session = Some(arc);
        }
        // v0.8.9 (P2-9): say so when the history has a hole in it. The
        // reader is deliberately tolerant of malformed lines, but
        // tolerance the user cannot see is its own kind of lie — a
        // silently shorter history reads as "that is all there was".
        let skipped = state.session_state.skipped_lines_on_load;
        if skipped > 0 {
            state.push_stop_notice(format!(
                "skipped {skipped} unreadable line(s) in this session file — \
                 the history above has gaps"
            ));
        }
        state.push_assistant(format!("(loaded session {session_id})"));
        state.push_divider();
        return CommandResult::output(vec![format!(
            "Resumed: {}{}",
            path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            if skipped > 0 {
                format!(" (skipped {skipped} damaged line(s))")
            } else {
                String::new()
            }
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
            lines.push(format!("(refine with /resume <query> to see the rest)"));
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
                format!(
                    "  [{} bytes{}]",
                    size,
                    if modified.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", modified)
                    }
                )
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

fn cmd_prompt(state: &mut AppState, _settings: &mut SettingsManager, args: &str) -> CommandResult {
    // /prompt <name> [args...]
    // Loads user-defined prompt templates from `.pi/prompts/` and
    // `~/.nini/agent/prompts/`, then substitutes $1, $@, $ARGUMENTS
    // placeholders with positional / quoted args.
    let cwd = std::env::current_dir().ok();
    let cwd = match cwd.as_ref() {
        Some(c) => c.clone(),
        None => {
            return CommandResult::output(vec!["(prompt: cannot determine cwd)".to_string()]);
        }
    };
    let args_trimmed = args.trim();
    if args_trimmed.is_empty() {
        return CommandResult::output(vec![
            "Usage: /prompt <name> [args...]".to_string(),
            "(prompt templates load from .nini/prompts/ and ~/.nini/agent/prompts/)".to_string(),
        ]);
    }
    // Split into template-name + remaining args via shell-style
    // quoting (matches Pi's parseCommandArgs).
    let name_tpl_args = nini_core::prompt_template::parse_command_args(args_trimmed);
    let (name, tpl_args) = match name_tpl_args.split_first() {
        Some((n, rest)) => (n.clone(), rest.to_vec()),
        None => {
            return CommandResult::output(vec!["(prompt: empty arguments)".to_string()]);
        }
    };

    // Load templates from user + project dirs.
    let paths: Vec<std::path::PathBuf> = vec![
        nini_core::prompt_template::user_prompts_dir().unwrap_or_default(),
        nini_core::prompt_template::project_prompts_dir(&cwd).unwrap_or_default(),
    ];
    let load_result = nini_core::prompt_template::load_prompt_templates(&paths);
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
                out.push("(no templates in .nini/prompts/ or ~/.nini/agent/prompts/)".to_string());
            }
            return CommandResult::output(out);
        }
    };

    // Apply args. Pi uses parseCommandArgs to handle quoting;
    // we delegate to the same parser for consistency.
    let final_prompt = nini_core::prompt_template::format_invocation(&template, &tpl_args);
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

fn cmd_reload(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Reload what can actually change mid-session.
    let cwd = std::env::current_dir().ok();
    let mut out: Vec<String> = Vec::new();
    // v0.10: this counted the skills on disk and threw the count away.
    // The skills a session carries live in the system prompt the CLI built
    // at startup, so counting them changes nothing the model can see — the
    // command reported a reload that did not happen.
    //
    // The honest reload is: re-read the settings from the same path they
    // were loaded from, resolve the theme again, re-derive the context
    // window (a model catalogue lookup, so a settings edit can change it),
    // and say what actually changed. Skills cannot be reloaded without a
    // new session — the prompt is fixed for the life of a run — so they are
    // reported as requiring a restart rather than claimed to be reloaded.
    let before_theme = state.model_state.settings_snapshot.theme.clone();
    let before_window = state.run_state.context_window;
    let before_model = state.model_state.model.clone();

    if let Some(path) = state.model_state.settings_path.clone() {
        if !path.exists() {
            out.push(format!("no settings file at {}", path.display()));
        } else {
            let reloaded = SettingsManager::load_from_disk(path.clone());
            state.model_state.settings_snapshot = reloaded.get();
            // The context window is derived from the catalogue for the
            // selected model, so a settings edit can move it even when the
            // model did not change.
            state.refresh_context_window();
            out.push(format!("reloaded settings from {}", path.display()));
        }
    } else {
        out.push("no settings file to reload".to_string());
    }

    let after = state.model_state.settings_snapshot.clone();
    if after.theme != before_theme {
        out.push(format!(
            "theme: {} → {} (applies on the next draw)",
            before_theme, after.theme
        ));
    }
    if state.run_state.context_window != before_window {
        out.push(format!(
            "context window: {before_window} → {}",
            state.run_state.context_window
        ));
    }
    // The model is deliberately *not* switched here. `RunConfig` is cloned
    // once per run, so changing it mid-session would leave the status bar
    // and the provider disagreeing until the next prompt — the same class
    // of bug R1e fixed for the TUI's own model selector.
    let _ = before_model;

    let on_disk = cwd
        .as_deref()
        .map(|c| nini_core::skills::load_skills(c).skills.len())
        .unwrap_or(0);
    out.push(format!(
        "skills: {on_disk} on disk (the system prompt is fixed for this          session — restart nini to pick them up)"
    ));

    if out.len() == 1 {
        out.push("nothing changed".to_string());
    }
    state.push_assistant(format!("(reload: {})", out.join("; ")));
    state.push_divider();
    CommandResult::output(out)
}

fn cmd_quit(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    state.run_state.mode = RunMode::Quitting;
    CommandResult::quit()
}

fn cmd_help(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Open the help overlay via the selector-open status flag.
    // The overlay itself lives in the TUI runtime / selector
    // infrastructure (F014 in the plan wires a polished version).
    state.run_state.status = "open_selector:help".to_string();
    CommandResult::output(vec![
        "Type /<tab> to see all commands; F1 toggles extended hints.".to_string(),
    ])
}

fn cmd_debug(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // v0.10: this used to flip `ui_state.debug_logging` and print a
    // confirmation. Nothing read the field, `tracing` was initialised
    // nowhere, and `--verbose` was dropped at the CLI boundary — so the one
    // diagnostic action the product documented produced no diagnostic
    // output.
    //
    // v0.10.2: the source of truth is the global switch, not the field.
    // It used to be `!state.ui_state.debug_logging`, which means the two
    // copies could disagree: the CLI sets the global via `--verbose`
    // (`nini-cli/src/app.rs`) while the field stays at its `AppState::new`
    // default, so the first `/debug` after a `--verbose` run would have
    // switched the flag *off*. Two sources of truth is the same defect as
    // two copies of a hint table — and here it also made a test flaky,
    // because a second `AppState` in the same process could write the
    // global out from under the first one's expectation.
    //
    // The field is still updated: the renderer and `/status` read it, and
    // keeping it in step with the switch it mirrors is the point.
    let new_state = !nini_core::logging::is_debug();
    state.ui_state.debug_logging = new_state;
    nini_core::logging::set_debug(new_state);

    let mut out = vec![format!(
        "debug logging: {}",
        if new_state { "on" } else { "off" }
    )];
    match nini_core::logging::log_path() {
        Some(p) => {
            out.push(format!("log: {}", p.display()));
            out.push("tail -f that file; it also takes effect immediately".to_string());
        }
        None => out.push(
            "no agent directory, so there is nowhere to log (set HOME or NINI_AGENT_DIR)"
                .to_string(),
        ),
    }
    if new_state {
        tracing::debug!("/debug: debug logging enabled");
    }
    CommandResult::output(out)
}

fn cmd_status(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Render a one-shot status block into the transcript. Matches
    // Pi's `/status` semantics: a snapshot, not a live view.
    let lines = build_status_lines(state);
    CommandResult::output(lines)
}

fn cmd_editor(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // Open the current input in $VISUAL / $EDITOR / nano.
    // Synchronous from the dispatcher's POV; the actual spawn is
    // handled by the runtime (which has access to the file
    // handles). We just signal it here via the dedicated flag
    // (not via state.run_state.status, which is a transient message
    // surface that gets cleared after each frame).
    state.run_state.pending_external_editor = true;
    state.run_state.status = "Opening editor…".to_string();
    CommandResult::output(vec!["Opening editor…".to_string()])
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

fn cmd_hotkeys(
    state: &mut AppState,
    _settings: &mut SettingsManager,
    _args: &str,
) -> CommandResult {
    // v0.8.4 restored this from a stub so it works as a textual fallback
    // for F1. It then shipped a hand-written table that had drifted from
    // `default_keymap()`: it advertised `Ctrl+Z` (never bound — undo is
    // `Ctrl+/`), `Alt+Backspace` (never bound), `Ctrl+E` (never bound),
    // and `Alt+D kill to end` when the binding is kill-word-forward.
    //
    // The same defect appeared in three independent copies — the four
    // footer branches, this table, and the README — and all three
    // disagreed with each other. Three copies of one fact is three
    // places to rot, so this one is now *derived*: it reads the keymap,
    // so it cannot claim a key the TUI does not have.
    //
    // v0.10.2. `tests/hotkeys_match_keymap.rs` pins the invariant.
    let _ = state;

    /// Render a normalized key the way a user would name it.
    ///
    /// Three details this got wrong in the first draft, all found by
    /// `slash_command_e2e` asserting on `Ctrl+C`:
    ///
    /// 1. Characters render **uppercase** (`Ctrl+c` is not how anyone
    ///    writes a key, and `tests/hotkeys_match_keymap.rs` compares
    ///    against the same convention).
    /// 2. Modifiers are tested with `contains`, not `==`. `Ctrl+Shift+P`
    ///    has `CTRL | SHIFT` as one value, so `== CTRL` is false and the
    ///    chord would have been printed as a bare `Shift+P`.
    /// 3. Order is Shift, Ctrl, Alt — the order people say them in.
    fn key_label(key: &crate::keys::Key) -> String {
        use crate::keys::{KeyCode, KeyModifiers};
        let mut s = String::new();
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            s.push_str("Shift+");
        }
        if key.modifiers.contains(KeyModifiers::CTRL) {
            s.push_str("Ctrl+");
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            s.push_str("Alt+");
        }
        let base = match key.code {
            KeyCode::Char(c) => c.to_uppercase().collect::<String>(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        s.push_str(&base);
        s
    }

    /// What the action does, in the user's vocabulary.
    fn action_label(a: crate::keys::KeyAction) -> &'static str {
        use crate::keys::KeyAction::*;
        match a {
            Submit => "send input",
            Newline => "insert newline",
            Abort => "abort the run / dismiss overlay",
            Quit => "quit (press twice)",
            MoveLeft => "cursor left",
            MoveRight => "cursor right",
            MoveUp => "move cursor up, else recall previous prompt",
            MoveDown => "move cursor down, else recall next prompt",
            MoveLineStart => "beginning of line",
            MoveLineEnd => "end of line",
            MoveWordLeft => "word left",
            MoveWordRight => "word right",
            Backspace => "delete character backward",
            Delete => "delete character forward",
            KillToLineStart => "delete to line start",
            KillToLineEnd => "delete to line end",
            KillWordBackward => "kill word backward",
            KillWordForward => "kill word forward",
            Yank => "yank",
            YankPop => "yank-pop",
            Undo => "undo",
            SwitchModel => "switch model",
            CycleModelNext => "cycle model +",
            CycleModelPrev => "cycle model -",
            CycleThinkingNext => "thinking level +",
            CycleThinkingPrev => "thinking level -",
            ShowHelp => "toggle extended keymap",
            ClearInput => "clear input",
            PasteImage => "paste image from clipboard",
            AcceptCompletionOrInsertTab => "accept completion / insert tab",
            ScrollUp => "scroll transcript up",
            ScrollDown => "scroll transcript down",
            ScrollToTop => "scroll transcript to the oldest message",
            ScrollToBottom => "scroll transcript to the latest message",
            ToggleCollapse => "toggle collapsed tool output",
            OpenSearch => "search the transcript",
            OpenCommandPalette => "open the command palette",
            OpenExternalEditor => "edit input in $EDITOR",
            Noop => "(no binding)",
            other => {
                // A new action must not silently vanish from /hotkeys.
                let _ = other;
                "?"
            }
        }
    }

    let keymap = crate::keys::default_keymap();
    let width = keymap
        .iter()
        .map(|b| key_label(&b.key).len())
        .max()
        .unwrap_or(0)
        .max(10);

    // No footer block: a first draft added "Esc dismiss overlay / abort",
    // "F1 toggle", "Enter / Tab accept" below the table. Every one of
    // those keys is already in the table, and duplicating a fact is how
    // the two copies drifted apart in the first place. It also pushed the
    // output to 45 lines, four more than a 50-row frame can show.
    let mut lines = vec![
        "Key bindings (nini TUI)".to_string(),
        format!("{} bindings — derived from the keymap", keymap.len()),
        String::new(),
    ];
    for b in &keymap {
        let label = key_label(&b.key);
        lines.push(format!("  {label:<width$}  {}", action_label(b.action)));
    }
    CommandResult::output(lines)
}

fn cmd_share(state: &mut AppState, _settings: &mut SettingsManager, _args: &str) -> CommandResult {
    // v0.6+: share = export to a secret GitHub gist. Not yet wired;
    // see Pi spec `references/spec-v0.85.1/slash-commands/share.md`.
    let _ = state;
    CommandResult::error("share: not yet implemented")
}

#[cfg(test)]
mod honesty_tests {
    //! v0.9.2 — regression tests for the two P0 "reported success but did
    //! nothing" commands. Both shipped through 937 green tests, because
    //! neither had a testable seam. These add one.

    use super::*;

    // ----------------------------------------------------------------
    // P0-4: /copy
    // ----------------------------------------------------------------

    /// The fallback message must not claim things that did not happen.
    #[test]
    fn the_copy_fallback_message_says_only_what_is_true() {
        let msg = copy_fallback_message("clipboard unavailable: no DISPLAY");
        assert!(
            msg.contains("clipboard unavailable"),
            "the message must say what failed: {msg}"
        );
        assert!(
            !msg.contains("copied"),
            "nothing was copied — the message must not say so: {msg}"
        );
        assert!(
            !msg.contains("bytes"),
            "no byte count describes what happened — the transcript got a \
             truncated notice, not the message: {msg}"
        );
        assert!(
            msg.contains("/export"),
            "the message must say where the full text is: {msg}"
        );
    }

    /// A fallback notice is a row, not a document.
    ///
    /// The whole answer used to go into the transcript: every line
    /// rendered, every line indexed for search, every line in the HTML
    /// export — for a fallback whose job is to show the reader
    /// something they can copy from somewhere better.
    #[test]
    fn a_copy_fallback_notice_is_truncated() {
        let long = "x".repeat(NOTICE_MAX_CHARS * 3);
        let notice = truncate_for_notice(&long);
        assert!(
            notice.chars().count() < long.chars().count(),
            "the notice must be shorter than the message it announces"
        );
        assert!(
            notice.contains("/export"),
            "the notice must say where the full text is: {notice}"
        );
        // The head of the message is what the reader needs.
        assert!(
            notice.starts_with("xxxx"),
            "the notice must start with the message: {notice}"
        );
    }

    /// A short message is shown whole — truncation is for the long ones.
    #[test]
    fn a_short_copy_fallback_notice_is_not_truncated() {
        let short = "a short answer";
        assert_eq!(truncate_for_notice(short), short);
    }

    /// The cut is at a character boundary: a CJK message must not end
    /// in half a character (the terminal would draw a replacement
    /// glyph — the very defect this whole exercise is about).
    #[test]
    fn a_copy_fallback_notice_cuts_at_a_character_boundary() {
        let cjk = "汉字测试".repeat(NOTICE_MAX_CHARS);
        let notice = truncate_for_notice(&cjk);
        let head: String = notice.chars().take_while(|c| *c != '\n').collect();
        assert!(
            !head.contains('\u{fffd}'),
            "the cut must not produce a replacement character"
        );
        assert!(
            cjk.starts_with(&head),
            "the notice must be the head of the message"
        );
    }

    #[test]
    fn copy_success_is_only_claimed_when_copy_really_succeeded() {
        let out = classify_copy("hello", Ok(()));
        assert_eq!(out, CopyOutcome::Clipboard { bytes: 5 });
    }

    #[test]
    fn copy_failure_is_never_reported_as_copied_to_clipboard() {
        let out = classify_copy("hello", Err("clipboard unavailable: no DISPLAY".into()));
        match out {
            CopyOutcome::Transcript { bytes, ref reason } => {
                assert_eq!(bytes, 5);
                assert!(reason.contains("clipboard"));
            }
            other => {
                panic!("a failed copy must NOT be reported as a clipboard success; got {other:?}")
            }
        }
    }

    /// The exact stub this replaces was `copy(_s) -> Ok(())`. Nothing in
    /// CI can tell a real clipboard from a fake one, so the only guard we
    /// can install is on the *classification*: a caller-supplied Err must
    /// never be laundered into a success message.
    #[test]
    fn copy_never_launders_an_error_into_success() {
        for reason in ["clipboard unavailable", "clipboard write failed", ""] {
            let out = classify_copy("data", Err(reason.into()));
            assert!(
                matches!(out, CopyOutcome::Transcript { .. }),
                "Err({reason:?}) was laundered into a clipboard success"
            );
        }
    }

    // ----------------------------------------------------------------
    // P0-5: /logout
    // ----------------------------------------------------------------

    fn logout_lines(state: &mut AppState, args: &str) -> Vec<String> {
        let mut settings = SettingsManager::default();
        let r = dispatch(state, &mut settings, CommandId::Logout, args);
        assert!(r.error.is_none(), "/logout errored: {:?}", r.error);
        match r.outcome {
            crate::commands::CommandOutcome::Output(l) => l,
            other => panic!("expected Output, got {other:?}"),
        }
    }

    /// The regression: v0.9.1 printed "credentials unset in this session"
    /// after doing nothing at all. The word "unset" in the imperative
    /// sense must not come back.
    #[test]
    fn logout_never_claims_to_have_unset_anything() {
        let mut state = AppState::new("test-model");
        let lines = logout_lines(&mut state, "anthropic");
        let blob = lines.join("\n");
        for forbidden in ["removed", "credentials unset in this session"] {
            assert!(
                !blob.contains(forbidden),
                "/logout must not claim {forbidden:?}; output was:\n{blob}"
            );
        }
    }

    /// `openai` / `openai-responses` / `openai-compat` all map to
    /// `OPENAI_API_KEY`. A bare `/logout` used to list it three times.
    #[test]
    fn logout_dedupes_env_vars_across_provider_aliases() {
        let mut state = AppState::new("test-model");
        let lines = logout_lines(&mut state, "");
        let joined = lines.join("\n");
        // Whatever the answer, a given variable may be named at most
        // once per `unset` line. Count occurrences of the alias target
        // appearing as a standalone listed command.
        let unset_lines: Vec<&String> = lines
            .iter()
            .filter(|l| l.trim_start().starts_with("unset "))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        for l in &unset_lines {
            let var = l.trim().trim_start_matches("unset ").trim();
            assert!(seen.insert(var), "duplicate `unset {var}` line: {l:?}");
        }
        // And it must never exceed the number of distinct known vars (7).
        assert!(
            unset_lines.len() <= 7,
            "at most 7 distinct env vars are known, got {}: {joined:?}",
            unset_lines.len()
        );
    }

    #[test]
    fn logout_rejects_unknown_provider() {
        let mut state = AppState::new("test-model");
        let lines = logout_lines(&mut state, "not-a-provider");
        assert!(lines[0].contains("unknown provider"), "{lines:?}");
    }
}
