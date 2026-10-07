//! Oracle: session-layer compaction, Pi 0.84.2's way.
//!
//! Ground truth is `session-manager.ts` in the reconstructed Pi source
//! (`/home/jin/pi-src-reconstructed/pi-coding-agent/src/core/`), which is
//! Pi 0.84.2 recovered from its own sourcemaps.
//!
//! Pi writes:
//!
//! ```ts
//! appendCompaction(summary, firstKeptEntryId, tokensBefore, details?, fromHook?, usage?)
//!   -> { type: "compaction", id, parentId: leafId, timestamp,
//!        summary, firstKeptEntryId, tokensBefore, ... }
//! ```
//!
//! and rebuilds context with `buildContextEntries`:
//!
//! ```ts
//! const compaction = last compaction entry on the leaf path
//! if (!compaction) return path
//! contextEntries = [compaction]
//! scan path[0..compactionIdx] FORWARD; start collecting once
//!   entry.id === compaction.firstKeptEntryId
//! contextEntries.push(...path.slice(compactionIdx + 1))
//! ```
//!
//! So the live context is
//! `[compaction] + (pre-compaction tail from firstKeptEntryId) + (everything after)`.
//!
//! nini before this: the Agent compacted in-memory and nothing reached the
//! session file; the TUI separately folded *transcript lines* in half for
//! display only. `firstKeptEntryId` was hardcoded `None` in two production
//! sites, which made Pi's reconstruction algorithm unimplementable.
//!
//! What these tests pin is the *algorithm* — the ordering rules and the role
//! of `firstKeptEntryId` — not the format of the generated ids.

use nini_core::entries::{
    AgentMessage, AssistantMessage, CompactionEntry, SessionEntry, SessionMessageEntry,
    StringOrContentBlocks, UserMessage,
};
use nini_session::{build_context_entries, Session};

fn user(id: &str, parent: Option<&str>, text: &str) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        timestamp: "2026-10-05T00:00:00.000Z".into(),
        message: AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::String(text.into()),
            timestamp: 0,
        }),
    })
}

fn assistant(id: &str, parent: Option<&str>, text: &str) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        timestamp: "2026-10-05T00:00:01.000Z".into(),
        message: AgentMessage::Assistant(AssistantMessage {
            content: vec![nini_core::entries::ContentBlock::Text { text: text.into() }],
            api: "anthropic".into(),
            provider: "anthropic".into(),
            model: "claude".into(),
            usage: Default::default(),
            stop_reason: nini_core::entries::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }),
    })
}

fn compaction(id: &str, parent: Option<&str>, summary: &str, first_kept: &str) -> SessionEntry {
    SessionEntry::Compaction(CompactionEntry {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        timestamp: "2026-10-05T00:01:00.000Z".into(),
        summary: summary.into(),
        tokens_before: Some(50_000),
        retained_tail: None,
        first_kept_entry_id: Some(first_kept.into()),
        details: None,
        usage: None,
        from_hook: None,
    })
}

fn session(entries: Vec<SessionEntry>) -> Session {
    Session {
        header: nini_session::SessionHeader::new("/tmp"),
        entries,
        skipped_lines: 0,
    }
}

fn ids(entries: &[SessionEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.id()).collect()
}

/// The linear case, which is what every real compaction hits.
///
///     m1 -> m2 -> m3 -> c1 -> m4 -> m5
///                    ^ compaction, firstKeptEntryId = m3
///
/// Pi yields [c1, m3, m4, m5]: the compaction itself, the pre-compaction
/// tail starting at the kept entry, then everything after.
#[test]
fn pi_reconstruction_keeps_the_marked_tail_plus_everything_after() {
    let s = session(vec![
        user("m1", None, "one"),
        assistant("m2", Some("m1"), "two"),
        user("m3", Some("m2"), "three"),
        compaction("c1", Some("m3"), "SUMMARY", "m3"),
        user("m4", Some("c1"), "four"),
        assistant("m5", Some("m4"), "five"),
    ]);

    assert_eq!(
        ids(&build_context_entries(&s)),
        vec!["c1", "m3", "m4", "m5"],
        "Pi: [compaction] + tail from firstKeptEntryId + everything after"
    );
}

/// A compaction with nothing after it yet.
#[test]
fn a_compaction_with_nothing_after_it_still_yields_the_tail() {
    let s = session(vec![
        user("m1", None, "one"),
        user("m2", Some("m1"), "two"),
        compaction("c1", Some("m2"), "SUMMARY", "m2"),
    ]);

    assert_eq!(ids(&build_context_entries(&s)), vec!["c1", "m2"]);
}

