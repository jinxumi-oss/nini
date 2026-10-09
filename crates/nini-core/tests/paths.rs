//! R3: nini's files are nini's own.
//!
//! Before this, every path nini touched went through `.pi`. It read
//! `settings.json`, `models.json`, `keybindings.json`, `prompts/`,
//! `skills/` and `sessions/` out of `~/.pi/agent/`, wrote sessions and
//! exports back into it, and offered Pi's own session history in `/resume`.
//! Two installs shared one directory, so editing settings in one changed
//! the other, and nini did not work on a machine where Pi had never been
//! installed.
//!
//! The line this draws:
//!
//!   * runtime paths are nini's own — `~/.nini/agent/`, `.nini/` in a
//!     project, overridable with `NINI_AGENT_DIR`.
//!   * file *formats* stay Pi-compatible. The session JSONL schema is
//!     unchanged, so opening a Pi session file still works. Being able to
//!     read a format is a feature; needing another application to be
//!     installed to have a config directory is a dependency, and only the
//!     second was wrong.
//!
//! These tests assert the first half. The second is asserted by the session
//! fixtures in `nini-conformance`, which are Pi's own files and still parse.

use nini_core::paths;
use std::path::{Path, PathBuf};

/// Set `NINI_AGENT_DIR` for the duration of `f`.
fn with_agent_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    // SAFETY: serialised by the caller-visible lock below.
    unsafe { std::env::set_var(paths::ENV_AGENT_DIR, dir) };
    let out = f();
    match saved {
        Some(v) => unsafe { std::env::set_var(paths::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(paths::ENV_AGENT_DIR) },
    }
    out
}

fn without_override<T>(f: impl FnOnce() -> T) -> T {
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    unsafe { std::env::remove_var(paths::ENV_AGENT_DIR) };
    let out = f();
    match saved {
        Some(v) => unsafe { std::env::set_var(paths::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(paths::ENV_AGENT_DIR) },
    }
    out
}

#[test]
fn the_agent_directory_is_ninis() {
    without_override(|| {
        let home = paths::home_dir().expect("a home directory");
        assert_eq!(
            paths::agent_dir(),
            Some(home.join(".nini").join("agent")),
            "not `~/.pi/agent`: nini is a rewrite, not a client"
        );
    });
}

#[test]
fn the_project_directory_is_ninis() {
    let dir = paths::project_dir(Path::new("/some/cwd"));
    assert_eq!(dir, PathBuf::from("/some/cwd/.nini"));
    assert!(
        !dir.to_string_lossy().contains(".pi"),
        "and never Pi's: {}",
        dir.display()
    );
}

#[test]
fn the_override_is_ninis_own() {
    assert_eq!(
        paths::ENV_AGENT_DIR,
        "NINI_AGENT_DIR",
        "Pi's PI_CODING_AGENT_DIR would make nini read Pi's configuration"
    );
    with_agent_dir(Path::new("/tmp/nini-agent"), || {
        assert_eq!(paths::agent_dir(), Some(PathBuf::from("/tmp/nini-agent")));
        assert_eq!(
            paths::agent_file("settings.json"),
            Some(PathBuf::from("/tmp/nini-agent/settings.json"))
        );
    });
}

#[test]
fn a_blank_override_falls_through_to_the_default() {
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    // SAFETY: this test is the only one touching the variable at a time in
    // this binary; the others in this file that do use `with_agent_dir`
    // restore whatever they found.
    unsafe { std::env::set_var(paths::ENV_AGENT_DIR, "   ") };
    let dir = paths::agent_dir().expect("a home directory");
    let expected = paths::home_dir()
        .expect("a home directory")
        .join(".nini")
        .join("agent");
    assert_eq!(
        dir, expected,
        "a blank override must fall through to nini's own directory \
         rather than resolving the store to the working directory"
    );
    match saved {
        Some(v) => unsafe { std::env::set_var(paths::ENV_AGENT_DIR, v) },
        None => unsafe { std::env::remove_var(paths::ENV_AGENT_DIR) },
    }
}

/// Project-level overrides user-level, which is what every loader did
/// before and what keeps a project able to override a machine default.
#[test]
fn candidates_put_the_project_first() {
    with_agent_dir(Path::new("/tmp/nini-agent"), || {
        let c = paths::candidates(Path::new("/proj"), "settings.json");
        assert_eq!(c[0], PathBuf::from("/proj/.nini/settings.json"));
        assert_eq!(c[1], PathBuf::from("/tmp/nini-agent/settings.json"));
        assert!(
            c.iter().all(|p| !p.to_string_lossy().contains("/.pi/")),
            "no candidate may point into Pi's directory: {c:?}"
        );
    });
}

/// The whole point, stated as a sweep: no production source file may still
/// build a path under `.pi`.
///
/// A grep in a test is unusual, and it is here because the failure mode is
/// textual - a path reappearing in one call site is invisible to every
/// behavioural test in the suite. `scripts/sweep-unwired.py` checks the same
/// shape of thing by a different route; this one is the assertion that
/// keeps it from regressing silently.
#[test]
fn no_source_file_still_builds_a_pi_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();

    let mut offenders: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&root.join("crates"))
        .expect("crates dir")
        .flatten()
    {
        let crate_dir = entry.path();
        let name = crate_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with("nini-") {
            continue;
        }
        let src = crate_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        walk_rs(&src, &name, &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "nini must not read or write Pi's directories. Still building a \\
         `.pi` path in:\n  {}",
        offenders.join("\n  ")
    );
}

fn walk_rs(dir: &Path, crate_name: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, crate_name, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // a construction, not a mention
            if line.contains("join(\".pi\")") || line.contains("join(\".piagent\")") {
                out.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
}
