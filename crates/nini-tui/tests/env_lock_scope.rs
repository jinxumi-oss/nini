//! invariant: 632a3617
//!
//! # The invariant
//!
//! In `crates/nini-tui/src/settings.rs` the process-environment lock is
//! declared **once, at file scope**, and **every** test in that file
//! which moves `HOME` / `NINI_AGENT_DIR` takes it — whichever `mod` it
//! happens to sit in.
//!
//! # Why nothing asserted it
//!
//! `632a3617` is the third time this file lost a race with itself. The
//! lock was declared *inside* `mod theme_resolution_tests`, so it excluded
//! that module only; `flush_round_trip` in the sibling `mod tests` moved
//! `HOME` without taking anything, ran on another thread in the same
//! binary, and one of the two lookups lost. The symptom was an
//! intermittent `flush failed` that passed in isolation — which is
//! exactly how a locking mistake presents, and is why it got recorded as
//! "an unrelated flake" once before it was diagnosed.
//!
//! A module-scoped lock is a lock that does not exclude, and **nothing in
//! the Rust type system or the test harness notices**: `cargo test` was
//! green, on this machine and on the slow one, more or less at random.
//!
//! # Why a structural assertion
//!
//! The defect is not reachable from any public API: it is a property of
//! *where a `static` is written* relative to *which `#[test]` functions
//! touch the environment*. No runtime test can observe it without
//! reproducing a scheduling race, which is exactly the kind of test that
//! passes when the machine is fast. So this reads `settings.rs` and
//! asserts the structure: one declaration, at brace depth 0, and no
//! env-mutating test without a `lock()` on it.
//!
//! The sweep is deliberately file-scoped, and says so when it finds
//! nothing: a second `ENV_LOCK` in a file it does not examine is this
//! file's business to catch, not this test's.

use std::path::PathBuf;

const SETTINGS: &str = include_str!("../src/settings.rs");

fn settings_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/settings.rs")
}

/// Brace depth at the start of each line, ignoring braces inside strings
/// and comments well enough for this file's shapes (the only string
/// literals with braces are format strings in tests, which are all at
/// greater depth than the ones we care about — so a small overcount is
/// checked against by the depth-0 assertion failing loudly).
fn depths(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    for line in src.lines() {
        out.push(depth.max(0) as usize);
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    out
}

/// The body of the function whose signature follows `fn <name>` at or
/// after byte offset `from`.
fn fn_body(src: &str, from: usize) -> String {
    let open = src[from..]
        .find('{')
        .map(|i| from + i)
        .unwrap_or_else(|| panic!("no body for the fn at offset {from}"));
    let bytes = src.as_bytes();
    let mut d = 0usize;
    for i in open..bytes.len() {
        match bytes[i] {
            b'{' => d += 1,
            b'}' => {
                d -= 1;
                if d == 0 {
                    return src[open..=i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after offset {from}");
}

/// The one property that makes the lock a lock: it is visible to every
/// module in the file. A declaration nested inside `mod X` excludes only
/// `mod X`, and the sibling module's env-mutating tests then run
/// unserialized against it.
#[test]
fn the_environment_lock_is_declared_once_at_file_scope() {
    let lines: Vec<&str> = SETTINGS.lines().collect();
    let dep = depths(SETTINGS);
    let decls: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("static ENV_LOCK"))
        .map(|(i, _)| (i + 1, dep[i]))
        .collect();

    assert_eq!(
        decls.len(),
        1,
        "settings.rs declares ENV_LOCK {} times (line, brace depth): {decls:?} — \
         one lock per file is the only shape that excludes; a second \
         declaration means some module is serializing against a lock \
         another module never takes",
        decls.len()
    );
    let (line_no, depth) = decls[0];
    assert_eq!(
        depth, 0,
        "ENV_LOCK is declared at line {line_no} inside a block (brace depth \
         {depth}), so it excludes only that module. `flush_round_trip` in \
         the sibling `mod tests` moves HOME and takes nothing — that is the \
         race 632a3617 fixed, and it is back."
    );
}

/// The second half: the declaration is worthless if the tests that move
/// the environment do not take it. Enumerated from the source, so a new
/// env-mutating test that forgets the lock is caught on its first commit
/// rather than on the next slow machine.
#[test]
fn every_env_mutating_test_in_the_file_takes_the_lock() {
    let lines: Vec<&str> = SETTINGS.lines().collect();
    let mut checked = 0usize;
    let mut unlocked: Vec<String> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test]")) {
            continue;
        }
        // Find the `fn` that this attribute belongs to.
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim_start().starts_with("fn ") {
            j += 1;
        }
        if j == lines.len() {
            continue;
        }
        let name = lines[j]
            .trim_start()
            .trim_start_matches("fn ")
            .split(['(', ' '])
            .next()
            .unwrap_or("?")
            .to_string();
        let offset: usize = lines[..j].iter().map(|l| l.len() + 1).sum();
        let body = fn_body(SETTINGS, offset);

        let moves_env = body.contains("set_var(") || body.contains("remove_var(");
        if !moves_env {
            continue;
        }
        checked += 1;
        if !body.contains("ENV_LOCK.lock()") {
            unlocked.push(format!("{name} (line {})", i + 1));
        }
    }

    assert!(
        checked >= 2,
        "only {checked} env-mutating test(s) found in settings.rs — the \
         sweep is broken, not the file: an env race cannot be asserted \
         against nothing"
    );
    assert!(
        unlocked.is_empty(),
        "these tests in settings.rs move the process environment without \
         taking the file-level ENV_LOCK: {unlocked:?}. The environment is \
         process-wide, so they race every other test in this binary — the \
         symptom is an intermittent failure that passes in isolation."
    );
}

/// The sweep above must actually be looking at this file, not at a stale
/// copy baked in at compile time from somewhere else. If this fails, the
/// other two assertions in this file are describing a fiction.
#[test]
fn the_assertions_are_reading_this_repository_s_settings_rs() {
    let on_disk = std::fs::read_to_string(settings_path()).expect("settings.rs is unreadable");
    assert_eq!(
        on_disk, SETTINGS,
        "the source this file asserts about has changed on disk without a \
         rebuild"
    );
}
