//! v0.15: `ToolExecutionMode` must be parseable out of a config file.
//!
//! Pi's `ToolExecutionMode` has exactly two values, `parallel` and
//! `sequential`, spelled in lower case
//! (`references/spec-v0.85.1/session/types.ts:192`:
//! `toolExecution: "sequential" | "parallel";`). nini's enum had the same
//! two values but no serde derive, so nothing could read the mode out of a
//! settings file — the batch-level switch existed in `RunConfig` and
//! could not be reached by a user.
//!
//! The values must match Pi's spelling, and the parse must be
//! case-sensitive: `"Parallel"` is not a value Pi accepts either, and
//! silently accepting it here would make nini's config dialect drift
//! away from Pi's without anybody noticing.

use nini_core::tool::ToolExecutionMode;

/// `"sequential"` parses to `Sequential`.
#[test]
fn sequential_parses_from_its_lower_case_name() {
    let mode: ToolExecutionMode =
        serde_json::from_str("\"sequential\"").expect("sequential parses");
    assert_eq!(mode, ToolExecutionMode::Sequential);
}

/// `"parallel"` parses to `Parallel`.
#[test]
fn parallel_parses_from_its_lower_case_name() {
    let mode: ToolExecutionMode = serde_json::from_str("\"parallel\"").expect("parallel parses");
    assert_eq!(mode, ToolExecutionMode::Parallel);
}

/// The spelling is case-sensitive: `"Parallel"` is an error, not a
/// quiet alias. Pi's own type has no case-insensitive parsing either,
/// so accepting it here would be nini inventing a third dialect.
#[test]
fn a_capitalised_value_is_refused() {
    let err = serde_json::from_str::<ToolExecutionMode>("\"Parallel\"")
        .expect_err("`Parallel` must not parse");
    let msg = err.to_string();
    assert!(
        msg.contains("Parallel"),
        "the error must name the offending value: {msg}"
    );
}

/// The two values are the whole dialect — nothing else is accepted.
#[test]
fn an_unknown_value_is_refused() {
    assert!(
        serde_json::from_str::<ToolExecutionMode>("\"bogus\"").is_err(),
        "`bogus` must not parse"
    );
}
