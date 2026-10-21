//! `8c8a009d` — **a binary must be able to name its own source.**
//!
//! # The invariant
//!
//! `nini --version` answers three questions a user of a symlinked
//! `target/release/nini` cannot answer any other way:
//!
//! | field | must be |
//! |---|---|
//! | commit | the `git rev-parse --short HEAD` the binary was built from — never `unknown` |
//! | time | a UTC instant — never `unknown` |
//! | profile | the profile **this binary** was compiled with, not a fixed guess |
//!
//! plus one modifier: `+dirty` when the work tree was dirty at build time, and
//! no marker when it was clean.
//!
//! # Why nothing asserted it
//!
//! Before this commit there was no `build.rs` at all: `build_identity()` read
//! `NINI_BUILD_COMMIT` / `NINI_BUILD_TIME` / `NINI_BUILD_DIRTY` from the
//! environment, and nothing set them — `ci.yml` did not — so the artifact CI
//! published identified itself as `unknown (unknown)`. Exactly the failure the
//! function exists to prevent, and the only check lived in a shell script
//! (`scripts/build-release.sh`), not in the suite: `cargo test` said nothing
//! about identity, so a build that could not name itself was still a green run.
//!
//! # Why these assertions are not same-source
//!
//! Every field is compared against what the *toolchain* says, not against a
//! copy of the format string: the commit against `git rev-parse --short HEAD`
//! run at test time, the profile against `option_env!("PROFILE")` as compiled
//! into this test, the dirty marker against `git status --porcelain` run from
//! the workspace root — the same three inputs `build.rs` reads. If `build.rs`
//! falls back to a constant, or hardcodes the wrong profile, or inverts the
//! dirty rule, the binary and the toolchain disagree and this file is red.

//! invariant: 8c8a009d —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，

use std::path::{Path, PathBuf};
use std::process::Command;

/// The workspace root, derived the same way `build.rs` derives it: the crate's
/// manifest directory's parent. Both sides must ask git about the same tree,
/// or the comparison is meaningless.
fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crates/nini-cli has a parent and a grandparent")
        .to_path_buf()
}

fn git(args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn version_line() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .arg("--version")
        .output()
        .expect("nini runs");
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        raw.trim_end().ends_with(']'),
        "`nini --version` must end with the [profile] field: {raw:?}"
    );
    raw
}

/// The identity half of the line: `<commit>[ +dirty] (<time>) [<profile>]`,
/// i.e. everything after clap's `nini <version> - `.
fn identity() -> String {
    let raw = version_line();
    let (_, tail) = raw
        .split_once(" - ")
        .unwrap_or_else(|| panic!("no `nini <version> - <identity>` shape in {raw:?}"));
    tail.trim().to_string()
}

