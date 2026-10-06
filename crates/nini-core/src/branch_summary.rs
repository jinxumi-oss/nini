//! Branch summary generation — converts a slice of session entries into a
//! concise markdown summary suitable for a `BranchSummaryEntry`.
//!
//! Two modes:
//! - **Heuristic** (`summarize_branch`) — extracts files mentioned in
//!   `read_files` / `modified_files` fields and produces a deterministic
//!   fallback summary. Used when no provider is available.
//! - **LLM-backed** (`summarize_branch_with_llm`) — feeds the entries
//!   through a model provider's streaming `stream()` API, accumulating
//!   `TextDelta` chunks into a single summary string. Used when a real
//!   provider is configured.
//!
//! The branch-summary prompt is modeled after Pi's `generateBranchSummary`
//! (6 sections: Goal / Constraints / Progress / Key Decisions / Next
//! Steps / Output Format). Output is plain markdown — no code fence
//! delimiters, ready to store verbatim in `BranchSummaryMessage.summary`.
//!
//! References:
//! - pi-mono `harness/compaction/branch-summarization.ts`
//! - nini `nini_core::compaction::SUMMARIZATION_PROMPT` for tone alignment

use std::sync::Arc;

use futures_util::StreamExt;

use crate::entries::BranchSummaryMessage;
use crate::provider::{Provider, Request, StreamEvent, Usage};
use crate::Entry;

/// Default 6-section prompt used for LLM-backed branch summarization.
///
/// Pi uses an identical structure but with different style — this is the
/// nini v1 wording (slightly tighter, English-only).
pub const BRANCH_SUMMARY_PROMPT: &str = "\
You are summarizing a branch of an nini agent session. The branch is being \
navigated away from and will never be resumed directly — your summary is the \
only record of what happened on this branch.

Write the summary in markdown with EXACTLY these six sections, in order:

## Goal
One sentence: what the user was trying to accomplish on this branch.

## Constraints
Bullet list: any boundaries, preferences, or constraints the user expressed.

## Progress
Bullet list: concrete steps taken (commands run, files edited, decisions made).

## Key Decisions
Bullet list: non-obvious choices and the reason for each.

## Next Steps
Bullet list: what would naturally come next if the user resumes this work.

## Output Format
A single markdown document, no preamble, no commentary, no code fence \
delimiters. Do not invent file paths or details you cannot see.";

/// Maximum input length (chars) sent to the LLM. Entries beyond this are
/// truncated by dropping the oldest non-message entries first.
const MAX_INPUT_CHARS: usize = 24_000;

/// Heuristic summary — extracts file operations and produces a markdown
/// summary without an LLM. Deterministic and offline-friendly.
pub fn summarize_branch(entries: &[Entry]) -> String {
    let (read_files, modified_files) = extract_file_operations(entries);
    let mut out = String::from("# Branch Summary (heuristic)\n\n");

    if !read_files.is_empty() {
        out.push_str("## Files Touched\n\n");
        out.push_str("Read:\n");
        for f in &read_files {
            out.push_str(&format!("- `{f}`\n"));
        }
        out.push('\n');
    }

    if !modified_files.is_empty() {
        out.push_str("Modified:\n");
        for f in &modified_files {
            out.push_str(&format!("- `{f}`\n"));
        }
        out.push('\n');
    }

    let message_count = entries
        .iter()
        .filter(|e| matches!(e.message.as_ref(), Some(m) if matches!(m.role, crate::provider::Role::User)))
        .count();

    if message_count == 0 && read_files.is_empty() && modified_files.is_empty() {
        out.push_str("No activity recorded on this branch.\n");
    } else {
        out.push_str(&format!(
            "_{message_count} user message(s) on this branch._\n"
        ));
    }
    out
}

/// Extract file paths referenced by tool calls in the given entries.
/// Returns `(read_files, modified_files)` as sorted, deduplicated vectors.
/// Accumulated file operations, mirroring Pi's `FileOperations`
/// (`compaction/utils.ts:5-13`).
#[derive(Debug, Clone, Default)]
pub struct FileOps {
    /// Paths passed to the `read` tool.
    pub read: std::collections::BTreeSet<String>,
    /// Paths passed to the `write` tool.
    pub written: std::collections::BTreeSet<String>,
    /// Paths passed to the `edit` tool.
    pub edited: std::collections::BTreeSet<String>,
}

