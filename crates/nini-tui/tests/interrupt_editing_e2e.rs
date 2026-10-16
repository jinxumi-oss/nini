//! After an interrupt, the editor accepts typing — immediately.
//!
//! Reported as: "while nini is running, pressing Esc means I cannot type
//! anything". Measured in tmux against a provider that takes eight seconds
//! to answer:
//!
//! ```text
//!   t≈ 2s after Esc   status "aborted"   footer "Enter retry  Esc clear"
//!   t≈ 6s after Esc   status "idle"      footer back to normal
//!   typing at t≈2s: discarded
//!   typing at t≈16s: works
//! ```
//!
//! The cause was a `RunMode::Aborted` that nothing else referenced: every
//! input path gates on `mode == Editing`, and no code path returned from
//! `Aborted` except the agent stream's `Done`, which arrives when the run
//! winds down on its own. The four seconds after an interrupt were dead
//! time in which every keystroke was dropped with no message.
//!
//! Pi has no such state. `onEscape` aborts and calls
//! `restoreQueuedMessagesToEditor({ abort: true })` — the editor is usable
//! the instant you press the key
//! (`@earendil-works/pi-coding-agent` `interactive-mode.js:2219`).
//!
//! # Why these assertions compile against the old code
//!
//! They name `RunMode::Editing` and nothing else. `RunMode::Aborted` is
//! deleted, so a test that referenced it would fail to *build* against the
//! fix's parent — which is not an oracle. This way the same assertion runs
//! on both sides: the old code puts the TUI somewhere that is not `Editing`
//! and goes red.

use nini_tui::keys::Key;
use nini_tui::render::render_frame_with_theme;
use nini_tui::runtime::apply_action;
use nini_tui::state::{AppState, RunMode};
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn frame(state: &AppState, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render_frame_with_theme(f, state, &Theme::dark()))
        .unwrap();
    let buf = t.backend().buffer().clone();
    let area = *buf.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// The reported symptom, at the level the key handler reaches it.
#[test]
fn an_interrupt_leaves_the_editor_editable() {
    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running;
    state.run_state.status = "working...".to_string();

    apply_action(&mut state, Key::esc());

    assert_eq!(
        state.run_state.mode,
        RunMode::Editing,
        "an interrupt must not park the TUI in a state no key handler accepts \
         input in — every insert and submit is gated on Editing"
    );
    // The turn being cut short is still visible to the user.
    assert_eq!(state.run_state.status, "aborted");
}

/// The second half of the report: the footer advertised `Enter retry`, and
/// no branch of the submit path ever handled that mode — the key press did
/// nothing at all.
#[test]
fn no_mode_advertises_a_key_that_does_nothing() {
    let mut state = AppState::new("test-model");
    state.run_state.status = "aborted".to_string();
    let text = frame(&state, 100, 30);
    assert!(
        !text.contains("retry"),
        "the footer promises a retry that is not implemented:\n{text}"
    );
    assert!(
        text.contains("send"),
        "and the editor is available again, which is what the user wanted:\n{text}"
    );
}

/// Esc on an idle editor clears it — the other half of Pi's `onEscape`, and
/// the reason the abort branch has to be reachable in the first place.
#[test]
fn esc_on_an_idle_editor_still_clears_it() {
    let mut state = AppState::new("test-model");
    assert_eq!(state.run_state.mode, RunMode::Editing);
    state.input.text.push('x');
    assert!(state.input.text.contains('x'));

    apply_action(&mut state, Key::esc());

    assert!(
        !state.input.text.contains('x'),
        "Esc on an idle editor clears it"
    );
}
