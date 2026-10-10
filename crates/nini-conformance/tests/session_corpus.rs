//! Oracle against the operator's whole real session corpus, when present.
//!
//! Skipped in CI (no personal sessions) but it is the measurement that
//! matters: the committed fixture is 24 lines, the corpus is ~60 000. Before
//! v0.10 (R7) nini dropped 28 808 of 59 959 lines (48%) here.
//!
//! Point `NINI_PI_SESSION_CORPUS` at a directory of Pi-written sessions to
//! run it, e.g.
//!   NINI_PI_SESSION_CORPUS=~/.nini/agent/sessions cargo test -p nini-conformance
//!
//! Files nini itself wrote are excluded: they use the old snake_case keys and
//! would otherwise be counted as Pi incompatibility rather than as the
//! backwards-compatibility check they actually are.

use nini_conformance::{diagnose, summarise};
use std::path::PathBuf;

/// Pi 0.84.2 writes `stopReason`; pre-R7 nini wrote `stop_reason`.
fn looks_like_pi(path: &PathBuf) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(200) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("message") {
            if let Some(m) = v.get("message") {
                if m.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                    return m.get("stopReason").is_some();
                }
            }
        }
    }
    false
}

#[test]
fn whole_real_corpus_is_read_without_loss() {
    let Ok(root) = std::env::var("NINI_PI_SESSION_CORPUS") else {
        eprintln!("skipped: set NINI_PI_SESSION_CORPUS to run against real Pi sessions");
        return;
    };
    let mut files: Vec<PathBuf> = walk(&PathBuf::from(&root));
    files.retain(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false));
    files.retain(looks_like_pi);
    assert!(!files.is_empty(), "no Pi-written sessions under {root}");
    files.sort();

    let (mut total, mut failed) = (0usize, 0usize);
    let mut by_type: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for f in &files {
        let (v, _) = diagnose(f);
        let (t, fa, table) = summarise(&v);
        total += t;
        failed += fa;
        for (ty, n, e) in table {
            let slot = by_type.entry(ty).or_insert((0, e.clone()));
            slot.0 += n;
        }
    }
    assert_eq!(
        failed,
        0,
        "nini dropped {failed}/{total} lines across {} real Pi sessions.\n\
         by entry type:\n{}",
        files.len(),
        by_type
            .iter()
            .map(|(t, (n, e))| format!("  {n:>6} × {t:<24} {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn sessions_nini_wrote_before_r7_are_still_readable() {
    // The `alias` attributes added in R7 exist for this: two sessions on the
    // operator's disk predate the fix and use snake_case keys. Dropping them
    // would trade a Pi-parity bug for a data-loss bug.
    let snake = r#"{"type":"message","id":"a1","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"api":"a","provider":"p","model":"m","usage":{"input":1,"output":2},"stop_reason":"toolUse","error_message":null,"timestamp":1}}"#;
    let v: nini_core::entries::SessionEntry =
        serde_json::from_str(snake).expect("pre-R7 nini output must still parse");
    match &v {
        nini_core::entries::SessionEntry::Message(m) => match &m.message {
            nini_core::entries::AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, nini_core::entries::StopReason::ToolUse);
            }
            other => panic!("expected assistant, got {other:?}"),
        },
        other => panic!("expected message entry, got {other:?}"),
    }
}

fn walk(dir: &PathBuf) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
