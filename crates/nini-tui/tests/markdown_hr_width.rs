//! A thematic break is `min(width, 80)` — Pi's rule, not a fixed sixty.
//!
//! Pi: `"─".repeat(Math.min(width, 80))`
//! (`@mariozechner/pi-tui` `dist/components/markdown.js`, `case "hr"`).
//!
//! nini drew `"\u{2500}".repeat(60)` because `render_markdown` was handed
//! the text and the theme and nothing else, and its own comment said so:
//! *"we don't know it here, so use a generous default. ratatui's wrap will
//! clip anyway"* — a premise that was false, since the caller had the width
//! in hand and never passed it down.
//!
//! Measured in tmux at 100 columns, a `---` between two sections of an
//! answer drew sixty characters: a rule stranded in the middle of the
//! screen. Models put `---` between sections constantly, so in a long task
//! this is not an edge case; it is why "long horizontal lines" kept coming
//! back after the turn divider was fixed.
//!
//! # Why these call `render_assistant_message`
//!
//! `render_markdown` gained a `width` parameter, so a test written against
//! it does not compile against the old code — it fails to build, which
//! proves nothing (the same mistake the turn-separator tests made once).
//! `render_assistant_message(text, theme, max_width)` had that third
//! parameter all along, on both sides of the fix, so these assertions run
//! against both implementations.
//!
//! One consequence is worth stating, because it is how the defect hid: at
//! widths *below* sixty, the old code was already correct. `rich.rs`
//! truncates finished lines to `max_width`, so a 60-column rule became a
//! 40-column rule on a 40-column terminal. The defect is only visible
//! **above** 60, which is why the assertions that matter here use 100 and
//! 200.

use nini_tui::rich::render_assistant_message;
use nini_tui::theme::Theme;

fn rule_len(max_width: Option<usize>) -> usize {
    render_assistant_message("---\n\ntext", &Theme::dark(), max_width)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>()
        .chars()
        .filter(|c| *c == '\u{2500}')
        .count()
}

#[test]
fn a_thematic_break_is_the_content_width_up_to_eighty() {
    for (width, expected) in [
        (20usize, 20usize),
        (40, 40),
        (60, 60),
        (80, 80),
        (100, 80),
        (200, 80),
    ] {
        assert_eq!(
            rule_len(Some(width)),
            expected,
            "at width {width} the rule should be {expected} chars (Pi: min(width, 80))"
        );
    }
}

/// Both halves of `min(width, 80)` matter, and either half alone is a
/// different wrong answer: uncapped fills a wide screen; capped at width
/// only, a rule becomes a full-width band on a narrow one.
#[test]
fn a_rule_never_fills_a_wide_screen() {
    assert!(
        rule_len(Some(200)) < 200,
        "Pi caps at 80 so a rule never spans the screen"
    );
    assert_eq!(
        rule_len(Some(200)),
        rule_len(Some(100)),
        "past 80 columns the length stops depending on the width"
    );
}

/// Without a width there is nothing to scale to, and the honest answer is
/// the shortest rule that still reads as one — not sixty columns of claim
/// about a screen this code cannot see.
#[test]
fn a_thematic_break_without_a_width_does_not_invent_sixty() {
    let len = rule_len(None);
    assert!(
        len <= 1,
        "expected the minimal rule when no width is known, got {len}"
    );
}

/// The end-to-end shape: a long answer with `---` between sections, which
/// is what a real model emits and what produced the report.
#[test]
fn a_multi_section_answer_scales_every_rule() {
    let body = "第一段内容。\n\n---\n\n第二段内容。\n\n---\n\n第三段内容。";
    let lines = render_assistant_message(body, &Theme::dark(), Some(120));
    let rules: Vec<usize> = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .chars()
                .filter(|c| *c == '\u{2500}')
                .count()
        })
        .filter(|n| *n > 0)
        .collect();
    assert_eq!(
        rules,
        vec![80, 80],
        "each rule must follow min(width, 80), not a constant: {rules:?}"
    );
}
