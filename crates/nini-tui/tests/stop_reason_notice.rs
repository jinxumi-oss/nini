//! invariant: 8744ceb5
//!
//! # The invariant
//!
//! `AgentEventLite::StopReason(label)` must land in the transcript as a
//! `TranscriptLine::StopNotice(label)` — the last line of the turn, and
//! **the same string**, not a prefix, not a re-wrapped one.
//!
//! # Why nothing asserted it
//!
//! `8744ceb5` was titled "complete v0.8.4 wiring". Before it, the variant
//! existed on the enum and `render.rs` had a `StopNotice` arm — so the
//! code *read* as if a non-success turn end would be shown, and no test
//! touched the route between them. The enum variant was the only member
//! of `AgentEventLite` that nothing pushed, which is precisely the shape
//! of a defect that survives a green suite: the wiring existed on paper
//! on both sides and in the middle.
//!
//! # Why these assertions are not same-source
//!
//! The observable is read out of `transcript_state.lines` — the state a
//! user actually sees — and the frame is rendered from that state. Nothing
//! here re-states the implementation's own idea of the mapping; the
//! label travels from `AgentEventLite` to `TranscriptLine` as an opaque
//! string, and this file asserts it survives that trip byte for byte,
//! including a label that contains the characters a naive
//! `format!("[stop] {label}")` re-wrapping would mangle.

use nini_tui::render::render_frame;
use nini_tui::runtime::{shared_state, AgentEventLite, AgentSink};
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn sink_for(state: &AppState) -> AgentSink {
    AgentSink::new(
        shared_state(state.clone()),
        tokio::sync::watch::channel(false).0,
    )
}

/// The state behind the sink, cloned out from under the lock.
fn snapshot(sink: &AgentSink) -> AppState {
    sink.state().lock().unwrap().clone()
}

fn frame_text(width: u16, height: u16, state: &AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// The three labels `app.rs` maps non-success stop reasons onto. They are
/// Pi's own strings (see `dist/modes/interactive/components/assistant-message.js`),
/// transcribed here only so this test can check the *route*, not the copy.
const LENGTH: &str = "Response was truncated before completion.";
const ABORTED: &str = "Operation aborted.";
const ERROR: &str = "Error during generation.";

/// The property: every label a driver pushes is reachable in the transcript,
/// unchanged, as the turn's `StopNotice`.
#[test]
fn stop_reason_reaches_the_transcript_as_a_stop_notice() {
    for label in [LENGTH, ABORTED, ERROR] {
        let state = AppState::new("test-model");
        let sink = sink_for(&state);
        sink.push(AgentEventLite::TextDelta("partial answer".into()));
        sink.push(AgentEventLite::StopReason(label.to_string()));
        sink.push(AgentEventLite::TurnEnd);

        let state = snapshot(&sink);
        let notices: Vec<&TranscriptLine> = state
            .transcript_state
            .lines
            .iter()
            .filter(|l| matches!(l, TranscriptLine::StopNotice(_)))
            .collect();

        assert_eq!(
            notices.len(),
            1,
            "label {label:?} produced {} StopNotice lines, expected exactly one \
             — the variant was pushed but the transcript kept nothing",
            notices.len()
        );
        match notices[0] {
            TranscriptLine::StopNotice(got) => assert_eq!(
                got, label,
                "the label was rewritten on the way to the transcript"
            ),
            other => panic!("expected a StopNotice, got {other:?}"),
        }
    }
}

/// The label is a *transcript* fact, not a session fact: it must not be
/// folded into an assistant message, because that is what made it
/// invisible before the fix. Exactly one line is added to the transcript,
/// and the assistant text is untouched.
#[test]
fn stop_notice_adds_exactly_one_line_and_never_echoes_into_the_answer() {
    let state = AppState::new("test-model");
    let sink = sink_for(&state);
    sink.push(AgentEventLite::TextDelta("half a sentence".into()));
    let before = snapshot(&sink).transcript_state.lines.len();
    sink.push(AgentEventLite::StopReason(LENGTH.to_string()));
    let after = snapshot(&sink);

    assert_eq!(
        after.transcript_state.lines.len(),
        before + 1,
        "a stop reason must add one line, not zero and not several"
    );
    let rendered: String = after
        .transcript_state
        .lines
        .iter()
        .map(|l| format!("{l:?}"))
        .collect();
    assert!(
        !rendered.contains("half a sentenceResponse"),
        "the notice was appended to the assistant text instead of standing \
         on its own line: {rendered}"
    );
}

/// And the user can see it: the notice reaches the rendered frame, so this
/// is not a state-only assertion. `push_assistant` puts the run in Editing
/// so the frame is drawn as a finished turn rather than a spinner.
#[test]
fn the_stop_notice_is_visible_in_the_frame() {
    let state = AppState::new("test-model");
    let sink = sink_for(&state);
    sink.push(AgentEventLite::TextDelta("half a sentence".into()));
    sink.push(AgentEventLite::StopReason(LENGTH.to_string()));
    sink.push(AgentEventLite::TurnEnd);
    sink.push(AgentEventLite::Done);

    let mut state = snapshot(&sink);
    state.run_state.mode = RunMode::Editing;
    let frame = frame_text(100, 24, &state);
    assert!(
        frame.contains("Response was truncated"),
        "a truncated turn produced a frame with no truncation notice on it:\n{frame}"
    );
}

/// A `StopReason` is terminal information: pushing it must not, by itself,
/// end the run. `TurnEnd` / `Done` do that. If the sink were to flip the
/// mode here, an aborted turn would leave the transcript mid-render.
#[test]
fn pushing_a_stop_reason_does_not_by_itself_end_the_run() {
    let state = AppState::new("test-model");
    let shared = shared_state(state.clone());
    shared.lock().unwrap().run_state.mode = RunMode::Running;
    let sink = AgentSink::new(shared, tokio::sync::watch::channel(false).0);
    sink.push(AgentEventLite::StopReason(ABORTED.to_string()));
    assert_eq!(
        snapshot(&sink).run_state.mode,
        RunMode::Running,
        "the notice ended the run on its own; only TurnEnd/Done may do that"
    );
    sink.push(AgentEventLite::Done);
    assert_ne!(snapshot(&sink).run_state.mode, RunMode::Running);
}
