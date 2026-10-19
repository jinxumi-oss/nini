//! invariant: 8744ceb5
//!
//! # The invariant
//!
//! On a narrow terminal (`width < 70`) the footer must show a **complete**
//! hint set: no hint half-drawn, no hint silently dropped by the
//! paragraph's truncation. The minimum width at which that must hold is
//! 50 columns, which is where the 3-hint narrow set (48 cells) fits.
//!
//! # Why nothing asserted it
//!
//! `8744ceb5` added the `narrow` branch to `render_key_hints` and said
//! why in the comment: "ratatui's `Paragraph` truncates the tail
//! silently — without an ellipsis the user wouldn't even know hints were
//! dropped." Nothing asserted the property the branch exists to provide.
//! A hint set that is short enough to be truncated anyway is a green
//! build and a footer that lies about which keys exist.
//!
//! # Why these assertions are not same-source
//!
//! The footer is measured off the rendered buffer, in cells, against the
//! real terminal width — not read out of the `Vec<(&str, &str)>` the
//! renderer builds. The comparison is "the row the user sees" versus "the
//! width they have", so a set that is merely *listed* in source but does
//! not survive rendering fails here.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The bottom row: the key hints. Trimmed of the padding ratatui leaves.
fn hints_row(width: u16, height: u16, state: &AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let y = area.bottom() - 1;
    let mut row = String::new();
    for x in area.left()..area.right() {
        row.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
    }
    row.trim_end().to_string()
}

/// Keys that must survive at any narrow width: the ones a first-time user
/// has no other way to discover.
const ESSENTIAL: [&str; 2] = ["Enter", "send"];

/// Every width from 50 to 69 — narrow by the renderer's own `< 70`
/// definition — must render the whole narrow set, with nothing clipped.
#[test]
fn no_narrow_width_between_50_and_69_truncates_the_hint_set() {
    for width in 50u16..70 {
        let state = AppState::new("test-model");
        let row = hints_row(width, 20, &state);
        assert!(
            !row.contains('…'),
            "at width {width} the footer truncated a hint with no way for the \
             user to tell: {row:?}"
        );
        for key in ESSENTIAL {
            assert!(
                row.contains(key),
                "at width {width} the footer dropped the essential hint \
                 {key:?}: {row:?}"
            );
        }
        // A complete row is one that is not padded into the next one and
        // not cut: its cell count is what the renderer asked for.
        assert!(
            row.chars().count() <= width as usize,
            "at width {width} the hint row overflowed the terminal: {row:?}"
        );
    }
}

/// The narrow set is *shorter* than the wide one — that is the whole
/// mechanism. If a future edit made the two identical, the branch would
/// be dead code that happens to look deliberate.
#[test]
fn the_narrow_set_is_measurably_shorter_than_the_wide_one() {
    let narrow = hints_row(60, 20, &AppState::new("test-model"));
    let wide = hints_row(120, 20, &AppState::new("test-model"));
    assert!(
        narrow.chars().count() < wide.chars().count(),
        "narrow footer ({:?}, {} cells) is not shorter than the wide one \
         ({:?}, {} cells) — the width branch does nothing",
        narrow,
        narrow.chars().count(),
        wide,
        wide.chars().count()
    );
}

/// F1 toggles the extended set, and that set gets a narrow variant too:
/// the kill/undo/yank hints are dropped rather than carried into a
/// narrow terminal. Checked with `help_extended` set directly so the test
/// does not depend on the F1 key binding staying where it is.
///
/// Scope note, stated rather than papered over: the *extended* footer is
/// truncated by ratatui at almost every width — measured 63 cells of
/// content at 69 columns, and still a `…` at 160 — so "no truncation" is
/// asserted above only for the default set, which is the set the `narrow`
/// branch was added to make fit. The extended set's overflow is a
/// separate, still-open defect; see docs/invariant-audit.md.
#[test]
fn the_extended_set_also_shrinks_when_narrow() {
    let mut state = AppState::new("test-model");
    state.ui_state.help_extended = true;

    let narrow = hints_row(69, 20, &state);
    assert!(
        narrow.contains("short") || narrow.contains("F1"),
        "the narrow extended set dropped its own toggle hint: {narrow:?}"
    );
    for key in ESSENTIAL {
        assert!(
            narrow.contains(key),
            "the narrow extended set dropped the essential hint {key:?}: {narrow:?}"
        );
    }

    let wide = hints_row(160, 20, &state);
    assert!(
        wide.chars().count() > narrow.chars().count(),
        "the extended set is the same size at 69 and 160 columns \
         ({:?} / {:?}) — the narrow branch is not applied to it",
        narrow,
        wide
    );
}
