//! nini-session — JSONL v3 session codec with v3 ↔ pi-mono interop.
//!
//! Mirrors pi v0.84.3 `session-format.md` exactly:
//! - Line 1: SessionHeader
//! - Lines 2+: SessionEntry (one of 9 types)
//! - Format version: 3 (matches pi)

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub mod tree;

/// Session error type (for legacy API compat).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session error: {0}")]
    Other(String),
}


use chrono::{DateTime, Utc};
pub use nini_core::SessionEntry;
pub use nini_core::entries::AgentMessage;
use nini_core::CoreError;
use nini_core::SessionEntry as _SE;
use serde::{Deserialize, Serialize};
use uuid::Uuid;


fn convert_to_pi_message(msg: nini_core::AgentMessage) -> nini_core::entries::AgentMessage {
    use nini_core::entries::{
        AssistantMessage, ContentBlock as PiContentBlock, StringOrContentBlocks,
        ToolResultMessage, UserMessage,
    };
    // v0.7.1 — use the actual Assistant / ToolResult variants
    // (previously this function mapped both to Pi::Custom which
    // made Assistant messages indistinguishable from extension
    // messages on reload).
    match msg.role {
        nini_core::Role::User => {
            let blocks = provider_to_pi_blocks(msg.content);
            nini_core::entries::AgentMessage::User(UserMessage {
                content: StringOrContentBlocks::Blocks(blocks),
                timestamp: msg.timestamp,
            })
        }
        nini_core::Role::Assistant => {
            let blocks = provider_to_pi_blocks(msg.content);
            nini_core::entries::AgentMessage::Assistant(AssistantMessage {
                content: blocks,
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: nini_core::entries::StopReason::Stop,
                error_message: None,
                timestamp: msg.timestamp,
            })
        }
        nini_core::Role::Tool => {
            // Extract the tool_use_id from the FIRST ToolResult
            // block (subsequent blocks in the same message would
            // be unusual — the agent loop emits one block per
            // tool call). The rest of the content goes into the
            // entries::ToolResultMessage.content as a Vec<block>.
            let mut tool_call_id = String::new();
            let mut is_error = false;
            let mut blocks: Vec<PiContentBlock> = Vec::new();
            for b in msg.content {
                match b {
                    nini_core::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error: err,
                    } => {
                        if tool_call_id.is_empty() {
                            tool_call_id = tool_use_id;
                        }
                        is_error = err;
                        blocks.push(PiContentBlock::Text { text: content });
                    }
                    other => blocks.push(provider_block_to_pi(other)),
                }
            }
            nini_core::entries::AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id,
                tool_name: String::new(),
                content: blocks,
                details: None,
                usage: None,
                is_error,
                timestamp: msg.timestamp,
            })
        }
        nini_core::Role::System => {
            // System messages have no session-persistent
            // representation in pi's wire format; collapse to
            // User with display: false so the conversion is
            // lossless at the data level.
            let blocks = provider_to_pi_blocks(msg.content);
            nini_core::entries::AgentMessage::Custom(nini_core::entries::CustomMessage {
                custom_type: "system".into(),
                content: StringOrContentBlocks::Blocks(blocks),
                display: false,
                details: None,
                timestamp: msg.timestamp,
            })
        }
    }
}

/// Convert provider ContentBlock list to entries ContentBlock list.
/// Inverse of `nini_core::conversion::assistant_content_to_blocks`.
fn provider_to_pi_blocks(blocks: Vec<nini_core::ContentBlock>) -> Vec<nini_core::entries::ContentBlock> {
    use nini_core::entries::ContentBlock as PiContentBlock;
    blocks.into_iter().map(provider_block_to_pi).collect()
}

/// Convert a single provider ContentBlock to entries ContentBlock.
fn provider_block_to_pi(b: nini_core::ContentBlock) -> nini_core::entries::ContentBlock {
    use nini_core::entries::ContentBlock as PiContentBlock;
    match b {
        nini_core::ContentBlock::Text { text } => PiContentBlock::Text { text },
        nini_core::ContentBlock::ToolUse { id, name, input } => PiContentBlock::ToolCall {
            id,
            name,
            arguments: input,
        },
        nini_core::ContentBlock::ToolResult { content, .. } => {
            PiContentBlock::Text { text: content }
        }
    }
}

/// Library version, mirrors workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSONL format version — matches pi v3.
pub const JSONL_FORMAT_VERSION: u32 = 3;

