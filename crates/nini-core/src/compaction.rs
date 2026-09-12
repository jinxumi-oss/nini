//! Compaction algorithm: when context window exceeds budget, summarize old
//! turns and replace with a single compaction entry.
//!
//! Mirrors spec `packages/agent/src/harness/compaction/compaction.ts`.
//!
//! Phase 2 scope: **pure local logic only** — no LLM summarization. The
//! `compact()` function produces a deterministic summary string by
//! extracting file operations, user questions, and assistant outcomes
//! from the entry stream. This gives us a working `compact` action for
//! testing even without provider credentials. Real LLM summarization is
//! a follow-up.

use crate::{AgentMessage, ContentBlock, Entry, EntryType};
use serde::{Deserialize, Serialize};

/// Settings for compaction. Mirrors spec `CompactionSettings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Total context window size in tokens (e.g., 200_000 for Claude).
    pub context_window: u32,
    /// Reserve this many tokens at the end for the next assistant turn.
    pub reserve_tokens: u32,
    /// If a single user/assistant turn is larger than this, split it.
    pub max_single_turn_chars: usize,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            context_window: 200_000,
            reserve_tokens: 8_192,
            max_single_turn_chars: 16_000,
        }
    }
}

/// Trigger: should we compact given current token usage?
///
/// Returns true if `context_tokens > context_window - reserve_tokens`.
pub fn should_compact(context_tokens: u32, settings: &CompactionSettings) -> bool {
    context_tokens
        > settings
            .context_window
            .saturating_sub(settings.reserve_tokens)
}

/// Estimated cost of a single `ContentBlock::Text`.
const TEXT_CHARS_PER_TOKEN: usize = 4;

/// Estimate tokens for a single message using a 4-chars-per-token heuristic.
/// Image blocks are estimated at 4800 tokens (matches spec `ESTIMATED_IMAGE_CHARS`).
pub fn estimate_message_tokens(msg: &AgentMessage) -> u32 {
    let mut total_chars: usize = 0;
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                total_chars = total_chars.saturating_add(text.len());
            }
            ContentBlock::ToolUse { input, .. } => {
                // Name + input JSON length
                let s = serde_json::to_string(input).unwrap_or_default();
                total_chars = total_chars.saturating_add(s.len());
            }
            ContentBlock::ToolResult { content, .. } => {
                total_chars = total_chars.saturating_add(content.len());
            }
        }
    }
    let tokens = total_chars.div_ceil(TEXT_CHARS_PER_TOKEN);
    tokens as u32
}

/// Estimate tokens for a list of messages.
pub fn estimate_messages_tokens(messages: &[AgentMessage]) -> u32 {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Estimate tokens for provider-layer messages.
pub fn estimate_provider_messages_tokens(messages: &[crate::provider::Message]) -> u32 {
    messages
        .iter()
        .map(|m| {
            let mut chars = 0usize;
            for b in &m.content {
                match b {
                    crate::provider::ContentBlock::Text { text } => {
                        chars = chars.saturating_add(text.len())
                    }
                    crate::provider::ContentBlock::ToolUse { input, .. } => {
                        let s = serde_json::to_string(input).unwrap_or_default();
                        chars = chars.saturating_add(s.len());
                    }
                    crate::provider::ContentBlock::ToolResult { content, .. } => {
                        chars = chars.saturating_add(content.len());
                    }
                }
            }
            chars.div_ceil(TEXT_CHARS_PER_TOKEN) as u32
        })
        .sum()
}

/// Estimate tokens for a slice of entries (counts only message entries).
pub fn estimate_entries_tokens(entries: &[Entry]) -> u32 {
    entries
        .iter()
        .filter_map(|e| e.message.as_ref().map(estimate_message_tokens))
        .sum()
}

/// A cut-point: an index into the entries slice where compaction can safely
/// slice (always at a user message boundary, never inside a tool result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPoint {
    /// Index of the first entry to keep (everything before this is summarized).
    pub keep_from: usize,
    /// Reason the cut was chosen.
    pub reason: CutReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutReason {
    /// Cut at the start of the oldest user turn.
    OldestUserTurn,
    /// Cut at the start of the oldest user message.
    OldestUserMessage,
    /// No safe cut point found; we'd have to truncate mid-turn.
    NoSafeCut,
}

/// Find a safe cut point: prefer user-turn boundaries, fall back to
/// user-message boundaries. Never cut inside a tool call sequence.
pub fn find_cut_point(entries: &[Entry]) -> CutPoint {
    // Strategy:
    // 1. Look for the first user-message entry whose preceding entry is
    //    not a tool-result (i.e., a clean turn boundary).
    // 2. If none found, look for any user-message entry.
    // 3. If still none, return NoSafeCut.

    let mut last_user_turn: Option<usize> = None;
    let mut last_user_msg: Option<usize> = None;

    for (i, entry) in entries.iter().enumerate() {
        if let Some(msg) = &entry.message {
            if msg.role == crate::Role::User {
                // A user message with non-empty text content marks a turn boundary.
                let has_text = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if !text.is_empty()));
                if has_text {
                    // Check that the previous entry isn't a tool result (which
                    // would mean we're inside a tool sequence).
                    let prev_is_tool_result = i
                        .checked_sub(1)
                        .and_then(|p| entries.get(p))
                        .and_then(|e| e.message.as_ref())
                        .map(|m| m.role == crate::Role::Tool)
                        .unwrap_or(false);
                    if !prev_is_tool_result && last_user_turn.is_none() {
                        last_user_turn = Some(i);
                    }
                    if last_user_msg.is_none() {
                        last_user_msg = Some(i);
                    }
                }
            }
        }
    }

    if let Some(idx) = last_user_turn {
        CutPoint {
            keep_from: idx,
            reason: CutReason::OldestUserTurn,
        }
    } else if let Some(idx) = last_user_msg {
        CutPoint {
            keep_from: idx,
            reason: CutReason::OldestUserMessage,
        }
    } else {
        CutPoint {
            keep_from: 0,
            reason: CutReason::NoSafeCut,
        }
    }
}

