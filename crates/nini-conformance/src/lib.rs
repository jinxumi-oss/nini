//! Pi conformance oracles — v0.10 (W0).
//!
//! **This crate contains no product code.** It exists so that "nini behaves
//! like Pi" stops being a claim in a README and becomes something the test
//! suite can falsify.
//!
//! # Why this exists
//!
//! Through v0.9.1 nini had 950 passing tests, zero clippy warnings, and a
//! `SPEC.lock` declaring Pi compatibility. Against that green baseline:
//!
//! - `/copy` printed "copied N bytes to clipboard" while the clipboard was
//!   untouched (`clipboard::copy` was `Ok(())`).
//! - `/logout` printed "credentials unset" after unsetting nothing.
//! - The TUI advertised "loaded N skills" to the user while the system
//!   prompt contained zero skills.
//! - `nini` silently dropped **48%** of the lines in real Pi session files.
//!
//! The common cause is not carelessness; it is that **every test round-tripped
//! nini against itself**. A self-consistent implementation cannot detect a
//! wire-format bug, because the bug is symmetric. nini wrote `stop_reason`
//! and read `stop_reason`; Pi writes and reads `stopReason`; every test in
//! the repo passed.
//!
//! # The rule
//!
//! Fixtures here are derived from **real Pi output**, never from nini's own
//! output. When nini is the only producer, the test cannot fail for the
//! reason that matters.
//!
//! # Baseline
//!
//! Pi **0.84.2** (`@earendil-works/pi-coding-agent`, the version actually
//! installed on the machine that produced these fixtures). Not 0.85.1: the
//! `references/spec-v0.85.1/` tree declares a v4 JSONL format that neither
//! the installed Pi nor its own bundled fixtures actually use, so
//! developing against it means guessing.

/// Absolute path to the committed schema-coverage fixture.
pub fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// How a single JSONL line was classified during a parity run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineVerdict {
    /// 1-based line number in the file.
    pub line: usize,
    /// Pi's `type` discriminator, when the line was valid JSON.
    pub pi_type: Option<String>,
    /// True for the session header (line 1), which is a `SessionHeader`
    /// rather than a `SessionEntry`.
    pub is_header: bool,
    /// Whether nini parsed it.
    pub parsed: bool,
    /// serde's complaint when it did not.
    pub error: Option<String>,
}

/// Read a Pi session file line by line, recording per-line verdicts.
///
/// This deliberately does **not** go through `nini_session::Session`, because
/// that type collapses damage into a single `skipped_lines` counter. The
/// per-line breakdown is what makes the failure actionable — "28 808 lines
/// dropped" tells you nothing; "every `message`/`assistant` line dropped with
/// `missing field stop_reason`" tells you exactly which `#[serde]` attribute
/// is missing.
///
/// Line 1 of a Pi session is the header (`{"type":"session",…}`), which is
/// **not** a `SessionEntry` — it is a distinct `SessionHeader`. v0.10's first
/// oracle run reported it as `unknown variant 'session'`, which was a bug in
/// this function, not in nini. Header and entry are now classified apart, and
/// `is_header` records which is which so a reader of the failure output can
/// tell them apart.
pub fn diagnose(path: &std::path::Path) -> (Vec<LineVerdict>, usize) {
    use nini_core::entries::SessionEntry;
    use nini_session::SessionHeader;
    let raw = std::fs::read_to_string(path).expect("fixture must be readable");
    let mut out = Vec::new();
    let mut parsed = 0usize;
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let is_header = i == 0
            && serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
                .as_deref()
                == Some("session");
        let pi_type = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from));
        let res = if is_header {
            serde_json::from_str::<SessionHeader>(line).map(|_| ())
        } else {
            serde_json::from_str::<SessionEntry>(line).map(|_| ())
        };
        match res {
            Ok(()) => {
                parsed += 1;
                out.push(LineVerdict {
                    line: i + 1,
                    pi_type,
                    is_header,
                    parsed: true,
                    error: None,
                });
            }
            Err(e) => out.push(LineVerdict {
                line: i + 1,
                pi_type,
                is_header,
                parsed: false,
                error: Some(e.to_string()),
            }),
        }
    }
    (out, parsed)
}

/// Aggregate verdicts into `total`, `failed`, and a per-type failure table.
pub fn summarise(verdicts: &[LineVerdict]) -> (usize, usize, Vec<(String, usize, String)>) {
    let total = verdicts.len();
    let failed: Vec<&LineVerdict> = verdicts.iter().filter(|v| !v.parsed).collect();
    let mut per_type: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for v in &failed {
        let t = v.pi_type.clone().unwrap_or_else(|| "<no type>".into());
        let e = per_type.entry(t).or_insert((0, String::new()));
        e.0 += 1;
        if e.1.is_empty() {
            e.1 = v.error.clone().unwrap_or_default();
        }
    }
    let table = per_type.into_iter().map(|(k, (n, e))| (k, n, e)).collect();
    (total, failed.len(), table)
}
