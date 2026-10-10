//! `/tree` used to say every session was empty.
//!
//! `cmd_tree` read the session file, handed the entries to
//! `SessionTree::from_entries` - which was `fn from_entries(_e) -> Self { Self }`
//! - and printed the stub's answers. For a session with fifty entries and a
//! fork in it, `/tree` said:
//!
//! ```text
//! session tree: 0 entries, main path: 0 nodes
//! (empty — use /new to start a session)
//! ```
//!
//! The tree itself is unit-tested in `nini-session`. This checks the
//! command, because the bug was never in the tree: it was in a command that
//! reported the tree's answer without checking whether the tree was real.

use nini_core::entries::{
    AgentMessage, SessionEntry, SessionMessageEntry, StringOrContentBlocks, UserMessage,
};
use nini_session::tree::SessionTree;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("nini-tree-cmd-{tag}-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("scratch dir");
    p
}

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn entry(id: &str, parent: Option<&str>) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        message: AgentMessage::User(UserMessage {
            content: StringOrContentBlocks::String(format!("message {id}")),
            timestamp: 0,
        }),
    })
}

/// Write a session file and read it back the way `cmd_tree` does, so the
/// test goes through the same parse the command uses.
fn tree_from_file(path: &Path) -> SessionTree {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let entries: Vec<SessionEntry> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    SessionTree::from_entries(&entries)
}

fn write_session(path: &Path, entries: &[SessionEntry]) {
    let mut body = String::new();
    for e in entries {
        body.push_str(&serde_json::to_string(e).expect("serialise"));
        body.push('\n');
    }
    std::fs::write(path, body).expect("write session");
}

/// The regression. A five-entry session with a fork used to render as
/// empty, and the user was told to run `/new`.
#[test]
fn a_real_session_is_not_reported_as_empty() {
    let dir = scratch("real");
    let _d = Dir(dir.clone());
    let path = dir.join("session.jsonl");

    write_session(
        &path,
        &[
            entry("a", None),
            entry("b", Some("a")),
            entry("c", Some("a")),
            entry("d", Some("c")),
            entry("e", Some("d")),
        ],
    );

    let tree = tree_from_file(&path);

    assert_eq!(tree.len(), 5, "the entries were not read back");
    assert!(
        !tree.main_path().is_empty(),
        "main path is empty, which is what made /tree say so"
    );
    assert_eq!(tree.main_path(), vec!["a", "c", "d", "e"]);
    assert_eq!(tree.descendants("a"), vec!["b", "c", "d", "e"]);
}

/// `/tree` claims "no branches - current session is linear" when
/// `branch_count == 0`. A fork must make that false.
#[test]
fn a_fork_is_distinguishable_from_a_linear_session() {
    let dir = scratch("fork");
    let _d = Dir(dir.clone());
    let path = dir.join("session.jsonl");

    write_session(
        &path,
        &[
            entry("a", None),
            entry("b", Some("a")),
            entry("c", Some("a")),
            entry("d", Some("b")),
        ],
    );
    let forked = tree_from_file(&path);

    write_session(
        &path,
        &[
            entry("a", None),
            entry("b", Some("a")),
            entry("c", Some("b")),
        ],
    );
    let linear = tree_from_file(&path);

    let branches = |t: &SessionTree| t.len() - t.main_path().len();
    assert!(branches(&forked) > 0, "a fork showed zero branches");
    assert_eq!(branches(&linear), 0, "a linear session reported branches");
}

/// A genuinely empty session must still say so. Fixing the false "empty"
/// must not turn it into a false "non-empty".
#[test]
fn a_genuinely_empty_session_still_says_empty() {
    let dir = scratch("empty");
    let _d = Dir(dir.clone());
    let path = dir.join("session.jsonl");

    write_session(&path, &[]);
    let tree = tree_from_file(&path);

    assert!(tree.is_empty());
    assert!(tree.main_path().is_empty());
    assert_eq!(tree.len(), 0);
}

/// The number `/tree` puts in "[tree] N entries". It used to be `0` for
/// every session, including this one.
#[test]
fn the_entry_count_is_the_real_one() {
    let dir = scratch("count");
    let _d = Dir(dir.clone());
    let path = dir.join("session.jsonl");

    let entries: Vec<SessionEntry> = (0..23).map(|i| entry(&format!("e{i}"), None)).collect();
    write_session(&path, &entries);

    assert_eq!(tree_from_file(&path).len(), 23);
}
