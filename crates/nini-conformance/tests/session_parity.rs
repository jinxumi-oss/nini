//! Oracle: nini must read Pi 0.84.2 session files without losing a line.
//!
//! Acceptance criterion, fixed in advance: **`skipped == 0`**.
//!
//! The fixture is real Pi output with values scrubbed (text bodies,
//! paths, UUIDs) — every key name, enum value, and container shape is
//! verbatim, so a schema divergence still fails here.
//!
//! **Scope, stated precisely because it is easy to over-read.** This is
//! line-level schema coverage. It says nothing about the session *graph*:
//! scrubbing replaced each entry id with an 8-character hash without
//! carrying the change through to `parentId` references, so most parents
//! in that file do not resolve. `the_fixture_cannot_prove_anything_about
//! _the_graph` below pins that fact so it cannot be mistaken for a
//! property of Pi, and `fixtures/README.md` records it in full.
//!
//! A related trap, already sprung once: the dangling links read like a
//! nini bug, and a tree test written against this file fails for reasons
//! that have nothing to do with nini. `nini-tui/tests/p02_tree.rs` builds
//! its own tree instead.

use nini_conformance::{diagnose, fixture, summarise};

#[test]
fn reads_every_line_of_a_real_pi_session() {
    let path = fixture("pi-0.84.2-session.jsonl");
    assert!(path.exists(), "fixture missing: {}", path.display());

    let (verdicts, _) = diagnose(&path);
    let (total, failed, table) = summarise(&verdicts);

    assert!(
        failed == 0,
        "nini dropped {failed}/{total} lines of a real Pi 0.84.2 session.\n\
         This is not a malformed-input case: every line below was written by Pi itself.\n\n\
         failures by Pi entry type:\n{}",
        if table.is_empty() {
            "  (none)".to_string()
        } else {
            table
                .iter()
                .map(|(t, n, e)| format!("  {n:>4} × {t:<24} {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );
}

/// Pin the fixture's known limitation.
///
/// Without this, the dangling `parentId` references look like a nini bug or
/// like a property of Pi, and the next person to write a tree test against
/// this file loses an hour to it. Asserting the limitation is the honest
/// form: it is true, it is deliberate, and it is not going to change under
/// us.
///
/// If this test ever fails because someone *did* repair the links, the
/// right response is to update `fixtures/README.md`, not to relax this.
#[test]
fn the_fixture_cannot_prove_anything_about_the_graph() {
    let path = fixture("pi-0.84.2-session.jsonl");

    let mut ids: Vec<String> = Vec::new();
    let mut parents: Vec<String> = Vec::new();
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("session") {
            continue;
        }
        ids.push(
            v.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .into(),
        );
        if let Some(p) = v.get("parentId").and_then(|p| p.as_str()) {
            parents.push(p.into());
        }
    }

    let known: std::collections::HashSet<&String> = ids.iter().collect();
    let resolving = parents.iter().filter(|p| known.contains(p)).count();

    assert!(
        resolving * 2 < parents.len(),
        "expected most parentId references in this fixture to dangle, since \
         scrubbing replaced ids without carrying the change through to \
         parents. {resolving}/{} now resolve, which means the file changed \
         — update fixtures/README.md rather than this assertion.",
        parents.len()
    );
}
