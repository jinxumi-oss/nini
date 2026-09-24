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

use crate::entries::SessionEntry;
use crate::entries::StringOrContentBlocks;
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

// =========================================================================
// Conversation serialization (Pi-compatible)
//
// Mirrors spec `packages/agent/src/harness/compaction/utils.ts`:
//   - `serializeConversation(messages)` — turn a `Message[]` stream into
//     a flat text suitable for an LLM summarizer prompt.
//   - `serializeSessionEntries(entries)` — same, but operates on the
//     full session entry list (which carries `Thinking` + `ToolCall`
//     blocks via `entries::ContentBlock`).
//
// Format produced:
//
//   [User]: <text>
//   [Assistant thinking]: <joined thinking blocks>
//   [Assistant]: <text blocks joined>
//   [Assistant tool calls]: name(arg=val, arg=val); name(...)
//   [Tool result]: <truncated to 2000 chars>
//
// Sections are joined by blank lines. The output is used by the LLM
// summarizer in `nini_ai::summarizer::make_llm_summarizer` to feed the
// conversation to the model in Pi's serialization format.

/// Maximum characters to keep from a single tool result before
/// truncation. Matches Pi's `TOOL_RESULT_MAX_CHARS` constant.
pub const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Extract concatenated text from a `provider::ContentBlock` slice.
/// Used as the equivalent of Pi's `contentText()` helper. Strips
/// leading/trailing whitespace before checking emptiness so that
/// messages whose payload is only whitespace (Pi's [User] empty
/// guard, etc.) are correctly dropped.
fn content_text_from_provider(blocks: &[ContentBlock], fallback: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for b in blocks {
        if let ContentBlock::Text { text } = b {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(text.as_str());
            }
        }
    }
    if parts.is_empty() {
        fallback.to_string()
    } else {
        parts.join("")
    }
}

/// Best-effort JSON serialization for tool-call argument values.
/// Returns `"undefined"` if `serde_json` returns `None` and
/// `"[unserializable]"` if serialization throws.
fn safe_json_stringify(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// Truncate a tool result body to at most `max_chars` characters. If
/// truncated, emit a `[... N more characters truncated]` marker.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated = text.len() - max_chars;
    let mut out = String::with_capacity(max_chars + 64);
    out.push_str(&text[..max_chars]);
    out.push_str("\n\n[... ");
    out.push_str(&truncated.to_string());
    out.push_str(" more characters truncated]");
    out
}

/// Render tool-call argument values as `key=val, key=val` (Pi format).
fn format_tool_call_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{}={}", k, safe_json_stringify(v)))
            .collect::<Vec<_>>()
            .join(", "),
        // Pi accepts Record<string, unknown>; fall back to JSON.
        _ => safe_json_stringify(args),
    }
}

/// Serialize a single tool-use invocation as `name(args)`.
fn format_tool_call(name: &str, args: &serde_json::Value) -> String {
    format!("{}({})", name, format_tool_call_args(args))
}

/// Serialize a slice of `provider::Message` into Pi's compact format
/// suitable for LLM summarizer prompts.
///
/// Empty content blocks are dropped; sections are joined by blank
/// lines. The output is byte-for-byte compatible with what Pi's
/// `serializeConversation(messages)` produces.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::User => {
                let content = content_text_from_provider(&msg.content, "");
                if !content.is_empty() {
                    parts.push(format!("[User]: {content}"));
                }
            }
            Role::Assistant => {
                let mut tool_calls: Vec<String> = Vec::new();

                for block in &msg.content {
                    if let ContentBlock::ToolUse { name, input, .. } = block {
                        tool_calls.push(format_tool_call(name, input));
                    }
                }

                // Note: nini's `provider::ContentBlock` doesn't currently
                // carry a `thinking` variant; that lives on the
                // entries-side type (see `serialize_session_entries`
                // below). If a future variant is added here, the
                // matching branch should mirror Pi's behavior.

                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }

                let assistant_text = content_text_from_provider(&msg.content, "");
                if !assistant_text.is_empty() {
                    parts.push(format!("[Assistant]: {assistant_text}"));
                }
            }
            Role::Tool => {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { content, .. } = block {
                        if !content.is_empty() {
                            parts.push(format!(
                                "[Tool result]: {}",
                                truncate_for_summary(content, TOOL_RESULT_MAX_CHARS)
                            ));
                        }
                    }
                }
            }
            Role::System => {
                // Pi skips system messages in conversation serialization.
            }
        }
    }

    parts.join("\n\n")
}

