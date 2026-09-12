//! nini-session — JSONL v4 session codec with legacy-v3 bridge.
//!
//! Phase 1 minimum viable:
//! - Write v4 native format (`JSONL_FORMAT_VERSION = 4`) to disk
//! - Read v4 native format back into `Vec<Entry>`
//! - Read legacy-v3 fixture files (10 entry types mapped onto 4 native + values)
//! - Roundtrip test for v4
//! - Legacy-v3 fixture parser for backwards-compat verification
//!
//! Spec source: `references/spec-v0.85.1/jsonl/{codec,legacy-v3,types}.ts`.

#![doc = "nini-session — JSONL v4 codec and legacy-v3 bridge."]

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use nini_core::{AgentMessage, Entry};
use serde::{Deserialize, Serialize};

/// Library version, mirrors workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSONL format version written by nini (mirrors spec `JSONL_FORMAT_VERSION = 4`).
pub const JSONL_FORMAT_VERSION: u32 = 4;

/// JSONL storage version (mirrors spec `JSONL_STORAGE_VERSION = 1`).
pub const JSONL_STORAGE_VERSION: u32 = 1;

/// Header line at the top of every v4 session JSONL file.
///
/// Mirrors `JsonlStorageHeader` from spec `jsonl/types.ts`. Field names follow
/// the spec's camelCase convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    /// Format version discriminator. Always `4` for nini-written sessions.
    pub v: u32,
    /// Discriminator (always `"header"` for the first line).
    pub kind: String,
    /// Unique session id (UUIDv7 recommended).
    pub id: String,
    /// Storage version. Always `1` for now.
    pub storage_version: u32,
    /// Unix epoch milliseconds when the session was created.
    pub created_at: i64,
    /// Working directory the session was started in.
    pub cwd: String,
    /// Optional parent session id (for forks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

impl SessionHeader {
    /// Construct a header for a new session.
    pub fn new(id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            v: JSONL_FORMAT_VERSION,
            kind: "header".to_string(),
            id: id.into(),
            storage_version: JSONL_STORAGE_VERSION,
            created_at: chrono::Utc::now().timestamp_millis(),
            cwd: cwd.into(),
            parent_session_id: None,
        }
    }
}

/// A session as it exists on disk: header + sequence of entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub header: SessionHeader,
    pub entries: Vec<Entry>,
}

impl Session {
    /// Create a new in-memory session.
    pub fn new(id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            header: SessionHeader::new(id, cwd),
            entries: Vec::new(),
        }
    }

    /// Append a message entry to the session. Sequence number is auto-assigned.
    pub fn push_message(&mut self, parent_id: Option<String>, msg: AgentMessage) -> uuid::Uuid {
        let id = uuid::Uuid::now_v7();
        let seq = (self.entries.len() + 1) as u64;
        self.entries
            .push(Entry::message(id.to_string(), parent_id, seq, msg));
        id
    }

    /// Write the session to a JSONL file in v4 format.
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<(), SessionError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        let header_line = serde_json::to_string(&self.header)?;
        writeln!(file, "{header_line}")?;
        for entry in &self.entries {
            writeln!(file, "{}", serde_json::to_string(entry)?)?;
        }
        file.flush()?;
        Ok(())
    }

    /// Read a v4 JSONL session file.
    pub fn read_from_file(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let header_line = lines
            .next()
            .ok_or(SessionError::EmptyFile)?
            .map_err(SessionError::Io)?;
        let header: SessionHeader = serde_json::from_str(&header_line)?;

        if header.v != JSONL_FORMAT_VERSION {
            return Err(SessionError::UnsupportedVersion(header.v));
        }

        let mut entries = Vec::new();
        for line in lines {
            let line = line.map_err(SessionError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry = serde_json::from_str(&line).map_err(SessionError::Json)?;
            entries.push(entry);
        }

        Ok(Session { header, entries })
    }
}

/// Top-level error type for nini-session operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("empty session file")]
    EmptyFile,
    #[error("unsupported session format version: {0}")]
    UnsupportedVersion(u32),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Classify a legacy-v3 entry by its `type` field. Used by the bridge layer to
/// route entries into native v4 categories or to skip discarded types.
///
/// Mirrors `RetainedLegacyV3Entry` / `DiscardedLegacyV3Entry` from spec
/// `jsonl/legacy-v3.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyV3Kind {
    /// Retained and migrated to v4 entry (5 types: message, custom, custom_message, branch_summary, compaction)
    Retained(LegacyV3Retained),
    /// Discarded (folded into values system in v0.85.1: model_change, thinking_level_change, active_tools_change, session_info, label)
    Discarded(LegacyV3Discarded),
    /// Header (first line in legacy v3 file)
    Header,
    /// Unknown / unrecognized type
    Unknown(String),
}

