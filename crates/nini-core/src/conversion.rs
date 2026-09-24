//! v0.7.1 — single chokepoint for `entries::AgentMessage → provider::Message`.
//!
//! The wiki (`docs/spec-v0.85.1` §message perspective conversion) describes
//! a 7→3 conversion: `AgentMessage` (internal) → LLM view (user / assistant
//! / toolResult). nini's session `AgentMessage` enum has 10 variants (the 7
//! wiki variants + 3 session-persistence-only: BashExecution, BranchSummary,
//! CompactionSummary). All 7 internal-only / session-only variants are
//! **dropped** per the wiki's "dropped" rule.
//!
//! ## Why this module exists
//!
//! Before v0.7.1, three crates had independent implementations of the
//! 7→3 conversion:
//!
//! * `nini-session::convert_to_pi_message` (WRITE direction: provider →
//!   entries; had bugs using Custom variants for Assistant / Tool)
//! * `nini-tui::commands::entry_legacy_message` (READ direction:
//!   entries → provider; matched User/Assistant only, dropped everything
//!   else via `_ => None`)
//! * `nini-cli::main::cli_entry_legacy` (same as nini-tui, duplicate)
//!
//! The READ direction was incomplete per the wiki — ToolResult messages
//! in the session file became `None` instead of becoming `Role::Tool`
//! messages. Sessions saved by v0.6.x that contained tool calls were
//! effectively invisible to the agent loop after reload.
//!
//! This module is the **single source of truth** for the READ direction.
//! The WRITE direction stays in nini-session but uses the correct
//! Assistant / ToolResult variants now (no more `Custom("assistant")`).
//!
//! ## Function shape
//!
//! Two free functions:
//!
//! * `session_entry_to_llm_message(&SessionEntry) -> Option<Message>` —
//!   one entry, returns None when the entry has no LLM representation
//!   (model changes, custom entries, internal-only message variants).
//! * `default_session_to_llm(entries: &[AgentMessage]) -> Vec<Message>` —
//!   convenience wrapper over the above for batched loads.
//!
//! Both are panic-free (no `unwrap` on user data). Missing fields
//! (timestamp, content) fall back to safe defaults rather than panicking.

use crate::entries::{
    AgentMessage as PiMsg, ContentBlock as PiBlock, SessionEntry, StringOrContentBlocks,
};
use crate::provider::{ContentBlock, Message, Role};

/// Single-entry conversion: a `SessionEntry::Message` carrying an
/// `AgentMessage` becomes a `provider::Message` per the wiki table.
///
/// Returns `None` when the entry has no LLM representation:
///   * `SessionEntry` is not the message kind (e.g. ModelChange, Custom,
///     Compaction, BranchSummary entries are session metadata only).
///   * `AgentMessage` is one of the 6 internal-only / session-only
///     variants (BashExecution, BranchSummary, CompactionSummary,
///     Notification, UiMessage, AppMessage) — per the wiki, dropped.
///
/// Returns `Some(Message)` for the 4 wiki-visible variants:
///   * `User`       → `Role::User` (timestamp preserved)
///   * `Assistant`  → `Role::Assistant` (timestamp preserved)
///   * `ToolResult` → `Role::Tool` with a `ContentBlock::ToolResult`
///                    block carrying the tool_use_id, flattened
///                    content, and is_error flag.
///   * `Custom`     → `Role::User` (per wiki: "custom → user (重写)").
///                    We lose the `custom_type` discrimination, which
///                    is intentional — Custom in nini v0.6.x was used
///                    as a generic carrier and the model doesn't need
///                    the type to interpret the content.
pub fn session_entry_to_llm_message(entry: &SessionEntry) -> Option<Message> {
    let SessionEntry::Message(m) = entry else {
        return None;
    };
    session_message_to_llm(&m.message)
}

/// Batch version: walk all message entries and emit one
/// `provider::Message` per visible variant. Order preserved.
/// Internal-only entries (SessionEntry::ModelChange etc.) and
/// internal-only variants (Notification, UiMessage, AppMessage,
/// BashExecution, BranchSummary, CompactionSummary) are skipped.
pub fn default_session_to_llm(entries: &[PiMsg]) -> Vec<Message> {
    // We accept the public-facing session entries, not the full
    // SessionEntry enum, so this function is callable from the
    // session codec and from any extension that walks raw
    // entries::AgentMessage slices.
    let mut out = Vec::with_capacity(entries.len());
    for msg in entries {
        if let Some(m) = session_message_to_llm(msg) {
            out.push(m);
        }
    }
    out
}

