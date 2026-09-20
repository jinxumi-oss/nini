//! nini-core — agent loop, message types, entry types, provider + tool abstractions.

use serde::{Deserialize, Serialize};

pub mod agent;

pub mod entries;
pub mod model_runtime;
pub mod overflow;
pub mod project_trust;
pub mod compaction;
pub mod prompt_template;
pub mod provider;
pub mod settings;
pub mod skills;
pub mod tool;

pub use agent::{AbortHandle, Agent, AgentError, AgentEvent, RunConfig};
pub use entries::{
    AssistantMessage, BranchSummaryEntry, BranchSummaryMessage, CompactionEntry,
    CompactionSummaryMessage, ContentBlock as PiContentBlock, Cost, CustomEntry, CustomMessage,
    CustomMessageEntry, EntryType, LabelEntry, ModelChangeEntry, SessionEntry, SessionInfoEntry,
    SessionMessageEntry, StopReason, StringOrContentBlocks, ThinkingLevelChangeEntry,
    ToolResultMessage, Usage, UserMessage,
};
pub use provider::{
    ContentBlock, Message as ProviderMessage, Provider, ProviderError, Request, Role,
    StreamEvent, ToolCall, ToolResult, Usage as ProviderUsage,
};

/// Legacy alias for `Message` — keeps existing call sites working.

pub type LegacyAgentMessage = AgentMessage;
pub type LegacyContentBlock = ContentBlock;

/// Library version, mirrors workspace version.
pub use compaction::{
    CompactionOutput, CompactionPreparation, CompactionSettings, CutPoint, CutReason, compact,
    estimate_entries_tokens, estimate_message_tokens, find_cut_point, generate_local_summary,
    make_compaction_entry, prepare_compaction, should_compact,
};
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Legacy entry type used internally. nini-session converts to/from SessionEntry
/// when reading/writing JSONL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub seq: u64,
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub entry_type: LegacyEntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<AgentMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyEntryType {
    Message,
    Compaction,
    BranchSummary,
    Custom,
}

