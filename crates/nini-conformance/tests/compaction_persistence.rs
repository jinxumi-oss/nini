//! Oracle: a compaction must survive the session file.
//!
//! v0.10 (R2a), found by the zero-read audit: `CompactionEntry::first_kept_entry_id`
//! had no readers anywhere, which prompted the question "what does nini use
//! instead to recover history after a compaction?" — the answer was: nothing.
//!
//! Two links were missing:
//!
//!   * `compact_history` splices the summary into the in-memory
//!     `self.messages` (agent.rs:728-740) and never calls `session_append`,
//!     so neither the summary nor the compacted prefix reaches disk;
//!   * `session_entry_to_llm_message` (conversion.rs:70-75) returns `None`
//!     for anything that is not a `Message` entry, so a stored
//!     `SessionEntry::Compaction` is dropped on the way back in.
//!
//! Net effect before this: compaction worked for the rest of the run and
//! then evaporated. `/resume`, or a crash and restart, gave the model a
//! transcript that began mid-conversation with no summary and no marker
//! that a compaction had happened.
//!
//! Pi keeps the summary and, via `firstKeptEntryId`, walks the parent chain
//! to find where the retained tail begins (`legacy-v3.ts:315-321`).

use nini_core::entries::{self, SessionEntry, StringOrContentBlocks};
use nini_core::provider::{ContentBlock, Message, Role};

const SUMMARY: &str = "The user asked to fix a login bug. I found three call sites.";

fn entry_summary_label(e: &SessionEntry) -> &'static str {
    match e {
        SessionEntry::ModelChange(_) => "ModelChange",
        SessionEntry::ThinkingLevelChange(_) => "ThinkingLevelChange",
        _ => "other",
    }
}

fn compaction_entry() -> SessionEntry {
    SessionEntry::Compaction(nini_core::entries::CompactionEntry {
        id: "c1".into(),
        parent_id: Some("m3".into()),
        timestamp: "2026-10-05T00:00:00Z".into(),
        summary: SUMMARY.into(),
        tokens_before: Some(50_000),
        retained_tail: None,
        first_kept_entry_id: None,
        details: None,
        usage: None,
        from_hook: None,
    })
}

#[test]
fn a_stored_compaction_converts_back_into_context() {
    let msg = nini_core::conversion::session_entry_to_llm_message(&compaction_entry())
        .expect("a Compaction entry must yield a context message, not None");
    let text: String = msg
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("CONTEXT SUMMARY"),
        "the marker `compact_history` writes in-process is missing, so a resumed \
         session and a live one would disagree: {text:?}"
    );
    assert!(
        text.contains(SUMMARY),
        "the summary text itself was lost: {text:?}"
    );
}

#[test]
fn the_conversion_is_ordered_and_role_safe() {
    // Pi injects the summary as a normal context message, not as a user turn
    // the model could be confused into "answering".
    let msg = nini_core::conversion::session_entry_to_llm_message(&compaction_entry()).unwrap();
    assert_eq!(
        msg.role,
        Role::User,
        "matches the in-process splice, which also uses Role::User"
    );
}

#[test]
fn a_full_session_with_a_compaction_round_trips() {
    // The end-to-end shape: tail messages, a compaction, more tail messages.
    let entries = vec![
        SessionEntry::Message(nini_core::entries::SessionMessageEntry {
            id: "m1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: nini_core::entries::AgentMessage::User(nini_core::entries::UserMessage {
                content: StringOrContentBlocks::String("fix the login bug".into()),
                timestamp: 0,
            }),
        }),
        compaction_entry(),
        SessionEntry::Message(nini_core::entries::SessionMessageEntry {
            id: "m2".into(),
            parent_id: Some("c1".into()),
            timestamp: "t".into(),
            message: nini_core::entries::AgentMessage::Assistant(
                nini_core::entries::AssistantMessage {
                    content: vec![entries::ContentBlock::Text {
                        text: "on it".into(),
                    }],
                    api: "a".into(),
                    provider: "p".into(),
                    model: "m".into(),
                    usage: Default::default(),
                    stop_reason: nini_core::entries::StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                },
            ),
        }),
    ];

    let mut out = Vec::new();
    for e in &entries {
        if let Some(m) = nini_core::conversion::session_entry_to_llm_message(e) {
            out.push(m);
        }
    }
    let texts: Vec<String> = out
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| match c {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let joined = texts.join("\n");

    assert!(
        joined.contains(SUMMARY),
        "resuming a compacted session must restore the summary.\n\
         transcript was:\n{joined}"
    );
    assert!(
        joined.contains("fix the login bug") && joined.contains("on it"),
        "the retained tail must survive alongside it.\ntranscript was:\n{joined}"
    );
    assert_eq!(out.len(), 3, "one message, the summary, one reply");
}

#[test]
fn non_message_metadata_entries_stay_out_of_context() {
    // The fix must be narrow: model/thinking changes are session metadata
    // and must NOT become context messages.
    for entry in [
        SessionEntry::ModelChange(nini_core::entries::ModelChangeEntry {
            id: "x".into(),
            parent_id: None,
            timestamp: "t".into(),
            provider: "p".into(),
            model_id: "m".into(),
        }),
        SessionEntry::ThinkingLevelChange(nini_core::entries::ThinkingLevelChangeEntry {
            id: "x".into(),
            parent_id: None,
            timestamp: "t".into(),
            thinking_level: "high".into(),
        }),
    ] {
        assert!(
            nini_core::conversion::session_entry_to_llm_message(&entry).is_none(),
            "{} is metadata and must not enter the model context",
            entry_summary_label(&entry)
        );
    }
}

/// The in-process splice uses this exact marker, so a live run and a resumed
/// one must read the same.
#[test]
fn the_marker_matches_the_one_compact_history_writes() {
    let msg: Message =
        nini_core::conversion::session_entry_to_llm_message(&compaction_entry()).unwrap();
    let text = msg
        .content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        text.starts_with("[CONTEXT SUMMARY]"),
        "marker drifted from agent.rs::compact_history: {text:?}"
    );
}
