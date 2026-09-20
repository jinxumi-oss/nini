//! nini-core — JSONL entry types aligned with pi v3 session format.
//!
//! This is a **minimal surface** stub that supplies exactly the types
//! needed by tracked code. The richer LLM-backed branch-summary work
//! (BRANCH_SUMMARY_PROMPT, summarize_branch_with_llm, etc.) is being
//! reintroduced in a follow-up commit.
//!
//! Public types used by tracked callers (see grep on crate::entries::*):
//! - SessionEntry (enum of 9 variants, tagged `type` snake_case)
//! - SessionMessageEntry / ModelChangeEntry / ThinkingLevelChangeEntry
//!   / CompactionEntry / BranchSummaryEntry / CustomEntry
//!   / CustomMessageEntry / LabelEntry / SessionInfoEntry
//! - AgentMessage (enum, tagged `role` camelCase — User/Assistant/Custom/ToolResult)
//! - UserMessage / AssistantMessage / ToolResultMessage / CustomMessage
//! - ContentBlock (Text only needed by callers; ToolCall for serde compat)
//! - Cost / Usage / StopReason
//! - StringOrContentBlocks (String | Blocks)

use serde::{Deserialize, Serialize};

pub use crate::provider::Role;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Message,
    ModelChange,
    ThinkingLevelChange,
    Compaction,
    BranchSummary,
    Custom,
    CustomMessage,
    Label,
    SessionInfo,
}

/// Base for every entry — flat fields on each variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessageEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelChangeEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingLevelChangeEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub thinking_level: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub summary: String,
    pub tokens_before: Option<u32>,
    pub retained_tail: Option<Vec<AgentMessage>>,
    pub first_kept_entry_id: Option<String>,
    pub details: Option<serde_json::Value>,
    pub usage: Option<Usage>,
    #[serde(default)]
    pub from_hook: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(rename = "fullOutputPath", skip_serializing_if = "Option::is_none", default)]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchSummaryMessage {
    pub summary: String,
    #[serde(default)]
    pub read_files: Vec<String>,
    #[serde(rename = "modifiedFiles", default)]
    pub modified_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchSummaryEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub from_id: String,
    pub summary: String,
    pub usage: Option<Usage>,
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub from_hook: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMessageEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub custom_type: String,
    pub content: StringOrContentBlocks,
    pub display: bool,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub target_id: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub name: String,
}

/// SessionEntry — enum of all 9 v3 entry kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    Message(SessionMessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Custom(CustomEntry),
    CustomMessage(CustomMessageEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
}

/// AgentMessage — discriminated by `role`. Re-exported as
/// `entries::AgentMessage` for compat with pi v3 wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
    #[serde(rename = "custom")]
    Custom(CustomMessage),
    #[serde(rename = "bashExecution")]
    BashExecution(BashExecutionMessage),
    #[serde(rename = "branchSummary")]
    BranchSummary(BranchSummaryMessage),
    #[serde(rename = "compactionSummary")]
    CompactionSummary(CompactionSummaryMessage),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrContentBlocks {
    String(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: StringOrContentBlocks,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub api: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    pub error_message: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: Vec<ContentBlock>,
    pub details: Option<serde_json::Value>,
    pub usage: Option<Usage>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMessage {
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: StringOrContentBlocks,
    pub display: bool,
    pub details: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// ContentBlock — only Text/ToolCall needed by callers.
/// Note: legacy code in nini-core lib.rs uses provider::ContentBlock
/// (with Text/ToolUse/ToolResult variants), but those are kept inside
/// the provider module so the two enums coexist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "image")]
    Image { image: String },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        #[serde(default)]
        arguments: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(rename = "cacheRead", default)]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite", default)]
    pub cache_write: f64,
    #[serde(default)]
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead", default)]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite", default)]
    pub cache_write: u64,
    #[serde(rename = "totalTokens", default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cost: Cost,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}



// Helper accessors used by nini-session — these work for any variant.
impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Message(e) => &e.id,
            Self::ModelChange(e) => &e.id,
            Self::ThinkingLevelChange(e) => &e.id,
            Self::Compaction(e) => &e.id,
            Self::BranchSummary(e) => &e.id,
            Self::Custom(e) => &e.id,
            Self::CustomMessage(e) => &e.id,
            Self::Label(e) => &e.id,
            Self::SessionInfo(e) => &e.id,
        }
    }

    pub fn timestamp(&self) -> &str {
        match self {
            Self::Message(e) => &e.timestamp,
            Self::ModelChange(e) => &e.timestamp,
            Self::ThinkingLevelChange(e) => &e.timestamp,
            Self::Compaction(e) => &e.timestamp,
            Self::BranchSummary(e) => &e.timestamp,
            Self::Custom(e) => &e.timestamp,
            Self::CustomMessage(e) => &e.timestamp,
            Self::Label(e) => &e.timestamp,
            Self::SessionInfo(e) => &e.timestamp,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Message(e) => e.parent_id.as_deref(),
            Self::ModelChange(e) => e.parent_id.as_deref(),
            Self::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            Self::Compaction(e) => e.parent_id.as_deref(),
            Self::BranchSummary(e) => e.parent_id.as_deref(),
            Self::Custom(e) => e.parent_id.as_deref(),
            Self::CustomMessage(e) => e.parent_id.as_deref(),
            Self::Label(e) => e.parent_id.as_deref(),
            Self::SessionInfo(e) => e.parent_id.as_deref(),
        }
    }
}

impl std::fmt::Display for SessionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionEntry({})", self.id())
    }
}



// Manual PartialEq/Eq/Hash for SessionEntry based on id only
impl PartialEq for SessionEntry {
    fn eq(&self, other: &Self) -> bool { self.id() == other.id() }
}
impl Eq for SessionEntry {}

impl std::hash::Hash for SessionEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}
