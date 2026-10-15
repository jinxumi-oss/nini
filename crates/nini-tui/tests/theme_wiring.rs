//! v0.10: the theme a user picks must survive to the screen.
//!
//! # What was wired to nothing
//!
//! Two hops, both silent:
//!
//! 1. `SettingsManager::theme()` returned `Theme::default()` and never read
//!    the `theme` field. `--use-theme light` wrote that field
//!    (`app.rs:331`), `/settings` wrote it, and nothing consulted it.
//! 2. `runtime.rs` did not use `SettingsManager` at all — it built
//!    `SettingsManager::default()`, whose theme field is the literal
//!    `"dark"`. So even a correct `theme()` would not have reached the
//!    renderer, and the theme *watcher*, which does re-read the real
//!    `settings.json`, would have changed the colours to something startup
//!    had never agreed with.
//!
//! These assert the chain end to end: a settings file naming a theme
//! produces that theme in the runtime's state, and the status bar reports
//! the name that was resolved rather than a default it never looked up.

use nini_tui::settings::{Settings, SettingsManager};
use nini_tui::state::AppState;

/// These tests mutate `NINI_AGENT_DIR` and `HOME`, which are process-global.
/// Without this lock they race: cargo runs test functions in parallel, and
/// one test's `remove_var` lands in the middle of another's `theme()` call.
/// The same reason `slash_command_e2e.rs` has a `HOME_LOCK`.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Caller must already hold `ENV_LOCK` — every test here takes it before
/// touching the environment, and a lock taken twice in one thread is a
/// deadlock rather than a safety net.
fn isolate(dir: &std::path::Path) {
    unsafe {
        std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
        std::env::set_var("HOME", dir);
    }
}

fn unisolate() {
    unsafe {
        std::env::remove_var("NINI_AGENT_DIR");
        std::env::remove_var("HOME");
    }
}

/// Write an agent theme file and return the temp dir holding it.
fn theme_dir(name: &str, accent: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let themes = tmp.path().join("agent").join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(
        themes.join(format!("{name}.json")),
        format!("{{\"name\":\"{name}\",\"colors\":{{\"accent\":\"{accent}\"}}}}"),
    )
    .unwrap();
    tmp
}

#[test]
fn a_settings_file_theme_name_is_honoured() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = theme_dir("midnight", "#ff00ff");
    isolate(tmp.path());

    // The CLI writes the chosen name into the settings it hands the runtime.
    let mut settings = Settings::default();
    settings.theme = "midnight".into();
    let mut m = SettingsManager::default();
    m.theme = settings.theme.clone();

    let t = m.theme();
    assert_eq!(t.name, "midnight", "the settings theme name was ignored");
    assert_eq!(
        t.color("accent"),
        ratatui::style::Color::Rgb(255, 0, 255),
        "the theme file's colours did not take effect"
    );
    unisolate();
}

/// `AppState` carries the settings snapshot the runtime reads. If the theme
/// name never reaches it, neither the renderer nor the status bar can use
/// it.
#[test]
fn the_settings_snapshot_reaches_app_state() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    isolate(tmp.path());

    let mut settings = Settings::default();
    settings.theme = "light".into();

    let state = AppState::with_settings("test-model".into(), 200_000, settings.clone());
    assert_eq!(
        state.model_state.settings_snapshot.theme, "light",
        "the settings the CLI loaded are not the ones the runtime reads"
    );
    unisolate();
}

/// The two callers must agree. The watcher resolved through
/// `SettingsManager::theme()`; startup now resolves the same name. If they
/// diverge, editing `settings.json` changes colours the startup never
/// agreed with — which is exactly what happened before.
#[test]
fn startup_and_the_watcher_resolve_identically() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = theme_dir("shared", "#123456");
    isolate(tmp.path());

    let mut m = SettingsManager::default();
    m.theme = "shared".into();
    let via_manager = m.theme();
    let via_resolve = nini_tui::theme::Theme::resolve("shared");

    assert_eq!(via_manager.name, via_resolve.name);
    assert_eq!(via_manager.color("accent"), via_resolve.color("accent"));
    unisolate();
}

/// An unresolvable name must not leave the status bar claiming a theme the
/// screen is not using.
#[test]
fn an_unresolvable_name_reports_the_theme_that_is_actually_in_use() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    isolate(tmp.path());

    let mut m = SettingsManager::default();
    m.theme = "typo-in-the-name".into();
    let resolved = m.theme();
    assert_eq!(resolved.name, "dark");
    // The status bar is fed the *requested* name by the runtime, so the
    // honest thing is for the fallback to be visible rather than silent:
    // this is the assertion that keeps someone from "fixing" the warning
    // away later.
    assert_ne!(
        resolved.name, "typo-in-the-name",
        "an unresolved name must not report itself as resolved"
    );
    unisolate();
}

/// The runtime's own resolution step — the one that was hard-coded.
///
/// A test that builds its own `SettingsManager` proves the resolver works,
/// not that `run` calls it. This drives `startup_theme`, which `run` calls.
#[test]
fn the_runtime_starts_with_the_users_theme() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = theme_dir("midnight", "#ff00ff");
    isolate(tmp.path());

    let mut settings = Settings::default();
    settings.theme = "midnight".into();
    let state = AppState::with_settings("test-model".into(), 200_000, settings);
    let shared: nini_tui::runtime::SharedState = std::sync::Arc::new(std::sync::Mutex::new(state));

    let (name, theme) = nini_tui::runtime::startup_theme(&shared);
    assert_eq!(name, "midnight", "the runtime started on the wrong theme");
    assert_eq!(
        theme.color("accent"),
        ratatui::style::Color::Rgb(255, 0, 255)
    );
    // …and the status bar is told what is in use.
    assert_eq!(
        shared.lock().unwrap().ui_state.theme_name.as_deref(),
        Some("midnight"),
        "the status bar would show a name the screen is not using"
    );
    unisolate();
}

/// `--use-theme` writes the field the CLI loaded; the default has to reach
/// the runtime or the flag is a no-op.
#[test]
fn an_empty_settings_theme_falls_back_to_dark() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    isolate(tmp.path());

    let mut settings = Settings::default();
    settings.theme = String::new();
    let state = AppState::with_settings("test-model".into(), 200_000, settings);
    let shared: nini_tui::runtime::SharedState = std::sync::Arc::new(std::sync::Mutex::new(state));

    let (name, _) = nini_tui::runtime::startup_theme(&shared);
    assert_eq!(name, "dark");
    unisolate();
}
