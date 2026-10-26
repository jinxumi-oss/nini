//! Oracle: `/reload` re-loads what can actually be re-loaded — and says
//! out loud what cannot.
//!
//! # The gap (B3)
//!
//! Pi's `/reload` (`interactive-mode.ts#handleReloadCommand` →
//! `agent-session.ts#reload`) re-loads:
//!
//! * the settings (`settingsManager.reload()`),
//! * the **resource loader** — skills, prompt templates, themes,
//!   `AGENTS.md` (`_resourceLoader.reload()`),
//! * the **keybindings** (`keybindings.reload()`),
//! * and rebuilds the runtime and the chat.
//!
//! nini re-loaded the settings and the theme, and *counted* the skills on
//! disk while throwing the count away. Two of Pi's four items were
//! missing, and one of them — the keybindings — was **impossible** to
//! reload: the user overrides were cached in a `OnceLock` for the
//! lifetime of the process.
//!
//! # What the oracle pins
//!
//! 1. the keybindings are re-read (the cache is reloadable);
//! 2. the user's `models.json` models are re-read;
//! 3. the output **does not claim** to have reloaded what nini cannot
//!    reload (skills / the system prompt);
//! 4. the settings still reload (the behaviour that already existed).

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;
use std::io::Write as _;

/// One isolated agent directory per test, so a `/reload` cannot see the
/// developer's own `settings.json` / `models.json` / `keybindings.json`.
///
/// `agent_lock()` is the project's own guard: it redirects the agent
/// directory **and** holds a process-wide lock for its whole lifetime —
/// the environment is process-global, so two tests redirecting it at
/// once would see each other's directories.
struct Sandbox {
    _guard: nini_core::paths::AgentDirGuard,
}

impl Sandbox {
    fn new() -> Self {
        Sandbox {
            _guard: nini_core::paths::agent_lock(),
        }
    }

    fn path(&self) -> &std::path::Path {
        self._guard.path()
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.path().join(name);
        let mut f = std::fs::File::create(path).expect("create");
        writeln!(f, "{}", contents).expect("write");
    }

    fn reload(&self, state: &mut AppState) -> String {
        let mut settings = SettingsManager::default();
        let r = dispatch(state, &mut settings, CommandId::Reload, "");
        match r.outcome {
            CommandOutcome::Output(lines) => lines.join("\n"),
            _ => String::new(),
        }
    }
}

#[test]
fn the_keybindings_are_reloaded() {
    let sb = Sandbox::new();
    // A user override for one action. The file's format is
    // `{ "<action>": "<key spec>" }` (see keybindings_manager.rs).
    // The documented format (see keybindings_manager.rs): a "bindings"
    // object mapping **action names** to key specs.
    sb.write("keybindings.json", r#"{"bindings": {"Quit": "Ctrl+Q"}}"#);

    let mut state = AppState::new("test-model");
    let out = sb.reload(&mut state);
    assert!(
        out.contains("keybindings"),
        "the reload must report the keybindings it re-read: {out:?}"
    );
    assert!(
        out.contains("1 user override"),
        "the one override in the file must be in effect: {out:?}"
    );
}

#[test]
fn the_models_json_models_are_reloaded() {
    let sb = Sandbox::new();
    sb.write(
        "models.json",
        r#"{"providers":{"gw":{"api":"openai-completions","baseUrl":"http://127.0.0.1:9877/v1",
        "models":[{"id":"MiniMax-M2.7-highspeed","reasoning":true,"contextWindow":204800,"maxTokens":49152}]}}}"#,
    );

    let mut state = AppState::new("test-model");
    let out = sb.reload(&mut state);
    assert!(
        out.contains("models: 1 from models.json"),
        "the user's declared model must be picked up: {out:?}"
    );
    assert!(
        state
            .model_state
            .user_models
            .iter()
            .any(|m| m.id.ends_with("MiniMax-M2.7-highspeed")),
        "the model must be in the state, not just in the report: {:?}",
        state.model_state.user_models
    );
}

#[test]
fn the_report_does_not_claim_to_reload_the_skills() {
    let sb = Sandbox::new();
    let mut state = AppState::new("test-model");
    let out = sb.reload(&mut state);
    assert!(
        out.contains("skills:") && out.contains("not reloaded"),
        "nini cannot reload the skills (the system prompt is fixed for the \
         session), and it must say exactly that: {out:?}"
    );
    assert!(
        !out.contains("reloaded skills") && !out.contains("skills reloaded"),
        "the report must not claim a reload that does not happen: {out:?}"
    );
}

#[test]
fn the_settings_still_reload() {
    let sb = Sandbox::new();
    sb.write("settings.json", r#"{"theme": "light"}"#);

    let mut state = AppState::new("test-model");
    state.model_state.settings_path = Some(sb.path().join("settings.json"));
    let out = sb.reload(&mut state);
    assert!(
        out.contains("reloaded settings"),
        "the settings reload (the behaviour that already existed) must \
         still happen: {out:?}"
    );
    assert_eq!(
        state.model_state.settings_snapshot.theme, "light",
        "the new theme must be in effect"
    );
}