/// The commit field, `+dirty` marker included (it is part of what the commit
/// claim means: this is HEAD *as of the build*, not HEAD now).
fn commit_field() -> String {
    identity()
        .split(" (")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The text between the first `open` and the next `close`.
fn field_between(open: char, close: char) -> String {
    let id = identity();
    let (_, tail) = id
        .split_once(open)
        .unwrap_or_else(|| panic!("no `{open} ... {close}` field in {id:?}"));
    tail.split(close)
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The headline: the binary names the commit it was built from, and that commit
/// is this checkout's HEAD.
///
/// This is the assertion CI's artefact would have failed. Before `build.rs`
/// existed this printed `unknown`, and nothing in `cargo test` noticed.
#[test]
fn the_binary_names_the_commit_it_was_built_from() {
    let head = git(&["rev-parse", "--short", "HEAD"]);
    assert!(
        !head.is_empty(),
        "this is not a git checkout; the test cannot verify the identity claim"
    );

    let claimed = commit_field();
    let claimed_sha = claimed.split_whitespace().next().unwrap_or_default();

    assert_ne!(
        claimed_sha,
        "unknown",
        "`nini --version` says `unknown`: the binary cannot name its own source, \
         which is the entire reason build.rs exists. Full line: {:?}",
        version_line()
    );
    assert_eq!(
        claimed_sha,
        head,
        "the binary claims commit {claimed_sha} but HEAD is {head} — the identity is \
         stale, i.e. this binary is not a build of this checkout. Full line: {:?}",
        version_line()
    );
}

/// The profile half: release and debug are different programs (`panic = abort`
/// makes every `catch_unwind` guard in the TUI inert), and a binary that does
/// not say which one it is cannot be told apart from its twin.
///
/// The expectation is derived from how *this* test was compiled, not from a
/// constant, so `cargo test` and `cargo test --release` both hold the binary to
/// the truth.
#[test]
fn the_binary_names_the_profile_it_was_compiled_with() {
    let claimed = field_between('[', ']');
    let expected = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    assert_ne!(
        claimed,
        "unknown",
        "the profile is missing from `nini --version`; full line: {:?}",
        version_line()
    );
    assert_eq!(
        claimed,
        expected,
        "this test binary was compiled with profile `{expected}`, but `nini --version` \
         claims `{claimed}`. Full line: {:?}",
        version_line()
    );
}

/// A dirty work tree must say so, and a clean one must not.
///
/// The check is against `git status --porcelain` run now — the same command
/// `build.rs` runs — so this also fails if the binary describes a tree that
/// is not the tree it was built from (which is what a stale build looks like).
///
/// But the binary's `+dirty` is a **snapshot taken at build time**, while
/// `git status` here is a **live reading**. Comparing the two without
/// noticing the gap made this test red for every developer who had touched
/// a file since their last build: the tree was dirty *now*, the binary said
/// "clean" because it was built *before* the edit, and neither statement
/// was wrong. The suite was reporting the age of a binary, not a lie in it.
///
/// So the comparison is made only when the two can be compared at all: if
/// any file in the tree was modified after the build's own timestamp, the
/// tree has changed since the binary was built and the two readings are
/// about different states. Then this test says so and stops — it does not
/// manufacture a verdict. When nothing moved (CI, a fresh checkout, a
/// developer who has not edited anything since building), the comparison is
/// exact and the assertion is the original one.
#[test]
fn a_dirty_worktree_is_reported_dirty() {
    let dirty_now = !git(&["status", "--porcelain"]).is_empty();
    let claims_dirty = commit_field().contains("+dirty");

    if let Some(changed_at) = newest_tree_change_epoch() {
        let built_at = build_epoch();
        if changed_at > built_at {
            eprintln!(
                "skipped: the tree changed at {changed_at}, after this binary was built \
                 at {built_at}; the binary's claim is a snapshot of an earlier state"
            );
            return;
        }
    }

    assert_eq!(
        claims_dirty,
        dirty_now,
        "`nini --version` says {claims_dirty} for a tree that `git status` reports as \
         {} dirty. Full line: {:?}",
        if dirty_now { "dirty" } else { "clean" },
        version_line()
    );
}

/// The instant this binary was built, as `build.rs` recorded it: the `time`
/// field of the identity line, in epoch seconds.
fn build_epoch() -> i64 {
    let raw = field_between('(', ')');
    let trimmed = raw.trim_end_matches(" UTC");
    chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
        .expect("the time field is a UTC instant")
        .and_utc()
        .timestamp()
}

/// The newest mtime among the files the tree is made of: every tracked file
/// (an edit, or a revert, moves one) plus everything `git status` reports
/// as untracked (a new file moves the tree's dirty state without touching
/// a tracked file). `None` when the tree is empty.
fn newest_tree_change_epoch() -> Option<i64> {
    use std::time::UNIX_EPOCH;

    let mut paths: Vec<PathBuf> = git(&["ls-files", "-z"])
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();

    for line in git(&["status", "--porcelain"]).lines() {
        // "XY <path>", and "XY <old> -> <new>" for a rename.
        if line.len() < 4 {
            continue;
        }
        let tail = &line[3..];
        let path = tail.rsplit(" -> ").next().unwrap_or(tail);
        paths.push(PathBuf::from(path));
    }

    paths
        .iter()
        .filter_map(|p| std::fs::metadata(repo_root().join(p)).ok())
        .filter_map(|m| m.modified().ok())
        .filter_map(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
}

/// The timestamp is the only thing that tells "three weeks old" from "built
/// just now", and `SOURCE_DATE_EPOCH` is how a vendored build keeps it stable
/// (reproducible), so the field must always be a real instant — never the
/// fallback string.
#[test]
fn a_reproducible_build_reports_a_utc_timestamp_not_unknown() {
    let when = field_between('(', ')');
    assert_ne!(
        when,
        "unknown",
        "the build time is missing; `nini --version` cannot say how old the binary is. \
         Full line: {:?}",
        version_line()
    );

    // `YYYY-MM-DD HH:MM:SS UTC`: the shape build.rs computes from the epoch
    // without a timezone database.
    let stamp = when
        .strip_suffix(" UTC")
        .unwrap_or_else(|| panic!("build time is not UTC-suffixed: {when:?}"));
    let (date, clock) = stamp
        .split_once(' ')
        .unwrap_or_else(|| panic!("build time is not `<date> <clock>`: {stamp:?}"));

    let digits = |p: &str, len: usize| p.len() == len && p.chars().all(|c| c.is_ascii_digit());
    let ok = match (
        date.split('-').collect::<Vec<_>>(),
        clock.split(':').collect::<Vec<_>>(),
    ) {
        (d, c) if d.len() == 3 && c.len() == 3 => {
            digits(d[0], 4)
                && digits(d[1], 2)
                && digits(d[2], 2)
                && digits(c[0], 2)
                && digits(c[1], 2)
                && digits(c[2], 2)
        }
        _ => false,
    };
    assert!(
        ok,
        "build time {when:?} is not the agreed `YYYY-MM-DD HH:MM:SS UTC` shape"
    );
}
