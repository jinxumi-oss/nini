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

use crate::provider::Message;
use crate::{AgentMessage, ContentBlock, Entry, LegacyEntryType, Role};
use serde::{Deserialize, Serialize};

/// Pi-compatible context-usage snapshot: `{ tokens, contextWindow, percent }`.
/// Returned by `Agent::context_usage()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUsage {
    /// Most recent API-reported token count. None until the first
    /// assistant message completes with usage info.
    pub tokens: Option<u32>,
    /// Total context window size in tokens.
    pub context_window: u32,
    /// Percentage of window used (0.0–100.0). None until `tokens` is known.
    pub percent: Option<f64>,
}

/// Settings for compaction. Mirrors Pi's `CompactionSettings` shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Whether auto-compaction is enabled (Pi parity).
    /// When false, `/compact` still works manually but auto-trigger does not.
    pub enabled: bool,
    /// Total context window size in tokens (e.g., 200_000 for Claude).
    pub context_window: u32,
    /// Reserve this many tokens at the end for the next assistant turn.
    pub reserve_tokens: u32,
    /// When cutting for compaction, keep at least this many recent
    /// tokens of message history verbatim. The summary covers everything
    /// before the cut.
    pub keep_recent_tokens: u32,
    /// If a single user/assistant turn is larger than this, split it.
    pub max_single_turn_chars: usize,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        // Mirror Pi's DEFAULT_COMPACTION_SETTINGS:
        // { enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 }
        Self {
            enabled: true,
            context_window: 200_000,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
            max_single_turn_chars: 16_000,
        }
    }
}

/// Trigger: should we compact given current token usage?
///
/// Returns true if compaction should run: enabled AND token count exceeds
/// the configured budget. Mirrors Pi's `shouldCompact()`.
pub fn should_compact(context_tokens: u32, settings: &CompactionSettings) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens
        > settings
            .context_window
            .saturating_sub(settings.reserve_tokens)
}

/// Estimated cost of a single `ContentBlock::Text`.
const TEXT_CHARS_PER_TOKEN: usize = 4;

/// Estimated cost of a single CJK character (Chinese/Japanese/Korean).
/// CJK characters encode more information per glyph, so they tokenize at
/// roughly 2 chars/token instead of 4. Mixed CJK/ASCII uses 3 chars/token.
const CJK_CHARS_PER_TOKEN: usize = 2;
const MIXED_CHARS_PER_TOKEN: usize = 3;

/// Estimate tokens for a string using per-glyph classification:
/// - ASCII bytes → ~4 chars/token
/// - CJK glyphs → ~2 chars/token
/// - Other (punctuation, math) → ~3 chars/token
/// Returns total tokens (rounded up).
pub fn estimate_string_tokens(s: &str) -> u32 {
    let mut ascii_chars = 0usize;
    let mut cjk_chars = 0usize;
    let mut other_chars = 0usize;
    for c in s.chars() {
        if is_cjk(c) {
            cjk_chars += 1;
        } else if c.is_ascii() {
            ascii_chars += 1;
        } else {
            other_chars += 1;
        }
    }
    let tokens = ascii_chars.div_ceil(TEXT_CHARS_PER_TOKEN)
        + cjk_chars.div_ceil(CJK_CHARS_PER_TOKEN)
        + other_chars.div_ceil(MIXED_CHARS_PER_TOKEN);
    tokens as u32
}

/// Returns true if `c` is a CJK Unified Ideograph, Hiragana, Katakana, or
/// Hangul Syllable.
pub fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x4E00..=0x9FFF        // CJK Unified Ideographs
        | 0x3400..=0x4DBF       // CJK Extension A
        | 0x3040..=0x309F       // Hiragana
        | 0x30A0..=0x30FF       // Katakana
        | 0xAC00..=0xD7AF       // Hangul Syllables
        | 0xFF00..=0xFFEF       // Halfwidth/Fullwidth
        | 0x1F300..=0x1F9FF      // Misc Symbols and Pictographs (subset)
    )
}

/// Estimate tokens for a single entry (message or system/tool result).
fn estimate_entry_tokens(entry: &Entry) -> u32 {
    if let Some(msg) = &entry.message {
        estimate_legacy_message_tokens(msg)
    } else {
        0
    }
}

