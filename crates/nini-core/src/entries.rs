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
///
/// v0.7 (M3a) — adds 3 internal-only variants per the wiki's
/// 7-variant model (Notification / UiMessage / AppMessage). These
/// are persisted to the session JSONL like any other message but
/// are FILTERED OUT before being sent to the LLM in
/// `convert_to_llm` (M3b). They carry metadata / UI affordances
/// / app-level events that have no place in the model's context.
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
    // v0.7 (M3a) — internal-only variants. Never reach the LLM.
    #[serde(rename = "notification")]
    Notification(NotificationMessage),
    #[serde(rename = "uiMessage")]
    UiMessage(UiMessage),
    #[serde(rename = "appMessage")]
    AppMessage(AppMessage),
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

/// v0.7 (M3a) — internal-only notification. Surfaces events like
/// "compaction started", "tool blocked", "abort signal received" to
/// the session log without polluting model context. `convert_to_llm`
/// (M3b) drops these from the LLM request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationMessage {
    /// Free-form event name, e.g. "compaction.started",
    /// "permission.denied", "abort.received".
    pub kind: String,
    pub data: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// v0.7 (M3a) — internal-only UI message. Drives TUI affordances
/// (status bar updates, progress bars, transient toasts) without
/// involving the model. `convert_to_llm` (M3b) drops these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiMessage {
    /// Component name, e.g. "status-bar", "toast", "spinner".
    pub component: String,
    pub props: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// v0.7 (M3a) — internal-only application message. Records app
/// lifecycle events (extension activated, skill loaded, theme
/// changed). `convert_to_llm` (M3b) drops these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppMessage {
    /// Source identifier, e.g. "extension:nini-ext-foo",
    /// "skill:my-skill", "theme:dark".
    pub source: String,
    pub payload: Option<serde_json::Value>,
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

/// v0.7 (M3a) — predicate for the 3 internal-only `AgentMessage`
/// variants that `convert_to_llm` (M3b) drops from the LLM request.
/// They are persisted to the session JSONL like any other message
/// but never appear in the model's context.
///
/// Pi's contract: notification / uiMessage / appMessage → []
/// (dropped in the LLM view).
impl AgentMessage {
    pub fn is_internal_only(&self) -> bool {
        matches!(
            self,
            Self::Notification(_) | Self::UiMessage(_) | Self::AppMessage(_)
        )
    }

    /// Free-form variant name for logging / debug.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::ToolResult(_) => "toolResult",
            Self::Custom(_) => "custom",
            Self::BashExecution(_) => "bashExecution",
            Self::BranchSummary(_) => "branchSummary",
            Self::CompactionSummary(_) => "compactionSummary",
            Self::Notification(_) => "notification",
            Self::UiMessage(_) => "uiMessage",
            Self::AppMessage(_) => "appMessage",
        }
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

#[cfg(test)]
mod m3a_message_variant_tests {
    //! v0.7 (M3a) — tests for the 3 new internal-only variants
    //! (Notification / UiMessage / AppMessage), their wire-format
    //! compatibility, and the `is_internal_only` predicate that
    //! `convert_to_llm` (M3b) uses to filter them out.
    use super::*;

    fn ts() -> i64 { 1_700_000_000_000 }

    fn sample_notification() -> AgentMessage {
        AgentMessage::Notification(NotificationMessage {
            kind: "compaction.started".into(),
            data: Some(serde_json::json!({"tokens_before": 12345})),
            timestamp: ts(),
        })
    }

    fn sample_ui_message() -> AgentMessage {
        AgentMessage::UiMessage(UiMessage {
            component: "status-bar".into(),
            props: Some(serde_json::json!({"text": "compacting…"})),
            timestamp: ts(),
        })
    }

    fn sample_app_message() -> AgentMessage {
        AgentMessage::AppMessage(AppMessage {
            source: "extension:nini-ext-foo".into(),
            payload: Some(serde_json::json!({"event": "activated"})),
            timestamp: ts(),
        })
    }