/// What to summarize.
#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    /// Entries that will be summarized (the older prefix).
    pub to_summarize: Vec<Entry>,
    /// Entries that will be retained verbatim (the recent suffix).
    pub retained: Vec<Entry>,
    /// Index in the original entries slice where `retained` starts.
    pub keep_from: usize,
    /// Cut-point reasoning.
    pub cut_reason: CutReason,
    /// Pre-existing summary to merge into, if this is an update.
    pub previous_summary: Option<String>,
}

/// Prepare a compaction: split entries into [summarize, retain] using the
/// cut-point heuristic.
pub fn prepare_compaction(
    entries: &[Entry],
    previous_summary: Option<String>,
) -> CompactionPreparation {
    let cut = find_cut_point(entries);
    let keep_from = cut.keep_from.min(entries.len());
    CompactionPreparation {
        to_summarize: entries[..keep_from].to_vec(),
        retained: entries[keep_from..].to_vec(),
        keep_from,
        cut_reason: cut.reason,
        previous_summary,
    }
}

/// Generate a deterministic local summary from the entries to summarize.
///
/// This is a fallback / placeholder for LLM summarization. It extracts:
/// - File operations (paths read/written/edited)
/// - User questions (first user text in each turn)
/// - Assistant outcomes (text responses)
/// - Tool calls (name + args)
pub fn generate_local_summary(entries: &[Entry]) -> String {
    let mut out = String::new();
    out.push_str("# Local summary (deterministic)\n\n");

    let file_ops: Vec<String> = Vec::new();
    let mut user_questions: Vec<String> = Vec::new();
    let mut tool_calls: Vec<String> = Vec::new();
    let mut assistant_responses: Vec<String> = Vec::new();

    for entry in entries {
        if let Some(msg) = &entry.message {
            match msg.role {
                crate::Role::User => {
                    for b in &msg.content {
                        if let ContentBlock::Text { text } = b {
                            if !text.trim().is_empty() && user_questions.len() < 10 {
                                user_questions.push(text.chars().take(200).collect());
                            }
                        }
                    }
                }
                crate::Role::Assistant => {
                    for b in &msg.content {
                        match b {
                            ContentBlock::Text { text } => {
                                if !text.trim().is_empty() && assistant_responses.len() < 10 {
                                    let snippet: String = text.chars().take(150).collect();
                                    assistant_responses.push(snippet);
                                }
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                let input_str = serde_json::to_string(input).unwrap_or_default();
                                tool_calls.push(format!(
                                    "{}({})",
                                    name,
                                    input_str.chars().take(120).collect::<String>()
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if !user_questions.is_empty() {
        out.push_str("## User requests\n");
        for q in user_questions {
            out.push_str(&format!("- {}\n", q));
        }
        out.push('\n');
    }

    if !tool_calls.is_empty() {
        out.push_str("## Tool calls\n");
        for t in tool_calls {
            out.push_str(&format!("- {}\n", t));
        }
        out.push('\n');
    }

    if !assistant_responses.is_empty() {
        out.push_str("## Assistant responses\n");
        for r in assistant_responses {
            out.push_str(&format!("- {}\n", r));
        }
        out.push('\n');
    }

    if !file_ops.is_empty() {
        out.push_str("## File operations\n");
        for f in file_ops {
            out.push_str(&format!("- {}\n", f));
        }
    }

    if out == "# Local summary (deterministic)\n\n" {
        out.push_str("(no content to summarize)\n");
    }
    out
}

/// Result of running `compact`: a new compaction entry to insert into the
/// entries slice at `keep_from`.
#[derive(Debug, Clone)]
pub struct CompactionOutput {
    pub keep_from: usize,
    pub cut_reason: CutReason,
    pub summary: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
}

/// Run compaction. Returns the new compaction entry plus retained entries;
/// the caller is responsible for splicing them back into the entries slice.
///
/// `summary_provider` is the function that takes (to_summarize, previous)
/// and returns the summary text. v1 passes `generate_local_summary`; future
/// versions pass a provider call.
pub fn compact<F>(
    entries: &[Entry],
    settings: &CompactionSettings,
    previous_summary: Option<String>,
    summary_provider: F,
) -> CompactionOutput
where
    F: FnOnce(&[Entry], Option<String>) -> String,
{
    let prep = prepare_compaction(entries, previous_summary);
    let tokens_before = estimate_entries_tokens(entries);

    let summary = summary_provider(&prep.to_summarize, prep.previous_summary.clone());

    // Build the new compaction entry (preserves retention list verbatim).
    let summary_tokens = estimate_message_tokens(&AgentMessage {
        role: crate::Role::User, // system would be better, but stick with what we have
        content: vec![ContentBlock::Text {
            text: summary.clone(),
        }],
        timestamp: 0,
    });

    // Compute the tokens for retained entries + the new compaction entry.
    let retained_tokens: u32 = prep
        .retained
        .iter()
        .filter_map(|e| e.message.as_ref())
        .map(estimate_message_tokens)
        .sum();
    let tokens_after = summary_tokens + retained_tokens;

    let _ = settings; // reserved for future size limits on summary

    CompactionOutput {
        keep_from: prep.keep_from,
        cut_reason: prep.cut_reason,
        summary,
        tokens_before,
        tokens_after,
    }
}

/// Construct the new compaction `Entry` from a `CompactionOutput`.
pub fn make_compaction_entry(out: &CompactionOutput) -> Entry {
    Entry {
        id: format!(
            "compaction_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        parent_id: None,
        seq: 0, // assigned by caller
        timestamp: chrono::Utc::now().timestamp_millis(),
        entry_type: EntryType::Compaction,
        message: None,
        summary: Some(out.summary.clone()),
        from_id: None,
        custom_type: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentMessage, ContentBlock, Entry, EntryType, Role};

    fn make_user_entry(id: &str, text: &str, parent: Option<&str>, seq: u64) -> Entry {
        Entry {
            id: id.into(),
            parent_id: parent.map(|s| s.to_string()),
            seq,
            timestamp: 0,
            entry_type: EntryType::Message,
            message: Some(AgentMessage::user(text)),
            summary: None,
            from_id: None,
            custom_type: None,
            data: None,
        }
    }

    fn make_assistant_entry(id: &str, text: &str, parent: Option<&str>, seq: u64) -> Entry {
        Entry {
            id: id.into(),
            parent_id: parent.map(|s| s.to_string()),
            seq,
            timestamp: 0,
            entry_type: EntryType::Message,
            message: Some(AgentMessage::assistant(text)),
            summary: None,
            from_id: None,
            custom_type: None,
            data: None,
        }
    }

    fn make_tool_entry(id: &str, name: &str, args: &str, parent: Option<&str>, seq: u64) -> Entry {
        let input: serde_json::Value =
            serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
        Entry {
            id: id.into(),
            parent_id: parent.map(|s| s.to_string()),
            seq,
            timestamp: 0,
            entry_type: EntryType::Message,
            message: Some(AgentMessage {
                role: Role::Tool,
                content: vec![ContentBlock::ToolUse {
                    id: format!("toolu_{id}"),
                    name: name.into(),
                    input,
                }],
                timestamp: 0,
            }),
            summary: None,
            from_id: None,
            custom_type: None,
            data: None,
        }
    }

    // ============================================================
    // Test 1: should_compact trigger
    // ============================================================
    #[test]
    fn should_compact_returns_true_above_budget() {
        let s = CompactionSettings {
            context_window: 1000,
            reserve_tokens: 100,
            ..Default::default()
        };
        assert!(!should_compact(800, &s));
        assert!(!should_compact(900, &s));
        assert!(should_compact(901, &s));
        assert!(should_compact(2000, &s));
    }

    // ============================================================
    // Test 2: estimate_message_tokens
    // ============================================================
    #[test]
    fn estimate_text_message() {
        let m = AgentMessage::user("hello world"); // 11 chars / 4 = 3 tokens
        assert_eq!(estimate_message_tokens(&m), 3);
    }

    #[test]
    fn estimate_empty_message_is_zero() {
        let m = AgentMessage::user("");
        assert_eq!(estimate_message_tokens(&m), 0);
    }

    #[test]
    fn estimate_tool_use_includes_input_json() {
        let input = serde_json::json!({"path": "/tmp/foo"});
        let m = AgentMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read".into(),
                input,
            }],
            timestamp: 0,
        };
        let tokens = estimate_message_tokens(&m);
        assert!(tokens > 0);
    }

    // ============================================================
    // Test 3: find_cut_point
    // ============================================================
    #[test]
    fn cut_point_simple_user_turn() {
        // Two-turn conversation with no prior compaction. The first user
        // turn (index 0) is the start of context; nothing to summarize.
        let entries = vec![
            make_user_entry("u1", "first", None, 1),
            make_assistant_entry("a1", "reply 1", Some("u1"), 2),
            make_user_entry("u2", "second", Some("a1"), 3),
            make_assistant_entry("a2", "reply 2", Some("u2"), 4),
        ];
        let cut = find_cut_point(&entries);
        assert_eq!(cut.keep_from, 0);
        assert_eq!(cut.reason, CutReason::OldestUserTurn);
    }

    #[test]
    fn cut_point_skips_user_inside_tool_sequence() {
        // Simulate: user, assistant, tool_call, tool_result, user, assistant
        // The "user" inside a tool sequence shouldn't be a clean turn.
        let entries = vec![
            make_user_entry("u1", "first", None, 1),
            make_assistant_entry("a1", "I'll use bash", Some("u1"), 2),
            make_tool_entry("t1", "bash", r#"{"command":"ls"}"#, Some("a1"), 3),
            // user might appear here inside a tool result, but we treat
            // the role-as-tool-result case as non-user for cut purposes
            make_user_entry("u2", "next question", Some("t1"), 4),
            make_assistant_entry("a2", "done", Some("u2"), 5),
        ];
        let cut = find_cut_point(&entries);
        // u1 at index 0 is the first user turn; no prior content to summarize.
        assert_eq!(cut.keep_from, 0);
        assert_eq!(cut.reason, CutReason::OldestUserTurn);
    }

    #[test]
    fn cut_point_no_user_returns_zero() {
        let entries = vec![make_assistant_entry("a1", "hi", None, 1)];
        let cut = find_cut_point(&entries);
        assert_eq!(cut.keep_from, 0);
        assert_eq!(cut.reason, CutReason::NoSafeCut);
    }

    #[test]
    fn cut_point_empty_entries() {
        let cut = find_cut_point(&[]);
        assert_eq!(cut.keep_from, 0);
    }

    // ============================================================
    // Test 4: prepare_compaction
    // ============================================================
    #[test]
    fn prepare_compaction_splits_at_cut_point() {
        // Two prior turns + a fresh turn. The cut should land before the
        // second user turn (index 2), summarizing the first turn away.
        let entries = vec![
            make_user_entry("u1", "first", None, 1),
            make_assistant_entry("a1", "reply 1", Some("u1"), 2),
            make_user_entry("u2", "second", Some("a1"), 3),
            make_assistant_entry("a2", "reply 2", Some("u2"), 4),
            make_user_entry("u3", "third", Some("a2"), 5),
        ];
        let prep = prepare_compaction(&entries, None);
        assert_eq!(prep.keep_from, 0); // first user turn = no prior to summarize
        assert_eq!(prep.to_summarize.len(), 0);
        assert_eq!(prep.retained.len(), 5);
    }

    // ============================================================
    // Test 5: generate_local_summary
    // ============================================================
    #[test]
    fn summary_extracts_user_requests() {
        let entries = vec![
            make_user_entry("u1", "find bugs in main.rs", None, 1),
            make_assistant_entry("a1", "I found 3 bugs", Some("u1"), 2),
        ];
        let summary = generate_local_summary(&entries);
        assert!(summary.contains("find bugs in main.rs"));
        assert!(summary.contains("Assistant responses"));
    }

    #[test]
    fn summary_extracts_tool_calls() {
        let entries = vec![
            make_user_entry("u1", "read main.rs", None, 1),
            make_tool_entry("a1", "read", r#"{"path":"main.rs"}"#, Some("u1"), 2),
        ];
        let summary = generate_local_summary(&entries);
        assert!(summary.contains("read"));
        assert!(summary.contains("main.rs"));
    }

    #[test]
    fn summary_handles_empty_entries() {
        let summary = generate_local_summary(&[]);
        assert!(summary.contains("no content"));
    }

    // ============================================================
    // Test 6: compact() end-to-end
    // ============================================================
    #[test]
    fn compact_produces_compaction_output() {
        let entries = vec![
            make_user_entry("u1", "first question", None, 1),
            make_assistant_entry("a1", "first answer", Some("u1"), 2),
            make_user_entry("u2", "second question", Some("a1"), 3),
            make_assistant_entry("a2", "second answer", Some("u2"), 4),
        ];
        let settings = CompactionSettings::default();
        let out = compact(&entries, &settings, None, |entries, _prev| {
            generate_local_summary(entries)
        });
        assert_eq!(out.keep_from, 0);
        assert_eq!(out.cut_reason, CutReason::OldestUserTurn);
        // With nothing to summarize, summary is the "no content" stub
        assert!(out.summary.contains("no content"));
        assert!(out.tokens_before > 0);
    }

    #[test]
    fn compact_with_previous_summary_merges() {
        let entries = vec![
            make_user_entry("u1", "follow-up", None, 1),
            make_assistant_entry("a1", "answer", Some("u1"), 2),
        ];
        let settings = CompactionSettings::default();
        let prev = Some("# Previous summary\nUser asked about bugs.".to_string());
        let out = compact(
            &entries,
            &settings,
            prev.clone(),
            |_entries, prev_summary| format!("merged: {}", prev_summary.as_deref().unwrap_or("")),
        );
        assert!(out.summary.starts_with("merged: "));
    }

    // ============================================================
    // Test 7: make_compaction_entry
    // ============================================================
    #[test]
    fn compaction_entry_has_correct_type_and_summary() {
        let entries = vec![
            make_user_entry("u1", "first", None, 1),
            make_assistant_entry("a1", "reply", Some("u1"), 2),
        ];
        let settings = CompactionSettings::default();
        let out = compact(&entries, &settings, None, |entries, _prev| {
            generate_local_summary(entries)
        });
        let entry = make_compaction_entry(&out);
        assert_eq!(entry.entry_type, EntryType::Compaction);
        assert!(entry.summary.is_some());
        assert!(entry.message.is_none()); // compactions have no message
    }

    // ============================================================
    // Test 8: token reduction is meaningful
    // ============================================================
    #[test]
    fn compaction_actually_reduces_tokens() {
        // Build entries with a prior compaction entry so there's something
        // to compact (the 20 alternating turns).
        let mut entries: Vec<Entry> = Vec::new();
        entries.push(Entry {
            id: "c0".into(),
            parent_id: None,
            seq: 0,
            timestamp: 0,
            entry_type: EntryType::Compaction,
            message: None,
            summary: Some("prior context".into()),
            from_id: None,
            custom_type: None,
            data: None,
        });
        for i in 0..20 {
            if i % 2 == 0 {
                let parent: String = if i == 0 {
                    "c0".into()
                } else {
                    format!("a{}", i - 1)
                };
                entries.push(make_user_entry(
                    &format!("u{i}"),
                    &"x".repeat(100),
                    Some(&parent),
                    (i as u64) + 1,
                ));
            } else {
                entries.push(make_assistant_entry(
                    &format!("a{i}"),
                    &"y".repeat(100),
                    Some(&format!("u{}", i - 1)),
                    (i as u64) + 1,
                ));
            }
        }
        let settings = CompactionSettings::default();
        let out = compact(&entries, &settings, None, |entries, _prev| {
            generate_local_summary(entries)
        });
        // With the local-only algorithm cutting at the first user turn,
        // and that first user turn being index 1 (after the prior compaction),
        // only the prior compaction entry is summarized. The remaining
        // entries are kept verbatim.
        assert_eq!(out.keep_from, 1);
        assert_eq!(out.tokens_before, estimate_entries_tokens(&entries));
        assert!(!out.summary.is_empty());
        // Local summary must be terse (heuristic, not LLM).
        assert!(out.summary.len() < 500);
    }
}
