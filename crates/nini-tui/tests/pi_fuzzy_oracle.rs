//! Oracle: nini's fuzzy filter is **Pi's** fuzzy filter, score for score.
//!
//! # Where the expected numbers come from
//!
//! Pi's `/model` selector ranks its matches with `fuzzyFilter` from
//! `@earendil-works/pi-tui` — the package version **0.85.1**, the same
//! version as the target (`references/TARGET.json`). The expected values
//! below were produced by running Pi's own function on this machine:
//!
//! ```console
//! $ node -e 'import {fuzzyMatch} from ".../@earendil-works/pi-tui/dist/fuzzy.js"; …'
//! ```
//!
//! They are **Pi's output**, not nini's, and they are pinned here so that
//! a change in the port has to argue with Pi's numbers rather than with
//! nini's own.
//!
//! # Why the numbers are pinned at all
//!
//! A "close enough" fuzzy filter is how the ordering difference came
//! about in the first place: nini's generic filter looked at the label
//! only, so a proxy whose id merely *contains* the query outranked the
//! model the user meant. A port that is 90% faithful is a port whose
//! remaining 10% is invisible — the score is what decides the order, so
//! the score is what has to match.
//!
//! One known, deliberate difference: Pi indexes `textLower` by **UTF-16
//! code unit** and this port indexes by **character**. For ASCII the two
//! agree, and every pair below is ASCII.

use nini_tui::selectors::pi_fuzzy::{fuzzy_filter_indices, fuzzy_match};

/// Pi's own answers, verbatim.
const PI_PAIRS: &[(&str, &str, bool, f64)] = &[
    // query, text, matches, score
    ("gpt", "openai/gpt-5", true, -22.6),
    ("gpt", "alongptxyz", true, -13.5),
    (
        "openai",
        "openai openai/gpt-5 openai gpt-5 GPT-5",
        true,
        -113.5,
    ),
    (
        "openai",
        "openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5",
        true,
        -15.099999999999998,
    ),
    ("gpt5", "5gpt", true, -154.4),
    ("5gpt", "gpt5", true, -154.4),
    ("nope", "openai openai/gpt-5 openai gpt-5", true, -16.3),
    ("zzzz", "openai openai/gpt-5 openai gpt-5", false, 0.0),
    ("gpt-5", "openai/gpt-5", true, -65.5),
    (
        "minimax m2.7",
        "gw gw/MiniMax-M2.7-highspeed gw MiniMax-M2.7-highspeed MiniMax M2.7 Highspeed",
        true,
        -73.10000000000001,
    ),
    (
        "claude",
        "anthropic claude-opus-4-7 anthropic claude-opus-4-7 Claude Opus 4.7",
        true,
        -38.7,
    ),
];

#[test]
fn the_scores_are_pis_scores() {
    for (query, text, matches, score) in PI_PAIRS {
        let got = fuzzy_match(query, text);
        assert_eq!(
            got.matches, *matches,
            "match flag differs for {query:?} against {text:?}"
        );
        if *matches {
            assert!(
                (got.score - score).abs() < 1e-9,
                "score differs for {query:?} against {text:?}: \
                 nini {got:?} vs Pi {score:?}"
            );
        }
    }
}

#[test]
fn the_filter_returns_pis_order() {
    // Pi's `fuzzyFilter` over the three search texts, in Pi's order.
    let texts: Vec<String> = [
        "openai openai/gpt-5 openai gpt-5 GPT-5",
        "openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5",
        "gw gw/MiniMax-M2.7-highspeed gw MiniMax-M2.7-highspeed MiniMax M2.7 Highspeed",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    // Pi: fuzzyFilter(items, "openai/gpt-5", x => x) → [0, 1]
    assert_eq!(fuzzy_filter_indices("openai/gpt-5", &texts), vec![0, 1]);
    // Pi: fuzzyFilter(items, "minimax m2.7", x => x) → [2]
    assert_eq!(fuzzy_filter_indices("minimax m2.7", &texts), vec![2]);
    // Pi: fuzzyFilter(items, "gpt", x => x) → [0, 1]
    assert_eq!(fuzzy_filter_indices("gpt", &texts), vec![0, 1]);
}