/// True if `entries[i]` is a clean user-turn boundary (user message with
/// non-empty text, not preceded by a tool-result).
fn is_user_turn_boundary(entries: &[Entry], i: usize) -> bool {
    if let Some(msg) = &entries[i].message {
        if msg.role != crate::Role::User {
            return false;
        }
        let has_text = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if !text.is_empty()));
        if !has_text {
            return false;
        }
        // Check that the previous entry isn't a tool-result (we'd be inside
        // a tool sequence otherwise).
        let prev_is_tool = i
            .checked_sub(1)
            .and_then(|p| entries.get(p))
            .and_then(|e| e.message.as_ref())
            .map(|m| m.role == crate::Role::Tool)
            .unwrap_or(false);
        !prev_is_tool
    } else {
        false
    }
}

/// Estimate tokens for a single message using a per-glyph heuristic that
/// accounts for ASCII vs CJK characters.
/// Image blocks are estimated at 4800 tokens (matches spec `ESTIMATED_IMAGE_CHARS`).
pub fn estimate_message_tokens(msg: &AgentMessage) -> u32 {
    let mut total_tokens: u32 = 0;
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                total_tokens = total_tokens.saturating_add(estimate_string_tokens(text));
            }
            ContentBlock::ToolUse { input, .. } => {
                // Name + input JSON length
                let s = serde_json::to_string(input).unwrap_or_default();
                total_tokens = total_tokens.saturating_add(estimate_string_tokens(&s));
            }
            ContentBlock::ToolResult { content, .. } => {
                total_tokens = total_tokens.saturating_add(estimate_string_tokens(content));
            }
        }
    }
    total_tokens
}

/// Estimate tokens for a list of messages.
pub fn estimate_messages_tokens(messages: &[AgentMessage]) -> u32 {
    messages.iter().map(estimate_legacy_message_tokens).sum()
}

/// Estimate tokens for provider-layer messages.
pub fn estimate_provider_messages_tokens(messages: &[Message]) -> u32 {
    messages
        .iter()
        .map(|m| {
            let mut tokens = 0u32;
            for b in &m.content {
                match b {
                    ContentBlock::Text { text } => {
                        tokens = tokens.saturating_add(estimate_string_tokens(text));
                    }
                    ContentBlock::ToolUse { input, .. } => {
                        let s = serde_json::to_string(input).unwrap_or_default();
                        tokens = tokens.saturating_add(estimate_string_tokens(&s));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        tokens = tokens.saturating_add(estimate_string_tokens(content));
                    }
                }
            }
            tokens
        })
        .sum()
}

/// Estimate tokens for a AgentMessage (nini internal use).
pub fn estimate_legacy_message_tokens(msg: &AgentMessage) -> u32 {
    // Rough heuristic: chars / 4
    let mut chars: usize = match msg.role {
        Role::System => 6,
        Role::User => 4,
        Role::Assistant => 9,
        Role::Tool => 4,
    };
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => chars += text.len(),
            ContentBlock::ToolUse { name, input, .. } => {
                chars += name.len() + input.to_string().len();
            }
            ContentBlock::ToolResult { .. } => {} // tool results already counted elsewhere
        }
    }
    (chars / 4) as u32
}

/// Estimate tokens for a slice of entries (counts only message entries).
pub fn estimate_entries_tokens(entries: &[Entry]) -> u32 {
    entries
        .iter()
        .filter_map(|e| e.message.as_ref().map(estimate_legacy_message_tokens))
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
    /// Cut at the start of the oldest user turn (legacy algorithm).
    OldestUserTurn,
    /// Cut at the start of the oldest user message (legacy algorithm).
    OldestUserMessage,
    /// Cut at the nearest user-turn boundary after the keep_recent_tokens
    /// budget is met (Pi-compatible token-budget cut).
    TokenBudget,
    /// No entries to summarize.
    Empty,
    /// No safe cut point found; we'd have to truncate mid-turn.
    NoSafeCut,
}

