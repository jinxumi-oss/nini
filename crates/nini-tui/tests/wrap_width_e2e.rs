//! A reply wider than the terminal is wrapped, not cut.
//!
//! Measured in tmux against a scripted OpenAI-compatible server, one reply
//! padded with CJK and nothing else:
//!
//! ```text
//!   terminal width 60   HEADMARK-2 填充段落内容。…填充段…   ← tail gone
//!   terminal width 100  HEADMARK-2 填充段落内容。… TAI…     ← tail gone
//!   terminal width 160  HEADMARK-2 填充段落内容。… TAILMARK-2  ← intact
//! ```
//!
//! A 90-row terminal changes nothing, so it is not the viewport. The cut
//! happens in the renderer: `render_markdown` emits one line per source line
//! and `rich.rs` then truncates it, because `width.rs` grew
//! `truncate_cells` and **no wrap**.
//!
//! Pi wraps instead — `wrapTextWithAnsi(line, width)`
//! (`@mariozechner/pi-tui` `dist/utils.js:562`), folding on token
//! boundaries and breaking a token that is wider than the width by
//! character, trimming the tail of each line and never starting a line with
//! whitespace.
//!
//! These call `render_assistant_message(text, theme, max_width)`, whose
//! signature is the same before and after the fix.

use nini_tui::rich::render_assistant_message;
use nini_tui::theme::Theme;
use nini_tui::width::display_width;

fn rendered(text: &str, width: usize) -> Vec<String> {
    render_assistant_message(text, &Theme::dark(), Some(width))
        .into_iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn joined(text: &str, width: usize) -> String {
    rendered(text, width).join("")
}

const BODY: &str =
    "填充段落内容。填充段落内容。填充段落内容。填充段落内容。填充段落内容。填充段落内容。";

#[test]
fn a_long_reply_keeps_its_tail_at_every_width() {
    let body = format!("HEADMARK {BODY} TAILMARK");
    for width in [30usize, 40, 60, 100, 160] {
        let out = joined(&body, width);
        assert!(
            out.contains("HEADMARK"),
            "width {width}: the head was lost: {out:?}"
        );
        assert!(
            out.contains("TAILMARK"),
            "width {width}: the tail was cut off — a {}-cell reply was \
             truncated to the screen instead of folded: {out:?}",
            display_width(&body)
        );
    }
}

/// Wrapping is not truncation wearing a different hat: more cells in means
/// more lines out.
#[test]
fn a_long_reply_becomes_several_lines() {
    let body = format!("HEADMARK {BODY} TAILMARK");
    for width in [30usize, 40, 60] {
        let lines = rendered(&body, width);
        assert!(
            lines.len() > 1,
            "width {width}: expected the reply to fold across lines, got {}",
            lines.len()
        );
    }
}

/// Pi: `wrapped.map(line => line.trimEnd())` and "Don't start new line with
/// whitespace" (`utils.js:619-628`). A wrapped terminal line that hangs
/// past the edge, or begins with a space, is a new rendering defect of the
/// same family.
#[test]
fn no_wrapped_line_exceeds_the_width_or_starts_with_space() {
    let body = format!("HEADMARK {BODY} TAILMARK");
    for width in [24usize, 30, 40, 60] {
        for line in rendered(&body, width) {
            assert!(
                display_width(&line) <= width,
                "width {width}: line is {} cells wide: {line:?}",
                display_width(&line)
            );
            assert!(
                !line.starts_with(' '),
                "width {width}: wrapped line starts with whitespace: {line:?}"
            );
        }
    }
}

/// A word longer than the width has to break by character — this is the case
/// that loses the least obvious content: no space to fold on.
#[test]
fn an_unbreakable_word_is_broken_by_character() {
    let word = "W".repeat(200);
    let out = joined(&format!("{word}END"), 40);
    assert!(
        out.contains("END") && out.contains(&"W".repeat(200)),
        "a 200-character word lost its tail at width 40: {} chars",
        out.chars().count()
    );
}

/// Narrow terminals: the same text, much less room.
#[test]
fn cjk_replies_survive_a_narrow_terminal() {
    let body = format!("开始 {BODY} 结束");
    let out = joined(&body, 24);
    assert!(out.contains("开始"), "head lost at width 24: {out:?}");
    assert!(out.contains("结束"), "tail lost at width 24: {out:?}");
}

/// A reply that already fits must come back unchanged: wrapping is not
/// allowed to reflow what it does not have to. The trailing blank line is
/// the markdown block pad every block gets (`outputPad` in Pi), so the
/// assertion is about the content line, not the padding.
#[test]
fn a_short_reply_is_untouched() {
    let lines = rendered("short answer", 80);
    assert_eq!(
        lines[0], "short answer",
        "the content line changed: {lines:?}"
    );
    assert!(
        lines[1..].iter().all(|l| l.trim().is_empty()),
        "and nothing was folded onto extra lines: {lines:?}"
    );
}
