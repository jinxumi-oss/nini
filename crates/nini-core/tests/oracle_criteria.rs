//! The oracle's **criteria set** must not shrink silently.
//!
//! v0.15.0: a review found the real hole in the differential oracle
//! `scripts/e2e-tool-execution.sh` — **the script's own assertions were
//! unguarded**. Deleting one of them (say, the Pi-side `PI_SEQ_PEAK == 1`)
//! left the script exiting 0 and the whole workspace green: from then on
//! nothing checked the Pi side at all, and the difference between "nini
//! matches Pi" and "nini matches nobody" was indistinguishable.
//!
//! The script now keeps its criteria in an explicit list and checks at the
//! end that every entry ran. That guards the script against a *silent*
//! removal — but a removal is still possible if the list itself is edited,
//! so this file pins the list's **contents**: the criteria the oracle is
//! supposed to have, by name.
//!
//! Both halves are needed: the list check catches "a criterion was deleted
//! but the list still names it"; this file catches "someone deleted the
//! criterion *and* its name from the list".

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

/// The criteria the differential oracle must keep.
///
/// Named after what each one proves, not after the variable it reads — a
/// rename in the script must not silently drop the guarantee.
const REQUIRED_CRITERIA: &[&str] = &[
    // The Pi side really ran in sequential mode, and really ran one at a
    // time. Two independent pieces of evidence, so a probe that measures
    // wrongly cannot pass by accident.
    "pi_mode_is_sequential",
    "pi_seq_peak_is_1",
    "pi_seq_overlap_is_1",
    // The control group: in parallel mode Pi really does run them all at
    // once. Without it, "nini is always sequential" would satisfy every
    // other criterion.
    "pi_mode_is_parallel",
    "pi_par_peak_equals_k",
    "pi_par_overlap_equals_k",
    // The nini side, same three shapes.
    "nini_mode_is_sequential",
    "nini_calls_match_k",
    "nini_seq_peak_is_1",
    "nini_mode_is_parallel",
    "nini_par_peak_equals_k",
];

#[test]
fn the_oracle_keeps_every_criterion_it_is_supposed_to_have() {
    let script = oracle_script();
    for name in REQUIRED_CRITERIA {
        assert!(
            script.contains(name),
            "the differential oracle lost the criterion `{name}`.\n\
             Every criterion must be both listed in ASSERTIONS and recorded \
             with record_assertion. Dropping one silently turns the oracle \
             into a script that passes whatever it is given."
        );
    }
}

#[test]
fn the_oracle_records_every_criterion_it_lists() {
    let script = oracle_script();
    // A name that is listed but never recorded would make the script
    // fail at the end — which is the intended behaviour, but it means the
    // list and the records must not drift. This catches the drift at the
    // Rust level, where it is much cheaper to notice.
    for name in REQUIRED_CRITERIA {
        assert!(
            script.contains(&format!("record_assertion \"{name}\"")),
            "the criterion `{name}` is named but never recorded — the oracle \
             would fail its own end-of-run check."
        );
    }
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
