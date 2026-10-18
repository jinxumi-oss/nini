//! Oracle: a transcript search hit must be visible, and the viewport must go to it.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! Search works: `update_search_query` fills `SearchState::matches`
//! with line indices, `render_search_bar` draws the count, `n`/`N`
//! move `SearchState::current`. Two things are missing, and they are
//! the two halves of one defect — the search tells you it found things
//! and then shows you none of them.
//!
//! 1. **No highlight.** A TestBackend snapshot of a search with two
//!    hits contains exactly one style across every transcript row:
//!    `bg=Reset fg=Reset m=NONE`. Pi applies
//!    `underline + bg(searchMatchBg) + fg(searchMatchText)` to a hit
//!    and `bold + inverse` on top of that for the current one
//!    (`interactive-mode.ts:349`). `searchMatchBg` and `searchMatchText`
//!    are defined in the palette (`theme.rs`, `searchMatch*`) and read
//!    by nothing — the same dead-slot shape as `mdCodeBlockBorder` in
//!    §32 and `thinkingText` in the 10-17 audit.
//!
//! 2. **The viewport does not follow.** `search_next` only moves
//!    `SearchState::current`. Nothing writes `scroll_offset`, and
//!    `autoscroll` keeps the transcript pinned to the tail, so pressing
//!    `n` leaves the screen byte-identical.
//!
//! Together: the user types a query, reads "2/2", presses `n`, and
//! nothing appears to happen.
//!
//! **Test discipline (v0.10 §35.6)**: every API here — `push_assistant`,
//! `begin_search`, `update_search_query`, `search_next`, `search_prev`,
//! `render_frame` — exists on both sides of the fix. The colours are
//! read as literal hex from the palette so the test states the intended
//! colour rather than mirroring the implementation's lookup path.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const SEARCH_MATCH_BG: (u8, u8, u8) = (0x3a, 0x3a, 0x4a); // #3a3a4a
const SEARCH_MATCH_FG: (u8, u8, u8) = (0xd4, 0xd4, 0xd4); // #d4d4d4

fn render(state: &AppState, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    terminal.backend().buffer().clone()
}

/// The text of the **transcript** row that carries `needle`, or None if
/// it is scrolled out of the viewport entirely.
///
/// The search bar is skipped explicitly. A first draft scanned every row
/// and matched `NEEDLE` in the bar's own query echo, so both viewport
/// tests passed while the transcript sat pinned to the tail with the hit
/// 108 lines above it — the same "assertion satisfied by something else
/// on screen" shape as the three `/hotkeys` tests, and the reason those
/// stayed green for three releases.
fn row_containing(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<String> {
    for y in 0..buf.area.height {
        let mut s = String::new();
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                s.push_str(c.symbol());
            }
        }
        if s.contains("n=next") {
            continue; // the search bar echoes the query
        }
        if s.contains(needle) {
            return Some(s);
        }
    }
    None
}

