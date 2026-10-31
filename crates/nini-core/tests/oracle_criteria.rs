//! The oracle's **criteria set** must not shrink, widen, or hollow out.
//!
//! v0.15.0, after two review rounds: the differential oracle
//! `scripts/e2e-tool-execution.sh` had **no guard on its own assertions**.
//! Three ways it was proven to pass while proving nothing, all measured:
//!
//! * **(h1)** deleting one criterion's registration — the script exited 0
//!   and the whole workspace stayed green, so from then on nobody checked
//!   the Pi side at all: "nini matches Pi" and "nini matches nobody" became
//!   indistinguishable.
//! * **(k)** deleting a criterion's *check* but keeping its registration —
//!   the criterion is now never evaluated, and the script still exits 0.
//! * **(j)** *widening* a criterion (`== "1"` → `== "1" || == "20"`) —
//!   the script then prints a conclusion that contradicts the very number
//!   it just measured ("sequential 两侧的峰值都是 1" while it measured 20).
//!
//! So the script was restructured: every criterion is a **one-line
//! function** whose body *is* the predicate, `run_criterion` records
//! pass/fail **from the predicate's own exit status**, and the end of the
//! run checks that every listed criterion was recorded as `pass`.
//!
//! That closes (h1) and (k) inside the script. (j) — and "replace the
//! body with `true`" — can only be caught from outside, because a widened
//! predicate still *passes*. **This file is that outside guard**: it pins
//! each criterion's predicate *text*, not just its name.
//!
//! Both halves are needed. The list check catches "a criterion was deleted
//! but the list still names it"; this file catches "someone widened the
//! predicate, or replaced it with `true`, and adjusted the list too".

use std::path::PathBuf;

/// The repository root, from this crate's manifest.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root must exist")
}