    #[test]
    fn notification_serializes_with_role_tag() {
        let json = serde_json::to_string(&sample_notification()).unwrap();
        assert!(json.contains(r#""role":"notification""#), "got: {json}");
        assert!(json.contains(r#""kind":"compaction.started""#));
        assert!(json.contains(r#""data""#));
    }

    #[test]
    fn ui_message_serializes_with_role_tag() {
        let json = serde_json::to_string(&sample_ui_message()).unwrap();
        assert!(json.contains(r#""role":"uiMessage""#), "got: {json}");
        assert!(json.contains(r#""component":"status-bar""#));
    }

    #[test]
    fn app_message_serializes_with_role_tag() {
        let json = serde_json::to_string(&sample_app_message()).unwrap();
        assert!(json.contains(r#""role":"appMessage""#), "got: {json}");
        assert!(json.contains(r#""source":"extension:nini-ext-foo""#));
    }

    #[test]
    fn all_three_variants_round_trip_through_json() {
        // Serialize then deserialize each variant. Field integrity
        // is the contract — the JSONL v4 wire format must round-trip
        // so existing tools (nini-session, jq, Pi) can read these
        // messages.
        for msg in [
            sample_notification(),
            sample_ui_message(),
            sample_app_message(),
        ] {
            let json = serde_json::to_string(&msg).unwrap();
            let back: AgentMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, back, "round-trip failed for {json}");
        }
    }

    #[test]
    fn is_internal_only_predicate() {
        // The 3 new variants MUST return true.
        assert!(sample_notification().is_internal_only());
        assert!(sample_ui_message().is_internal_only());
        assert!(sample_app_message().is_internal_only());

        // All other variants MUST return false.
        let user = AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::String("hi".into()),
            timestamp: ts(),
        });
        assert!(!user.is_internal_only());
        let custom = AgentMessage::Custom(CustomMessage {
            custom_type: "x".into(),
            content: StringOrContentBlocks::String("y".into()),
            display: true,
            details: None,
            timestamp: ts(),
        });
        // Custom IS user-visible in the model view (it gets
        // converted to a user message by `convert_to_llm`); it's
        // not internal-only.
        assert!(!custom.is_internal_only());
    }

    #[test]
    fn variant_name_distinguishes_internal_from_visible() {
        assert_eq!(sample_notification().variant_name(), "notification");
        assert_eq!(sample_ui_message().variant_name(), "uiMessage");
        assert_eq!(sample_app_message().variant_name(), "appMessage");
        let user = AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::String("hi".into()),
            timestamp: ts(),
        });
        assert_eq!(user.variant_name(), "user");
    }

    #[test]
    fn v0_6_1_session_messages_still_round_trip() {
        // REGRESSION: the 4 existing variants (User / Assistant /
        // ToolResult / Custom) were already serialized in v0.6.1
        // sessions. Adding new variants must not change their
        // JSON shape (the tag is `role` and that's it).
        let user = AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::String("hello".into()),
            timestamp: ts(),
        });
        let json = serde_json::to_string(&user).unwrap();
        // v0.6.1 shape: {"role":"user","content":"hello","timestamp":...}
        assert!(json.starts_with(r#"{"role":"user""#), "got: {json}");
        // Round-trip
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(user, back);
    }

    #[test]
    fn session_entry_message_round_trip_with_internal_variant() {
        // A full `SessionEntry::Message` carrying a Notification
        // must round-trip too (the wire format is JSONL v4).
        let entry = SessionEntry::Message(SessionMessageEntry {
            id: "e1".into(),
            parent_id: None,
            timestamp: "2026-09-23T00:00:00.000Z".into(),
            message: sample_notification(),
        });
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains(r#""type":"message""#));
        assert!(json.contains(r#""role":"notification""#));
        let back: SessionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }
}