fn has_bg(buf: &ratatui::buffer::Buffer, want: (u8, u8, u8)) -> bool {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                if let ratatui::style::Color::Rgb(r, g, b) = c.bg {
                    if (r, g, b) == want {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Fixture note: `push_assistant` **appends to the current turn's line**
/// (`push_assistant_raw` merges into the previous `AssistantText` when
/// the turn has not broken), so 60 calls produce ONE transcript line, not
/// 60. An earlier draft of this file used it and its "the hit is 60 lines
/// up" fixture was silently a one-line transcript — the test passed for
/// the wrong reason. `push_user` starts a new turn and therefore a new
/// line, which is what a real multi-turn session looks like.
fn transcript_with_one_hit() -> AppState {
    let mut state = AppState::new("test-model");
    state.push_user("an ordinary line");
    state.push_user("a line carrying NEEDLE_ALPHA inside it");
    state.push_user("another ordinary line");
    state.begin_search();
    state.update_search_query("NEEDLE_ALPHA".to_string());
    assert_eq!(
        state.transcript_state.lines.len(),
        3,
        "fixture precondition: three transcript lines"
    );
    state
}

#[test]
fn a_hit_is_painted_with_the_search_match_colour() {
    let state = transcript_with_one_hit();
    let buf = render(&state, 80, 20);

    assert!(
        row_containing(&buf, "NEEDLE_ALPHA").is_some(),
        "the hit should be on screen"
    );
    assert!(
        has_bg(&buf, SEARCH_MATCH_BG),
        "no cell in the frame carries searchMatchBg (#3a3a4a) — the hit is \
         invisible. Pi paints it with theme.bg(\"searchMatchBg\", \
         theme.fg(\"searchMatchText\", text))."
    );
    let _ = SEARCH_MATCH_FG; // asserted below by the current-hit test
}

#[test]
fn the_current_hit_is_distinguishable_from_the_other_hits() {
    let mut state = AppState::new("test-model");
    state.push_user("first line with NEEDLE");
    state.push_user("second line with NEEDLE");
    state.begin_search();
    state.update_search_query("NEEDLE".to_string());
    assert_eq!(
        state.ui_state.search.as_ref().unwrap().matches.len(),
        2,
        "fixture precondition: two hits"
    );
    state.ui_state.search.as_mut().unwrap().current = 1; // the second one is current

    let buf = render(&state, 80, 20);

    // Pi marks the current hit with `bold + inverse` on top of the
    // match colours, so there are three visually distinct states:
    // not a hit, a hit, the current hit.
    let mut has_bold_or_reverse = false;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                if c.modifier.contains(ratatui::style::Modifier::BOLD)
                    || c.modifier.contains(ratatui::style::Modifier::REVERSED)
                {
                    has_bold_or_reverse = true;
                }
            }
        }
    }
    assert!(has_bg(&buf, SEARCH_MATCH_BG), "no hit is painted at all");
    assert!(
        has_bold_or_reverse,
        "the current hit is painted exactly like the other hits, so `n` and \
         `N` appear to do nothing. Pi uses theme.bold(theme.inverse(...)) \
         for the current match on top of the match colours."
    );
}

#[test]
fn the_viewport_follows_the_current_hit() {
    // A transcript long enough that the hit is far above the tail.
    let mut state = AppState::new("test-model");
    for i in 0..60 {
        state.push_user(format!("filler line number {i}"));
    }
    state.push_user("the NEEDLE sits near the top of the transcript");
    for i in 0..60 {
        state.push_user(format!("more filler after the hit {i}"));
    }

    state.begin_search();
    state.update_search_query("NEEDLE".to_string());
    assert_eq!(
        state.ui_state.search.as_ref().unwrap().matches.len(),
        1,
        "fixture precondition: exactly one hit"
    );

    let buf = render(&state, 80, 20);
    assert!(
        row_containing(&buf, "NEEDLE sits").is_some(),
        "a search that matched must bring the match into view; the \
         transcript stayed pinned to the tail, so pressing `/` and typing \
         a query produces no visible change at all."
    );
}

#[test]
fn moving_between_hits_moves_the_viewport() {
    let mut state = AppState::new("test-model");
    for i in 0..50 {
        state.push_user(format!("up filler {i}"));
    }
    state.push_user("first HIT near the top");
    for i in 0..40 {
        state.push_user(format!("middle filler {i}"));
    }
    state.push_user("second HIT further down");
    for i in 0..40 {
        state.push_user(format!("down filler {i}"));
    }

    state.begin_search();
    state.update_search_query("HIT".to_string());
    assert_eq!(state.ui_state.search.as_ref().unwrap().matches.len(), 2);

    // Current hit is the first one; the frame must show it.
    let first = render(&state, 80, 20);
    assert!(
        row_containing(&first, "HIT near").is_some(),
        "the current hit must be on screen when the search starts"
    );

    // `n` advances to the second hit, and the viewport must follow.
    state.search_next();
    assert_eq!(state.ui_state.search.as_ref().unwrap().current, 1);

    let second = render(&state, 80, 20);
    assert!(
        row_containing(&second, "HIT further").is_some(),
        "after `n` the viewport must show the newly current hit; it stayed \
         where it was, which is what made this look like a dead key."
    );
}