/// Retained legacy-v3 entry kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyV3Retained {
    Message,
    Custom,
    CustomMessage,
    BranchSummary,
    Compaction,
}

/// Discarded legacy-v3 entry kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyV3Discarded {
    ModelChange,
    ThinkingLevelChange,
    ActiveToolsChange,
    SessionInfo,
    Label,
}

/// Classify a legacy-v3 entry by inspecting its raw `type` field.
///
/// Used to count and audit legacy fixtures. Full entry parsing for retained
/// types arrives in Phase 5; discarded types are reported for completeness.
pub fn classify_legacy_v3(raw_type: &str) -> LegacyV3Kind {
    match raw_type {
        "header" | "session" => LegacyV3Kind::Header,
        "message" => LegacyV3Kind::Retained(LegacyV3Retained::Message),
        "custom" => LegacyV3Kind::Retained(LegacyV3Retained::Custom),
        "custom_message" => LegacyV3Kind::Retained(LegacyV3Retained::CustomMessage),
        "branch_summary" => LegacyV3Kind::Retained(LegacyV3Retained::BranchSummary),
        "compaction" => LegacyV3Kind::Retained(LegacyV3Retained::Compaction),
        "model_change" => LegacyV3Kind::Discarded(LegacyV3Discarded::ModelChange),
        "thinking_level_change" => LegacyV3Kind::Discarded(LegacyV3Discarded::ThinkingLevelChange),
        "active_tools_change" => LegacyV3Kind::Discarded(LegacyV3Discarded::ActiveToolsChange),
        "session_info" => LegacyV3Kind::Discarded(LegacyV3Discarded::SessionInfo),
        "label" => LegacyV3Kind::Discarded(LegacyV3Discarded::Label),
        other => LegacyV3Kind::Unknown(other.to_string()),
    }
}

/// Counts of legacy-v3 entry kinds within a single file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyV3Counts {
    pub header: usize,
    pub retained_message: usize,
    pub retained_custom: usize,
    pub retained_custom_message: usize,
    pub retained_branch_summary: usize,
    pub retained_compaction: usize,
    pub discarded_model_change: usize,
    pub discarded_thinking_level_change: usize,
    pub discarded_active_tools_change: usize,
    pub discarded_session_info: usize,
    pub discarded_label: usize,
    pub unknown: std::collections::BTreeMap<String, usize>,
}

impl LegacyV3Counts {
    /// Parse a legacy-v3 JSONL file and tally entry kinds.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);
        let mut counts = LegacyV3Counts::default();
        for line in reader.lines() {
            let line = line.map_err(SessionError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            let raw_type = extract_type_field(&line).unwrap_or_default();
            match classify_legacy_v3(&raw_type) {
                LegacyV3Kind::Header => counts.header += 1,
                LegacyV3Kind::Retained(LegacyV3Retained::Message) => counts.retained_message += 1,
                LegacyV3Kind::Retained(LegacyV3Retained::Custom) => counts.retained_custom += 1,
                LegacyV3Kind::Retained(LegacyV3Retained::CustomMessage) => {
                    counts.retained_custom_message += 1;
                }
                LegacyV3Kind::Retained(LegacyV3Retained::BranchSummary) => {
                    counts.retained_branch_summary += 1;
                }
                LegacyV3Kind::Retained(LegacyV3Retained::Compaction) => {
                    counts.retained_compaction += 1
                }
                LegacyV3Kind::Discarded(LegacyV3Discarded::ModelChange) => {
                    counts.discarded_model_change += 1;
                }
                LegacyV3Kind::Discarded(LegacyV3Discarded::ThinkingLevelChange) => {
                    counts.discarded_thinking_level_change += 1;
                }
                LegacyV3Kind::Discarded(LegacyV3Discarded::ActiveToolsChange) => {
                    counts.discarded_active_tools_change += 1;
                }
                LegacyV3Kind::Discarded(LegacyV3Discarded::SessionInfo) => {
                    counts.discarded_session_info += 1;
                }
                LegacyV3Kind::Discarded(LegacyV3Discarded::Label) => counts.discarded_label += 1,
                LegacyV3Kind::Unknown(t) => {
                    *counts.unknown.entry(t).or_insert(0) += 1;
                }
            }
        }
        Ok(counts)
    }

