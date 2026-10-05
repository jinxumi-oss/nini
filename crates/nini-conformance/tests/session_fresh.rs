//! Oracle: a session written *minutes ago* by the installed Pi 0.84.2.
//!
//! This fixture is the strongest available evidence — it is not a historical
//! file, not a hand-written sample, and not nini's own output. It was
//! produced by `pi -p` on this machine, then scrubbed of values only.
//!
//! It exists because the version/format question produced three mutually
//! contradictory readings before it was settled empirically:
//!
//!   1. `references/spec-v0.85.1/` declares a v4 format with a `v` field.
//!   2. A single session on disk uses that v4 shape (`{"v":4,...}`).
//!   3. `pi-agent-core/dist/harness/session/jsonl/codec.js` implements v4 with
//!      a `version` field and rejects anything else — and contains no v3 code.
//!
//! Reading (3) suggested Pi 0.84.2 could not read or write v3 at all, which
//! would have made the whole v3 parity target wrong. Running the installed
//! binary settled it: the CLI writes v3. The v4 codec is a second storage
//! path that the default session writer does not use.
//!
//! Do not "fix" this test by regenerating the fixture from a different
//! source. The provenance is the point.

use nini_conformance::{diagnose, fixture, summarise};

#[test]
fn reads_a_session_written_by_the_installed_pi_minutes_ago() {
    let path = fixture("pi-0.84.2-fresh.jsonl");
    assert!(path.exists(), "fixture missing: {}", path.display());

    let (verdicts, _) = diagnose(&path);
    let (total, failed, table) = summarise(&verdicts);

    assert_eq!(
        total, 5,
        "expected header + model_change + thinking_level_change + user + assistant"
    );
    assert!(
        failed == 0,
        "nini dropped {failed}/{total} lines of a session Pi 0.84.2 wrote minutes ago.\n\
         failures by Pi entry type:\n{}",
        table
            .iter()
            .map(|(t, n, e)| format!("  {n:>4} × {t:<24} {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