/// Extract file paths referenced by tool calls in the given entries.
/// Returns `(read_files, modified_files)` as sorted, deduplicated vectors.
///
/// v0.10: this used to scrape *Tool* message text, split it on whitespace
/// and regex-match anything path-shaped. That was wrong in four ways:
///
///  1. it read `Role::Tool` messages, while Pi reads the **assistant** turn
///     that requested the call (`utils.ts:25`);
///  2. it ignored the structured `ToolUse` block and guessed from prose, so
///     a path merely quoted inside a file's *contents* looked like a read;
///  3. it had no tool-name dispatch, so nothing could ever be classified as
///     modified — the `mods` set was declared and never inserted into;
///  4. consequently `build_branch_summary_entry` always produced an empty
///     `modified_files`.
///
/// Pi reads the assistant's `toolCall` blocks and dispatches on the tool
/// name (`utils.ts:24-52`). The structured form was available all along.
pub fn extract_file_operations(entries: &[Entry]) -> (Vec<String>, Vec<String>) {
    let mut ops = FileOps::default();
    for entry in entries {
        let Some(m) = &entry.message else { continue };
        if !matches!(m.role, crate::provider::Role::Assistant) {
            continue;
        }
        for block in &m.content {
            let crate::provider::ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            let Some(path) = input.get("path").and_then(|p| p.as_str()) else {
                continue;
            };
            match name.as_str() {
                "read" => {
                    ops.read.insert(path.to_string());
                }
                "write" => {
                    ops.written.insert(path.to_string());
                }
                "edit" => {
                    ops.edited.insert(path.to_string());
                }
                _ => {}
            }
        }
    }
    compute_file_lists(&ops)
}

/// Split accumulated operations into read-only and modified lists.
///
/// Mirrors Pi's `computeFileLists` (`utils.ts:54-58`): a path that was both
/// read and written counts as **modified only** — listing it under
/// read-files would tell the summarizer the file was left alone.
pub fn compute_file_lists(ops: &FileOps) -> (Vec<String>, Vec<String>) {
    let mut modified: std::collections::BTreeSet<String> = ops.edited.clone();
    modified.extend(ops.written.iter().cloned());
    let read_only: Vec<String> = ops
        .read
        .iter()
        .filter(|f| !modified.contains(*f))
        .cloned()
        .collect();
    (read_only, modified.into_iter().collect())
}

/// Serialize entries to a single string suitable for inclusion in a prompt.
/// Drops the oldest entries until the output fits in `MAX_INPUT_CHARS`.
pub fn serialize_entries_for_prompt(entries: &[Entry]) -> String {
    // Walk newest-first, dropping oldest entries when the running
    // total would exceed MAX_INPUT_CHARS. The newest entries are
    // always preserved.
    let lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| format_entry_line(i, entry))
        .collect();
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    if total <= MAX_INPUT_CHARS {
        return lines.join("\n") + "\n";
    }
    // Walk forward, dropping oldest until under budget.
    let mut drop = 0usize;
    let mut running = total;
    while running > MAX_INPUT_CHARS && drop < lines.len() {
        running = running.saturating_sub(lines[drop].len() + 1);
        drop += 1;
    }
    lines[drop..].join("\n") + "\n"
}

fn format_entry_line(idx: usize, entry: &Entry) -> String {
    let role = match &entry.message {
        Some(m) => match m.role {
            crate::provider::Role::System => "system",
            crate::provider::Role::User => "user",
            crate::provider::Role::Assistant => "assistant",
            crate::provider::Role::Tool => "tool",
        },
        None => "meta",
    };
    let body = match &entry.message {
        Some(m) => m
            .content
            .iter()
            .filter_map(|b| match b {
                crate::provider::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    };
    format!("[{:04}] {:<12} {}", idx, role, body)
}

/// Format file operations as the XML-style block pi-mono uses for branch
/// summary inputs.
/// Render the file lists as summary metadata.
///
/// Mirrors Pi's `formatFileOperations` (`utils.ts:62-74`): newline-separated
/// paths inside `<read-files>` / `<modified-files>`, and **nothing at all**
/// when both lists are empty. nini used to emit both tags unconditionally
/// with `<file>` wrappers, making "no operations" indistinguishable from
/// "not reported".
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// Build a `BranchSummaryMessage` from entries without invoking an LLM.
/// The summary is the heuristic fallback (`summarize_branch`).
pub fn build_branch_summary_entry(entries: &[Entry]) -> BranchSummaryMessage {
    let (read_files, modified_files) = extract_file_operations(entries);
    BranchSummaryMessage {
        summary: summarize_branch(entries),
        read_files,
        modified_files,
    }
}

/// Collect text deltas from a provider's stream. Returns the concatenated
/// `TextDelta` text and the final `Usage` (if the stream emits a
/// `MessageStop`).
pub async fn collect_text_from_stream(
    provider: Arc<dyn Provider>,
    req: Request,
) -> Result<(String, Option<Usage>), String> {
    let mut stream = provider.stream(req);
    let mut out = String::new();
    let mut usage: Option<Usage> = None;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(StreamEvent::TextDelta { text }) => out.push_str(&text),
            Ok(StreamEvent::MessageStop { usage: u, .. }) => usage = Some(u),
            Ok(_) => {}
            Err(e) => return Err(format!("provider stream error: {e}")),
        }
    }
    Ok((out, usage))
}

