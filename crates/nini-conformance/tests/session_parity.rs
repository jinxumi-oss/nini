//! Oracle: nini must read Pi 0.84.2 session files without losing a line.
//!
//! Acceptance criterion, fixed in advance: **`skipped == 0`**.
//!
//! The fixture is real Pi output with only *values* scrubbed (text bodies,
//! paths, UUIDs) — every key name, enum value, and container shape is
//! verbatim, so a schema divergence still fails here.

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

#[test]
fn the_fixture_itself_covers_the_whole_schema() {
    // A conformance fixture that silently stops covering something is worse
    // than no fixture: it reports green while the gap is untested. This
    // guards the guard.
    let path = fixture("pi-0.84.2-session.jsonl");
    let (verdicts, _) = diagnose(&path);

    let mut types: std::collections::BTreeSet<&str> = Default::default();
    let mut roles: std::collections::BTreeSet<String> = Default::default();
    let mut blocks: std::collections::BTreeSet<String> = Default::default();
    for v in &verdicts {
        if let Some(t) = v.pi_type.as_deref() {
            types.insert(t);
        }
    }
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let Some(m) = v.get("message") else { continue };
        if let Some(r) = m.get("role").and_then(|r| r.as_str()).map(str::to_owned) {
            roles.insert(r);
        }
        if let Some(cs) = m.get("content").and_then(|c| c.as_array()) {
            for b in cs {
                if let Some(t) = b.get("type").and_then(|t| t.as_str()).map(str::to_owned) {
                    blocks.insert(t);
                }
            }
        }
    }

    for t in [
        "session",
        "message",
        "model_change",
        "thinking_level_change",
        "custom",
        "custom_message",
        "compaction",
    ] {
        assert!(
            types.contains(t),
            "fixture no longer covers entry type `{t}`"
        );
    }
    for r in ["user", "assistant", "toolResult", "custom"] {
        assert!(
            roles.contains(r),
            "fixture no longer covers message role `{r}`"
        );
    }
    for b in ["text", "thinking", "toolCall", "image"] {
        assert!(
            blocks.contains(b),
            "fixture no longer covers content block `{b}`"
        );
    }
}