impl Entry {
    pub fn message(
        id: impl Into<String>,
        parent_id: Option<String>,
        seq: u64,
        msg: AgentMessage,
    ) -> Self {
        Self {
            id: id.into(),
            parent_id,
            seq,
            timestamp: chrono::Utc::now().timestamp_millis(),
            entry_type: LegacyEntryType::Message,
            message: Some(msg),
            summary: None,
            from_id: None,
            custom_type: None,
            data: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid entry: {0}")]
    InvalidEntry(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl AgentMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Conversion: legacy Entry ↔ pi-compatible SessionEntry.
impl From<Entry> for SessionEntry {
    fn from(e: Entry) -> Self {
        let ts_iso = chrono::DateTime::from_timestamp_millis(e.timestamp)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string());
        match e.entry_type {
            LegacyEntryType::Message => {
                let msg = e.message.unwrap_or_else(|| AgentMessage::user(""));
                let blocks: Vec<PiContentBlock> = msg
                    .content
                    .into_iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => PiContentBlock::Text { text },
                        ContentBlock::ToolUse { id, name, input } => PiContentBlock::ToolCall {
                            id,
                            name,
                            arguments: input,
                        },
                        ContentBlock::ToolResult { tool_use_id, content, .. } => {
                            PiContentBlock::Text { text: format!("[tool {}]: {}", tool_use_id, content) }
                        }
                    })
                    .collect();
                let pi_msg = match msg.role {
                    Role::User => entries::AgentMessage::User(UserMessage {
                        content: StringOrContentBlocks::Blocks(blocks),
                        timestamp: msg.timestamp,
                    }),
                    Role::Assistant => entries::AgentMessage::Assistant(AssistantMessage {
                        content: blocks,
                        api: "unknown".to_string(),
                        provider: "unknown".to_string(),
                        model: "unknown".to_string(),
                        usage: Usage::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp: msg.timestamp,
                    }),
                    Role::System => entries::AgentMessage::Custom(CustomMessage {
                        custom_type: "system".to_string(),
                        content: StringOrContentBlocks::Blocks(blocks),
                        display: false,
                        details: None,
                        timestamp: msg.timestamp,
                    }),
                    Role::Tool => entries::AgentMessage::ToolResult(ToolResultMessage {
                        tool_call_id: "unknown".to_string(),
                        tool_name: "unknown".to_string(),
                        content: blocks,
                        details: None,
                        usage: None,
                        is_error: false,
                        timestamp: msg.timestamp,
                    }),
                };
                SessionEntry::Message(SessionMessageEntry {
                    id: e.id,
                    parent_id: e.parent_id,
                    timestamp: ts_iso,
                    message: pi_msg,
                })
            }
            LegacyEntryType::BranchSummary => {
                SessionEntry::BranchSummary(BranchSummaryEntry {
                    id: e.id,
                    parent_id: e.parent_id,
                    timestamp: ts_iso,
                    from_id: e.from_id.unwrap_or_default(),
                    summary: e.summary.unwrap_or_default(),
                    usage: None,
                    details: e.data,
                    from_hook: None,
                })
            }
            LegacyEntryType::Compaction => SessionEntry::Compaction(CompactionEntry {
                id: e.id,
                parent_id: e.parent_id,
                timestamp: ts_iso,
                summary: e.summary.unwrap_or_default(),
                tokens_before: None,
                retained_tail: None,
                first_kept_entry_id: None,
                details: e.data,
                from_hook: None,
                usage: None,
            }),
            LegacyEntryType::Custom => SessionEntry::Custom(CustomEntry {
                id: e.id,
                parent_id: e.parent_id,
                timestamp: ts_iso,
                custom_type: e.custom_type.unwrap_or_default(),
                data: e.data,
            }),
        }
    }
}

impl From<SessionEntry> for Entry {
    fn from(s: SessionEntry) -> Self {
        let ts_ms = |iso: &str| -> i64 {
            chrono::DateTime::parse_from_rfc3339(iso)
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0)
        };
        match s {
            SessionEntry::Message(m) => {
                let ts = ts_ms(&m.timestamp);
                let (role, blocks) = match m.message {
                    entries::AgentMessage::User(u) => {
                        let blocks = match u.content {
                            StringOrContentBlocks::String(s) => {
                                vec![ContentBlock::Text { text: s }]
                            }
                            StringOrContentBlocks::Blocks(bs) => bs
                                .into_iter()
                                .map(legacy_from_block)
                                .collect(),
                        };
                        (Role::User, blocks)
                    }
                    entries::AgentMessage::Assistant(a) => {
                        let blocks = a.content.into_iter().map(legacy_from_block).collect();
                        (Role::Assistant, blocks)
                    }
                    entries::AgentMessage::ToolResult(_) => (Role::Tool, vec![]),
                    _ => (Role::User, vec![]),
                };
                Entry {
                    id: m.id,
                    parent_id: m.parent_id,
                    seq: 0,
                    timestamp: ts,
                    entry_type: LegacyEntryType::Message,
                    message: Some(AgentMessage { role, content: blocks, timestamp: ts }),
                    summary: None,
                    from_id: None,
                    custom_type: None,
                    data: None,
                }
            }
            SessionEntry::BranchSummary(b) => Entry {
                id: b.id,
                parent_id: b.parent_id,
                seq: 0,
                timestamp: ts_ms(&b.timestamp),
                entry_type: LegacyEntryType::BranchSummary,
                message: None,
                summary: Some(b.summary),
                from_id: Some(b.from_id),
                custom_type: None,
                data: b.details,
            },
            SessionEntry::Compaction(c) => Entry {
                id: c.id,
                parent_id: c.parent_id,
                seq: 0,
                timestamp: ts_ms(&c.timestamp),
                entry_type: LegacyEntryType::Compaction,
                message: None,
                summary: Some(c.summary),
                from_id: None,
                custom_type: None,
                data: c.details,
            },
            SessionEntry::Custom(c) => Entry {
                id: c.id,
                parent_id: c.parent_id,
                seq: 0,
                timestamp: ts_ms(&c.timestamp),
                entry_type: LegacyEntryType::Custom,
                message: None,
                summary: None,
                from_id: None,
                custom_type: Some(c.custom_type),
                data: c.data,
            },
            _ => Entry {
                id: s.id().to_string(),
                parent_id: s.parent_id().map(|x| x.to_string()),
                seq: 0,
                timestamp: 0,
                entry_type: LegacyEntryType::Custom,
                message: None,
                summary: None,
                from_id: None,
                custom_type: Some("unknown".to_string()),
                data: None,
            },
        }
    }
}

fn legacy_from_block(b: PiContentBlock) -> ContentBlock {
    match b {
        PiContentBlock::Text { text } => ContentBlock::Text { text },
        PiContentBlock::Image { .. } => ContentBlock::Text { text: "[image]".to_string() },
        PiContentBlock::Thinking { thinking } => ContentBlock::Text { text: thinking },
        PiContentBlock::ToolCall { id, name, arguments } => ContentBlock::ToolUse {
            id,
            name,
            input: arguments,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_entry_roundtrip() {
        let entry = Entry::message("01JABCDEFGH", None, 1, AgentMessage::user("hello"));
        let session: SessionEntry = entry.into();
        assert_eq!(session.id(), "01JABCDEFGH");
    }
}pub type AgentMessage = provider::Message;
