//! First run: does nini have a home yet?
//!
//! # What Pi does
//!
//! Pi 0.84.2, `pi-coding-agent/src/cli/startup-ui.ts:115`:
//!
//! ```ts
//! export function shouldRunFirstTimeSetup(settingsPath = getSettingsPath()): boolean {
//!     if (!isOfficialDistribution({ packageName, appName, configDirName })) return false;
//!     if (!areExperimentalFeaturesEnabled()) return false;
//!     if (process.env[ENV_AGENT_DIR]) return false;
//!     return !existsSync(settingsPath);
//! }
//! ```
//!
//! called from `main.ts:663`, gated on
//! `appMode === "interactive" && !help && !listModels`. It then shows a
//! wizard for theme and analytics opt-in, before any runtime services are
//! created so the answers apply everywhere.
//!
//! # What nini does, and why it is not the same predicate
//!
//! Two of Pi's four conditions do not transfer:
//!
//!   * `isOfficialDistribution` compares the installed npm package name.
//!     nini has no package to verify; there is nothing to check.
//!   * `areExperimentalFeaturesEnabled` gates Pi's feature flag. Adding an
//!     experimental flag to nini would be a product decision, not a
//!     rewrite decision, and inventing one here would be making policy.
//!
//! One does transfer directly, and it is the interesting one: **Pi skips
//! first-run when the agent directory is overridden**, because a user who
//! pointed `PI_CODING_AGENT_DIR` somewhere has already decided where things
//! live. nini honours that with `NINI_AGENT_DIR`.
//!
//! The last condition is where nini deliberately differs. Pi keys on
//! `settings.json` because its wizard *writes* `settings.json`, so the
//! state ends after one run. nini has no wizard — there is no theme or
//! analytics question to ask, `--use-theme` already covers themes, and
//! nini collects no analytics — so keying on `settings.json` would leave
//! the condition permanently true for anyone who never changes a setting,
//! and the notice would repeat on every single start.
//!
//! nini keys on **the agent directory existing**, because creating it is
//! the thing that both ends the state and is worth doing. This is a
//! product decision for nini, not parity with Pi, and it is recorded here
//! so nobody later reads it as an observed Pi behaviour.
//!
//! # The directory is created lazily elsewhere regardless
//!
//! `/export`, `/new`, the session writer and `SettingsManager::flush`
//! each `create_dir_all` their own parent. Creating it here is not load
//! bearing for correctness; it is so a first-run user has somewhere to
//! put prompts, skills and keybindings, and is told where that is.

use std::path::Path;

/// True when nini has no agent directory yet.
///
/// `None` when the location cannot be worked out at all — no `HOME` and no
/// `NINI_AGENT_DIR`. That is not a first run, it is a broken environment,
/// and the two must not be reported the same way.
pub fn needs_first_time_setup(agent_dir: Option<&Path>) -> bool {
    match agent_dir {
        Some(dir) => !dir.exists(),
        None => false,
    }
}

/// Create the agent directory.
///
/// Idempotent: an existing directory is `Ok`, not an error. Callers are
/// expected to run this only when [`needs_first_time_setup`] said yes, but
/// two nini processes starting at once is not a failure.
pub fn init_agent_dir(agent_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(agent_dir)?;
    Ok(())
}

/// What to say on a first run, or `None` when this is not one.
///
/// Returns `None` when the agent directory is explicitly overridden: a
/// user who set `NINI_AGENT_DIR` knows where their files are and does not
/// need to be told, and Pi makes the same exemption.
pub fn first_run_notice(agent_dir: Option<&Path>, overridden: bool) -> Option<String> {
    if overridden {
        return None;
    }
    let dir = agent_dir?;
    if dir.exists() {
        return None;
    }
    Some(format!(
        "first run: nini will keep its files in {}\n  \
         sessions, exports, trust and settings live there; \
         prompts, skills and keybindings can go in {}/",
        dir.display(),
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p =
            std::env::temp_dir().join(format!("nini-startup-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn a_missing_directory_is_a_first_run() {
        let dir = scratch("missing");
        assert!(needs_first_time_setup(Some(&dir)));
    }

    #[test]
    fn an_existing_directory_is_not() {
        let dir = scratch("existing");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!needs_first_time_setup(Some(&dir)));
    }

    #[test]
    fn init_creates_it_and_the_predicate_flips() {
        let dir = scratch("init");
        assert!(needs_first_time_setup(Some(&dir)));

        init_agent_dir(&dir).expect("init");

        assert!(dir.is_dir(), "init did not create the directory");
        assert!(!needs_first_time_setup(Some(&dir)));
    }

    #[test]
    fn init_twice_is_fine() {
        let dir = scratch("twice");
        init_agent_dir(&dir).unwrap();
        init_agent_dir(&dir).expect("second call must not fail");
    }

    /// Two nini processes starting at once is not a failure.
    #[test]
    fn init_on_an_existing_directory_succeeds() {
        let dir = scratch("race");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(init_agent_dir(&dir).is_ok());
    }

    /// A broken environment is not a first run. Reporting the two the same
    /// way would tell a user with no `HOME` that nini is about to create
    /// files, which is the opposite of what is happening.
    #[test]
    fn no_location_at_all_is_not_a_first_run() {
        assert!(!needs_first_time_setup(None));
        assert!(first_run_notice(None, false).is_none());
    }

    #[test]
    fn the_notice_names_the_directory() {
        let dir = scratch("notice");
        let notice = first_run_notice(Some(&dir), false).expect("a first run has a notice");
        assert!(
            notice.contains(&dir.display().to_string()),
            "the notice does not say where files will go: {notice}"
        );
    }

    /// Pi skips first-run when the agent directory is overridden, because
    /// someone who pointed `PI_CODING_AGENT_DIR` somewhere has already
    /// decided. nini honours that with `NINI_AGENT_DIR`.
    #[test]
    fn an_overridden_location_gets_no_notice() {
        let dir = scratch("override");
        assert!(needs_first_time_setup(Some(&dir)), "still a first run");
        assert!(
            first_run_notice(Some(&dir), true).is_none(),
            "a user who set the override must not be told where their files are"
        );
    }

    #[test]
    fn no_notice_once_the_directory_exists() {
        let dir = scratch("settled");
        init_agent_dir(&dir).unwrap();
        assert!(first_run_notice(Some(&dir), false).is_none());
    }
}