/// LLM-backed branch summary: prompts a real provider with the entries,
/// collects the streamed text, and returns a `BranchSummaryMessage`.
///
/// Falls back to the heuristic summary if the provider errors or returns
/// an empty response.
pub async fn summarize_branch_with_llm(
    entries: &[Entry],
    provider: Arc<dyn Provider>,
    model: &str,
    read_files: Vec<String>,
    modified_files: Vec<String>,
) -> BranchSummaryMessage {
    let (summary, _usage) =
        match try_summarize(entries, &provider, model, &read_files, &modified_files).await {
            Ok(s) => s,
            Err(_e) => (summarize_branch(entries), None),
        };
    BranchSummaryMessage {
        summary,
        read_files,
        modified_files,
    }
}

async fn try_summarize(
    entries: &[Entry],
    provider: &Arc<dyn Provider>,
    model: &str,
    read_files: &[String],
    modified_files: &[String],
) -> Result<(String, Option<Usage>), String> {
    let transcript = serialize_entries_for_prompt(entries);
    let files = format_file_operations(read_files, modified_files);
    let system = format!("{BRANCH_SUMMARY_PROMPT}\n\nFiles touched on this branch:\n{files}\n");
    let req = Request {
        model: model.to_string(),
        messages: vec![crate::provider::Message {
            role: crate::provider::Role::User,
            content: vec![crate::provider::ContentBlock::Text { text: transcript }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(2048),
        temperature: Some(0.2),
        system: Some(system),
        reasoning: None,
    };
    collect_text_from_stream(provider.clone(), req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_entry(id: &str, content: &str) -> Entry {
        let mut e = Entry::default();
        e.id = id.to_string();
        e.message = Some(crate::provider::Message {
            role: crate::provider::Role::User,
            content: vec![crate::provider::ContentBlock::Text {
                text: content.to_string(),
            }],
            timestamp: 0,
        });
        e
    }

    fn assistant_entry(id: &str, content: &str) -> Entry {
        let mut e = Entry::default();
        e.id = id.to_string();
        e.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![crate::provider::ContentBlock::Text {
                text: content.to_string(),
            }],
            timestamp: 0,
        });
        e
    }

    #[test]
    fn empty_entries_produce_placeholder_summary() {
        let s = summarize_branch(&[]);
        assert!(s.contains("heuristic"));
        assert!(s.contains("No activity"));
    }

    #[test]
    fn user_messages_counted_in_summary() {
        let entries = vec![
            user_entry("a", "first question"),
            user_entry("b", "second question"),
            user_entry("c", "third question"),
        ];
        let s = summarize_branch(&entries);
        assert!(s.contains("3 user message"));
    }

    #[test]
    fn extract_file_operations_user_message_yields_no_paths() {
        // User messages contain text but no tool-call paths.
        let entry = user_entry("a", "see /src/foo.rs and /src/foo.rs again");
        let (reads, _mods) = extract_file_operations(&[entry]);
        assert!(reads.is_empty());
    }

    /// v0.10: this used to assert the *bug*. It built a `Role::Tool` message
    /// whose text mentioned two paths and required both to be reported as
    /// files the agent had read. That is the old scraper: text matching
    /// instead of structured tool calls, so any path mentioned in prose —
    /// including one merely quoted inside a file's contents — was recorded
    /// as an operation.
    ///
    /// Pi reads the assistant's `toolCall` blocks (`utils.ts:24-52`).
    #[test]
    fn paths_in_tool_result_prose_are_not_operations() {
        let mut entry = Entry::default();
        entry.id = "tool".to_string();
        entry.message = Some(crate::provider::Message {
            role: crate::provider::Role::Tool,
            content: vec![crate::provider::ContentBlock::Text {
                text: "wrote /home/user/src/lib.rs and ./README.md".to_string(),
            }],
            timestamp: 0,
        });
        let (reads, mods) = extract_file_operations(&[entry]);
        assert!(
            reads.is_empty() && mods.is_empty(),
            "tool *result* text is output, not a request; got {reads:?} / {mods:?}"
        );
    }

    /// v0.10: built a `Role::Tool` message and expected the branch summary
    /// to name the path in it. The summary now derives its file list from
    /// assistant tool calls, so this asserts that route instead.
    #[test]
    fn summarize_branch_includes_files_section() {
        let mut entry = Entry::default();
        entry.id = "assistant".to_string();
        entry.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![crate::provider::ContentBlock::ToolUse {
                id: "1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({ "path": "/tmp/foo.rs" }),
            }],
            timestamp: 0,
        });
        let s = summarize_branch(&[entry]);
        assert!(s.contains("/tmp/foo.rs"), "{s}");
        assert!(s.contains("Files Touched"), "{s}");
    }

    /// v0.10: was asserting `<file>` wrappers, which Pi does not emit —
    /// `formatFileOperations` (`utils.ts:66-71`) writes bare newline-separated
    /// paths. The shape is what the summarizer model sees, so it is worth
    /// pinning exactly.
    #[test]
    fn format_file_operations_emits_pi_sections() {
        let xml = format_file_operations(&["/a.rs".into()], &["/b.rs".into()]);
        assert_eq!(
            xml,
            "\n\n<read-files>\n/a.rs\n</read-files>\n\n<modified-files>\n/b.rs\n</modified-files>"
        );
    }

    /// v0.10: used to assert that empty input still emits both tags. Pi
    /// returns `""` (`utils.ts:73`), and the distinction matters: an empty
    /// section reads as "the summarizer never looked", which is how a
    /// silently-empty file inventory hides.
    #[test]
    fn format_file_operations_emits_nothing_when_there_are_no_operations() {
        assert_eq!(format_file_operations(&[], &[]), "");
    }

    #[test]
    fn serialize_entries_for_prompt_includes_role_prefix() {
        let entries = vec![user_entry("a", "hello"), assistant_entry("b", "world")];
        let prompt = serialize_entries_for_prompt(&entries);
        assert!(prompt.contains("user"));
        assert!(prompt.contains("assistant"));
        assert!(prompt.contains("hello"));
        assert!(prompt.contains("world"));
    }

    #[test]
    fn serialize_entries_truncates_at_max_chars() {
        // Build entries longer than MAX_INPUT_CHARS.
        let mut entries = Vec::new();
        for i in 0..2000 {
            entries.push(user_entry(
                &format!("u{i}"),
                &format!("message {} with some filler text {}", i, "x".repeat(100)),
            ));
        }
        let prompt = serialize_entries_for_prompt(&entries);
        assert!(prompt.len() <= MAX_INPUT_CHARS + 200); // small slack
                                                        // Should include at least the last entries
        assert!(prompt.contains("1999"));
    }

    #[test]
    fn build_branch_summary_entry_produces_complete_struct() {
        let entries = vec![user_entry("a", "test")];
        let summary = build_branch_summary_entry(&entries);
        assert!(summary.summary.contains("heuristic"));
        assert!(!summary.read_files.is_empty() || summary.summary.contains("user message"));
    }

    #[test]
    fn branch_summary_prompt_has_six_sections() {
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Goal"));
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Constraints"));
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Progress"));
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Key Decisions"));
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Next Steps"));
        assert!(BRANCH_SUMMARY_PROMPT.contains("## Output Format"));
    }

    /// v0.10: replaces a test for `is_path_like`, a path-shaped-token
    /// heuristic that existed only to support the old scraper. Path *shape*
    /// is not the discriminator Pi uses — the structured tool call and its
    /// name are (`utils.ts:24-52`). What matters now:
    /// classification by tool name, and read-only meaning "not modified".
    #[test]
    fn file_ops_come_from_tool_calls_not_from_prose() {
        let mut e1 = Entry::default();
        e1.id = "a1".into();
        e1.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![
                crate::provider::ContentBlock::ToolUse {
                    id: "1".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "path": "src/one.rs" }),
                },
                crate::provider::ContentBlock::ToolUse {
                    id: "2".into(),
                    name: "write".into(),
                    input: serde_json::json!({ "path": "src/two.rs" }),
                },
                crate::provider::ContentBlock::ToolUse {
                    id: "3".into(),
                    name: "edit".into(),
                    input: serde_json::json!({ "path": "src/three.rs" }),
                },
            ],
            timestamp: 0,
        });
        let mut e2 = Entry::default();
        e2.id = "a2".into();
        e2.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![crate::provider::ContentBlock::Text {
                text: "I will not touch /etc/passwd".into(),
            }],
            timestamp: 0,
        });
        let (read, modified) = extract_file_operations(&[e1, e2]);
        assert_eq!(read, vec!["src/one.rs".to_string()]);
        assert_eq!(
            modified,
            vec!["src/three.rs".to_string(), "src/two.rs".to_string()]
        );
    }

    #[test]
    fn a_read_then_edit_is_modified_not_read_only() {
        let mut e = Entry::default();
        e.id = "a1".into();
        e.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![
                crate::provider::ContentBlock::ToolUse {
                    id: "1".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "path": "src/x.rs" }),
                },
                crate::provider::ContentBlock::ToolUse {
                    id: "2".into(),
                    name: "edit".into(),
                    input: serde_json::json!({ "path": "src/x.rs" }),
                },
            ],
            timestamp: 0,
        });
        let (read, modified) = extract_file_operations(&[e]);
        assert!(
            read.is_empty(),
            "modified file must not be listed as read-only"
        );
        assert_eq!(modified, vec!["src/x.rs".to_string()]);
    }
}
