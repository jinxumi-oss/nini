//! P0.2: nini's session must be a tree, because Pi's algorithms are
//! defined over a tree.
//!
//! Every entry nini wrote carried `parentId: null`. A four-entry session
//! therefore had a `get_path()` of length 1: `get_path` finds the leaf as
//! "an entry nothing parents to", every entry qualified, and the newest won.
//! With no tree, `build_context_entries` (R2a/A1) and `firstKeptEntryId`
//! have nothing to walk — the compaction work was landed and unreachable.
//!
//! Pi chains. From the committed fixture
//! (`fixtures/pi-0.84.2-session.jsonl`):
//!
//! ```text
//!   model_change          parentId: null            <- root
//!   thinking_level_change parentId: 7d7c3d8e        <- chains
//! ```
//!
//! Two halves are pinned here:
//!   * new writes chain, so the tree exists going forward;
//!   * a flat legacy file is linked on read, in file order, filling only
//!     nulls — which is a no-op on a genuine Pi file, where the only null is
//!     the root.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::sync::Arc;

fn new_state(
    tag: &str,
) -> (
    Arc<std::sync::Mutex<AppState>>,
    AgentSink,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("nini-p02-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink, dir)
}

fn with_session<R>(
    state: &Arc<std::sync::Mutex<AppState>>,
    f: impl FnOnce(&nini_session::Session) -> R,
) -> R {
    let arc = state
        .lock()
        .unwrap()
        .session_state
        .session
        .as_ref()
        .unwrap()
        .clone();
    let g = nini_tui::state::lock_session(&arc);
    f(&g)
}

/// Mutating sibling of `with_session`, for the tests that append.
fn with_session_mut<R>(
    state: &Arc<std::sync::Mutex<AppState>>,
    f: impl FnOnce(&mut nini_session::Session) -> R,
) -> R {
    let arc = state
        .lock()
        .unwrap()
        .session_state
        .session
        .as_ref()
        .unwrap()
        .clone();
    let mut g = nini_tui::state::lock_session(&arc);
    f(&mut g)
}

/// The text of a message entry, for readable assertions. Content is either a
/// bare string or a block list depending on how it was written, and a helper
/// that only handles one of them makes a passing test look like a failure.
fn text_of(e: &nini_core::SessionEntry) -> Option<String> {
    use nini_core::entries::{AgentMessage, StringOrContentBlocks};
    let nini_core::SessionEntry::Message(m) = e else {
        return None;
    };
    let content = match &m.message {
        AgentMessage::User(u) => &u.content,
        AgentMessage::Assistant(a) => {
            return a.content.iter().find_map(|b| match b {
                nini_core::entries::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
        }
        _ => return None,
    };
    match content {
        StringOrContentBlocks::String(t) => Some(t.clone()),
        StringOrContentBlocks::Blocks(bs) => bs.iter().find_map(|b| match b {
            nini_core::entries::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        }),
    }
}

/// New writes must chain. Without this the tree only ever exists for
/// sessions loaded from a file, never for the one being written.
#[tokio::test]
async fn a_new_session_is_written_as_a_chain() {
    let (state, sink, dir) = new_state("chain");
    state.lock().unwrap().push_user("read the config");
    for e in [
        AgentEventLite::TextDelta("on it".into()),
        AgentEventLite::ToolCallStart {
            id: "c1".into(),
            name: "read".into(),
        },
        AgentEventLite::ToolCallStop {
            id: "c1".into(),
            args: "{}".into(),
        },
        AgentEventLite::ToolResult {
            id: "c1".into(),
            name: "read".into(),
            ok: true,
            content: "body".into(),
            details: None,
            duration_ms: 1,
        },
        AgentEventLite::TurnEnd,
    ] {
        sink.push(e);
    }
    state.lock().unwrap().push_user("thanks");

    let (path_len, parents) = with_session(&state, |s| {
        (
            s.get_path().len(),
            s.entries
                .iter()
                .map(|e| e.parent_id().map(str::to_string))
                .collect::<Vec<_>>(),
        )
    });

    assert_eq!(
        path_len, 4,
        "all four entries must lie on one path, got parents {parents:?}"
    );
    assert_eq!(
        parents[0], None,
        "the first entry is the root and keeps a null parent: {parents:?}"
    );
    assert!(
        parents[1..].iter().all(|p| p.is_some()),
        "every entry after the root must name its parent: {parents:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Build a session file on disk, writing entries with the parent links
/// given. Written by serialising real entries rather than hand-rolled JSON,
/// so the file is exactly the shape the reader accepts and the test is
/// about parenting rather than about serde.
fn write_session(tag: &str, links: &[Option<&str>]) -> (std::path::PathBuf, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("nini-p02-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");

    let mut s = nini_session::Session::new("/tmp");
    let mut ids = Vec::new();
    for (i, parent) in links.iter().enumerate() {
        let id = format!("m{i}");
        s.append(nini_core::SessionEntry::Message(
            nini_core::entries::SessionMessageEntry {
                id: id.clone(),
                parent_id: parent.map(str::to_string),
                timestamp: format!("2026-10-05T00:00:0{i}.000Z"),
                message: nini_core::entries::AgentMessage::User(nini_core::entries::UserMessage {
                    content: nini_core::entries::StringOrContentBlocks::String(format!("t{i}")),
                    timestamp: 0,
                }),
            },
        ));
        ids.push(id);
    }
    s.write_to_file(&path).unwrap();
    (path, ids)
}

/// A legacy flat file — the shape nini wrote for months — must become a
/// chain on read, in file order.
#[test]
fn a_flat_legacy_file_is_linked_in_file_order() {
    let (path, ids) = write_session("legacy", &[None, None, None, None]);
    let s = nini_session::Session::read_from_file(&path).unwrap();
    let path_ids: Vec<String> = s.get_path().iter().map(|e| e.id().to_string()).collect();

    assert_eq!(
        path_ids, ids,
        "a flat file must read back as one ordered path"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The backfill's safety property: it fills nulls and never rewrites a
/// parent that is already set.
///
/// Asserted directly rather than via a fixture. The committed
/// `pi-0.84.2-session.jsonl` cannot serve that purpose: entries 5-22
/// reference parent ids that exist nowhere in the file (`e661654d`,
/// `6f023d67`, `192c8af8`, ...), so only its first four entries form a real
/// chain, and its last entry carries a uuid-style id with a null parent -
/// the signature of a nini write appended to it. A file that does not
/// chain cannot demonstrate that the backfill leaves real structure alone.
#[test]
fn the_backfill_fills_nulls_and_never_rewrites_a_real_parent() {
    // root(null) -> mid(null) -> branch(already points at ROOT, not at its
    // own predecessor) -> tail(null)
    //
    // The branch is what makes this test able to fail. An earlier version
    // pointed it at its immediate predecessor, where "fill the nulls" and
    // "overwrite everything" produce byte-identical results - so an
    // injected unconditional-overwrite regression passed 5/5. Pointing it
    // across is what separates the two behaviours.
    let (path, ids) = write_session("safe", &[None, None, Some("m0"), None]);
    let s = nini_session::Session::read_from_file(&path).unwrap();
    let parents: Vec<(String, Option<String>)> = s
        .entries
        .iter()
        .map(|e| (e.id().to_string(), e.parent_id().map(str::to_string)))
        .collect();

    assert_eq!(
        parents,
        vec![
            (ids[0].clone(), None),
            (ids[1].clone(), Some(ids[0].clone())),
            (ids[2].clone(), Some(ids[0].clone())),
            (ids[3].clone(), Some(ids[2].clone())),
        ],
        "the first entry is the root; later nulls chain to their \
         predecessor; the branch keeps its real, non-adjacent parent"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A file that already has a real parent link is a tree, not a flat list,
/// and must be returned untouched - not relinked.
#[test]
fn a_file_that_already_chains_is_not_relinked() {
    let (path, ids) = write_session("tree", &[None, Some("m0"), Some("m1"), Some("m2")]);
    let s = nini_session::Session::read_from_file(&path).unwrap();
    let parents: Vec<Option<String>> = s
        .entries
        .iter()
        .map(|e| e.parent_id().map(str::to_string))
        .collect();

    assert_eq!(
        parents,
        vec![
            None,
            Some(ids[0].clone()),
            Some(ids[1].clone()),
            Some(ids[2].clone())
        ],
        "a real chain must come back exactly as written"
    );
    assert_eq!(s.get_path().len(), 4, "and must reconstruct in full");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The end the whole exercise was for: compaction reconstruction running on
/// a session nini actually produced, not one built by hand with real
/// parent ids.
#[tokio::test]
async fn compaction_reconstruction_works_on_a_session_nini_wrote() {
    let (state, _sink, dir) = new_state("compaction");
    state.lock().unwrap().push_user("first question");
    state.lock().unwrap().push_assistant("first answer");
    state.lock().unwrap().push_user("second question");

    let (cid, kept_id) = with_session_mut(&state, |s| {
        let kept = s
            .entries
            .iter()
            .find(|e| text_of(e).as_deref() == Some("second question"))
            .expect("the second user message must be on disk")
            .id()
            .to_string();
        let id = s.append_compaction("SUMMARY OF THE FIRST HALF".into(), kept.clone(), 40_000);
        (id, kept)
    });

    state.lock().unwrap().push_user("third question");

    let (ctx, third_id) = with_session(&state, |s| {
        let third = s
            .entries
            .iter()
            .find(|e| text_of(e).as_deref() == Some("third question"))
            .expect("the third user message must be on disk")
            .id()
            .to_string();
        let ctx: Vec<String> = nini_session::build_context_entries(s)
            .iter()
            .map(|e| e.id().to_string())
            .collect();
        (ctx, third)
    });

    assert_eq!(
        ctx,
        vec![cid, kept_id, third_id],
        "Pi: [compaction] + the kept entry + everything written after it"
    );

    // And the point of the whole exercise: the summarised-away first half
    // must NOT be in the live context.
    let summarised_away = with_session(&state, |s| {
        s.entries
            .iter()
            .find(|e| text_of(e).as_deref() == Some("first question"))
            .map(|e| e.id().to_string())
            .unwrap()
    });
    assert!(
        !ctx.contains(&summarised_away),
        "the entry the compaction summarised away must not be in the live \
         context: {ctx:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
