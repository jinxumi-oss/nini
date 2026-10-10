//! The shape of a session: which entry is the parent of which.
//!
//! # What this was
//!
//! ```text
//! //! Placeholder SessionTree stub.
//! pub struct SessionTree;
//! from_entries(_e) -> Self { Self }
//! main_path() -> vec![]
//! len() -> 0
//! is_empty() -> true
//! get_path_to(_id) -> vec![]
//! descendants(_id) -> vec![]
//! ```
//!
//! The doc comment was honest, and it did not matter, because `cmd_tree`
//! calls this. `/tree` read the session file, handed every entry to
//! `from_entries`, and then printed the stub's answers:
//!
//! ```text
//! session tree: 0 entries, main path: 0 nodes
//! (empty — use /new to start a session)
//! ```
//!
//! for every session, including one with fifty entries and a full parent
//! chain. The command told the user their conversation did not exist.
//!
//! `cmd_tree` also computed a label per node through
//! `get_path_to(..).map(|_| String::new())` - it asked the tree two
//! questions and then discarded both answers to produce an empty string.
//!
//! # What the entries already carry
//!
//! `SessionEntry` has had `parentId` since the session-tree work, and
//! `Session` already routes writes through `set_parent_id` so the shape is
//! a property of the session rather than of whoever rendered it. Nothing
//! here invents a format; it reads the one already on disk.
//!
//! # Choices this makes
//!
//! * **The main path is the active branch**: the chain from the root to the
//!   most recently appended entry. `/tree` renders that as the spine and
//!   everything else as a branch off it, which is what its own comment
//!   describes ("Show non-main branches (descendants of nodes not in the
//!   main path)").
//! * **A dangling `parentId` is treated as a root**, not as an error. The
//!   conformance fixture had 21 non-null parents of which only 4 resolved,
//!   and a tree that refuses to render because a reference is broken is
//!   less useful than one that renders what it can. The node is still
//!   counted by [`SessionTree::len`].
//! * **A cycle cannot hang the render.** `get_path_to` and `descendants`
//!   both bound their work by the entry count; a hand-edited file with
//!   `a -> b -> a` cannot produce an infinite walk.

use nini_core::entries::SessionEntry;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Default, Clone)]
pub struct SessionTree {
    /// Every entry's parent, in file order. The order is the append
    /// order, which is what "most recent" means.
    order: Vec<String>,
    parent_of: HashMap<String, String>,
    children_of: HashMap<String, Vec<String>>,
    roots: Vec<String>,
}

impl SessionTree {
    pub fn from_entries(entries: &[SessionEntry]) -> Self {
        let mut t = Self::default();

        for e in entries {
            let id = e.id().to_string();
            if t.parent_of.contains_key(&id) {
                // A duplicated id would otherwise make `children_of` hold
                // the same child twice and inflate every count.
                continue;
            }
            t.order.push(id.clone());
            t.parent_of.insert(id.clone(), String::new());
            if let Some(p) = e.parent_id() {
                t.parent_of.insert(id.clone(), p.to_string());
            }
        }

        let known: HashSet<&String> = t.order.iter().collect();
        for id in &t.order {
            match t.parent_of.get(id).filter(|p| !p.is_empty()) {
                Some(p) if known.contains(p) => {
                    t.children_of.entry(p.clone()).or_default().push(id.clone());
                }
                _ => t.roots.push(id.clone()),
            }
        }

        t
    }

    /// How many entries the tree was built from.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The active branch: root first, most recent entry last.
    pub fn main_path(&self) -> Vec<String> {
        let Some(leaf) = self.order.last() else {
            return Vec::new();
        };
        self.get_path_to(leaf)
    }

    /// Every ancestor of `id`, root first, with `id` itself last.
    ///
    /// Unknown ids give an empty path rather than a partial one: a caller
    /// asking about something not in the session has a bug, and handing it
    /// half a chain would hide that.
    pub fn get_path_to(&self, id: impl std::fmt::Display) -> Vec<String> {
        let id = id.to_string();
        if !self.parent_of.contains_key(&id) {
            return Vec::new();
        }
        let mut path = vec![id.clone()];
        let mut seen: HashSet<&String> = HashSet::new();
        seen.insert(self.order.iter().find(|x| **x == id).unwrap());
        let mut cur = id;
        // Bounded by the entry count: a cycle in a hand-edited file stops
        // here instead of looping.
        while path.len() <= self.order.len() {
            let Some(parent) = self.parent_of.get(&cur) else {
                break;
            };
            if parent.is_empty() || !self.order.contains(parent) {
                break;
            }
            if !seen.insert(self.order.iter().find(|x| **x == *parent).unwrap()) {
                break;
            }
            path.push(parent.clone());
            cur = parent.clone();
        }
        path.reverse();
        path
    }