    /// Total entries counted.
    pub fn total(&self) -> usize {
        self.header
            + self.retained_message
            + self.retained_custom
            + self.retained_custom_message
            + self.retained_branch_summary
            + self.retained_compaction
            + self.discarded_model_change
            + self.discarded_thinking_level_change
            + self.discarded_active_tools_change
            + self.discarded_session_info
            + self.discarded_label
            + self.unknown.values().sum::<usize>()
    }
}

/// Cheap field extractor that pulls `type` out of a JSONL line without full parsing.
/// Used for legacy-v3 classification where we only need the type discriminator.
fn extract_type_field(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    v.get("type")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// Resolve the spec fixture path under `references/spec-v0.85.1/fixtures/`.
pub fn spec_fixture_path(name: &str) -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/nini-session -> references/spec-v0.85.1/fixtures
    here.parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|p| {
            p.join("references")
                .join("spec-v0.85.1")
                .join("fixtures")
                .join(name)
        })
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn v4_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jsonl");

        let mut session = Session::new("01JARGV4TEST", "/home/user/project");
        session.push_message(None, AgentMessage::user("hello"));
        session.push_message(Some("prev-id".into()), AgentMessage::assistant("hi there"));
        session.write_to_file(&path).unwrap();

        let loaded = Session::read_from_file(&path).unwrap();
        assert_eq!(loaded.header.v, JSONL_FORMAT_VERSION);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].entry_type, nini_core::EntryType::Message);
        let msg = loaded.entries[0].message.as_ref().unwrap();
        assert_eq!(msg.role, nini_core::Role::User);
        assert_eq!(
            msg.content[0],
            nini_core::ContentBlock::Text {
                text: "hello".to_string()
            }
        );
    }

    #[test]
    fn v4_header_format_matches_spec() {
        let session = Session::new("abc-123", "/cwd");
        let json = serde_json::to_string(&session.header).unwrap();
        // Mirror spec's JsonlStorageHeader field names
        assert!(json.contains("\"v\":4"), "got: {json}");
        assert!(json.contains("\"kind\":\"header\""), "got: {json}");
        assert!(json.contains("\"storageVersion\":1"), "got: {json}");
        assert!(json.contains("\"cwd\":\"/cwd\""), "got: {json}");
    }

    #[test]
    fn legacy_v3_classify_all_known_types() {
        for t in [
            "header",
            "session",
            "message",
            "custom",
            "custom_message",
            "branch_summary",
            "compaction",
            "model_change",
            "thinking_level_change",
            "active_tools_change",
            "session_info",
            "label",
        ] {
            assert!(
                !matches!(classify_legacy_v3(t), LegacyV3Kind::Unknown(_)),
                "should classify {t}"
            );
        }
        assert!(matches!(
            classify_legacy_v3("not_a_real_type"),
            LegacyV3Kind::Unknown(_)
        ));
    }

    #[test]
    fn legacy_v3_fixture_large_session() {
        // Fixtures copied to references/spec-v0.85.1/fixtures/ during P000.
        // The `legacy-v3-large.jsonl` has 914 message + 1 model_change + 1 header + 103 thinking_level_change.
        let path = spec_fixture_path("legacy-v3-large.jsonl");
        if !path.exists() {
            eprintln!("fixture missing: {} (skipped)", path.display());
            return;
        }
        let counts = LegacyV3Counts::from_file(&path).unwrap();
        assert_eq!(counts.header, 1);
        assert_eq!(counts.retained_message, 914);
        assert_eq!(counts.discarded_model_change, 1);
        assert_eq!(counts.discarded_thinking_level_change, 103);
        assert_eq!(counts.discarded_active_tools_change, 0);
        assert!(
            counts.unknown.is_empty(),
            "unexpected types: {:?}",
            counts.unknown
        );
    }

    #[test]
    fn legacy_v3_fixture_before_compaction() {
        let path = spec_fixture_path("legacy-v3-before-compaction.jsonl");
        if !path.exists() {
            eprintln!("fixture missing: {} (skipped)", path.display());
            return;
        }
        let counts = LegacyV3Counts::from_file(&path).unwrap();
        assert_eq!(counts.header, 1);
        assert_eq!(counts.retained_compaction, 2);
        assert_eq!(counts.discarded_model_change, 5);
        assert_eq!(counts.discarded_thinking_level_change, 5);
        assert!(
            counts.unknown.is_empty(),
            "unexpected types: {:?}",
            counts.unknown
        );
    }
}