/// Serialize a slice of `entries::SessionEntry` into Pi's compact
/// format. Use this when feeding session-history-derived data to the
/// LLM summarizer. `SessionEntry::Message` carries the richer
/// `entries::ContentBlock` (Text, Image, Thinking, ToolCall), which
/// lets us emit `[Assistant thinking]` and `[Assistant tool calls]`
/// sections — both of which are missing from the `provider::Message`
/// path.
///
/// `Entry` (the legacy in-memory form) holds `provider::Message`
/// content, which has neither Thinking nor ToolCall variants. For that
/// type, callers should convert to `SessionEntry` first or use
/// `serialize_conversation` directly.
pub fn serialize_session_entries(entries: &[SessionEntry]) -> String {
    use crate::entries::{AgentMessage as EAM, ContentBlock as EntriesBlock};

    let mut parts: Vec<String> = Vec::new();

    for entry in entries {
        let SessionEntry::Message(m) = entry else { continue };
        match &m.message {
            EAM::User(u) => {
                let mut text_buf = String::new();
                match &u.content {
                    StringOrContentBlocks::String(s) => text_buf.push_str(s),
                    StringOrContentBlocks::Blocks(blocks) => {
                        for block in blocks {
                            if let EntriesBlock::Text { text } = block {
                                text_buf.push_str(text);
                            }
                        }
                    }
                }
                if !text_buf.is_empty() {
                    parts.push(format!("[User]: {text_buf}"));
                }
            }
            EAM::Assistant(a) => {
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<String> = Vec::new();
                let mut text_parts: Vec<String> = Vec::new();

                for block in &a.content {
                    match block {
                        EntriesBlock::Thinking { thinking } => {
                            thinking_parts.push(thinking.clone());
                        }
                        EntriesBlock::ToolCall { name, arguments, .. } => {
                            tool_calls.push(format_tool_call(name, arguments));
                        }
                        EntriesBlock::Text { text } => {
                            if !text.is_empty() {
                                text_parts.push(text.clone());
                            }
                        }
                        EntriesBlock::Image { .. } => {
                            // Pi drops images in conversation serialization.
                        }
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!("[Assistant thinking]: {}", thinking_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("")));
                }
            }
            EAM::ToolResult(t) => {
                let mut buf = String::new();
                for block in &t.content {
                    if let EntriesBlock::Text { text } = block {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(text);
                    }
                }
                if !buf.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&buf, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            EAM::BashExecution(b) => {
                if !b.output.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&b.output, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            EAM::Custom(_)
            | EAM::BranchSummary(_)
            | EAM::CompactionSummary(_)
            | EAM::Notification(_)
            | EAM::UiMessage(_)
            | EAM::AppMessage(_) => {
                // Pi skips custom messages, branch summaries,
                // compaction summaries, and v0.7 internal-only
                // messages (Notification / UiMessage / AppMessage)
                // in conversation serialization; each carries its
                // own summary field or is purely metadata.
            }
        }
    }

    parts.join("\n\n")
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

    // =================================================================
    // serialize_conversation / serialize_session_entries
    //
    // These mirror Pi's `serializeConversation` in
    // `packages/agent/src/harness/compaction/utils.ts`. We verify the
    // emitted format byte-for-byte against Pi's expected output.
    // =================================================================

    #[test]
    fn truncate_for_summary_basic() {
        // 10 chars input, max 5 → first 5 chars + truncation marker
        // (truncated count = 10 - 5 = 5).
        let s = "x".repeat(10);
        let out = truncate_for_summary(&s, 5);
        assert_eq!(out, "xxxxx\n\n[... 5 more characters truncated]");
    }

    #[test]
    fn truncate_for_summary_under_max() {
        let s = "x".repeat(3);
        assert_eq!(truncate_for_summary(&s, 5), "xxx");
    }

    #[test]
    fn truncate_for_summary_marker() {
        let s = "x".repeat(100);
        let truncated = truncate_for_summary(&s, 10);
        assert!(truncated.starts_with("xxxxxxxxxx\n\n[... 90 more characters truncated]"));
    }

    #[test]
    fn truncate_for_summary_no_truncation() {
        let s = "short text";
        assert_eq!(truncate_for_summary(s, 100), "short text");
    }

    #[test]
    fn format_tool_call_args_basic() {
        // serde_json::Map is BTreeMap by default → keys are sorted
        // alphabetically. Document this ordering so callers know what
        // to expect.
        let args = serde_json::json!({"path": "/tmp/foo.rs", "limit": 10});
        let out = format_tool_call_args(&args);
        // Sorted keys: limit, path.
        assert_eq!(out, "limit=10, path=\"/tmp/foo.rs\"");
    }

    #[test]
    fn format_tool_call_args_handles_nested() {
        let args = serde_json::json!({"nested": {"k": "v"}});
        let out = format_tool_call_args(&args);
        assert_eq!(out, "nested={\"k\":\"v\"}");
    }

    #[test]
    fn safe_json_stringify_falls_back_on_error() {
        // serde_json::Value can't actually fail to serialize, but the
        // signature mirrors Pi's safeJsonStringify for forward
        // compatibility.
        let v = serde_json::json!("hello");
        assert_eq!(safe_json_stringify(&v), "\"hello\"");
    }

    #[test]
    fn serialize_conversation_user_only() {
        let msgs = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "refactor auth".to_string(),
            }],
            timestamp: 0,
        }];
        let s = serialize_conversation(&msgs);
        assert_eq!(s, "[User]: refactor auth");
    }

    #[test]
    fn serialize_conversation_assistant_text_and_tools() {
        let msgs = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "show me login.rs".to_string(),
                }],
                timestamp: 0,
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path": "/src/login.rs"}),
                    },
                    ContentBlock::Text {
                        text: "Found JWT-based session handling.".to_string(),
                    },
                ],
                timestamp: 1,
            },
        ];
        let s = serialize_conversation(&msgs);
        assert!(s.contains("[User]: show me login.rs"));
        // Tool call uses name(args) format with JSON-stringified values.
        assert!(
            s.contains("[Assistant tool calls]: read(path=\"/src/login.rs\")"),
            "got: {s}"
        );
        assert!(s.contains("[Assistant]: Found JWT-based session handling."));
        // Sections separated by blank lines.
        assert!(s.contains("\n\n"));
    }

    #[test]
    fn serialize_conversation_truncates_tool_results() {
        let big = "x".repeat(3000);
        let msgs = vec![Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: big,
                is_error: false,
            }],
            timestamp: 0,
        }];
        let s = serialize_conversation(&msgs);
        assert!(s.contains("[Tool result]: "));
        assert!(s.contains("[... 1000 more characters truncated]"));
        // Truncated content should not contain the original 3000 chars.
        assert!(s.len() < 2200);
    }

    #[test]
    fn serialize_conversation_drops_empty_sections() {
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: " ".to_string(),
            }],
            timestamp: 0,
        }];
        let s = serialize_conversation(&msgs);
        assert!(s.is_empty(), "empty assistant text should be dropped, got: {s:?}");
    }

    #[test]
    fn serialize_conversation_system_messages_skipped() {
        let msgs = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "secret system prompt".to_string(),
                }],
                timestamp: 0,
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 1,
            },
        ];
        let s = serialize_conversation(&msgs);
        assert!(!s.contains("secret system prompt"));
        assert_eq!(s, "[User]: hi");
    }

    #[test]
    fn serialize_conversation_full_pi_shape() {
        // Reproduce the exact shape Pi emits for a 3-message exchange.
        let msgs = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "refactor auth".to_string(),
                }],
                timestamp: 0,
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path": "/auth.rs"}),
                    },
                ],
                timestamp: 1,
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "I see JWT usage.".to_string(),
                }],
                timestamp: 2,
            },
        ];
        let s = serialize_conversation(&msgs);
        // Expected Pi format:
        // [User]: refactor auth
        //
        // [Assistant tool calls]: read(path="/auth.rs")
        //
        // [Assistant]: I see JWT usage.
        let expected = "[User]: refactor auth\n\n[Assistant tool calls]: read(path=\"/auth.rs\")\n\n[Assistant]: I see JWT usage.";
        assert_eq!(s, expected);
    }

    // -----------------------------------------------------------------
    // serialize_session_entries
    // -----------------------------------------------------------------

    #[test]
    fn serialize_session_entries_thinking_and_tool_call() {
        use crate::entries::AgentMessage as EAM;
        use crate::entries::{AssistantMessage, SessionEntry, SessionMessageEntry, StopReason, Usage};

        let entries = vec![SessionEntry::Message(SessionMessageEntry {
            id: "m1".into(),
            parent_id: None,
            timestamp: "2026-09-15T00:00:00Z".into(),
            message: EAM::Assistant(AssistantMessage {
                content: vec![
                    crate::entries::ContentBlock::Thinking {
                        thinking: "User wants JWT removed.".into(),
                    },
                    crate::entries::ContentBlock::ToolCall {
                        id: "t1".into(),
                        name: "edit".into(),
                        arguments: serde_json::json!({"path": "/a.rs", "oldText": "x", "newText": "y"}),
                    },
                    crate::entries::ContentBlock::Text {
                        text: "Done.".into(),
                    },
                ],
                api: "anthropic".into(),
                provider: "anthropic".into(),
                model: "claude".into(),
                usage: Usage {
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 0,
                    cost: crate::entries::Cost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                        total: 0.0,
                    },
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            }),
        })];

        let s = serialize_session_entries(&entries);
        assert!(s.contains("[Assistant thinking]: User wants JWT removed."));
        // Keys are alphabetized by serde_json's BTreeMap-backed Map.
        assert!(s.contains("[Assistant tool calls]: edit(newText=\"y\", oldText=\"x\", path=\"/a.rs\")"));
        assert!(s.contains("[Assistant]: Done."));
    }

    #[test]
    fn serialize_session_entries_user_string_and_blocks() {
        use crate::entries::AgentMessage as EAM;
        use crate::entries::{SessionEntry, SessionMessageEntry, UserMessage};
        use crate::entries::StringOrContentBlocks;

        let entries = vec![
            SessionEntry::Message(SessionMessageEntry {
                id: "u1".into(),
                parent_id: None,
                timestamp: "t".into(),
                message: EAM::User(UserMessage {
                    content: StringOrContentBlocks::String("plain user msg".into()),
                    timestamp: 0,
                }),
            }),
        ];

        let s = serialize_session_entries(&entries);
        assert_eq!(s, "[User]: plain user msg");
    }

    #[test]
    fn serialize_session_entries_skips_non_message() {
        use crate::entries::{ModelChangeEntry, SessionEntry};

        let entries = vec![SessionEntry::ModelChange(ModelChangeEntry {
            id: "x".into(),
            parent_id: None,
            timestamp: "t".into(),
            provider: "anthropic".into(),
            model_id: "claude".into(),
        })];

        let s = serialize_session_entries(&entries);
        assert!(s.is_empty());
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

#[cfg(test)]
mod demo {
    use super::*;
    use crate::entries::{
        AgentMessage as EAM, AssistantMessage, SessionEntry, SessionMessageEntry,
        StopReason as ESR, StringOrContentBlocks, ToolResultMessage, Usage, UserMessage,
    };
    use crate::entries::ContentBlock as ECB;
    use crate::entries::Cost as ECost;

    #[test]
    fn demo_real_world_pi_format() {
        let entries = vec![
            SessionEntry::Message(SessionMessageEntry {
                id: "u1".into(),
                parent_id: None,
                timestamp: "t".into(),
                message: EAM::User(UserMessage {
                    content: StringOrContentBlocks::String(
                        "用 sqlx 写一个 user repository，不要用 diesel".into(),
                    ),
                    timestamp: 0,
                }),
            }),
            SessionEntry::Message(SessionMessageEntry {
                id: "a1".into(),
                parent_id: Some("u1".into()),
                timestamp: "t".into(),
                message: EAM::Assistant(AssistantMessage {
                    content: vec![
                        ECB::Thinking {
                            thinking: "User wants sqlx not diesel, prefers t_ prefix on tables."
                                .into(),
                        },
                        ECB::ToolCall {
                            id: "t1".into(),
                            name: "edit".into(),
                            arguments: serde_json::json!({
                                "path": "/src/db/users.rs",
                                "oldText": "diesel",
                                "newText": "sqlx",
                            }),
                        },
                    ],
                    api: "anthropic".into(),
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    usage: Usage {
                        input: 100,
                        output: 50,
                        cache_read: 0,
                        cache_write: 0,
                        total_tokens: 150,
                        cost: ECost {
                            input: 0.0,
                            output: 0.0,
                            cache_read: 0.0,
                            cache_write: 0.0,
                            total: 0.0,
                        },
                    },
                    stop_reason: ESR::ToolUse,
                    error_message: None,
                    timestamp: 5,
                }),
            }),
            SessionEntry::Message(SessionMessageEntry {
                id: "tr1".into(),
                parent_id: Some("a1".into()),
                timestamp: "t".into(),
                message: EAM::ToolResult(ToolResultMessage {
                    tool_call_id: "t1".into(),
                    tool_name: "edit".into(),
                    content: vec![ECB::Text {
                        text: "Successfully replaced 1 block(s).".into(),
                    }],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: 6,
                }),
            }),
        ];

        let s = serialize_session_entries(&entries);
        eprintln!("\n--- DEMO: real-world Pi-format conversation serialization ---\n{s}\n--- END ---\n");
        // Sanity assertions
        assert!(s.contains("[User]: 用 sqlx"));
        assert!(s.contains("[Assistant thinking]: User wants sqlx"));
        assert!(s.contains("[Assistant tool calls]: edit("));
        assert!(s.contains("[Tool result]: Successfully replaced"));
    }
}
