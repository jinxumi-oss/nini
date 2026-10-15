//! Oracle: `/fork` must create a real session branch.
//!
//! # What Pi does
//!
//! `sessionManager.createBranchedSession(leafId)`
//! (`pi-coding-agent/src/core/session-manager.ts:1412-1489`):
//!
//! ```text
//! 1414  const path = this.getBranch(leafId)        // root → leaf
//! 1424  // filter LabelEntry out, re-chaining parentId as we go, because
//! 1425  // "removing labels requires re-chaining the retained path to avoid
//! 1426  //  orphaned subtrees"
//! 1430  const newSessionId = createSessionId()
//! 1435  const header = { …, parentSession: previousSessionFile }
//! 1461  this.fileEntries = [header, ...pathWithoutLabels, ...labelEntries]
//! 1463  this.sessionId = newSessionId; this.sessionFile = newSessionFile
//! 1482  // write now only if the path already contains an assistant message,
//! 1483  // otherwise defer to _persist() on the first assistant turn
//! ```
//!
//! And the caller (`agent-session-runtime.ts:262-345`):
//!
//! ```text
//!  position "before" → targetLeafId = selectedEntry.parentId
//!  position "at"     → targetLeafId = selectedEntry.id
//! 318  const forkedSessionPath = sessionManager.createBranchedSession(targetLeafId)
//! 322  await this.teardownCurrent("fork", …)
//! 325  this.apply(await this.createRuntime({ sessionManager, … }))
//! ```
//!
//! # What nini does
//!
//! `cmd_fork` (`nini-tui/src/commands/dispatch.rs:579`) copies
//! `transcript_state.lines[..cut_at]` into a local variable, appends a
//! text marker, and reports it. No session is created, no entry tree is
//! touched, and the "branch" exists only as the text of one line in the
//! current transcript.
//!
//! So `/tree` has nothing to draw a branch from (v0.9.2's gap list says
//! so: *"/fork creates no branches"*), a fork cannot be resumed, and the
//! 585 lines in `branch_summary.rs` have no caller.
//!
//! These tests pin the *session-level* semantics, because that is where
//! the contract is: a fork is a new file whose entries are the retained
//! path, re-chained, under a header that names its parent.

use nini_core::provider::{ContentBlock, Message, Role};
use nini_session::Session;

/// A user message, built the way the TUI builds one: through
/// `push_message`, which allocates the entry id and chains the parent.
///
/// Hand-built entries with an empty `id` make `get_path()` return nothing —
/// the test would then be exercising a shape that cannot occur in a session
/// the TUI actually wrote, and every assertion past the first would pass or
/// fail for the wrong reason.
fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }
}

/// A session with four entries chained in order, as the TUI writes them.
fn four_entry_session() -> Session {
    let mut s = Session::new("/tmp/proj");
    for t in ["first", "second", "third", "fourth"] {
        s.push_message(None, user_msg(t));
    }
    s
}

/// The new session must be a *different* session, not a second view of the
/// same one.
#[test]
fn a_fork_is_a_new_session_with_its_own_id() {
    let s = four_entry_session();
    let original_id = s.header.id.clone();

    let forked = s.fork_at(2).expect("fork point 2 exists");

    assert_ne!(
        forked.header.id, original_id,
        "the fork reused the parent's id; /resume would then show two files \
         that are the same conversation"
    );
}

/// Pi writes `parentSession` on the fork's header (`session-manager.ts:1441`)
/// so the session tree is reconstructible from the files alone.
#[test]
fn the_fork_records_its_parent_session() {
    let s = four_entry_session();
    let forked = s.fork_at(2).expect("fork point 2 exists");
    assert_eq!(
        forked.header.parent_session.as_deref(),
        Some(s.header.id.as_str()),
        "the fork does not name the session it came from — the on-disk tree \
         cannot be rebuilt"
    );
}

/// `position: "before"` drops the selected entry — Pi returns that
/// message's text to the editor to be re-sent
/// (`agent-session-runtime.ts:281`), so it must not be in the branch.
#[test]
fn fork_before_the_branch_point_drops_that_entry() {
    let s = four_entry_session();
    let forked = s.fork_at(2).expect("fork point 2 exists");
    assert_eq!(
        forked.entries.len(),
        1,
        "\"before\" retains only what precedes the branch point, got {}",
        forked.entries.len()
    );
}

/// `position: "at"` keeps it.
#[test]
fn fork_at_the_branch_point_keeps_that_entry() {
    let s = four_entry_session();
    let forked = s.fork_at_inclusive(2).expect("fork point 2 exists");
    assert_eq!(
        forked.entries.len(),
        2,
        "\"at\" retains the branch point itself, got {}",
        forked.entries.len()
    );
}

/// …and the entries that came after it are gone from the branch, not merely
/// unreachable. A fork that kept them would replay abandoned work.
#[test]
fn entries_after_the_branch_point_are_not_in_the_fork() {
    let s = four_entry_session();
    let forked = s.fork_at(3).expect("fork point 3 exists");
    let text: Vec<String> = forked.entries.iter().map(|e| format!("{e:?}")).collect();
    assert!(
        !text.iter().any(|t| t.contains("fourth")),
        "the fork retained entries past the branch point"
    );
}

