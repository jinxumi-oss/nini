//! Oracle: transcript search must see the whole line, not a 60-char preview.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! `line_summary_text_impl` builds the text that `update_search_query`
//! matches against, and for assistant / thinking / tool-result lines it
//! ends with `truncate(s, 60)`. The doc comment above it calls the
//! result "a compact one-line summary **suitable for transcript search**"
//! — and it is the *only* input the search has. So every answer longer
//! than 60 characters has an invisible tail, and `Ctrl+F` cannot reach
//! it.
//!
//! Measured before the fix, on an 88-character assistant message with a
//! marker at offset 70:
//!
//! ```text
//! 索引含 NEEDLE_AT_70 ? false
//! 原文含 NEEDLE_AT_70 ? true
//! ```
//!
//! Model answers are routinely longer than 60 characters, which makes
//! this "the search does not work on real content" rather than an edge
//! case.
//!
//! **Test discipline (v0.10 §35.6)**: every API called here —
//! `AppState::new`, `push_assistant`, `begin_search`,
//! `update_search_query`, `ui_state.search` — exists on both sides of
//! the fix. Nothing was made public to make this file compile.

use nini_tui::state::AppState;

fn search_hits(haystack: &str, needle: &str) -> usize {
    let mut state = AppState::new("test-model");
    state.push_assistant(haystack);
    state.begin_search();
    state.update_search_query(needle.to_string());
    state.ui_state.search.as_ref().unwrap().matches.len()
}

#[test]
fn search_reaches_the_end_of_a_long_assistant_answer() {
    // The marker is at the END. An earlier draft put it at offset 0 and
    // therefore passed against the unfixed code — a green test that
    // cannot go red is not an oracle (v0.10 §35.6).
    let tail_marker = "MARKER_AT_POSITION_150";
    let long = format!("{}{tail_marker}", "filler ".repeat(30));

    let hits = search_hits(&long, tail_marker);

    assert_eq!(
        hits,
        1,
        "a marker at the end of a {}-character assistant answer must be \
         findable; search matched {hits}. The text the search sees is \
         built by `line_summary_text_impl`, which truncated every \
         assistant / thinking / tool-result line to 60 characters.",
        long.len()
    );
}

#[test]
fn search_is_not_bounded_by_a_padded_body() {
    // The boundary itself: 59 characters must match, 61 must too.
    // A fix that moves the limit instead of removing it still fails here.
    for len in [1usize, 59, 60, 61, 120, 400] {
        let body: String = std::iter::repeat('a').take(len).collect();
        let text = format!("prefix {body} suffix");
        let hits = search_hits(&text, "suffix");
        assert_eq!(
            hits, 1,
            "the trailing word must be findable in a {len}-character line"
        );
    }
}

#[test]
fn a_mid_line_marker_is_found() {
    // Marker in the middle, past the old 60-character cut.
    let text = format!("{}MARKER{}", "a".repeat(80), "b".repeat(80));
    assert_eq!(
        search_hits(&text, "MARKER"),
        1,
        "a marker at offset 80 of a {}-character line must be findable",
        text.len()
    );
}

#[test]
fn case_insensitive_matching_still_works_over_the_full_text() {
    let text = format!("{}MixedCaseTail{}", "a".repeat(80), "");
    assert_eq!(
        search_hits(&text, "mixedcasetail"),
        1,
        "matching is case-insensitive over the whole line, not just a prefix"
    );
}