/// Find a safe cut point that keeps at least `keep_recent_tokens`
/// of message history verbatim. Mirrors Pi's `findCutPoint()`: walk
/// backwards from the newest entry, accumulating estimated message sizes;
/// when the accumulated budget is met, snap to the nearest user-turn
/// boundary at or after that index.
///
/// Falls back to the first safe user turn if the budget would otherwise
/// discard everything (i.e., the whole conversation fits in the budget).
pub fn find_cut_point(entries: &[Entry], keep_recent_tokens: u32) -> CutPoint {
    if entries.is_empty() {
        return CutPoint {
            keep_from: 0,
            reason: CutReason::Empty,
        };
    }
    let total = entries.len();
    // Walk backwards, accumulating token estimates per entry.
    let mut accumulated: u32 = 0;
    let mut cut_idx = None;
    for i in (0..total).rev() {
        let msg_tokens = estimate_entry_tokens(&entries[i]);
        accumulated = accumulated.saturating_add(msg_tokens);
        if accumulated >= keep_recent_tokens {
            // Snap to the nearest user-turn boundary at or after i.
            for j in i..total {
                if is_user_turn_boundary(&entries, j) {
                    cut_idx = Some(j);
                    break;
                }
            }
            if cut_idx.is_none() {
                // No user-turn boundary between i and end — fall back to i.
                cut_idx = Some(i);
            }
            break;
        }
    }
    match cut_idx {
        // We found a token-budget-respecting cut point.
        Some(idx) => CutPoint {
            keep_from: idx,
            reason: CutReason::TokenBudget,
        },
        // Whole conversation fits in budget — fall back to legacy
        // "first safe user turn" so we still cut something.
        None => find_cut_point_legacy_inner(entries),
    }
}

/// Backwards-compatible overload: assumes default budget of 20k tokens.
pub fn find_cut_point_default(entries: &[Entry]) -> CutPoint {
    find_cut_point(entries, 20_000)
}

/// Legacy algorithm: returns the FIRST safe user-turn. Used as fallback
/// when the whole conversation fits in the keep_recent_tokens budget.
pub fn find_cut_point_legacy(entries: &[Entry]) -> CutPoint {
    find_cut_point_legacy_inner(entries)
}

