//! Oracle: `/trust` **without an argument opens the trust selector**.
//!
//! # The gap
//!
//! Pi answers a bare `/trust` with `showTrustSelector()`
//! (`interactive-mode.ts#showTrustSelector`), reached from the
//! `text === "/trust"` arm of the slash-command dispatcher
//! (`interactive-mode.ts#setupEditorSubmitHandler`). The user is asked; nothing
//! is written.
//!
//! nini had `crates/nini-tui/src/selectors/trust.rs` — a complete
//! `TrustSelector` — and **nothing ever called it**. The command layer
//! answered the no-argument case by falling back to `sub = "trusted"`:
//! typing `/trust` *marked the working directory trusted*, silently,
//! with no question asked and no list shown.
//!
//! The argument forms (`/trust ask`, `/trust distrust`, `/trust list`,
//! `/trust clear`) are nini's own and must keep working.
//!
//! # Why this is a real oracle
//!
//! The assertion is on `AppState::run_state.status`, which is exactly the
//! flag the TUI runtime watches for `open_selector:<kind>`
//! (`runtime.rs`'s `status.strip_prefix("open_selector:")` arm) to build
//! the selector. So "the status says `open_selector:trust`" is not a
//! proxy for "the selector opens" — it *is* the mechanism the runtime
//! uses.
//!
//! The second half of the test is the one that catches the old bug: a
//! bare `/trust` must **not** write a trust decision. The old fallback
//! wrote `trusted` for the cwd; that write is visible in the sandboxed
//! trust store.

use nini_core::paths::agent_lock;
use nini_core::project_trust::{ProjectTrustStore, TrustDecision};
use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;

/// The trust store the command would write to, inside the sandbox.
fn trust_store_path() -> std::path::PathBuf {
    nini_core::paths::trust_file().expect("a sandboxed agent dir has a trust file")
}

#[test]
fn a_bare_trust_opens_the_trust_selector() {
    let _guard = agent_lock();

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();

    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "");

    assert!(
        matches!(r.outcome, CommandOutcome::Output(_)),
        "/trust should answer, not error"
    );
    assert_eq!(
        state.run_state.status, "open_selector:trust",
        "a bare /trust must ask the user which trust level to apply \
         (Pi: interactive-mode.ts#showTrustSelector)"
    );
}

#[test]
fn a_bare_trust_writes_no_decision() {
    let _guard = agent_lock();

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();

    dispatch(&mut state, &mut settings, CommandId::Trust, "");

    // The old fallback marked the cwd trusted and wrote the store.
    // Nothing may be written when the user was only asked a question.
    let path = trust_store_path();
    assert!(
        !path.exists(),
        "a bare /trust wrote a trust store without asking: {}",
        path.display()
    );

    // And the decision in force must still be the default one.
    let store = ProjectTrustStore::load(&path).unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    assert_eq!(
        store.get_for_cwd(&cwd),
        TrustDecision::Ask,
        "a bare /trust changed the decision in force"
    );
}

#[test]
fn trust_with_an_argument_keeps_the_old_path() {
    let _guard = agent_lock();

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();

    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "ask");

    assert!(
        matches!(r.outcome, CommandOutcome::Output(_)),
        "/trust ask should answer"
    );
    assert_ne!(
        state.run_state.status, "open_selector:trust",
        "/trust ask is the explicit form; it must not open the selector"
    );

    // And it must have written the decision it was asked for.
    let path = trust_store_path();
    let store = ProjectTrustStore::load(&path).unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    assert_eq!(
        store.get_for_cwd(&cwd),
        TrustDecision::Ask,
        "/trust ask must record the decision it was given"
    );
}

#[test]
fn trust_list_does_not_open_the_selector() {
    let _guard = agent_lock();

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();

    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "list");

    assert!(
        matches!(r.outcome, CommandOutcome::Output(_)),
        "/trust list should answer"
    );
    assert_ne!(
        state.run_state.status, "open_selector:trust",
        "/trust list prints the store; it must not open the selector"
    );
}
