//! invariant: 8744ceb5
//!
//! # The invariant
//!
//! The stop-reason notice must have a **producer**. `app.rs` is the only
//! place in nini that sees `AgentEvent::TurnEnd { stop_reason, .. }`, so
//! if that match arm stops emitting `AgentEventLite::StopReason`, the
//! variant and its renderer become dead code and no user is ever told
//! their turn was truncated, aborted, or failed.
//!
//! Concretely: inside the `AgentEvent::TurnEnd` arm, `length` /
//! `aborted` / `error` each push a `StopReason` carrying a distinct
//! label, the notice is pushed **before** the terminal `TurnEnd` event,
//! and every other stop reason (the happy path: `end_turn`,
//! `tool_use`) pushes nothing.
//!
//! # Why nothing asserted it
//!
//! The exact shape of this defect is stated in `8744ceb5`'s own message:
//! "Without this, the variant existed but was never produced." A variant
//! nobody pushes is not a crash, not a lint, and not a type error — the
//! whole 1400-test suite was green while the feature did nothing. Worse,
//! the *consumer* half was fully tested-looking (`render.rs` had a
//! `StopNotice` arm), so the code read as wired on both ends and in the
//! middle there was air.
//!
//! # Why this file reads the source instead of calling a function
//!
//! The match lives inside `pub(crate) async fn run_tui` in a
//! **binary-only** crate: there is no lib target, so no integration test
//! can call it, and spawning the TUI against a provider that ends a turn
//! with `finish_reason: "length"` would be a slow, flaky stand-in for a
//! two-line structural fact. So this file parses the arm out of
//! `src/app.rs` and asserts its shape — which is exactly the property
//! that regressed. The *strings* it asserts are not the implementation's
//! own: they are cross-checked against the installed Pi's own
//! `assistant-message.js` when it can be found, so a "parity" label that
//! drifted from Pi is caught too.

use std::path::{Path, PathBuf};

const APP_RS: &str = include_str!("../src/app.rs");

