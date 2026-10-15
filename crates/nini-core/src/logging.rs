//! The logging switch, in the crate both the TUI and the CLI can reach.
//!
//! # What was missing
//!
//! v0.9.2's gap list: *"`/debug` toggles a flag with no consumers; `tracing`
//! is declared in four crates and initialised nowhere. There is no logging."*
//!
//! Both halves were true. `cmd_debug` flipped `ui_state.debug_logging` and
//! printed a confirmation; nothing read the field. `tracing-subscriber` sat
//! in `Cargo.toml` as an unused dependency. A user who hit a problem had one
//! documented action (`/debug`) which produced no diagnostic output.
//!
//! # Why a global switch rather than a reloaded subscriber
//!
//! `tracing_subscriber::reload` would rebuild the subscriber stack on every
//! toggle, and its handle has to be threaded to every construction site to
//! stay reachable from a slash command. The thing being switched is one
//! predicate — "is debug on?" — so the subscriber is installed once at TRACE
//! and its *filter* consults this flag. Toggling is one atomic store, takes
//! effect on the next event, and cannot fail halfway.
//!
//! That matches Pi, where `/debug` changes verbosity rather than rebuilding
//! anything.
//!
//! The flag lives here, not in `nini-tui`, because the CLI needs it too:
//! `--verbose` sets it before the first event so a startup failure is still
//! captured, and `nini-tui` cannot be imported by the subscriber that writes
//! the log.

use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG: AtomicBool = AtomicBool::new(false);

/// Turn debug logging on or off. Takes effect on the next event.
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

/// Whether debug logging is on.
pub fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Where the log goes: `~/.nini/agent/log`, or `$NINI_AGENT_DIR/log`.
///
/// Matches the location `/debug`'s help text has always named. `None` when
/// there is no agent directory at all — nini must not invent a place to
/// write, the same rule `session_init_ephemeral` follows.
pub fn log_path() -> Option<std::path::PathBuf> {
    crate::paths::agent_file("log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_switch_round_trips() {
        let before = is_debug();
        set_debug(true);
        assert!(is_debug());
        set_debug(false);
        assert!(!is_debug());
        set_debug(before);
    }

    /// The log must follow the agent directory and never escape into the
    /// project — writing a log into someone's repository is its own bug.
    #[test]
    fn log_path_follows_the_agent_directory() {
        let _guard = crate::paths::agent_lock();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("NINI_AGENT_DIR", tmp.path()) };
        let p = log_path().expect("agent dir is set");
        assert!(
            p.starts_with(tmp.path()),
            "log escaped the agent dir: {p:?}"
        );
        unsafe { std::env::remove_var("NINI_AGENT_DIR") };
    }
}