/// Inner single-message conversion. Same semantics as
/// `session_entry_to_llm_message` but takes the inner
/// `entries::AgentMessage` directly.
pub fn session_message_to_llm(msg: &PiMsg) -> Option<Message> {
    match msg {
        PiMsg::User(u) => Some(Message {
            role: Role::User,
            content: user_content_to_blocks(&u.content),
            timestamp: u.timestamp,
        }),
        PiMsg::Assistant(a) => Some(Message {
            role: Role::Assistant,
            content: assistant_content_to_blocks(&a.content),
            timestamp: a.timestamp,
        }),
        PiMsg::ToolResult(t) => {
            // Wiki 7→3: toolResult → toolResult (provider's Role::Tool).
            // Flatten the entries::ContentBlock list into a single
            // Text content (concatenated) for the tool_use_id
            // payload, preserving is_error.
            let flattened = t
                .content
                .iter()
                .map(|b| match b {
                    PiBlock::Text { text } => text.clone(),
                    // Image/Image/ToolCall → empty text placeholder.
                    // Tool call args aren't useful in a tool result
                    // message; the model needs the result string.
                    PiBlock::Image { .. }
                    | PiBlock::Thinking { .. }
                    | PiBlock::ToolCall { .. } => String::new(),
                })
                .collect::<Vec<_>>()
                .join("");
            Some(Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: t.tool_call_id.clone(),
                    content: flattened,
                    is_error: t.is_error,
                }],
                timestamp: t.timestamp,
            })
        }
        PiMsg::Custom(c) => {
            // Wiki: custom → user (重写). We lose the custom_type
            // discrimination intentionally — see module docs.
            Some(Message {
                role: Role::User,
                content: user_content_to_blocks(&c.content),
                timestamp: c.timestamp,
            })
        }
        // 6 internal-only / session-only variants — drop.
        PiMsg::BashExecution(_)
        | PiMsg::BranchSummary(_)
        | PiMsg::CompactionSummary(_)
        | PiMsg::Notification(_)
        | PiMsg::UiMessage(_)
        | PiMsg::AppMessage(_) => None,
    }
}

/// Convert `entries::StringOrContentBlocks` into a Vec of
/// `provider::ContentBlock`. Used by User and Custom messages.
/// Loses the String vs Blocks distinction but preserves the text
/// payload either way.
fn user_content_to_blocks(content: &StringOrContentBlocks) -> Vec<ContentBlock> {
    match content {
        StringOrContentBlocks::String(s) => vec![ContentBlock::Text { text: s.clone() }],
        StringOrContentBlocks::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                PiBlock::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
                // Image/Thinking/ToolCall → empty text placeholders,
                // matching the previous nini-tui/nini-cli behavior.
                PiBlock::Image { .. }
                | PiBlock::Thinking { .. }
                | PiBlock::ToolCall { .. } => None,
            })
            .collect(),
    }
}

