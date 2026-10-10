//! Where nini keeps its files.
//!
//! v0.10 (R3): every one of these used to say `.pi`. nini read its settings,
//! models, keybindings, prompts, skills and sessions out of
//! `~/.pi/agent/`, wrote its sessions and exports back into it, and scanned
//! Pi's session history to offer it in `/resume`. 894 references across 81
//! files, 14 of them actual path construction and the rest commentary
//! following from them.
//!
//! That made nini a *channel* rather than a rewrite: two installs shared
//! one directory, so editing settings in one changed the other, and nini
//! did not work on a machine where Pi had never been installed.
//!
//! The boundary this module draws:
//!
//!   * **Runtime paths are nini's own.** `~/.nini/agent/`, and `.nini/` in
//!     a project. Nothing here reads Pi's installation or its config
//!     directory.
//!   * **File *formats* stay Pi-compatible.** The session JSONL schema and
//!     the model-metadata schema are unchanged. Being able to open a Pi
//!     session file is a feature; needing Pi to be installed to have a
//!     config directory is a dependency. Those are different things, and
//!     only the second one was wrong.
//!
//! `NINI_AGENT_DIR` overrides the user directory, the way Pi's
//! `PI_CODING_AGENT_DIR` does, because tests and sandboxes need it and
//! hardcoding `$HOME` makes that impossible.

use std::path::{Path, PathBuf};

/// Override for the user-level agent directory.
pub const ENV_AGENT_DIR: &str = "NINI_AGENT_DIR";

/// The directory name under `$HOME`.
pub const HOME_DIR_NAME: &str = ".nini";

/// The directory name under the project root and under the home directory.
pub const PROJECT_DIR_NAME: &str = ".nini";

/// The subdirectory that holds the agent's own state, mirroring Pi's
/// `~/.pi/agent/` so the two layouts are legible side by side.
pub const AGENT_SUBDIR: &str = "agent";

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `~/.nini/agent`, or `$NINI_AGENT_DIR` when set.
pub fn agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_AGENT_DIR) {
        let s = dir.to_string_lossy().into_owned();
        if !s.trim().is_empty() {
            return Some(expand_tilde(&s));
        }
    }
    Some(home_dir()?.join(HOME_DIR_NAME).join(AGENT_SUBDIR))
}

/// `<cwd>/.nini`.
pub fn project_dir(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_DIR_NAME)
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home_dir().unwrap_or_else(|| PathBuf::from(".")).join(rest),
        None => PathBuf::from(p),
    }
}

/// A file in the user-level agent directory.
pub fn agent_file(name: &str) -> Option<PathBuf> {
    Some(agent_dir()?.join(name))
}

/// A file in the project-level directory.
pub fn project_file(cwd: &Path, name: &str) -> PathBuf {
    project_dir(cwd).join(name)
}

/// `~/.nini/agent/sessions` - where a conversation is recorded.
pub fn sessions_dir() -> Option<PathBuf> {
    agent_dir().map(|d| d.join("sessions"))
}

/// `~/.nini/agent/exports` - where `/export` writes its HTML snapshot.
pub fn exports_dir() -> Option<PathBuf> {
    agent_dir().map(|d| d.join("exports"))
}

/// `~/.nini/agent/trust.json` - per-project trust decisions.
pub fn trust_file() -> Option<PathBuf> {
    agent_file("trust.json")
}

/// The candidates for a project file, project first then user-level.
///
/// Project-level overrides user-level, which is what every one of these
/// loaders did before, and the order is kept so a project can override a
/// machine-wide default without deleting it.
pub fn candidates(cwd: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = vec![project_file(cwd, name)];
    if let Some(user) = agent_file(name) {
        if !out.contains(&user) {
            out.push(user);
        }
    }
    out
}