/// The parent chain must be rebuilt as a chain. Pi drops `LabelEntry` from
/// the retained path and re-links `parentId` precisely so nothing is
/// orphaned (`session-manager.ts:1424-1428`); nini's re-chain has to do
/// the same or `get_path()` on the fork returns a single entry and
/// compaction, forking and `/tree` all walk one node.
#[test]
fn the_retained_path_is_rechained_into_a_chain() {
    let s = four_entry_session();
    let forked = s.fork_at_inclusive(4).expect("fork point 4 exists");

    let path = forked.get_path();
    assert_eq!(
        path.len(),
        forked.entries.len(),
        "the fork's entries are not reachable as one path — the parent links \
         do not chain (get_path returned {} of {} entries)",
        path.len(),
        forked.entries.len()
    );
    assert_eq!(path.first().unwrap().parent_id(), None);
    for pair in path.windows(2) {
        assert_eq!(
            pair[1].parent_id(),
            Some(pair[0].id()),
            "entry {} does not point at its predecessor {}",
            pair[1].id(),
            pair[0].id()
        );
    }
}

/// Entry ids are preserved so that anything referring to an entry (a label,
/// a compaction's `firstKeptEntryId`) still resolves inside the fork.
#[test]
fn entry_ids_survive_the_fork() {
    let s = four_entry_session();
    let before: Vec<String> = s.get_path().iter().map(|e| e.id().to_string()).collect();
    let forked = s.fork_at(3).expect("fork point 3 exists");
    let after: Vec<String> = forked
        .get_path()
        .iter()
        .map(|e| e.id().to_string())
        .collect();
    assert_eq!(
        &before[..2],
        &after[..],
        "forking renumbered the retained entries"
    );
}

/// Forking at a point that is not in the session is a caller error, not a
/// silent empty fork.
#[test]
fn an_unknown_fork_point_is_rejected() {
    let s = four_entry_session();
    assert!(
        s.fork_at(999).is_err(),
        "forking at a nonexistent entry produced a session instead of an error"
    );
}

/// Forking from the very first entry yields a session with a header and no
/// entries — the "start over from the top" case.
#[test]
fn forking_at_the_first_entry_yields_an_empty_branch() {
    let s = four_entry_session();
    let forked = s.fork_at(1).expect("fork point 1 exists");
    assert_eq!(
        forked.entries.len(),
        0,
        "branching before the very first message leaves nothing behind"
    );
    assert_eq!(forked.header.cwd, s.header.cwd, "the cwd must carry over");
}

/// The fork must be written to a **different file** — Pi derives
/// `<timestamp>_<newSessionId>.jsonl` (`session-manager.ts:1433`). Writing
/// over the parent's path would destroy the conversation the user forked
/// from.
#[test]
fn the_fork_path_differs_from_the_parent_path() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("a.jsonl");
    let s = four_entry_session();
    s.write_to_file(&parent).expect("parent written");

    let forked = s.fork_at(2).expect("fork point 2 exists");
    let path = nini_session::fork_path_for(&dir.path().to_path_buf(), &forked.header);
    assert_ne!(path, parent);
    assert_eq!(
        path.extension().and_then(|e| e.to_str()),
        Some("jsonl"),
        "the fork file is not a session file: {path:?}"
    );
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&forked.header.id),
        "the fork file name must carry the new session id, got {:?}",
        path.file_name()
    );
}

/// The point of a branch is that it can be *continued*: the model context
/// derived from the forked file must be exactly the retained prefix, with
/// nothing from the abandoned branch leaking in.
///
/// This is the property that `/fork` exists for. A fork that writes a file
/// but derives the wrong context is worse than no fork, because the
/// conversation the user resumes silently contains work they just discarded.
#[test]
fn the_fork_derives_a_usable_model_context() {
    let s = four_entry_session();
    let forked = s.fork_at_inclusive(3).expect("fork point 3 exists");

    let context = nini_session::build_context_entries(&forked);
    let llm = nini_core::conversion::session_entry_to_llm_message;
    let texts: Vec<String> = context
        .iter()
        .filter_map(llm)
        .map(|m| {
            m.content
                .iter()
                .map(|b| match b {
                    nini_core::provider::ContentBlock::Text { text } => text.clone(),
                    _ => String::new(),
                })
                .collect::<String>()
        })
        .collect();

    assert_eq!(
        texts,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "the fork's model context is not the retained prefix"
    );

    // And it round-trips through the file format, because that is where a
    // fork actually lives.
    let dir = tempfile::tempdir().unwrap();
    let path = nini_session::fork_path_for(dir.path(), &forked.header);
    forked.write_to_file(&path).expect("fork written");
    let reloaded = nini_session::Session::read_from_file(&path).expect("fork readable");
    assert_eq!(reloaded.entries.len(), forked.entries.len());
    assert_eq!(
        reloaded.header.id, forked.header.id,
        "the id changed across a write/read cycle — /resume and the file name \
         would name a session that does not exist"
    );
    assert_eq!(
        reloaded.header.parent_session, forked.header.parent_session,
        "the parent link did not survive the round trip"
    );
}

/// `/tree` and `/fork` share one thing: a tree with more than one leaf. Before
/// this change a fork produced no second leaf, so `/tree` could only ever
/// draw a straight line — the gap v0.9.2 recorded as *"/fork creates no
/// branches"*. The branch exists in the **fork's own file**; the parent's
/// tree is untouched, which is why the parent is still resumable.
#[test]
fn the_parent_session_is_left_intact() {
    let s = four_entry_session();
    let before = s.get_path().len();
    let forked = s.fork_at(2).expect("fork point 2 exists");
    // The fork must not mutate its source.
    assert_eq!(s.get_path().len(), before, "forking mutated the parent");
    assert_eq!(s.entries.len(), 4, "entries were removed from the parent");
    assert_eq!(
        s.header.id,
        forked.header.parent_session.clone().unwrap(),
        "the parent no longer identifies itself"
    );
}
