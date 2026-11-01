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

/// The **black-box** oracle: it runs the compiled binary and judges the
/// batch's concurrency by wall-clock time.
///
/// It needs the same kind of outside guard. A wall-clock criterion has a
/// threshold, and a threshold can be relaxed until it can no longer fail:
/// `MID = 1` (or a `MID` computed from nothing at all) makes every
/// comparison pass — the script would then exit 0 while proving nothing,
/// exactly like case (j) above.
fn black_box_script() -> String {
    let path = repo_root().join("scripts/e2e-tool-execution-cli.sh");
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

/// The black-box oracle's threshold must stay the **midpoint** of the two
/// true values, and its three criteria must stay wired to it.
///
/// The midpoint is not a taste: the two true values are `SLEEP + overhead`
/// (concurrent) and `K*SLEEP + overhead` (sequential), so the midpoint is
/// the point that maximises *both* margins at once — the margin against a
/// slow machine (a false red) and the margin against a mutation that
/// collapses the two sides (a false green).
#[test]
fn the_black_box_oracle_keeps_its_threshold_and_its_three_criteria() {
    let script = black_box_script();
    for (what, line) in [
        (
            "the midpoint formula",
            r#"MID_MS="$(python3 -c "print(int($SLEEP * ($K + 1) * 1000 / 2))")""#,
        ),
        ("the sequential bound", r#"SEQ_LOW="$MID_MS""#),
        ("the parallel bound", r#"PAR_HIGH="$MID_MS""#),
        (
            "the sequential criterion",
            r#"if [[ "$T_SEQ" -lt "$SEQ_LOW" ]]; then"#,
        ),
        (
            "the parallel criterion",
            r#"if [[ "$T_PAR" -gt "$PAR_HIGH" ]]; then"#,
        ),
        (
            "the criterion that the two runs really differ",
            r#"if [[ "$T_SEQ" -le "$T_PAR" ]]; then"#,
        ),
    ] {
        assert!(
            script.contains(line),
            "the black-box oracle's {what} is no longer\n\
             {line}\n\
             A wall-clock threshold can be relaxed until it can no longer \
             fail, and then the oracle exits 0 while proving nothing — the \
             same trap as widening a predicate. The threshold and the \
             three criteria are pinned here so that cannot happen silently."
        );
    }
}

/// The black-box oracle must check that the batch really ran — not just
/// that it was slow or fast.
///
/// The tool *calls* are invisible from the outside; the tool *results* are
/// not: the mock logs `tool_results=N`, and the oracle asserts it equals
/// K. Without that check, "the model never asked for a tool and nini
/// answered at once" would be indistinguishable from "the batch ran
/// concurrently" — the timing would say "fast", and the reason would be
/// "nothing happened".
#[test]
fn the_black_box_oracle_checks_that_k_tools_actually_ran() {
    let script = black_box_script();
    assert!(
        script.contains(r#"grep -o 'tool_results=[0-9]*'"#),
        "the black-box oracle must read the tool-result count the mock logs"
    );
    assert!(
        script.contains(r#"if [[ "$got" != "$K" ]]; then"#),
        "the black-box oracle must fail when the batch did not bring back \
         K tool results — otherwise \"nothing ran\" looks like \"it was \
         fast\"."
    );
    let mock = std::fs::read_to_string(repo_root().join("scripts/mock-provider.py"))
        .expect("the mock provider must be readable");
    assert!(
        mock.contains("tool_results={tool_results}"),
        "the mock provider must log the tool-result count — the oracle \
         reads it, and a mock that stopped logging it would make the \
         black-box oracle fail for the wrong reason."
    );
}

/// CI must still run **both** tool-execution oracles.
///
/// Removing a CI step is the cheapest way to make a gate green again, and
/// unlike a predicate in a script, nothing else in the repository would
/// notice: the script would sit in the tree, correct and idle.
#[test]
fn ci_still_runs_both_tool_execution_oracles() {
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("the CI workflow must be readable");
    for script in [
        "scripts/e2e-tool-execution.sh",
        "scripts/e2e-tool-execution-cli.sh",
    ] {
        assert!(
            ci.contains(script),
            "CI no longer runs `{script}` — the oracle sits in the tree, \
             correct and idle. Deleting the step is the cheapest way to \
             make a gate green again, so the step itself is pinned here."
        );
    }
}
