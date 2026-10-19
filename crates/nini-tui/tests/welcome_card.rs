//! invariant: 8744ceb5
//!
//! # The invariant
//!
//! Cold start — empty transcript **and** empty input — must render a
//! welcome card inside the transcript area: the brand line, the "press
//! Enter" instruction, and the five example commands. The moment either
//! the transcript has content or the user has typed a character, the
//! card must be gone.
//!
//! # Why nothing asserted it
//!
//! `8744ceb5` says the card "was already in the working tree before the
//! v0.8.4 commit but got dropped from the diff when I rolled it back".
//! A card that was dropped and then re-wired by hand is exactly the kind
//! of change that ships green: there was never an assertion saying the
//! cold-start frame is anything other than blank, so removing the card
//! again — or moving it behind the wrong condition, or drawing it after
//! the user starts typing — would not fail a single test.
//!
//! # Why these assertions are not same-source
//!
//! The card is read out of the rendered frame, not out of
//! `render_welcome_card`'s own `Vec<(&str, &str)>`. What is asserted is
//! what a user sees: the strings, and the *condition* under which they
//! appear, at three widths. A card that is emitted but clipped, or
//! rendered into a zero-height chunk, or drawn on top of real
//! conversation, fails here.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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

/// The five commands the card promises. A user who reads this screen is
/// being told what they can type; dropping one silently is a regression
/// even though the card still draws.
const EXAMPLES: [&str; 5] = ["/help", "/model", "/theme", "/clear", "!ls -la"];

/// Cold start: the card is there, with its instruction and all five
/// examples, at every width the card claims to survive.
#[test]
fn cold_start_renders_the_welcome_card() {
    for width in [60u16, 80, 120, 200] {
        let state = AppState::new("test-model");
        let frame = frame_text(width, 30, &state);
        assert!(
            frame.contains("Type a prompt and press Enter"),
            "at width {width}, cold start shows no welcome card:\n{frame}"
        );
        assert!(
            frame.contains("nini"),
            "at width {width}, the card lost its brand line:\n{frame}"
        );
        for cmd in EXAMPLES {
            assert!(
                frame.contains(cmd),
                "at width {width}, the welcome card is missing the example \
                 command {cmd:?}:\n{frame}"
            );
        }
    }
}

/// The card is not decoration drawn over the conversation: once there is
/// a transcript it must be gone, so the first thing a returning user
/// sees is their own history.
#[test]
fn the_card_is_gone_once_there_is_a_transcript() {
    let mut state = AppState::new("test-model");
    state.push_user("what did we do yesterday");
    state.push_divider();
    state.push_assistant("a fair amount");

    let frame = frame_text(120, 30, &state);
    assert!(
        !frame.contains("Type a prompt and press Enter"),
        "the welcome card is drawn on top of a live conversation:\n{frame}"
    );
    assert!(frame.contains("what did we do yesterday"));
}

/// And it is gone as soon as the user starts typing — before anything is
/// sent. A card in the middle of the screen while someone is engaged
/// with the editor is the distraction the fix explicitly set out to
/// avoid, and the transcript is still empty at that moment, so only the
/// input condition can be keeping it away.
#[test]
fn the_card_is_gone_once_the_user_types_even_without_sending() {
    let mut state = AppState::new("test-model");
    state.input.insert_char('h');

    let frame = frame_text(120, 30, &state);
    assert!(
        state.transcript_state.lines.is_empty(),
        "this test needs an empty transcript — it is testing the input \
         condition alone"
    );
    assert!(
        !frame.contains("Type a prompt and press Enter"),
        "the welcome card survived the first keystroke:\n{frame}"
    );
    assert!(
        frame.contains('h'),
        "the typed character is not on screen at all:\n{frame}"
    );
}
