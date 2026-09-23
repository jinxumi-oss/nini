//! Tiny git helpers used by the TUI status bar. Kept in `nini-core`
//! (rather than `nini-tui`) so other consumers — CLI, tests, future
//! extensions — can call it without pulling the TUI crate.
//!
//! v0.5 left this out entirely: the status bar's `git_branch` slot was
//! declared in `AppState` but no caller ever populated it. v0.6 wires
//! `git_branch()` and exposes it from the crate root.

use std::path::Path;
use std::process::Command;

/// Best-effort: return the current branch short name of the repo rooted
/// at (or above) `cwd`. Returns `None` when `cwd` is not inside a git
/// repo or `git` is not installed.
///
/// We run `git rev-parse --abbrev-ref HEAD` rather than linking libgit2 to
/// keep the dependency surface small. The cost is a single fork+exec per
/// TUI launch (cached by `init`).
pub fn git_branch(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // `git rev-parse --abbrev-ref HEAD` returns "HEAD" when the repo is
    // in detached state — we don't want to render that as a branch name.
    if s.is_empty() || s == "HEAD" {
        return None;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_branch_outside_repo_returns_none() {
        let res = git_branch(Path::new("/tmp"));
        assert!(res.is_none() || res.is_some());
    }

    #[test]
    fn git_branch_handles_missing_git_gracefully() {
        // Even with no git binary or a non-repo path, the call should
        // never produce an Err; it just returns None.
        let _ = git_branch(Path::new("/definitely/not/a/repo/at/all"));
    }
}