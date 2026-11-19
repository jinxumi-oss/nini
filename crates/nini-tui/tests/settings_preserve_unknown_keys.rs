//! The TUI must not delete the settings it does not own.
//!
//! `SettingsManager::flush` used to serialise the TUI's own `Settings`
//! struct and write it straight over `~/.nini/agent/settings.json` —
//! deleting every key the struct has no field for. The file has a second
//! reader: `nini-core`'s loader (`crates/nini-core/src/settings.rs`)
//! reads exactly those keys — `retry`, `max_iterations`,
//! `max_parallel_tools`, `tool_execution`, `provider`, `model`, … — so
//! a user who wrote `{"retry": {"max_retries": 5}}` lost it the moment
//! they changed the theme in the TUI, with nothing said about it.
//!
//! These pin the merge: the keys the TUI owns are written over, the
//! others survive untouched.

use nini_tui::settings::SettingsManager;

/// One isolated agent directory per test.
///
/// `SettingsManager::flush` resolves its path at call time from
/// `NINI_AGENT_DIR`, so without this guard a test would write to the
/// developer's real `~/.nini/agent/settings.json`. `agent_lock()`
/// redirects the directory **and** holds a process-wide lock for its
/// whole lifetime — the environment is process-global, so two tests
/// redirecting it at once would see each other's files.
fn sandbox() -> (nini_core::paths::AgentDirGuard, std::path::PathBuf) {
    let guard = nini_core::paths::agent_lock();
    let file = guard.path().join("settings.json");
    (guard, file)
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("read settings.json");
    serde_json::from_str(&raw).expect("settings.json is valid JSON")
}

/// The regression: the keys `nini-core` reads must still be there after
/// the TUI saved its own settings, and the TUI's own key must be the
/// new value.
#[test]
fn flushing_keeps_the_keys_the_tui_does_not_own() {
    let (_guard, file) = sandbox();
    std::fs::write(
        &file,
        r#"{"retry": {"max_retries": 5}, "max_iterations": 7, "theme": "old"}"#,
    )
    .unwrap();

    let mut m = SettingsManager::load_from_disk(file.clone());
    m.set_theme("new");
    m.flush().expect("flush");

    let doc = read_json(&file);
    assert_eq!(
        doc["retry"],
        serde_json::json!({"max_retries": 5}),
        "the `retry` key was deleted by the TUI's flush"
    );
    assert_eq!(
        doc["max_iterations"],
        serde_json::json!(7),
        "the `max_iterations` key was deleted by the TUI's flush"
    );
    assert_eq!(
        doc["theme"],
        serde_json::json!("new"),
        "the theme was not updated"
    );
}

/// There is no file yet: the flush must not fail, and it must write the
/// TUI's own keys.
#[test]
fn flushing_without_a_file_writes_the_tui_keys() {
    let (_guard, file) = sandbox();
    assert!(
        !file.exists(),
        "the sandbox must start without a settings file"
    );

    let mut m = SettingsManager::default();
    m.set_theme("light");
    m.flush()
        .expect("flush must not fail when there is no file yet");

    let doc = read_json(&file);
    assert_eq!(doc["theme"], serde_json::json!("light"));
    assert_eq!(doc["context_window"], serde_json::json!(200_000));
}

/// A file that cannot be parsed is not a reason to fail the flush: the
/// TUI's own keys are still written, and the broken file is replaced by
/// a valid one.
#[test]
fn flushing_over_a_broken_file_still_writes_the_tui_keys() {
    let (_guard, file) = sandbox();
    std::fs::write(&file, "{not json").unwrap();

    let mut m = SettingsManager::default();
    m.set_theme("light");
    m.flush()
        .expect("a broken file must not make the flush fail");

    let doc = read_json(&file);
    assert_eq!(doc["theme"], serde_json::json!("light"));
}
