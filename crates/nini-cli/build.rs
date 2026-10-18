//! Embed the build's identity, so `nini --version` can answer
//! "am I running what I just built?".
//!
//! Why this exists: nini ships as a single binary that people install by
//! symlinking `target/release/nini` into `~/.local/bin`, and `nini 0.10.1`
//! on its own cannot distinguish a binary built from the tagged release
//! from one built three commits later, from one built from a dirty tree.
//! The README's claims are verified *against a specific binary*, so the
//! binary has to be able to name itself.
//!
//! This was previously done by exporting `NINI_BUILD_COMMIT`,
//! `NINI_BUILD_TIME` and `NINI_BUILD_DIRTY` by hand — which is why CI's
//! release artifact reported `unknown (unknown)`: `ci.yml` never set them,
//! and there was no other source of the truth. Here every value has a
//! fallback, and an explicit environment variable always wins so that a
//! reproducible or vendored build can still say what it is.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// `git <args>` in `dir`, or `None` if git is missing or the command fails.
fn git(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Seconds since the epoch, formatted the way `--version` prints it.
///
/// Honours `SOURCE_DATE_EPOCH` (the reproducible-builds convention) so a
/// vendored build gets a stable string instead of "whenever we compiled".
fn build_time() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    // Civil time from days, so the build needs no timezone database:
    // Howard Hinnant's `civil_from_days`.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02} UTC")
}

fn main() {
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo"),
    );
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| manifest.clone());

    // The workspace root is the git work tree. If nini is vendored without
    // `.git`, everything below falls back to "unknown" rather than failing
    // the build: an unidentifiable binary is better than no binary.
    let commit = std::env::var("NINI_BUILD_COMMIT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| git(&root, &["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = std::env::var("NINI_BUILD_DIRTY")
        .ok()
        .map(|v| matches!(v.trim(), "true" | "1" | "yes"))
        .unwrap_or_else(|| git(&root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty()));

    let when = std::env::var("NINI_BUILD_TIME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(build_time);

    // Which profile this is, from cargo's own `PROFILE` (set when the
    // build script runs). It is part of the identity because release and
    // debug are different programs: `panic = "abort"` makes every
    // `catch_unwind` guard in the TUI inert, and `opt-level = "z"` turns
    // off the overflow checks that the 1428 tests run under. A binary
    // that does not say which one it is cannot be told apart from its
    // twin by anything a user can see.
    let profile = std::env::var("NINI_BUILD_PROFILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("PROFILE")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=NINI_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=NINI_BUILD_TIME={when}");
    println!("cargo:rustc-env=NINI_BUILD_PROFILE={profile}");
    println!(
        "cargo:rustc-env=NINI_BUILD_DIRTY={}",
        if dirty { "true" } else { "false" }
    );
    // Rebuild when the tree's state changes, but not on every unrelated
    // file edit elsewhere in the workspace.
    println!("cargo:rerun-if-env-changed=NINI_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=NINI_BUILD_TIME");
    println!("cargo:rerun-if-env-changed=NINI_BUILD_DIRTY");
    println!("cargo:rerun-if-env-changed=NINI_BUILD_PROFILE");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // `.git` alone is not enough. Editing the `[profile.release]` table —
    // `panic` most of all — changes the binary without touching a source
    // file, and the identity would then keep reporting the *previous*
    // build's timestamp. Observed exactly that: flipping `panic` to
    // `unwind` relinked the binary while `--version` still read the abort
    // build's time.
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}