/// Convert `entries::AssistantMessage.content` into
/// `provider::ContentBlock` list. Text blocks pass through;
/// ToolCall blocks become ToolUse; Image/Thinking → empty text.
fn assistant_content_to_blocks(blocks: &[PiBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|b| match b {
            PiBlock::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
            PiBlock::ToolCall {
                id,
                name,
                arguments,
            } => Some(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: arguments.clone(),
            }),
            PiBlock::Image { .. } | PiBlock::Thinking { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{
        AssistantMessage, BashExecutionMessage, BranchSummaryMessage,
        CompactionSummaryMessage, CustomMessage, NotificationMessage, SessionMessageEntry,
        ToolResultMessage, UiMessage, AppMessage, UserMessage,
    };

    fn user_msg(text: &str, ts: i64) -> PiMsg {
        PiMsg::User(UserMessage {
            content: StringOrContentBlocks::String(text.into()),
            timestamp: ts,
        })
    }

    fn assistant_msg(text: &str, ts: i64) -> PiMsg {
        PiMsg::Assistant(AssistantMessage {
            content: vec![PiBlock::Text { text: text.into() }],
            api: "anthropic".into(),
            provider: "anthropic".into(),
            model: "claude-3".into(),
            usage: Default::default(),
            stop_reason: crate::entries::StopReason::Stop,
            error_message: None,
            timestamp: ts,
        })
    }

    fn tool_result_msg(id: &str, content: &str, is_err: bool, ts: i64) -> PiMsg {
        PiMsg::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "bash".into(),
            content: vec![PiBlock::Text { text: content.into() }],
            details: None,
            usage: None,
            is_error: is_err,
            timestamp: ts,
        })
    }

    fn custom_msg(text: &str, ts: i64) -> PiMsg {
        PiMsg::Custom(CustomMessage {
            custom_type: "my_ext".into(),
            content: StringOrContentBlocks::String(text.into()),
            display: true,
            details: None,
            timestamp: ts,
        })
    }

    fn notification_msg(kind: &str) -> PiMsg {
        PiMsg::Notification(NotificationMessage {
            kind: kind.into(),
            data: None,
            timestamp: 0,
        })
    }

    fn ui_msg(component: &str) -> PiMsg {
        PiMsg::UiMessage(UiMessage {
            component: component.into(),
            props: None,
            timestamp: 0,
        })
    }

    fn app_msg(source: &str) -> PiMsg {
        PiMsg::AppMessage(AppMessage {
            source: source.into(),
            payload: None,
            timestamp: 0,
        })
    }

    fn bash_exec_msg() -> PiMsg {
        PiMsg::BashExecution(BashExecutionMessage {
            command: "ls".into(),
            output: "x".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
        })
    }

    fn branch_summary_msg() -> PiMsg {
        PiMsg::BranchSummary(BranchSummaryMessage {
            summary: "x".into(),
            read_files: vec![],
            modified_files: vec![],
        })
    }

    fn compaction_msg() -> PiMsg {
        PiMsg::CompactionSummary(CompactionSummaryMessage {
            summary: "x".into(),
            details: None,
        })
    }

    fn session_message_entry(msg: PiMsg, id: &str) -> SessionEntry {
        SessionEntry::Message(SessionMessageEntry {
            id: id.into(),
            parent_id: None,
            timestamp: "2026-09-23T00:00:00.000Z".into(),
            message: msg,
        })
    }

    // -- The 4 visible variants (per wiki) --

    #[test]
    fn user_becomes_role_user() {
        let m = session_message_to_llm(&user_msg("hi", 100)).unwrap();
        assert_eq!(m.role, Role::User);
        assert_eq!(m.timestamp, 100);
        assert_eq!(m.content.len(), 1);
        match &m.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hi"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn assistant_becomes_role_assistant() {
        let m = session_message_to_llm(&assistant_msg("ok", 200)).unwrap();
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.timestamp, 200);
        assert_eq!(m.content.len(), 1);
    }

    #[test]
    fn tool_result_becomes_role_tool_with_tool_use_id() {
        let m = session_message_to_llm(&tool_result_msg("tc-1", "ls output", false, 300)).unwrap();
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.timestamp, 300);
        assert_eq!(m.content.len(), 1);
        match &m.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tc-1");
                assert_eq!(content, "ls output");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_result_is_error_flag_preserved() {
        let m = session_message_to_llm(&tool_result_msg("tc-2", "boom", true, 0)).unwrap();
        match &m.content[0] {
            ContentBlock::ToolResult { is_error, .. } => assert!(*is_error),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn custom_becomes_user_per_wiki() {
        let m = session_message_to_llm(&custom_msg("extension payload", 400)).unwrap();
        assert_eq!(m.role, Role::User);
        match &m.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "extension payload"),
            _ => panic!("expected Text"),
        }
    }

    // -- The 6 dropped variants (per wiki + nini extensions) --

    #[test]
    fn notification_dropped() {
        assert!(session_message_to_llm(&notification_msg("compaction.started")).is_none());
    }

    #[test]
    fn ui_message_dropped() {
        assert!(session_message_to_llm(&ui_msg("status-bar")).is_none());
    }

    #[test]
    fn app_message_dropped() {
        assert!(session_message_to_llm(&app_msg("extension:foo")).is_none());
    }

    #[test]
    fn bash_execution_dropped() {
        // nini's session-persistence variant — wiki doesn't list it
        // explicitly but it's a session-internal record, not LLM
        // context. Per "drop internal-only" rule, it goes.
        assert!(session_message_to_llm(&bash_exec_msg()).is_none());
    }

    #[test]
    fn branch_summary_dropped() {
        assert!(session_message_to_llm(&branch_summary_msg()).is_none());
    }

    #[test]
    fn compaction_summary_dropped() {
        assert!(session_message_to_llm(&compaction_msg()).is_none());
    }

    // -- SessionEntry wrapper --

    #[test]
    fn session_message_entry_wraps_correctly() {
        let entry = session_message_entry(user_msg("hi", 1), "e1");
        let m = session_entry_to_llm_message(&entry).unwrap();
        assert_eq!(m.role, Role::User);
    }

    #[test]
    fn non_message_session_entries_return_none() {
        use crate::entries::ModelChangeEntry;
        let mc = SessionEntry::ModelChange(ModelChangeEntry {
            id: "m1".into(),
            parent_id: None,
            timestamp: "2026-09-23T00:00:00.000Z".into(),
            provider: "anthropic".into(),
            model_id: "claude-3".into(),
        });
        assert!(session_entry_to_llm_message(&mc).is_none());

        use crate::entries::LabelEntry;
        let label = SessionEntry::Label(LabelEntry {
            id: "l1".into(),
            parent_id: None,
            timestamp: "2026-09-23T00:00:00.000Z".into(),
            target_id: "t1".into(),
            label: Some("important".into()),
        });
        assert!(session_entry_to_llm_message(&label).is_none());
    }

    // -- Batch conversion --

    #[test]
    fn batch_preserves_order_and_skips_dropped() {
        let entries = vec![
            user_msg("a", 1),
            notification_msg("x"),
            assistant_msg("b", 2),
            ui_msg("c"),
            tool_result_msg("tc", "d", false, 3),
            app_msg("e"),
            custom_msg("f", 4),
        ];
        let out = default_session_to_llm(&entries);
        // Visible: User(a), Assistant(b), Tool(d), User(f) = 4 messages.
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[3].role, Role::User);
    }

    #[test]
    fn batch_empty_input_returns_empty() {
        assert!(default_session_to_llm(&[]).is_empty());
    }

    // -- Edge cases --

    #[test]
    fn user_with_blocks_format_works() {
        let msg = PiMsg::User(UserMessage {
            content: StringOrContentBlocks::Blocks(vec![
                PiBlock::Text { text: "first".into() },
                PiBlock::Text { text: "second".into() },
            ]),
            timestamp: 0,
        });
        let m = session_message_to_llm(&msg).unwrap();
        assert_eq!(m.content.len(), 2);
    }

    #[test]
    fn user_with_image_block_skips_it() {
        let msg = PiMsg::User(UserMessage {
            content: StringOrContentBlocks::Blocks(vec![
                PiBlock::Text { text: "ok".into() },
                PiBlock::Image { image: "data:...".into() },
            ]),
            timestamp: 0,
        });
        let m = session_message_to_llm(&msg).unwrap();
        // Image is dropped, only Text remains.
        assert_eq!(m.content.len(), 1);
    }

    #[test]
    fn assistant_with_tool_call_becomes_tool_use() {
        let msg = PiMsg::Assistant(AssistantMessage {
            content: vec![
                PiBlock::Text { text: "I'll run a command".into() },
                PiBlock::ToolCall {
                    id: "tc-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                },
            ],
            api: "anthropic".into(),
            provider: "anthropic".into(),
            model: "claude-3".into(),
            usage: Default::default(),
            stop_reason: crate::entries::StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
        });
        let m = session_message_to_llm(&msg).unwrap();
        assert_eq!(m.content.len(), 2);
        match &m.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "tc-1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn tool_result_with_multiple_blocks_concatenates_text() {
        let msg = PiMsg::ToolResult(ToolResultMessage {
            tool_call_id: "tc-1".into(),
            tool_name: "bash".into(),
            content: vec![
                PiBlock::Text { text: "first\n".into() },
                PiBlock::Text { text: "second".into() },
            ],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        });
        let m = session_message_to_llm(&msg).unwrap();
        match &m.content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "first\nsecond");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    /// Round-trip: a ToolResult written to a session file by
    /// nini-session's `convert_to_pi_message` should reload back
    /// into a Role::Tool provider message. This is the test that
    /// catches the v0.6.1 bug where ToolResult became _ => None
    /// in the read direction.
    #[test]
    fn tool_result_round_trip() {
        // Take a ToolResult AgentMessage and confirm the
        // chokepoint emits Role::Tool.
        let pi = PiMsg::ToolResult(ToolResultMessage {
            tool_call_id: "tc-xyz".into(),
            tool_name: "bash".into(),
            content: vec![PiBlock::Text { text: "exit code 0\n".into() }],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 12345,
        });
        let back = session_message_to_llm(&pi).unwrap();
        assert_eq!(back.role, Role::Tool);
        assert_eq!(back.timestamp, 12345);
        match &back.content[0] {
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "tc-xyz");
                assert_eq!(content, "exit code 0\n");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }
}