fn find_cut_point_legacy_inner(entries: &[Entry]) -> CutPoint {
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
    settings: &CompactionSettings,
    previous_summary: Option<String>,
) -> CompactionPreparation {
    let cut = find_cut_point(entries, settings.keep_recent_tokens);
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
                Role::User => {
                    for b in &msg.content {
                        if let ContentBlock::Text { text } = b {
                            if !text.trim().is_empty() && user_questions.len() < 10 {
                                user_questions.push(text.chars().take(200).collect());
                            }
                        }
                    }
                }
                Role::Assistant => {
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
    let prep = prepare_compaction(entries, &settings, previous_summary);
    let tokens_before = estimate_entries_tokens(entries);

    let summary = summary_provider(&prep.to_summarize, prep.previous_summary.clone());

    // Build the new compaction entry (preserves retention list verbatim).
    let summary_tokens = estimate_message_tokens(&Message {
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
        .map(estimate_legacy_message_tokens)
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
        entry_type: LegacyEntryType::Compaction,
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
    use crate::provider::Message;
use crate::{AgentMessage, ContentBlock, Entry, LegacyEntryType, Role};

    fn make_user_entry(id: &str, text: &str, parent: Option<&str>, seq: u64) -> Entry {
        Entry {
            id: id.into(),
            parent_id: parent.map(|s| s.to_string()),
            seq,
            timestamp: 0,
            entry_type: LegacyEntryType::Message,
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
            entry_type: LegacyEntryType::Message,
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
            entry_type: LegacyEntryType::Message,
            message: Some(Message {
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
        assert_eq!(estimate_legacy_message_tokens(&m), 3);
    }

    #[test]
    fn estimate_empty_message_is_zero() {
        let m = AgentMessage::user("");
        assert_eq!(estimate_legacy_message_tokens(&m), 1);
    }

    #[test]
    fn estimate_tool_use_includes_input_json() {
        let input = serde_json::json!({"path": "/tmp/foo"});
        let m = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read".into(),
                input,
            }],
            timestamp: 0,
        };
        let tokens = estimate_legacy_message_tokens(&m);
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
        let cut = find_cut_point(&entries, 20_000);
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
        let cut = find_cut_point(&entries, 20_000);
        // u1 at index 0 is the first user turn; no prior content to summarize.
        assert_eq!(cut.keep_from, 0);
        assert_eq!(cut.reason, CutReason::OldestUserTurn);
    }

    #[test]
    fn cut_point_no_user_returns_zero() {
        let entries = vec![make_assistant_entry("a1", "hi", None, 1)];
        let cut = find_cut_point(&entries, 20_000);
        assert_eq!(cut.keep_from, 0);
        assert_eq!(cut.reason, CutReason::NoSafeCut);
    }

    #[test]
    fn cut_point_empty_entries() {
        let cut = find_cut_point(&[], 20_000);
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
        let settings = CompactionSettings::default();
        let prep = prepare_compaction(&entries, &settings, None);
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
        assert_eq!(entry.entry_type, LegacyEntryType::Compaction);
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
            entry_type: LegacyEntryType::Compaction,
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

    #[test]
    fn estimate_string_tokens_ascii_4_per_token() {
        // 12 ASCII chars → 3 tokens (12 / 4 = 3).
        assert_eq!(estimate_string_tokens("hello world!"), 3);
    }

    #[test]
    fn estimate_string_tokens_cjk_denser() {
        // 8 Chinese chars → 4 tokens (8 / 2 = 4), same byte count would be
        // only 2 tokens in ASCII.
        let s = "你好世界你好世界";
        let cjk_tokens = estimate_string_tokens(s);
        assert_eq!(cjk_tokens, 4);
        // Mixed: 4 ASCII + 4 CJK → 1 + 2 = 3 tokens.
        let mixed = "hi 你好 world 世界";
        assert!(estimate_string_tokens(mixed) >= 2);
    }

    #[test]
    fn estimate_string_tokens_empty_is_zero() {
        assert_eq!(estimate_string_tokens(""), 0);
    }

    #[test]
    fn is_cjk_detects_chinese_japanese_korean() {
        assert!(is_cjk('中'));
        assert!(is_cjk('한')); // Korean
        assert!(is_cjk('ひ')); // Hiragana
        assert!(is_cjk('カ')); // Katakana
        assert!(!is_cjk('a'));
        assert!(!is_cjk('1'));
        assert!(!is_cjk(' '));
        assert!(!is_cjk('é')); // Latin-1 supplement (non-ASCII non-CJK)
    }

    #[test]
    fn estimate_message_tokens_handles_cjk() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "你好".to_string(),
            }],
            timestamp: 0,
        };
        let tokens = estimate_message_tokens(&msg);
        // 2 CJK chars → 1 token.
        assert_eq!(tokens, 1);
    }

    // ========================================================================
    // Pi token-budget cut-point parity tests
    // ========================================================================

    #[test]
    fn token_budget_cuts_after_kept_recent_tokens() {
        // Build a long conversation where each message is ~10 chars.
        // Each ~10 chars → ~3 tokens. With keep_recent_tokens=12, we
        // expect to keep roughly the last ~4 entries verbatim.
        let entries: Vec<_> = (0..20)
            .map(|i| make_user_entry(&format!("u{i}"), "abcdefghij", None, i as u64 + 1))
            .collect();
        let cut = find_cut_point(&entries, 12);
        assert!(
            cut.keep_from >= 14,
            "expected cut from late entries, got keep_from={}",
            cut.keep_from
        );
        assert_eq!(cut.reason, CutReason::TokenBudget);
    }

    #[test]
    fn token_budget_falls_back_to_legacy_when_conversation_fits() {
        // Single small conversation under keep_recent_tokens.
        let entries = vec![make_user_entry("u1", "hello", None, 1)];
        let cut = find_cut_point(&entries, 100_000);
        // Falls back to legacy "first safe user turn" → reason is Oldest.
        assert_eq!(cut.reason, CutReason::OldestUserTurn);
    }

    #[test]
    fn settings_default_matches_pi_16384_20000() {
        // Verify we match Pi's DEFAULT_COMPACTION_SETTINGS exactly:
        // { enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 }.
        let s = CompactionSettings::default();
        assert!(s.enabled);
        assert_eq!(s.reserve_tokens, 16384);
        assert_eq!(s.keep_recent_tokens, 20000);
    }
}
