//! `/thinking` changed a number on screen and forgot it.
//!
//! Four breaks, all silent:
//!
//!  1. `runtime.rs`'s cycling path called
//!     `set_model_thinking_level(&model, level)` whenever a model was
//!     selected - the normal case - and that was
//!     `fn set_model_thinking_level(&mut self, _model, _level) {}`.
//!  2. Neither branch of that path called `flush`.
//!  3. The `ThinkingSelector` result handler built
//!     `SettingsManager::default()`, wrote the level into it, and dropped it.
//!  4. `cmd_thinking` wrote the level and returned without flushing.
//!
//! Coming back in, `app.rs` read `nini_core::settings::Settings::thinking_level`
//! - a field **nothing in production ever assigned**. `/thinking` wrote
//! `default_thinking_level`. Its comment claimed the field "has been
//! written and read back since v0.8"; it had been neither.
//!
//! So a user set `/thinking high`, watched the footer change, restarted,
//! and got "off".
//!
//! # A trap this file walked into
//!
//! `SettingsManager::flush()` resolves its path at call time. A test that
//! flushes outside `NINI_AGENT_DIR` therefore writes to the developer's
//! real `~/.nini/agent/settings.json` - which is exactly what the first
//! version of this file did, once, to this machine. Every test here runs
//! its whole body inside `sandboxed()`, including the write.
//!
//! The lock is a plain `Mutex` and therefore not reentrant, so the
//! helpers below assume they are already inside it and do not take it
//! again.

use nini_core::paths;
use nini_core::provider::ThinkingLevel;
use nini_tui::settings::SettingsManager;
use std::path::{Path, PathBuf};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` with `NINI_AGENT_DIR` pointed at `dir`.
fn sandboxed<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved = std::env::var_os(paths::ENV_AGENT_DIR);
    // SAFETY: the lock is held across the whole set-restore cycle, so no
    // other thread in this binary can observe a half-applied change.
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

/// A scratch agent directory, unique per call: these run concurrently in
/// one process and a shared name would have one test delete another's.
fn scratch(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("nini-thinking-{tag}-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("scratch dir");
    p
}

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read through `nini-core`, exactly as `app.rs` does.
///
/// Takes no path: `load_settings` resolves `<agent dir>/settings.json` from
/// the environment itself, so the caller is responsible for the sandbox
/// being in place. Assuming the caller holds it is the point - the lock is
/// not reentrant, and locking here would deadlock.
fn read_back() -> nini_core::settings::Settings {
    nini_core::settings::load_settings(Path::new("/"))
}

fn level_of() -> Option<String> {
    let saved = read_back();
    saved.default_thinking_level.or(saved.thinking_level)
}

/// The regression: write, drop the manager, read it back with a fresh one.
#[test]
fn a_thinking_level_survives_a_restart() {
    let dir = scratch("restart");
    let _d = Dir(dir.clone());
    sandboxed(&dir, || {
        let file = dir.join("settings.json");

        let mut settings = SettingsManager::load_from_disk(file.clone());
        settings.set_default_thinking_level("high");
        settings.flush().expect("flush");

        assert_eq!(
            level_of().as_deref(),
            Some("high"),
            "the level did not survive the round trip"
        );
        let next = SettingsManager::load_from_disk(file);
        assert_eq!(next.effective_thinking_level(), Some("high"));
    });
}

/// Bug 3, kept honest: writing into a throwaway manager must change
/// nothing. This is the shape the selector handler had.
#[test]
fn writing_into_a_throwaway_manager_changes_nothing_on_disk() {
    let dir = scratch("throwaway");
    let _d = Dir(dir.clone());
    sandboxed(&dir, || {
        let mut throwaway = SettingsManager::default();
        throwaway.set_default_thinking_level("high");
        // deliberately no flush

        assert!(!dir.join("settings.json").exists());
        assert_eq!(level_of(), None);
    });
}

/// `flush` rewrites the whole `Settings`. Recording one word must not cost
/// the user their theme and model.
#[test]
fn persisting_thinking_keeps_the_rest_of_the_settings() {
    let dir = scratch("preserve");
    let _d = Dir(dir.clone());
    sandboxed(&dir, || {
        let file = dir.join("settings.json");

        let mut settings = SettingsManager::load_from_disk(file.clone());
        settings.set_theme("gruvbox-dark");
        settings.set_default_model("anthropic/claude-sonnet-4");
        settings.flush().expect("first flush");

        let mut settings = SettingsManager::load_from_disk(file.clone());
        settings.set_default_thinking_level("low");
        settings.flush().expect("second flush");

        let back = read_back();
        assert_eq!(back.theme.as_deref(), Some("gruvbox-dark"));
        // `/model` writes `defaultModel`; `nini-core` used to read only
        // `model`, so the saved default was invisible on the next run.
        assert_eq!(
            back.default_model.as_deref().or(back.model.as_deref()),
            Some("anthropic/claude-sonnet-4"),
            "the saved default model did not survive"
        );
        assert_eq!(back.default_thinking_level.as_deref(), Some("low"));
    });
}

/// Pi writes `defaultThinkingLevel`
/// (`pi-coding-agent/src/core/settings-manager.ts:753`). A settings file
/// from Pi has to load.
#[test]
fn a_pi_written_settings_file_is_understood() {
    let dir = scratch("pi");
    let _d = Dir(dir.clone());
    std::fs::write(
        dir.join("settings.json"),
        br#"{"defaultThinkingLevel":"xhigh"}"#,
    )
    .unwrap();

    sandboxed(&dir, || {
        assert_eq!(
            level_of().as_deref(),
            Some("xhigh"),
            "Pi's spelling did not load"
        );
    });
}

/// An older nini wrote `thinking_level`. Kept as a read alias so those
/// files keep working rather than being silently ignored.
#[test]
fn an_older_nini_settings_file_is_still_understood() {
    let dir = scratch("legacy");
    let _d = Dir(dir.clone());
    std::fs::write(dir.join("settings.json"), br#"{"thinking_level":"medium"}"#).unwrap();

    sandboxed(&dir, || {
        assert_eq!(level_of().as_deref(), Some("medium"));
    });
}

/// Both spellings present: the persisted one wins, because that is the one
/// nini writes.
#[test]
fn the_persisted_spelling_wins_over_the_legacy_one() {
    let dir = scratch("both");
    let _d = Dir(dir.clone());
    std::fs::write(
        dir.join("settings.json"),
        br#"{"default_thinking_level":"high","thinking_level":"off"}"#,
    )
    .unwrap();

    sandboxed(&dir, || {
        assert_eq!(level_of().as_deref(), Some("high"));
    });
}

/// Persisting it is pointless unless it reaches the provider.
#[test]
fn the_persisted_level_reaches_the_provider() {
    let dir = scratch("provider");
    let _d = Dir(dir.clone());
    sandboxed(&dir, || {
        let mut settings = SettingsManager::load_from_disk(dir.join("settings.json"));
        settings.set_default_thinking_level("xhigh");
        settings.flush().unwrap();

        let parsed = level_of().and_then(|l| l.parse::<ThinkingLevel>().ok());
        assert!(
            parsed.is_some(),
            "the persisted level did not parse into a ThinkingLevel: {parsed:?}"
        );
    });
}

/// With no settings file there is nowhere to write, and no path may be
/// invented for it.
#[test]
fn with_no_settings_file_nothing_is_created() {
    let dir = scratch("nosettings");
    let _d = Dir(dir.clone());
    sandboxed(&dir, || {
        assert!(!dir.join("settings.json").exists());
        assert_eq!(level_of(), None);
    });
}
