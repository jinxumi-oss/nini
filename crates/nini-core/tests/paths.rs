//! R3: nini's files are nini's own.
//!
//! Before this, every path nini touched went through `.pi`. It read
//! `settings.json`, `models.json`, `keybindings.json`, `prompts/`,
//! `skills/` and `sessions/` out of `~/.nini/agent/`, wrote sessions and
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

/// `NINI_AGENT_DIR` and `HOME` are process-wide, and Cargo runs the tests
/// in one binary on many threads.
///
/// The first version of this file carried the comment "serialised by the
/// caller-visible lock below" above a `set_var` and had no lock. Four
/// tests read and wrote the same variable concurrently, so
/// `the_agent_directory_is_ninis` - which asserts the variable is *not*
/// set - could observe another test's value. It passed because the
/// interleaving was unlikely, not because anything was synchronised.
///
/// One lock covers the whole set-restore cycle, held across `f` as well as
/// the writes, because the window that matters is between setting the
/// value and the assertion reading it.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Set `NINI_AGENT_DIR` for the duration of `f`.
fn with_agent_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = env_lock();
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    // SAFETY: every mutation in this file and in override_wiring.rs holds
    // `env_lock`, so no other thread can observe a half-applied change.
    unsafe { std::env::set_var(paths::ENV_AGENT_DIR, dir) };
    let out = f();
    unsafe {
        match saved {
            Some(v) => std::env::set_var(paths::ENV_AGENT_DIR, v),
            None => std::env::remove_var(paths::ENV_AGENT_DIR),
        }
    }
    out
}

fn without_override<T>(f: impl FnOnce() -> T) -> T {
    let _guard = env_lock();
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    unsafe { std::env::remove_var(paths::ENV_AGENT_DIR) };
    let out = f();
    unsafe {
        match saved {
            Some(v) => std::env::set_var(paths::ENV_AGENT_DIR, v),
            None => std::env::remove_var(paths::ENV_AGENT_DIR),
        }
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
            "not `~/.nini/agent`: nini is a rewrite, not a client"
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
    let _guard = env_lock();
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    // SAFETY: `env_lock` is held for the whole set-restore cycle here and in
    // `with_agent_dir` above, so no other thread can observe the blank value.
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

/// The whole point, stated as a sweep: no production source file may build a
/// path under `.pi` without registering it.
///
/// A grep in a test is unusual, and it is here because the failure mode is
/// textual - a path reappearing in one call site is invisible to every
/// behavioural test in the suite. `scripts/sweep-unwired.py` checks the same
/// shape of thing by a different route; this one is the assertion that
/// keeps it from regressing silently.
///
/// # Narrowed in v0.10 (M3) — and why that made it stricter, not weaker
///
/// This started as a blanket ban. R3 introduced it when nini read *everything*
/// out of Pi's directory, including state that only makes sense with Pi
/// installed. The intent was right and the assertion was broader than the
/// intent: it treated reading Pi's skills directory and reading Pi's
/// credential store as the same act.
///
/// They are not the same act. `SKILL.md` is the Anthropic agent-skills
/// format — prose on disk, not Pi's private state. Not reading it stops nini
/// using an ecosystem it is meant to plug into, which is the opposite of the
/// failure this test was written to prevent. On this machine that was 19 Pi
/// skills and 15 Claude Code skills, none of them visible to nini.
///
/// So the rule is now the boundary rather than a blanket ban:
///
/// > **Assets are shared, credentials are not; formats are compatible,
/// > runtime dependencies are not.**
///
/// Narrowing an assertion usually weakens it, so the exceptions are held to
/// a stricter standard than the ban they replace. An ecosystem read must be
/// registered in [`ALLOWED_ECOSYSTEM_READS`] **and** the total number of
/// `.pi` constructions must equal the registered count, so adding one more
/// read of anything - skills, state, or `auth.json` - changes the count and
/// fails whether or not anyone remembered to register it. The credential
/// boundary also gets its own test below, so it does not rest on this one.
#[test]
fn no_source_file_reads_pi_state_without_registering_it() {
    let offenders = pi_path_constructs();

    let allowed = ALLOWED_ECOSYSTEM_READS
        .iter()
        .map(|(file, suffix)| (*file, *suffix))
        .collect::<Vec<_>>();

    // Counted before filtering, so an unregistered read fails even when the
    // line it adds happens to contain the word "skills".
    assert_eq!(
        offenders.len(),
        allowed.len(),
        "nini reads Pi's state without registering it. Expected exactly {} \
         registered ecosystem reads, found {}:\n  {}",
        allowed.len(),
        offenders.len(),
        offenders.join("\n  ")
    );

    let mut unexplained: Vec<String> = offenders
        .iter()
        .filter(|offender| {
            !allowed
                .iter()
                .any(|(file, suffix)| offender.contains(file) && offender.contains(suffix))
        })
        .cloned()
        .collect();
    unexplained.sort();

    assert!(
        unexplained.is_empty(),
        "nini must not read Pi's directories beyond the registered asset \
         reads. Unregistered:\n  {}",
        unexplained.join("\n  ")
    );
}

/// The only `join(".pi")` constructions allowed in the workspace: nini-core's
/// skills loader reading the two **skills** directories.
///
/// Both end in `"skills"`, and the total count is asserted by
/// [`no_source_file_reads_pi_state_without_registering_it`], so this list
/// cannot quietly grow. Note what is absent and must stay absent: Pi's
/// `auth.json`, its sessions, its settings.
const ALLOWED_ECOSYSTEM_READS: &[(&str, &str)] = &[
    // `h.join(".pi").join("agent").join("skills")` — user-level Pi skills
    ("src/skills.rs", "join(\"skills\")"),
    // `cwd.join(".pi").join("skills")` — project-level Pi skills
    ("src/skills.rs", "join(\"skills\")"),
];

/// Every `.pi` / `.piagent` path construction in the workspace, as
/// `path:line: text`.
fn pi_path_constructs() -> Vec<String> {
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
    offenders
}

/// The boundary an allowlist cannot express on its own: no source file may
/// name a credential store belonging to another agent.
///
/// The skills exception is deliberately not extended here. Reading
/// `~/.pi/agent/skills` is reading prose; reading `~/.pi/agent/auth.json` is
/// reading somebody's API key. `6abadd9` is the record of what happens when
/// that gets argued as parity instead of decided.
#[test]
fn no_source_file_can_reach_another_agents_credentials() {
    // Calibrated, and the first version was not. Matching the bare word
    // "credentials" flagged `/login` and `/logout`, whose user-visible
    // strings say "nini uses environment variables for credentials" -
    // text about credentials, not a path to one. The signal that matters is
    // a credential *filename* being joined into a path, so that is what is
    // matched. Same lesson as the detector calibration note in the plan:
    // an uncalibrated check hands you its own false positives dressed up
    // as findings.
    const CREDENTIAL_FILENAMES: &[&str] = &[
        "auth.json",
        "credentials.json",
        ".credentials",
        "api-keys.json",
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let mut offenders: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(root.join("crates"))
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
        walk_for_credential_paths(&src, CREDENTIAL_FILENAMES, &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "nini must never construct a path to another agent's credentials:\n  {}",
        offenders.join("\n  ")
    );
}

fn walk_for_credential_paths(dir: &Path, filenames: &[&str], out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_for_credential_paths(&path, filenames, out);
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
            // The `join(` guard is what separates "a path to a credential
            // store" from "a sentence about credentials".
            if line.contains("join(") {
                for name in filenames {
                    if line.contains(name) {
                        out.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
    }
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
