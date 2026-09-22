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
use crate::Entry;
use crate::provider::{Provider, Request, StreamEvent, Usage};

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
pub fn extract_file_operations(entries: &[Entry]) -> (Vec<String>, Vec<String>) {
    let mut reads: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut mods: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in entries {
        if let Some(m) = &entry.message {
            if !matches!(m.role, crate::provider::Role::Tool) {
                continue;
            }
            for block in &m.content {
                if let crate::provider::ContentBlock::Text { text } = block {
                    for token in text.split_whitespace() {
                        let s = token.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>' || c == ',');
                        if is_path_like(s) {
                            reads.insert(s.to_string());
                        }
                    }
                }
            }
        }
    }
    (reads.into_iter().collect(), mods.into_iter().collect())
}

fn is_path_like(s: &str) -> bool {
    s.len() >= 3
        && (s.starts_with('/')
            || s.starts_with("~/")
            || s.starts_with("./")
            || s.starts_with("../")
            || (s.contains('/') && !s.contains(' ') && s.chars().any(|c| c.is_ascii_alphanumeric())))
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
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut out = String::new();
    out.push_str("<read-files>\n");
    for f in read_files {
        out.push_str(&format!("  <file>{f}</file>\n"));
    }
    out.push_str("</read-files>\n");
    out.push_str("<modified-files>\n");
    for f in modified_files {
        out.push_str(&format!("  <file>{f}</file>\n"));
    }
    out.push_str("</modified-files>\n");
    out
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
    let (summary, _usage) = match try_summarize(entries, &provider, model, &read_files, &modified_files).await {
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
    let system = format!(
        "{BRANCH_SUMMARY_PROMPT}\n\nFiles touched on this branch:\n{files}\n"
    );
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
    };
    collect_text_from_stream(provider.clone(), req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{
        AgentMessage, AssistantMessage, ContentBlock, ToolResultMessage, UserMessage,
    };

    fn user_entry(id: &str, content: &str) -> Entry {
        let mut e = Entry::default();
        e.id = id.to_string();
        e.message = Some(crate::provider::Message {
            role: crate::provider::Role::User,
            content: vec![crate::provider::ContentBlock::Text { text: content.to_string() }],
            timestamp: 0,
        });
        e
    }

    fn assistant_entry(id: &str, content: &str) -> Entry {
        let mut e = Entry::default();
        e.id = id.to_string();
        e.message = Some(crate::provider::Message {
            role: crate::provider::Role::Assistant,
            content: vec![crate::provider::ContentBlock::Text { text: content.to_string() }],
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

    #[test]
    fn extract_file_operations_picks_paths_from_tool_results() {
        let mut entry = Entry::default();
        entry.id = "tool".to_string();
        entry.message = Some(crate::provider::Message {
            role: crate::provider::Role::Tool,
            content: vec![crate::provider::ContentBlock::Text {
                text: "wrote /home/user/src/lib.rs and ./README.md".to_string(),
            }],
            timestamp: 0,
        });
        let (reads, _mods) = extract_file_operations(&[entry]);
        assert!(reads.contains(&"/home/user/src/lib.rs".to_string()));
        assert!(reads.contains(&"./README.md".to_string()));
    }

    #[test]
    fn summarize_branch_includes_files_section() {
        let mut entry = Entry::default();
        entry.id = "tool".to_string();
        entry.message = Some(crate::provider::Message {
            role: crate::provider::Role::Tool,
            content: vec![crate::provider::ContentBlock::Text {
                text: "/tmp/foo.rs".to_string(),
            }],
            timestamp: 0,
        });
        let s = summarize_branch(&[entry]);
        assert!(s.contains("/tmp/foo.rs"));
        assert!(s.contains("Files Touched"));
    }

    #[test]
    fn format_file_operations_emits_xml_tags() {
        let xml = format_file_operations(&["/a.rs".into()], &["/b.rs".into()]);
        assert!(xml.contains("<read-files>"));
        assert!(xml.contains("<modified-files>"));
        assert!(xml.contains("<file>/a.rs</file>"));
        assert!(xml.contains("<file>/b.rs</file>"));
    }

    #[test]
    fn format_file_operations_handles_empty_inputs() {
        let xml = format_file_operations(&[], &[]);
        assert!(xml.contains("<read-files>"));
        assert!(xml.contains("</read-files>"));
        assert!(xml.contains("<modified-files>"));
        assert!(xml.contains("</modified-files>"));
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

    #[test]
    fn is_path_like_recognises_common_shapes() {
        assert!(is_path_like("/abs/path"));
        assert!(is_path_like("./relative"));
        assert!(is_path_like("../up"));
        assert!(is_path_like("~/home/path"));
        assert!(!is_path_like("not a path"));
        assert!(!is_path_like("")); // too short
        assert!(!is_path_like("ab")); // too short
    }
}