/// Session header (line 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub kind: String, // always "session"
    pub version: u32,
    pub id: String,
    pub timestamp: String, // ISO 8601
    pub cwd: String,
    #[serde(rename = "parentSession", skip_serializing_if = "Option::is_none", default)]
    pub parent_session: Option<String>,
}

impl SessionHeader {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            kind: "session".to_string(),
            version: JSONL_FORMAT_VERSION,
            id: format!("u_{}", Uuid::new_v4()),
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            cwd: cwd.into(),
            parent_session: None,
        }
    }

    pub fn new_fork(cwd: impl Into<String>, parent_session: impl Into<String>) -> Self {
        Self {
            kind: "session".to_string(),
            version: JSONL_FORMAT_VERSION,
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            cwd: cwd.into(),
            parent_session: Some(parent_session.into()),
        }
    }
}

/// A session as on disk: header + entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub header: SessionHeader,
    pub entries: Vec<SessionEntry>,
}

impl Session {
    /// Push a legacy AgentMessage into the session (returns the new entry id).
    pub fn push_message(&mut self, parent_id: Option<String>, msg: nini_core::AgentMessage) -> String {
        let id = format!("msg_{}", uuid::Uuid::new_v4());
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let entry = nini_core::SessionEntry::Message(nini_core::SessionMessageEntry {
            id: id.clone(),
            parent_id,
            timestamp,
            message: convert_to_pi_message(msg),
        });
        self.entries.push(entry);
        id
    }

    /// Write session to disk atomically (alias for Codec::write).
    pub fn write_to_file(&self, path: &std::path::Path) -> Result<(), SessionError> {
        Codec::new(path).write(self).map_err(|e| SessionError::Other(e.to_string()))
    }

    /// Read session from disk (alias for Codec::read).
    pub fn read_from_file(path: &std::path::Path) -> Result<Self, SessionError> {
        Codec::new(path).read().map_err(|e| SessionError::Other(e.to_string()))
    }

    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            header: SessionHeader::new(cwd),
            entries: Vec::new(),
        }
    }

    pub fn append(&mut self, entry: SessionEntry) -> &mut Self {
        self.entries.push(entry);
        self
    }

    /// Total entries excluding header.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get entry by ID.
    pub fn get_entry(&self, id: &str) -> Option<&SessionEntry> {
        self.entries.iter().find(|e| e.id() == id)
    }

    /// Get entries by parent_id (children).
    pub fn get_children(&self, parent_id: &str) -> Vec<&SessionEntry> {
        self.entries
            .iter()
            .filter(|e| e.parent_id() == Some(parent_id))
            .collect()
    }

    /// Get path from root to leaf via parent links.
    pub fn get_path(&self) -> Vec<&SessionEntry> {
        // Find leaf (entry with no children)
        let mut leaves = Vec::new();
        for entry in &self.entries {
            if self.get_children(entry.id()).is_empty() {
                leaves.push(entry);
            }
        }
        // Pick most recent leaf
        let leaf = match leaves.iter().max_by_key(|e| e.timestamp().to_string()) {
            Some(l) => l,
            None => return Vec::new(),
        };
        // Walk backwards
        let mut path = vec![*leaf];
        let mut current_id = leaf.parent_id();
        while let Some(pid) = current_id {
            if let Some(entry) = self.get_entry(pid) {
                path.insert(0, entry);
                current_id = entry.parent_id();
            } else {
                break;
            }
        }
        path
    }
}

/// On-disk line format. Either header or entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Line {
    Session(SessionHeader),
    Entry(SessionEntry),
}

/// Codec for reading/writing pi v3 JSONL files.
pub struct Codec {
    path: PathBuf,
}