/// No compaction anywhere: the whole path survives untouched. Without this
/// the algorithm could "pass" by always returning something plausible.
#[test]
fn without_a_compaction_the_whole_path_is_the_context() {
    let s = session(vec![
        user("m1", None, "one"),
        assistant("m2", Some("m1"), "two"),
        user("m3", Some("m2"), "three"),
    ]);

    assert_eq!(ids(&build_context_entries(&s)), vec!["m1", "m2", "m3"]);
}

/// Pi's scan runs forward from the root and flips a flag on the first id
/// match. A `firstKeptEntryId` that matches nothing therefore contributes
/// nothing from the prefix — it is not an error, and it must not silently
/// fall back to "keep everything".
#[test]
fn an_unresolvable_first_kept_id_contributes_nothing_from_the_prefix() {
    let s = session(vec![
        user("m1", None, "one"),
        user("m2", Some("m1"), "two"),
        compaction("c1", Some("m2"), "SUMMARY", "does-not-exist"),
        user("m4", Some("c1"), "four"),
    ]);

    assert_eq!(
        ids(&build_context_entries(&s)),
        vec!["c1", "m4"],
        "an unknown firstKeptEntryId drops the prefix rather than keeping it"
    );
}

/// Two compactions on one path: Pi keeps only the LAST one, and the
/// `firstKeptEntryId` that belongs to it. This is the case that separates
/// "last wins" from "first wins", which is the easy thing to get backwards.
#[test]
fn only_the_last_compaction_on_the_path_is_honoured() {
    let s = session(vec![
        user("m1", None, "one"),
        compaction("c1", Some("m1"), "FIRST SUMMARY", "m1"),
        user("m2", Some("c1"), "two"),
        compaction("c2", Some("m2"), "SECOND SUMMARY", "m2"),
        user("m3", Some("c2"), "three"),
    ]);

    assert_eq!(
        ids(&build_context_entries(&s)),
        vec!["c2", "m2", "m3"],
        "Pi takes the last compaction and its own firstKeptEntryId; \
         c1 and m1 are summarised away"
    );
}

/// append_compaction must parent the new entry at the current leaf, exactly
/// like Pi's `parentId: this.leafId`. Parenting at the root instead lands the
/// entry on a different branch, and resume silently loses it.
#[test]
fn append_compaction_parents_at_the_current_leaf() {
    let mut s = session(vec![
        user("m1", None, "one"),
        assistant("m2", Some("m1"), "two"),
    ]);

    let id = s.append_compaction("SUMMARY".into(), "m2".into(), 50_000);

    assert_ne!(id, "m1", "a fresh id, not reusing an existing entry");
    let e = s.get_entry(&id).expect("the entry must be findable by id");
    assert_eq!(
        e.parent_id(),
        Some("m2"),
        "Pi: parentId is the leaf, not the root"
    );
    match e {
        SessionEntry::Compaction(c) => {
            assert_eq!(c.summary, "SUMMARY");
            assert_eq!(c.first_kept_entry_id.as_deref(), Some("m2"));
            assert_eq!(c.tokens_before, Some(50_000));
        }
        other => panic!("expected a Compaction entry, got {other:?}"),
    }
}

/// The whole point of the refactor: a compaction that survives the file and
/// rebuilds to the same context after a round trip through JSONL.
///
/// Ids are generated, so this compares *shapes* — how many entries of each
/// kind, and whether the compaction and the tail survived — not literal ids.
#[test]
fn a_compaction_survives_the_file_and_rebuilds_identically() {
    let dir = std::env::temp_dir().join(format!("nini-a-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");

    fn shape(s: &Session) -> Vec<String> {
        build_context_entries(s)
            .iter()
            .map(|e| match e {
                SessionEntry::Compaction(c) => format!("compaction:{}", c.summary),
                SessionEntry::Message(_) => "message".to_string(),
                other => format!("other:{}", other.id()),
            })
            .collect()
    }

    let before = {
        let mut s = session(vec![
            user("m1", None, "one"),
            assistant("m2", Some("m1"), "two"),
            user("m3", Some("m2"), "three"),
        ]);
        let cid = s.append_compaction("SUMMARY OF EARLIER WORK".into(), "m2".into(), 50_000);
        // Parent at the compaction, not at m3: parenting anywhere else would
        // put the new message on a different branch and it would vanish from
        // the reconstructed context.
        s.append(user("m4", Some(&cid), "four"));
        s.write_to_file(&path).unwrap();
        shape(&s)
    };

    assert_eq!(
        before,
        vec![
            "compaction:SUMMARY OF EARLIER WORK",
            "message", // m2, the retained tail
            "message", // m3, kept because it is after the kept entry
            "message", // the post-compaction message
        ],
        "sanity: compaction first, then the tail, then what came after"
    );

    let after = {
        let s = Session::read_from_file(&path).unwrap();
        shape(&s)
    };

    assert_eq!(
        before, after,
        "a compaction must survive the file with the same reconstruction"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
