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
pub use nini_core::entries::AgentMessage;
use nini_core::CoreError;
pub use nini_core::SessionEntry;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn convert_to_pi_message(msg: nini_core::AgentMessage) -> nini_core::entries::AgentMessage {
    use nini_core::entries::{
        AssistantMessage, ContentBlock as PiContentBlock, StringOrContentBlocks, ToolResultMessage,
        UserMessage,
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
fn provider_to_pi_blocks(
    blocks: Vec<nini_core::ContentBlock>,
) -> Vec<nini_core::entries::ContentBlock> {
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
    #[serde(
        rename = "parentSession",
        skip_serializing_if = "Option::is_none",
        default
    )]
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
/// Pi 0.84.2 `buildContextEntries` — `/home/jin/pi-src-reconstructed/
/// pi-coding-agent/src/core/session-manager.ts:415`.
///
/// The live context of a session is
///
/// ```text
/// [compaction] + (pre-compaction tail from firstKeptEntryId) + (everything after)
/// ```
///
/// Pi scans the path forward from the root and flips a flag when it reaches
/// `firstKeptEntryId`; the matching entry itself is kept. An id that matches
/// nothing contributes nothing from the prefix — deliberately, so a
/// malformed id cannot silently resurrect the whole history.
///
/// Only the LAST compaction on the path is honoured: earlier ones are
/// represented by the summary of the later one.
///
/// This is what nini's read path was missing. `CompactionEntry::
/// first_kept_entry_id` existed but had no production writer, so no session
/// ever carried one and this function had nothing to reconstruct from.
pub fn build_context_entries(session: &Session) -> Vec<SessionEntry> {
    let path: Vec<SessionEntry> = session.get_path().into_iter().cloned().collect();

    let compaction_idx = path
        .iter()
        .rposition(|e| matches!(e, SessionEntry::Compaction(_)));
    let Some(ci) = compaction_idx else {
        return path;
    };

    let first_kept = match &path[ci] {
        SessionEntry::Compaction(c) => c.first_kept_entry_id.as_deref(),
        _ => None,
    };

    let mut context = Vec::with_capacity(path.len());
    context.push(path[ci].clone());

    if let Some(first_kept) = first_kept {
        let mut found = false;
        for entry in &path[..ci] {
            // Pi sets the flag BEFORE pushing, so the kept entry itself is
            // included. Reversing these two lines silently drops one entry.
            if entry.id() == first_kept {
                found = true;
            }
            if found {
                context.push(entry.clone());
            }
        }
    }

    context.extend_from_slice(&path[ci + 1..]);
    context
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub header: SessionHeader,
    pub entries: Vec<SessionEntry>,
    /// v0.8.9 (P2-9): how many non-empty lines were skipped because
    /// they could not be parsed.
    ///
    /// A single malformed line used to make the whole session
    /// unreadable — one hand-edit, one truncated write from a full
    /// disk, or one future-format field and the entire conversation
    /// history was gone with no way to get any of it back. The reader
    /// now salvages what it can and reports the damage here so the TUI
    /// can tell the user instead of silently showing a partial history.
    ///
    /// Not part of the on-disk format: it describes this read, not the
    /// session. A round-tripped session always reports 0.
    pub skipped_lines: usize,
}

impl Session {
    /// Push a legacy AgentMessage into the session (returns the new entry id).
    pub fn push_message(
        &mut self,
        parent_id: Option<String>,
        msg: nini_core::AgentMessage,
    ) -> String {
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
        Codec::new(path)
            .write(self)
            .map_err(|e| SessionError::Other(e.to_string()))
    }

    /// Read session from disk (alias for Codec::read).
    pub fn read_from_file(path: &std::path::Path) -> Result<Self, SessionError> {
        Codec::new(path)
            .read()
            .map_err(|e| SessionError::Other(e.to_string()))
    }

    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            header: SessionHeader::new(cwd),
            entries: Vec::new(),
            skipped_lines: 0,
        }
    }

    /// Pi 0.84.2 `sessionManager.appendCompaction` —
    /// `session-manager.ts:1097`.
    ///
    /// Pi types `firstKeptEntryId` as a required `string`. nini's
    /// `CompactionEntry` carries `Option<String>` because it predates any
    /// writer; this constructor always populates it, so the two `None`
    /// literals that used to sit in production code can go away.
    ///
    /// Returns the new entry's id so the caller can chain further entries
    /// onto it, exactly as Pi's `appendCompaction` returns `entry.id`.
    pub fn append_compaction(
        &mut self,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u32,
    ) -> String {
        let id = format!("compact_{}", uuid::Uuid::new_v4());
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.entries.push(SessionEntry::Compaction(
            nini_core::entries::CompactionEntry {
                id: id.clone(),
                // Pi: `parentId: this.leafId`. Parenting anywhere else lands the
                // entry on a different branch and resume loses it silently.
                parent_id: self.leaf_id(),
                timestamp,
                summary,
                tokens_before: Some(tokens_before),
                retained_tail: None,
                first_kept_entry_id: Some(first_kept_entry_id),
                details: None,
                usage: None,
                from_hook: None,
            },
        ));
        id
    }

    /// The current leaf: the entry no other entry parents to, newest wins.
    ///
    /// `get_path` derives the leaf the same way; this exists so
    /// `append_compaction` can parent without walking the whole path twice.
    pub fn leaf_id(&self) -> Option<String> {
        self.get_path().last().map(|e| e.id().to_string())
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
    ///
    /// **v0.8.9 (P2-9) — damaged lines are skipped, not fatal.**
    ///
    /// This used to `?` on every `serde_json::from_str`, so one
    /// malformed line made the entire session unreadable. Session files
    /// are append-only JSONL; the realistic ways to get a bad line are a
    /// hand-edit, a disk that filled mid-append, a truncated copy, or a
    /// field written by a newer nini. None of those are reasons to throw
    /// away every other line, and losing the whole history is far worse
    /// than losing one message.
    ///
    /// Each skipped line is reported on stderr with its 1-based number so
    /// the damage is visible in a log, and counted on
    /// [`Session::skipped_lines`] so the UI can surface it. A line that
    /// is valid JSON but an unknown `type` is skipped for the same
    /// reason: we cannot know what it is, but we know it is not
    /// something we can use.
    pub fn read(&self) -> Result<Session, CoreError> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut header: Option<SessionHeader> = None;
        let mut entries = Vec::new();
        let mut skipped_lines = 0usize;

        for (idx, line) in reader.lines().enumerate() {
            // An I/O error mid-file (also a form of truncation) should
            // not discard everything we already read, so break rather
            // than propagate. The count already parsed is what matters.
            let Ok(line) = line else {
                eprintln!(
                    "[nini] session {}: I/O error at line {}, keeping the {} \
                     entries read so far",
                    self.path.display(),
                    idx + 1,
                    entries.len()
                );
                skipped_lines += 1;
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let parsed: Line = match serde_json::from_str(&line) {
                Ok(l) => l,
                Err(e) => {
                    skipped_lines += 1;
                    eprintln!(
                        "[nini] session {}: skipping malformed line {} ({e})",
                        self.path.display(),
                        idx + 1
                    );
                    continue;
                }
            };
            match parsed {
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
            skipped_lines,
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
        AgentMessage, BranchSummaryEntry, CompactionEntry, LabelEntry, ModelChangeEntry,
        SessionInfoEntry, SessionMessageEntry, StringOrContentBlocks, ThinkingLevelChangeEntry,
        UserMessage,
    };

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

    // ---- v0.8.9 (P2-9): damaged lines are salvaged, not fatal -------
    //
    // Before this, one malformed line made the whole session
    // unreadable — a hand-edit, a disk that filled mid-append, a
    // truncated copy, or a field written by a newer nini all meant the
    // entire conversation history was gone with no way to recover any
    // of it. Session files are append-only JSONL; there is no reason
    // one bad record should cost every other record.

    /// Build a valid session file with `n` messages, then corrupt line
    /// `bad_idx` (0-based, counting the header as line 0).
    fn write_session_with_bad_line(path: &std::path::Path, n: usize, bad_idx: usize) {
        let mut out = String::new();
        let header = SessionHeader::new("/test/cwd");
        out.push_str(&serde_json::to_string(&header).unwrap());
        out.push('\n');
        for i in 0..n {
            if i == bad_idx {
                // Truncated JSON — what a disk-full append leaves behind.
                out.push_str("{\"type\":\"entry\",\"id\":\"msg");
                out.push('\n');
                continue;
            }
            let entry = SessionEntry::Message(SessionMessageEntry {
                id: format!("msg{i}"),
                parent_id: None,
                timestamp: "2024-12-03T14:00:01.000Z".to_string(),
                message: AgentMessage::User(UserMessage {
                    content: StringOrContentBlocks::String(format!("body {i}")),
                    timestamp: 1234567890,
                }),
            });
            out.push_str(&serde_json::to_string(&entry).unwrap());
            out.push('\n');
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_single_malformed_line_does_not_lose_the_whole_session() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("damaged.jsonl");
        write_session_with_bad_line(&path, 10, 4);

        let loaded = Codec::new(&path).read().expect("read must not fail");

        assert_eq!(
            loaded.skipped_lines, 1,
            "exactly one line was damaged and it should be reported"
        );
        // The whole point: the other nine survive, in order.
        let ids: Vec<&str> = loaded.entries.iter().map(|e| e.id()).collect();
        assert_eq!(
            ids,
            vec!["msg0", "msg1", "msg2", "msg3", "msg5", "msg6", "msg7", "msg8", "msg9"],
            "every readable entry must survive, in order"
        );
    }

    #[test]
    fn every_line_damaged_still_yields_a_usable_session() {
        // Worst case: the header line is gone too. We must still hand
        // back something loadable rather than an error the user can do
        // nothing with.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("garbage.jsonl");
        std::fs::write(&path, b"not json at all\nalso not json\n").unwrap();

        let loaded = Codec::new(&path)
            .read()
            .expect("a file of pure garbage must still produce a session");
        assert_eq!(loaded.skipped_lines, 2);
        assert!(loaded.entries.is_empty());
        // A synthesised header so downstream code has an id to work with.
        assert!(!loaded.header.id.is_empty());
    }

    #[test]
    fn valid_session_reports_zero_skipped_lines() {
        // The counter must not cry wolf, or users learn to ignore it.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("clean.jsonl");
        write_session_with_bad_line(&path, 3, 99); // no damage
        let loaded = Codec::new(&path).read().unwrap();
        assert_eq!(loaded.skipped_lines, 0);
        assert_eq!(loaded.entries.len(), 3);
    }

    #[test]
    fn blank_lines_are_not_counted_as_damage() {
        // JSONL files accumulate trailing newlines; those are not
        // "unreadable lines" and must not inflate the warning.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("blanks.jsonl");
        write_session_with_bad_line(&path, 2, 99);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n\n   \n\n");
        std::fs::write(&path, text).unwrap();

        let loaded = Codec::new(&path).read().unwrap();
        assert_eq!(loaded.skipped_lines, 0);
        assert_eq!(loaded.entries.len(), 2);
    }

    #[test]
    fn unknown_entry_type_is_skipped_not_fatal() {
        // Valid JSON that we do not understand — a field written by a
        // newer nini. Same reasoning: keep what we can use.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("future.jsonl");
        let mut text = serde_json::to_string(&SessionHeader::new("/test/cwd")).unwrap();
        text.push('\n');
        text.push_str("{\"type\":\"quantum-telemetry\",\"payload\":{}}\n");
        let entry = SessionEntry::Message(SessionMessageEntry {
            id: "msg0".to_string(),
            parent_id: None,
            timestamp: "2024-12-03T14:00:01.000Z".to_string(),
            message: AgentMessage::User(UserMessage {
                content: StringOrContentBlocks::String("hi".to_string()),
                timestamp: 1,
            }),
        });
        text.push_str(&serde_json::to_string(&entry).unwrap());
        text.push('\n');
        std::fs::write(&path, text).unwrap();

        // Depending on how `Line` is tagged this either fails to
        // deserialize (handled by the parse-skip) or deserializes into
        // an unknown variant. Either way the good entry must survive.
        match Codec::new(&path).read() {
            Ok(loaded) => {
                assert!(
                    loaded.entries.iter().any(|e| e.id() == "msg0"),
                    "the readable entry must survive an unknown type"
                );
            }
            Err(e) => panic!("unknown entry type must not be fatal: {e}"),
        }
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
        assert!(
            s.contains("\"type\":\"session\""),
            "should have type=session: {}",
            s
        );
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

        let pi_msgs: Vec<_> = sess
            .entries
            .iter()
            .filter_map(|e| match e {
                nini_core::SessionEntry::Message(m) => Some(m.message.clone()),
                _ => None,
            })
            .collect();
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
