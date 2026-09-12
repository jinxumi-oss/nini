//! nini-core — agent loop, message types, entry types, provider + tool abstractions.

#![doc = "nini-core — message types, entry types, agent loop, provider + tool abstractions."]

use serde::{Deserialize, Serialize};

pub mod agent;
pub mod compaction;
pub mod provider;
pub mod settings;
pub mod skills;
pub mod tool;

pub use agent::{AbortHandle, Agent, AgentError, AgentEvent, RunConfig};
pub use provider::{
    Capabilities, ContentBlock, Message as ProviderMessage, Provider, ProviderError, Request, Role,
    StreamEvent, ToolCall, ToolResult, Usage,
};
pub use tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};

/// Library version, mirrors workspace version.
pub use compaction::{
    CompactionOutput, CompactionPreparation, CompactionSettings, CutPoint, CutReason, compact,
    estimate_entries_tokens, estimate_message_tokens, estimate_provider_messages_tokens,
    find_cut_point, generate_local_summary, make_compaction_entry, prepare_compaction,
    should_compact,
};
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// An agent message exchanged between user, assistant, and tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub timestamp: i64,
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

/// JSONL entry type for the native (v4) session format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Message,
    Compaction,
    BranchSummary,
    Custom,
}

/// A persisted session entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub seq: u64,
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
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
            entry_type: EntryType::Message,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_roundtrip() {
        let m = AgentMessage::user("hello");
        let s = serde_json::to_string(&m).unwrap();
        let m2: AgentMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn message_entry_roundtrip() {
        let entry = Entry::message("01JABCDEFGH", None, 1, AgentMessage::user("hello"));
        let s = serde_json::to_string(&entry).unwrap();
        let e2: Entry = serde_json::from_str(&s).unwrap();
        assert_eq!(entry, e2);
        assert!(s.contains("\"type\":\"message\""));
        assert!(s.contains("\"role\":\"user\""));
    }
}