    /// Everything below `id`, breadth first, excluding `id` itself.
    pub fn descendants<D: std::fmt::Display>(&self, id: D) -> Vec<String> {
        let id = id.to_string();
        if !self.parent_of.contains_key(&id) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(id.clone());
        let mut queue: VecDeque<String> = VecDeque::from(vec![id]);
        while let Some(node) = queue.pop_front() {
            for child in self.children_of.get(&node).into_iter().flatten() {
                if seen.insert(child.clone()) {
                    out.push(child.clone());
                    queue.push_back(child.clone());
                }
            }
        }
        out
    }

    /// Entries at the top of the tree: no parent, or a parent this file
    /// does not contain.
    pub fn roots(&self) -> &[String] {
        &self.roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nini_core::entries::{
        AgentMessage, SessionMessageEntry, StringOrContentBlocks, UserMessage,
    };

    /// Built from the real wire types, copied from how `nini-session`'s own
    /// tests construct entries - an earlier version of this file invented a
    /// `SessionEntry::message(..)` constructor that does not exist and did
    /// not compile.
    fn entry(id: &str, parent: Option<&str>) -> SessionEntry {
        SessionEntry::Message(SessionMessageEntry {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            message: AgentMessage::User(UserMessage {
                content: StringOrContentBlocks::String(String::new()),
                timestamp: 0,
            }),
        })
    }

    #[test]
    fn a_single_entry_is_its_own_main_path() {
        let t = SessionTree::from_entries(&[entry("a", None)]);
        assert_eq!(t.len(), 1);
        assert!(!t.is_empty());
        assert_eq!(t.main_path(), vec!["a"]);
        assert!(t.descendants("a").is_empty());
    }

    #[test]
    fn a_linear_session_reads_as_a_chain() {
        let t = SessionTree::from_entries(&[
            entry("a", None),
            entry("b", Some("a")),
            entry("c", Some("b")),
        ]);
        assert_eq!(t.len(), 3);
        assert_eq!(t.main_path(), vec!["a", "b", "c"]);
        assert_eq!(t.get_path_to("c"), vec!["a", "b", "c"]);
        assert_eq!(t.get_path_to("a"), vec!["a"]);
        assert_eq!(t.descendants("a"), vec!["b", "c"]);
        assert!(t.descendants("c").is_empty());
    }

    /// `/tree` exists to show where a conversation forked, so this is the
    /// case the command is for.
    #[test]
    fn a_fork_puts_the_newer_branch_on_the_main_path() {
        let t = SessionTree::from_entries(&[
            entry("a", None),
            entry("b", Some("a")),
            entry("c", Some("a")), // fork
            entry("d", Some("b")),
            entry("e", Some("c")),
        ]);
        // `e` was appended last, so the active branch is a-c-e.
        assert_eq!(t.main_path(), vec!["a", "c", "e"]);
        assert_eq!(t.descendants("a"), vec!["b", "c", "d", "e"]);
        assert_eq!(t.descendants("c"), vec!["e"]);
    }

    #[test]
    fn an_empty_session_is_empty_rather_than_wrong() {
        let t = SessionTree::from_entries(&[]);
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert!(t.main_path().is_empty());
        assert!(t.descendants("nothing").is_empty());
        assert!(t.get_path_to("nothing").is_empty());
    }

    /// The conformance fixture had 21 non-null parents of which only 4
    /// resolved. A tree that refuses to render is less useful than one that
    /// renders what it can.
    #[test]
    fn a_dangling_parent_is_a_root_not_an_error() {
        let t =
            SessionTree::from_entries(&[entry("a", Some("never-written")), entry("b", Some("a"))]);
        assert_eq!(t.len(), 2);
        assert_eq!(t.roots(), ["a"]);
        assert_eq!(t.main_path(), vec!["a", "b"]);
        assert_eq!(t.get_path_to("b"), vec!["a", "b"]);
    }

    /// A hand-edited file must not hang `/tree`.
    #[test]
    fn a_cycle_terminates() {
        let mut entries = vec![entry("a", Some("b")), entry("b", Some("a"))];
        entries.push(entry("z", Some("a")));
        let t = SessionTree::from_entries(&entries);

        let path = t.get_path_to("a");
        assert!(
            path.len() <= 3,
            "cycle produced an unbounded path: {path:?}"
        );
        let desc = t.descendants("a");
        assert!(
            desc.len() <= 2,
            "cycle produced an unbounded walk: {desc:?}"
        );
    }

    #[test]
    fn an_unknown_id_gives_nothing_rather_than_a_partial_chain() {
        let t = SessionTree::from_entries(&[entry("a", None), entry("b", Some("a"))]);
        assert!(t.get_path_to("nope").is_empty());
        assert!(t.descendants("nope").is_empty());
    }

    /// A duplicated id would otherwise appear twice in `children_of` and
    /// inflate every count `/tree` prints.
    #[test]
    fn a_duplicated_id_is_counted_once() {
        let t =
            SessionTree::from_entries(&[entry("a", None), entry("a", None), entry("b", Some("a"))]);
        assert_eq!(t.len(), 2);
        assert_eq!(t.descendants("a"), vec!["b"]);
    }
}