/// Extract the body of the match arm introduced by `needle`: from the
/// `{` that opens its **block** to its matching `}`. The `=>` is the
/// anchor, because a struct-pattern arm's own braces (`TurnEnd { usage,
/// .. }`) come before it.
fn arm_body(needle: &str) -> String {
    let start = APP_RS
        .find(needle)
        .unwrap_or_else(|| panic!("no match arm starting with {needle:?} in app.rs"));
    let arrow = APP_RS[start..]
        .find("=>")
        .map(|i| start + i)
        .unwrap_or_else(|| panic!("match arm {needle:?} has no body"));
    let open = APP_RS[arrow..]
        .find('{')
        .map(|i| arrow + i)
        .unwrap_or_else(|| panic!("match arm {needle:?} has no block"));
    let bytes = APP_RS.as_bytes();
    let mut depth = 0usize;
    for i in open..bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return APP_RS[open..=i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in the {needle:?} arm");
}

fn turn_end_arm() -> String {
    arm_body("Ok(AgentEvent::TurnEnd {")
}

/// Locate Pi's compiled `assistant-message.js` — the component that
/// draws the end-of-turn stop notice, and therefore the string nini is
/// claiming parity with. Overridable for machines that install Pi
/// somewhere unusual.
fn pi_assistant_message() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("NINI_PI_ROOT") {
        let p = Path::new(&root).join("dist/modes/interactive/components/assistant-message.js");
        if p.exists() {
            return Some(p);
        }
    }
    let bases = [
        "/home/jin/.nvm/versions/node",
        "/usr/lib/node_modules",
        "/usr/local/lib/node_modules",
    ];
    for base in bases {
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e
                .path()
                .join("lib/node_modules/@earendil-works/pi-coding-agent")
                .join("dist/modes/interactive/components/assistant-message.js");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// The labels nini emits, as (stop_reason, label) pairs, read off the arm.
fn labels() -> Vec<(String, String)> {
    let arm = turn_end_arm();
    let mut out = Vec::new();
    for reason in ["length", "aborted", "error"] {
        let at = arm
            .find(&format!("\"{reason}\""))
            .unwrap_or_else(|| panic!("the TurnEnd arm does not mention stop reason {reason:?}"));
        // The label is the first string literal after the reason literal.
        let rest = &arm[at + reason.len() + 2..];
        let open = rest
            .find('"')
            .unwrap_or_else(|| panic!("no label string after {reason:?}"));
        let rest = &rest[open + 1..];
        let close = rest
            .find('"')
            .unwrap_or_else(|| panic!("unterminated label string after {reason:?}"));
        out.push((reason.to_string(), rest[..close].to_string()));
    }
    out
}

/// The heart of it: the arm that receives the turn end must produce the
/// notice. Everything else in this file is a refinement.
#[test]
fn the_turn_end_arm_produces_the_stop_reason_notice() {
    let arm = turn_end_arm();
    assert!(
        arm.contains("AgentEventLite::StopReason("),
        "the AgentEvent::TurnEnd arm no longer pushes a StopReason — the \
         variant exists in nini-tui and has a renderer, but nothing ever \
         produces it, so a truncated or aborted turn ends silently:\n{arm}"
    );
}

/// And it must not produce it for the happy path. `end_turn` and
/// `tool_use` are the normal endings; a notice on every turn would be a
/// permanent error-coloured line under every answer.
#[test]
fn the_happy_path_stop_reasons_produce_no_notice() {
    let arm = turn_end_arm();
    let match_at = arm
        .find("match stop_reason")
        .unwrap_or_else(|| panic!("the TurnEnd arm no longer matches on stop_reason:\n{arm}"));
    let rest = &arm[match_at..];
    assert!(
        rest.contains("_ => {}"),
        "the stop-reason match has no catch-all arm, so an unrecognised \
         stop reason falls through to whatever comes next — or to nothing \
         at all:\n{rest}"
    );
    for happy in ["\"end_turn\"", "\"tool_use\""] {
        assert!(
            !arm.contains(happy),
            "the TurnEnd arm special-cases {happy}: a successful turn must \
             not draw a stop notice\n{arm}"
        );
    }
}

/// Order matters: the notice belongs to the turn it ends, so it must be
/// pushed before the terminal `TurnEnd` event. After it, the transcript
/// reads as if the turn ended cleanly.
#[test]
fn the_notice_is_pushed_before_the_terminal_turn_end_event() {
    let arm = turn_end_arm();
    let notice = arm
        .find("AgentEventLite::StopReason(")
        .expect("no StopReason in the TurnEnd arm");
    let end = arm
        .rfind("AgentEventLite::TurnEnd")
        .expect("no terminal TurnEnd in the arm");
    assert!(
        notice < end,
        "the notice is pushed after the terminal TurnEnd event, so it lands \
         on the wrong side of the turn boundary"
    );
}

/// The three labels are distinct — three different failures, three
/// different sentences. Two arms sharing a string is a copy/paste that
/// made the fix look done.
#[test]
fn the_three_stop_reasons_get_three_distinct_labels() {
    let got = labels();
    let mut seen = std::collections::BTreeSet::new();
    for (reason, label) in &got {
        assert!(
            !label.is_empty(),
            "stop reason {reason:?} maps to an empty label — the user sees \
             a blank coloured line and no explanation"
        );
        assert!(
            seen.insert(label.clone()),
            "two stop reasons share the label {label:?}: {got:?}"
        );
    }
    assert_eq!(got.len(), 3, "expected length/aborted/error, got {got:?}");
}

/// Pi's source, if this machine has it. See `pi_assistant_message`.
fn pi_source() -> Option<String> {
    let path = pi_assistant_message()?;
    Some(std::fs::read_to_string(&path).expect("pi's assistant-message.js is unreadable"))
}

/// Parity, checked against Pi's own source rather than against a comment
/// in nini that says it matches.
///
/// Two different claims are checked, because Pi only makes one of them
/// literal:
///
/// * `length` is a **fixed literal** in Pi. nini must use it byte for
///   byte, so that is asserted exactly.
/// * `aborted` / `error` are **composed** in Pi (`message.errorMessage`,
///   `Error: ${errorMsg}`), so there is no single literal to match.
///   What must agree is the *set of stop reasons that get a notice*.
///
/// Measured divergence, recorded rather than asserted, because nini is
/// the one shipping the fixed label: Pi writes `"Operation aborted"`
/// (no full stop) where nini writes `"Operation aborted."`, and Pi
/// prints the provider's message after `Error: ` where nini prints
/// `"Error during generation."`. So nini's `pi-parity` comment in
/// `app.rs` is accurate for the truncated case and loose for the other
/// two — noted here so the next reader does not take it as verified.
#[test]
fn the_truncated_label_is_pi_s_literal_byte_for_byte() {
    let Some(src) = pi_source() else {
        eprintln!(
            "Pi's assistant-message.js not found — skipping the parity \
             cross-check. Set NINI_PI_ROOT to the pi-coding-agent package \
             directory to run it."
        );
        return;
    };
    let labels = labels();
    let length_label = labels
        .iter()
        .find(|(r, _)| r == "length")
        .expect("no length label")
        .1
        .clone();
    assert!(
        src.contains(&length_label),
        "nini's truncated-turn label {length_label:?} is not Pi's literal — \
         Pi draws a different sentence for a truncated turn, so this is no \
         longer parity"
    );
}

/// The set of stop reasons that earn a notice is the thing parity really
/// depends on: miss one and the user gets silence for that failure.
#[test]
fn nini_and_pi_agree_on_which_stop_reasons_get_a_notice() {
    let Some(src) = pi_source() else {
        eprintln!("Pi not installed — skipping the stop-reason set comparison.");
        return;
    };
    for reason in ["length", "aborted", "error"] {
        assert!(
            src.contains(&format!("stopReason === \"{reason}\"")),
            "Pi's assistant-message.js no longer special-cases {reason:?}, so \
             nini's handling of it has no counterpart to be parity with"
        );
    }
    let nini_reasons: Vec<String> = labels().into_iter().map(|(r, _)| r).collect();
    assert_eq!(
        nini_reasons,
        vec!["length", "aborted", "error"],
        "nini handles a different set of stop reasons than Pi does"
    );
}
