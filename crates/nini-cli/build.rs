//! Embed the build's identity in the binary.
//!
//! v0.10. This exists because of a specific, repeatable failure:
//!
//!     ~/.local/bin/nini -> target/release/nini
//!
//! `install.sh` downloads a prebuilt archive and never compiles anything, so
//! "nini is installed" and "nini matches this checkout" are unrelated facts.
//! On 2026-10-12 the user ran the TUI after a session's worth of fixes and
//! saw none of them, because the release binary was a week old. `--version`
//! printed `nini 0.9.1` in both cases, so there was nothing to compare.
//!
//! Now it prints the commit and the build time, which makes staleness visible
//! without anybody having to remember to run a build.
//!
//! # The rebuild cost, stated plainly
//!
//! `rerun-if-changed` points at `.git/HEAD` so that committing changes the
//! embedded commit and re-links the binary. That is the point, but it means
//! a commit is no longer free: the next `cargo build` after one rebuilds the
//! binary crate. Roughly 36 seconds at the current size, which is a fair
//! price for a binary that can say what it is.

use std::process::Command;

/// `$NINI_BUILD_COMMIT` overrides detection, for a release built from an
/// exported tree with no `.git` (a tarball, a vendored copy, CI).
fn env_override(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// "3caa078" — short enough to read, long enough to be unambiguous.
fn commit() -> String {
    env_override("NINI_BUILD_COMMIT")
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string())
}

/// "2026-10-14 09:54:11 +08:00" — when the binary was linked, not when the
/// source was last edited. Those differ: a tree can sit unbuilt for days, and
/// that difference is exactly what went wrong.
fn build_time() -> String {
    if let Some(v) = env_override("NINI_BUILD_TIME") {
        return v;
    }
    // `date` rather than a crate: the build script already shells out for
    // git, and adding a dependency to a build script to format a timestamp
    // is not a trade worth making.
    let output = Command::new("date").arg("+%Y-%m-%d %H:%M:%S %z").output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8(o.stdout)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        _ => "unknown".to_string(),
    }
}

/// True when the working tree had uncommitted changes at build time.
///
/// Worth recording because "commit 3caa078" can otherwise be read as "exactly
/// 3caa078" when the tree actually had more.
fn dirty() -> bool {
    if env_override("NINI_BUILD_COMMIT").is_some() {
        return false;
    }
    matches!(
        git(&["status", "--porcelain"]),
        Some(s) if !s.is_empty()
    )
}

fn main() {
    println!("cargo:rustc-env=NINI_BUILD_COMMIT={}", commit());
    println!("cargo:rustc-env=NINI_BUILD_TIME={}", build_time());
    println!("cargo:rustc-env=NINI_BUILD_DIRTY={}", dirty());

    // Re-run when the checked-out commit changes. See the module docs for the
    // cost this deliberately accepts.
    if let Ok(git_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let git_dir = std::path::Path::new(&git_dir).join("../..");
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(".git/HEAD").display()
        );
        // A commit writes a new object file; HEAD's mtime moves with it on
        // every checkout, so HEAD alone is enough in practice, but packed
        // refs can change without HEAD's timestamp. Cheap to include.
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(".git/refs").display()
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
}
