//! `b4d74570` — a refusal reason is prose a user reads, not a source literal.
//!
//! # The invariant
//!
//! Every reason nini prints after "N flag(s) accepted but not applied" must
//! be one readable sentence. Concretely, two things it must never do:
//!
//! 1. **Carry a run of two or more spaces.** `b4d74570` was found by running
//!    the published binary: a heredoc had written the message through a
//!    backslash-continued string literal, and the continuation's indentation
//!    shipped inside the sentence. Users saw
//!    `... use the              \`fixture\` provider to run without a network`.
//! 2. **Describe a working feature as unimplemented.** The same commit had to
//!    fix `--no-themes`, which still said "theme discovery is not
//!    implemented" after the release that wired `themes/<name>.json` loading.
//!
//! # Why the tests that existed could not catch either
//!
//! `cli_flag_contract::a_refused_flag_names_itself_and_its_own_reason` — and
//! every other test that touched these strings — asserts that the **flag
//! name** appears in stderr. That was equally true before and after the fix.
//! The defect lived in the text *after* the flag name, which nothing read.
//! Checking for the flag is what these strings were always good at.
//!
//! # External input, not same-source
//!
//! This file runs the real `nini` binary (`CARGO_BIN_EXE_nini`) with every
//! flag that can be refused in one invocation, so the assertion is against
//! what a user actually sees on stderr — not against a copy of the literals.

use std::process::Command;

/// Every flag whose refusal can be triggered at once without clap rejecting
/// the command line itself. `--continue-session` conflicts with `--session-id`,
/// `--fork` and `--name` are session selectors, and `--prompt-template` needs
/// a value; they are covered by the individual runs below.
const REFUSABLE: &[&str] = &[
    "--offline",
    "--no-themes",
    "--no-session",
    "--no-context-files",
    "--no-extensions",
    "--no-prompt-templates",
    "--tui-mode",
];

fn run(extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .args(extra)
        .args(["--provider", "fixture", "-p", "hi"])
        .output()
        .expect("nini runs");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The lines nini prints between the count and the end of the refusal block.
fn refusal_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.trim_start().starts_with("[nini]"))
        .map(|l| l.trim_start().trim_start_matches("[nini]").trim())
        .filter(|l| l.starts_with("--"))
        .collect()
}

/// The invariant, part 1: **no refusal reason may contain a double space.**
///
/// A run of spaces inside a sentence is always a string-literal accident —
/// `\` continuation, heredoc indentation, or a formatter nobody ran. It is
/// not a style preference: it is the fingerprint of the exact defect
/// `b4d74570` fixed, and the fingerprint outlived the fix (see below).
#[test]
fn no_refusal_reason_carries_a_run_of_spaces() {
    let stderr = run(&[
        "--offline",
        "--no-themes",
        "--no-session",
        "--no-context-files",
        "--no-extensions",
        "--no-prompt-templates",
        "--tui-mode",
        "fullscreen",
        "--approve",
        "--continue-session",
        "--session-dir",
        "/tmp/nini-refusal-test",
        "--export",
        "/tmp/nini-refusal-test.html",
        "--extension",
        "e",
    ]);

    let lines = refusal_lines(&stderr);
    assert!(
        lines.len() >= 10,
        "expected most of the refusal list, got {} line(s):\n{stderr}",
        lines.len()
    );

    for line in &lines {
        assert!(
            !line.contains("  "),
            "`{line}`\n— this reason shipped with the indentation of its own source \
             literal inside the sentence. Rust keeps the whitespace after a \
             backslash continuation, so the artefact is invisible in the source and \
             obvious on stderr. Full stderr:\n{stderr}"
        );
    }
}

/// The invariant, part 2, for `--no-themes` specifically: the reason must
/// describe what nini actually does.
///
/// nini loads `themes/<name>.json` on demand and simply has no enumeration
/// (no `--list-themes`). The old reason claimed "theme discovery is not
/// implemented; the built-in theme is always used", which had stopped being
/// true in the release that wired theme loading — a refusal that explains a
/// nonexistent limitation is worse than no refusal.
#[test]
fn nini_loads_themes_on_demand_but_cannot_enumerate_them() {
    let stderr = run(&["--no-themes"]);
    let line = refusal_lines(&stderr)
        .into_iter()
        .find(|l| l.starts_with("--no-themes"))
        .unwrap_or_else(|| panic!("--no-themes was not refused; stderr:\n{stderr}"));

    assert!(
        line.contains("loads") && line.contains("demand"),
        "the reason must say that nini loads a theme on demand, which is what it \
         does. Got: {line}"
    );
    assert!(
        line.contains("--use-theme"),
        "a refusal must say what to do instead. Got: {line}"
    );
    assert!(
        !line.contains("not implemented"),
        "theme loading is implemented; only enumeration is missing. Saying \
         'not implemented' here is the stale claim b4d74570 removed and it is \
         back. Got: {line}"
    );
}

/// Every refusal names the flag it is refusing, and says one thing. Cheap to
/// assert, and it is the half the existing tests already had — kept so this
/// file reads as the *replacement* for that check, not an addition to it.
#[test]
fn every_refusal_names_its_own_flag() {
    for flag in REFUSABLE {
        let value = if *flag == "--tui-mode" {
            "fullscreen"
        } else {
            ""
        };
        let mut args = vec![*flag];
        if !value.is_empty() {
            args.push(value);
        }
        let stderr = run(&args);
        assert!(
            stderr.contains(flag),
            "`{flag}` was accepted and dropped without ever being named:\n{stderr}"
        );
    }
}