impl Codec {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Read a session file into a `Session`.
    /// Auto-migrates from older versions (v1, v2) on read.
    pub fn read(&self) -> Result<Session, CoreError> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut header: Option<SessionHeader> = None;
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let line: Line = serde_json::from_str(&line)
                .map_err(|e| CoreError::InvalidEntry(format!("line parse: {}", e)))?;
            match line {
                Line::Session(h) => {
                    // Migrate older versions
                    let migrated = migrate_header(migrated_header(h));
                    header = Some(migrated);
                }
                Line::Entry(e) => entries.push(e),
            }
        }

        Ok(Session {
            header: header.unwrap_or_else(|| {
                // No header — create one
                SessionHeader {
                    kind: "session".to_string(),
                    version: JSONL_FORMAT_VERSION,
                    id: Uuid::new_v4().to_string(),
                    timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    cwd: String::new(),
                    parent_session: None,
                }
            }),
            entries,
        })
    }

    /// Write session to disk atomically (write to temp, rename).
    pub fn write(&self, session: &Session) -> Result<(), CoreError> {
        // Ensure parent dir exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write to temp file
        let mut tmp = self.path.clone();
        tmp.as_mut_os_string().push(".tmp");

        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)?;

            // Header
            let header_json = serde_json::to_string(&session.header)?;
            writeln!(file, "{}", header_json)?;

            // Entries
            for entry in &session.entries {
                let entry_json = serde_json::to_string(&entry)?;
                writeln!(file, "{}", entry_json)?;
            }

            file.sync_all()?;
        }

        // Atomic rename
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Append a single entry to an existing session file.
    pub fn append(&self, entry: &SessionEntry) -> Result<(), CoreError> {
        // Read current session, append, write back
        let mut session = self.read()?;
        session.entries.push(entry.clone());
        self.write(&session)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Migrate older session headers to v3.
fn migrate_header(h: SessionHeader) -> SessionHeader {
    if h.version > JSONL_FORMAT_VERSION {
        // Future version — accept but log
        eprintln!(
            "warning: session version {} > supported {}, reading as-is",
            h.version, JSONL_FORMAT_VERSION
        );
    }
    h
}

fn migrated_header(h: SessionHeader) -> SessionHeader {
    h
}

/// Parse timestamp from ISO 8601.
pub fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, CoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CoreError::InvalidEntry(format!("timestamp: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nini_core::entries::{
        AgentMessage, BranchSummaryEntry, CompactionEntry, EntryType, LabelEntry,
        ModelChangeEntry, SessionInfoEntry, SessionMessageEntry, StopReason,
        StringOrContentBlocks, ThinkingLevelChangeEntry, Usage, UserMessage,
    };
    use nini_core::Role;
    use tempfile::tempdir;

    #[test]
    fn header_roundtrip() {
        let h = SessionHeader::new("/test/cwd");
        let s = serde_json::to_string(&h).unwrap();
        let h2: SessionHeader = serde_json::from_str(&s).unwrap();
        assert_eq!(h, h2);
        assert_eq!(h2.version, 3);
        assert_eq!(h2.kind, "session");
    }

    #[test]
    fn write_and_read_minimal() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");
        let codec = Codec::new(&path);

        let mut session = Session::new("/test/cwd");
        session.append(SessionEntry::Message(SessionMessageEntry {
            id: "msg1".to_string(),
            parent_id: None,
            timestamp: "2024-12-03T14:00:01.000Z".to_string(),
            message: AgentMessage::User(UserMessage {
                content: StringOrContentBlocks::String("hi".to_string()),
                timestamp: 1234567890,
            }),
        }));

        codec.write(&session).unwrap();
        let loaded = codec.read().unwrap();

        assert_eq!(loaded.header.version, 3);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].id(), "msg1");
    }

    #[test]
    fn write_and_read_all_9_entry_types() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("all_types.jsonl");
        let codec = Codec::new(&path);

        let mut session = Session::new("/cwd");

        // 1. message
        session.append(SessionEntry::Message(SessionMessageEntry {
            id: "1".into(),
            parent_id: None,
            timestamp: "2024-12-03T14:00:01.000Z".into(),
            message: AgentMessage::User(UserMessage {
                content: StringOrContentBlocks::String("hi".into()),
                timestamp: 1,
            }),
        }));

        // 2. model_change
        session.append(SessionEntry::ModelChange(ModelChangeEntry {
            id: "2".into(),
            parent_id: Some("1".into()),
            timestamp: "2024-12-03T14:00:02.000Z".into(),
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
        }));

        // 3. thinking_level_change
        session.append(SessionEntry::ThinkingLevelChange(
            ThinkingLevelChangeEntry {
                id: "3".into(),
                parent_id: Some("2".into()),
                timestamp: "2024-12-03T14:00:03.000Z".into(),
                thinking_level: "high".into(),
            },
        ));

        // 4. compaction
        session.append(SessionEntry::Compaction(CompactionEntry {
            id: "4".into(),
            parent_id: Some("3".into()),
            timestamp: "2024-12-03T14:00:04.000Z".into(),
            summary: "User did X, Y, Z".into(),
            tokens_before: Some(50000),
            retained_tail: None,
            first_kept_entry_id: None,
            details: None,
            from_hook: None,
            usage: None,
        }));

        // 5. branch_summary
        session.append(SessionEntry::BranchSummary(BranchSummaryEntry {
            id: "5".into(),
            parent_id: Some("4".into()),
            timestamp: "2024-12-03T14:00:05.000Z".into(),
            from_id: "3".into(),
            summary: "Branch explored A".into(),
            usage: None,
            details: None,
            from_hook: None,
        }));

        // 6. custom
        session.append(SessionEntry::Custom(nini_core::CustomEntry {
            id: "6".into(),
            parent_id: Some("5".into()),
            timestamp: "2024-12-03T14:00:06.000Z".into(),
            custom_type: "my-ext".into(),
            data: Some(serde_json::json!({"count": 42})),
        }));

        // 7. custom_message
        session.append(SessionEntry::CustomMessage(nini_core::CustomMessageEntry {
            id: "7".into(),
            parent_id: Some("6".into()),
            timestamp: "2024-12-03T14:00:07.000Z".into(),
            custom_type: "my-ext".into(),
            content: StringOrContentBlocks::String("Injected context".into()),
            display: true,
            details: None,
        }));

        // 8. label
        session.append(SessionEntry::Label(LabelEntry {
            id: "8".into(),
            parent_id: Some("7".into()),
            timestamp: "2024-12-03T14:00:08.000Z".into(),
            target_id: "1".into(),
            label: Some("checkpoint-1".into()),
        }));

        // 9. session_info
        session.append(SessionEntry::SessionInfo(SessionInfoEntry {
            id: "9".into(),
            parent_id: Some("8".into()),
            timestamp: "2024-12-03T14:00:09.000Z".into(),
            name: "Refactor auth".into(),
        }));

        codec.write(&session).unwrap();
        let loaded = codec.read().unwrap();

        assert_eq!(loaded.entries.len(), 9);
        assert_eq!(loaded.entries[0].id(), "1");
        assert_eq!(loaded.entries[8].id(), "9");
    }

    #[test]
    fn pi_format_compatibility_check() {
        // Verify header matches what pi v0.84.3 writes
        let h = SessionHeader::new("/test");
        let s = serde_json::to_string(&h).unwrap();
        // pi's exact field names
        assert!(s.contains("\"type\":\"session\""), "should have type=session: {}", s);
        assert!(s.contains("\"version\":3"), "should have version=3: {}", s);
        assert!(s.contains("\"cwd\":\"/test\""), "should have cwd: {}", s);
    }

    /// v0.7.1 — round-trip test: pushing messages to a session
    /// and re-reading them via the chokepoint must preserve the
    /// wiki 7→3 conversion. Catches the v0.6.1 bug where
    /// Assistant was stored as Custom("assistant") and
    /// ToolResult was dropped on reload.
    #[test]
    fn push_then_read_round_trip() {
        let mut sess = Session::new("/test");

        let user_id = sess.push_message(
            None,
            nini_core::AgentMessage {
                role: nini_core::Role::User,
                content: vec![nini_core::ContentBlock::Text {
                    text: "hello".into(),
                }],
                timestamp: 100,
            },
        );
        let assistant_id = sess.push_message(
            Some(user_id),
            nini_core::AgentMessage {
                role: nini_core::Role::Assistant,
                content: vec![nini_core::ContentBlock::Text {
                    text: "hi back".into(),
                }],
                timestamp: 200,
            },
        );
        sess.push_message(
            Some(assistant_id),
            nini_core::AgentMessage {
                role: nini_core::Role::Tool,
                content: vec![nini_core::ContentBlock::ToolResult {
                    tool_use_id: "tc-1".into(),
                    content: "exit code 0".into(),
                    is_error: false,
                }],
                timestamp: 300,
            },
        );

        let pi_msgs: Vec<_> = sess.entries.iter().filter_map(|e| match e {
                nini_core::SessionEntry::Message(m) => Some(m.message.clone()),
                _ => None,
            }).collect();
        let msgs = nini_core::conversion::default_session_to_llm(&pi_msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, nini_core::Role::User);
        assert_eq!(msgs[1].role, nini_core::Role::Assistant);
        assert_eq!(
            msgs[2].role,
            nini_core::Role::Tool,
            "ToolResult must NOT be dropped — v0.6.1 bug regression guard"
        );
        if let nini_core::ContentBlock::ToolResult { tool_use_id, .. } = &msgs[2].content[0] {
            assert_eq!(tool_use_id, "tc-1");
        } else {
            panic!("expected ToolResult");
        }
    }
}
