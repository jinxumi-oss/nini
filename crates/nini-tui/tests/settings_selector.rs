//! `/settings` showed settings the user had not chosen.
//!
//! Three defects, one per line of the branch that handled the result.
//!
//! `runtime.rs` built the selector as
//! `SettingsSelector::new(SettingsManager::default())` - a default manager,
//! not the loaded one - so every row showed a fabricated value whatever the
//! user had configured. It is the same throwaway-default shape the
//! `/thinking` selector had.
//!
//! The result handler then said:
//!
//! ```text
//! // Apply the toggle/cycle. The settings manager inside the selector
//! // persists to disk via its internal mechanism.
//! let new_model = settings_sel.apply(0).map(|_| settings_sel.settings.model_name());
//! ```
//!
//! There is no manager inside the selector, only a snapshot, and nothing
//! was persisted. `apply` ignored its index, so picking "theme" or
//! "thinking" returned the model name - and the function returns
//! `Option<String>` meaning "the new model", so the runtime was told the
//! model had changed when the user had picked a theme.

use nini_tui::selector::SelectorState;
use nini_tui::selectors::settings::{SettingsSelector, SettingsSnapshot};
use nini_tui::settings::SettingsManager;

/// `SettingsManager::flush` resolves its path at call time from
/// `NINI_AGENT_DIR`, so a test that saves settings without this guard
/// writes to the developer's real `~/.nini/agent/settings.json`.
///
/// That happened while writing this file - the first version of the test
/// below saved to the real settings file and then read an empty one back,
/// which is how the missing value showed up rather than as a permission
/// error. `nini_core::paths::agent_lock` exists precisely so that the
/// mistake is one line to avoid rather than one investigation to repeat.
///
/// It also supplies the directory: `agent_lock` creates its own, so a test
/// that kept a separate scratch path would save to one place and read from
/// another - which is how the first sandboxed version of this file still
/// failed, with "(unset)" and no error anywhere.
fn sandbox() -> (nini_core::paths::AgentDirGuard, std::path::PathBuf) {
    let guard = nini_core::paths::agent_lock();
    let file = guard.path().join("settings.json");
    (guard, file)
}

fn description_of(sel: &SettingsSelector, value: &str) -> String {
    sel.state_items()
        .into_iter()
        .find(|i| i.value == value)
        .and_then(|i| i.description)
        .unwrap_or_default()
}

/// The regression: a saved model, theme and thinking level must appear.
#[test]
fn the_selector_shows_the_saved_settings() {
    let (_guard, file) = sandbox();

    let mut m = SettingsManager::load_from_disk(file.clone());
    m.set_default_model("anthropic/claude-sonnet-4");
    m.set_theme("gruvbox-dark");
    m.set_default_thinking_level("high");
    m.flush().expect("flush");

    let sel = SettingsSelector::new(SettingsManager::load_from_disk(file));

    assert_eq!(description_of(&sel, "model"), "anthropic/claude-sonnet-4");
    assert_eq!(description_of(&sel, "theme"), "gruvbox-dark");
    assert_eq!(description_of(&sel, "thinking"), "high");
}

/// A default manager would show nothing for any of these, which is the
/// whole bug.
#[test]
fn a_default_manager_would_have_shown_none_of_them() {
    let sel = SettingsSelector::new(SettingsManager::default());
    assert_ne!(description_of(&sel, "theme"), "anthropic/claude-sonnet-4");
}

/// An unset value used to render as an empty string, which in a list is
/// indistinguishable from a row that failed to load.
///
/// Only `model` and `thinking` are unset on a default manager - `theme`
/// has a default of its own, and asserting otherwise would have been
/// asserting that nini had no theme, which it does.
#[test]
fn an_unset_value_says_so() {
    let sel = SettingsSelector::new(SettingsManager::default());

    for key in ["model", "thinking"] {
        let d = description_of(&sel, key);
        assert!(!d.is_empty(), "{key} rendered as an empty description");
        assert_eq!(d, "(unset)", "{key} should say it is unset, said {d:?}");
    }

    // theme is a real default, and it must be shown as a value
    assert_eq!(description_of(&sel, "theme"), "dark");
}

/// `apply(0)` ignored its index and returned the same answer for every
/// category, so the runtime reported a model change for any pick.
#[test]
fn the_index_selects_the_category() {
    let sel = SettingsSelector::with_snapshot(SettingsSnapshot {
        model: "some/model".into(),
        theme: "dark".into(),
        thinking: "high".into(),
    });

    assert_eq!(sel.picked_category(0), Some("model"));
    assert_eq!(sel.picked_category(1), Some("theme"));
    assert_eq!(sel.picked_category(2), Some("thinking"));
    assert_eq!(sel.picked_category(9), None, "out of range must not answer");
}

/// Picking a category is what puts it in `result`; asking before that must
/// not claim a choice was made.
#[test]
fn nothing_is_reported_before_a_pick() {
    let mut sel = SettingsSelector::with_snapshot(SettingsSnapshot::default());
    assert!(
        sel.result.is_none(),
        "a result existed before anything was picked"
    );

    sel.state_set_selected(1);
    let _ = sel.state_on_select();
    assert_eq!(sel.result.as_deref(), Some("theme"));
    assert_eq!(sel.picked_category(sel.selected), Some("theme"));
}