fn oracle_script() -> String {
    let path = repo_root().join("scripts/e2e-tool-execution.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The criteria the differential oracle must keep, with the **exact
/// predicate line** each one must be.
///
/// The line is pinned verbatim: `pi_seq_peak_is_1() { [[ "$PI_SEQ_PEAK" == "1" ]]; }`
/// must appear in the script, character for character. Widening it to
/// `[[ "$PI_SEQ_PEAK" == "1" || "$PI_SEQ_PEAK" == "$K" ]]` or replacing it
/// with `true` removes this exact line — and that is the point.
const REQUIRED_CRITERIA: &[(&str, &str)] = &[
    // The Pi side really ran in sequential mode, and really ran one at a
    // time. Two independent pieces of evidence, so a probe that measures
    // wrongly cannot pass by accident.
    (
        "pi_mode_is_sequential",
        r#"pi_mode_is_sequential() { [[ "$PI_SEQ_MODE" == "sequential" ]]; }"#,
    ),
    (
        "pi_seq_peak_is_1",
        r#"pi_seq_peak_is_1() { [[ "$PI_SEQ_PEAK" == "1" ]]; }"#,
    ),
    (
        "pi_seq_overlap_is_1",
        r#"pi_seq_overlap_is_1() { [[ "$PI_SEQ_OVERLAP" == "1" ]]; }"#,
    ),
    // The control group: in parallel mode Pi really does run them all at
    // once. Without it, "nini is always sequential" would satisfy every
    // other criterion.
    (
        "pi_mode_is_parallel",
        r#"pi_mode_is_parallel() { [[ "$PI_PAR_MODE" == "parallel" ]]; }"#,
    ),
    (
        "pi_par_peak_equals_k",
        r#"pi_par_peak_equals_k() { [[ "$PI_PAR_PEAK" == "$K" ]]; }"#,
    ),
    (
        "pi_par_overlap_equals_k",
        r#"pi_par_overlap_equals_k() { [[ "$PI_PAR_OVERLAP" == "$K" ]]; }"#,
    ),
    // The nini side, same three shapes.
    (
        "nini_mode_is_sequential",
        r#"nini_mode_is_sequential() { [[ "$NINI_SEQ_MODE" == "sequential" ]]; }"#,
    ),
    (
        "nini_calls_match_k",
        r#"nini_calls_match_k() { [[ "$NINI_SEQ_CALLS" == "$K" && "$NINI_PAR_CALLS" == "$K" ]]; }"#,
    ),
    (
        "nini_seq_peak_is_1",
        r#"nini_seq_peak_is_1() { [[ "$NINI_SEQ_PEAK" == "1" ]]; }"#,
    ),
    (
        "nini_mode_is_parallel",
        r#"nini_mode_is_parallel() { [[ "$NINI_PAR_MODE" == "parallel" ]]; }"#,
    ),
    (
        "nini_par_peak_equals_k",
        r#"nini_par_peak_equals_k() { [[ "$NINI_PAR_PEAK" == "$K" ]]; }"#,
    ),
];

#[test]
fn every_criterion_keeps_its_exact_predicate() {
    let script = oracle_script();
    for (name, predicate) in REQUIRED_CRITERIA.iter() {
        assert!(
            script.contains(predicate),
            "the differential oracle's criterion `{name}` no longer reads\n\
             {predicate}\n\
             A criterion's predicate may be *widened* (`== \"1\"` → \
             `== \"1\" || == \"20\"`) or *hollowed out* (`true`) without the \
             script noticing — the script only sees a pass. This test is the \
             only guard against that, so the predicate is pinned verbatim."
        );
    }
}

#[test]
fn every_criterion_is_listed_and_executed() {
    let script = oracle_script();
    for (name, _) in REQUIRED_CRITERIA.iter() {
        assert!(
            script.contains(&format!("run_criterion {name}")),
            "the criterion `{name}` must be *executed* — a defined but \
             unrun criterion is exactly case (k): the check is gone while \
             the name survives."
        );
    }
}

#[test]
fn the_oracle_records_whether_a_criterion_passed_or_failed() {
    // The recording is not "this criterion ran" but "this criterion ran and
    // passed": `run_criterion` must take the predicate's exit status.
    // Otherwise emptying the check while keeping the registration would
    // still look like a pass.
    let script = oracle_script();
    assert!(
        script.contains(r#"record_assertion "$name" pass"#)
            && script.contains(r#"record_assertion "$name" fail"#),
        "the oracle must record pass *and* fail for each criterion"
    );
    assert!(
        script.contains(r#"if "$name"; then"#),
        "the oracle must decide pass/fail from the criterion's own exit \
         status, not from a separate hand-written check"
    );
}

#[test]
fn the_oracle_fails_when_the_nini_side_measurement_failed() {
    // v0.15.0: `run_nini` used to print a note and carry on when the
    // measurement test exited non-zero. The script then judged a number
    // that had just been proven unreliable, and could end with a message
    // contradicting itself (`OK: … NINI_SEQ_PEAK=20 …` — where 20 is
    // precisely the value a sequential run must not produce).
    let script = oracle_script();
    assert!(
        script.contains("这次测量不可信"),
        "the oracle must treat a failed nini-side measurement as a failed \
         gate, not as a number to keep judging."
    );
}

#[test]
fn the_oracle_runs_on_the_bash_that_macos_ships() {
    // macOS ships /bin/bash **3.2**. `declare -A` (associative arrays)
    // arrived in bash 4. Using it here made the macOS CI job fail with
    // exit status 1 — which is indistinguishable from "the oracle ran and
    // the difference exists", instead of the agreed 2 ("the environment
    // could not do it, nothing was compared").
    let script = oracle_script();
    // Only the *code* counts: the file explains in a comment why it does
    // not use `declare -A`, and that explanation must not trip the check.
    let code: String = script
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("declare -A"),
        "the oracle must not use bash-4-only syntax: macOS's /bin/bash is \
         3.2 and a failure there must mean \"not done\" (2), not \
         \"the difference exists\" (1)."
    );
}
