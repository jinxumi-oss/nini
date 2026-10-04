//! Display-width helpers for terminal layout.
//!
//! `chars().count()` is the wrong unit for column arithmetic: it counts
//! Unicode scalar values, not the number of terminal cells the text will
//! actually occupy. East-Asian wide characters (CJK), fullwidth
//! punctuation, and emoji are 2 cells each; combining marks are 0.
//!
//! Use [`display_width`] for any value that is padded, truncated, or
//! right-aligned in the rendered frame; use `chars().count()` only when
//! counting code points is semantically what you want (token estimation,
//! tests that pin a string shape, etc.).
//!
//! The wrapper exists so the dependency on `unicode-width` is in one
//! place; replacing it later (e.g. with a faster manual table) is a
//! single-file change.

/// Terminal display width of `s` in cells.
///
/// Always returns a finite `usize`; non-printable control bytes
/// contribute 0 (matches what most terminals render anyway). Empty
/// string returns 0.
#[inline]
pub fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(s)
}

/// Like [`display_width`] but clamps each `&str` slice to a maximum,
/// returning early once the running total exceeds `max`. Useful for
/// "does this fit in N columns?" checks without scanning the whole
/// string.
#[inline]
pub fn display_width_clamped(s: &str, max: usize) -> usize {
    if s.is_empty() {
        return 0;
    }
    let mut total = 0usize;
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        total = total.saturating_add(w);
        if total > max {
            return max + 1; // signal "definitely too wide"
        }
    }
    total
}

/// v0.8.4 (bugfix): returns the largest byte index ≤ `at` that
/// is a UTF-8 character boundary in `s`. Equivalent to
/// `str::floor_char_boundary` (stabilised in Rust 1.91); we inline
/// the 4-line walk because nini's MSRV is 1.85.
///
/// Use this everywhere we previously did `&s[..N]` or
/// `String::truncate(N)` where `N` is a byte budget — CJK
/// characters are 3 bytes and emoji are 4, so any byte-aligned
/// slice on multi-byte text will panic with
/// `byte index N is not a char boundary; it is inside '某字'`.
#[inline]
pub fn floor_char_boundary(s: &str, at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    let mut i = at;
    // Continuation bytes match `10xxxxxx`. Walk back while we're
    // sitting on one.
    while i > 0 && (s.as_bytes()[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

/// Like [`floor_char_boundary`] but for the start of a slice (`at` is a
/// *byte* index where we want to start the next slice, so we may need to
/// step *forward* if `at` is mid-char).
#[inline]
pub fn ceil_char_boundary(s: &str, at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    let mut i = at;
    while i < s.len() && (s.as_bytes()[i] & 0b1100_0000) == 0b1000_0000 {
        i += 1;
    }
    i
}

/// Truncate `s` to at most `max` terminal cells, appending `…` when
/// anything was dropped.
///
/// **v0.8.9 (P1-3)**: this is the single place the transcript decides
/// "how much of this fits". Before it existed, only `render_tool_result`
/// had a width budget — every other line type (user message, assistant
/// markdown, tool-call header, bash output) was handed to ratatui's
/// `List`, which does **not** wrap: it cut the line at the screen edge
/// with no marker at all, so a 210-character user message rendered as
/// `> STARTAAAA…` with the tail simply gone.
///
/// Width is measured in cells, not bytes or `char`s: a CJK character is
/// 1 scalar value and 3 bytes but occupies 2 cells.
///
/// # Contract
///
/// * Fits within `max` cells — always. Callers can paste the result
///   straight into a `Span` and trust the frame width.
/// * Appends `…` iff content was actually dropped, so "the line ends
///   with `…`" is a reliable "there is more" signal.
/// * Never panics, never splits a character.
/// * `max == 0` → `""`; `max == 1` → `"…"`; content already fitting is
///   returned unchanged (no `…`).
pub fn truncate_cells(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // Reserve one cell for the ellipsis.
    let budget = max - 1;
    let mut out = String::with_capacity(budget.min(s.len()) + 3);
    let mut consumed = 0usize;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if consumed + cw > budget {
            break;
        }
        out.push(c);
        consumed += cw;
    }
    out.push('…');
    out
}

/// [`truncate_cells`] for a styled ratatui `Line`, preserving each
/// span's style. The `…` inherits the style of the span it was cut
/// inside, so a truncated colored line still reads as one object.
///
/// Used for assistant output, where the line is produced by the
/// markdown renderer and arrives as styled spans rather than a plain
/// string.
pub fn truncate_line_cells(
    line: &ratatui::text::Line<'_>,
    max: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;

    let mut out: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut consumed = 0usize;
    let mut truncated = false;

    for span in &line.spans {
        if truncated {
            break;
        }
        let w = display_width(span.content.as_ref());
        if consumed + w <= max {
            out.push(Span::styled(span.content.to_string(), span.style));
            consumed += w;
            continue;
        }
        // This span doesn't fit. Take what we can, mark the ellipsis
        // with the same style, and stop — the rest of the line is gone.
        let remaining = max.saturating_sub(consumed);
        let cut = truncate_cells(span.content.as_ref(), remaining);
        if !cut.is_empty() {
            out.push(Span::styled(cut, span.style));
        }
        truncated = true;
    }

    if !truncated {
        return ratatui::text::Line::from(out);
    }

    // `truncate_cells` already spent the last cell on its own `…`; if
    // the span cut exactly filled the budget, append the marker here
    // instead so the signal is never lost.
    let used: usize = out.iter().map(|s| display_width(s.content.as_ref())).sum();
    if used < max {
        let style = out.last().map(|s| s.style).unwrap_or(Style::default());
        out.push(Span::styled("…", style));
    }

    ratatui::text::Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_one_per_char() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn emoji_is_two_cells() {
        // 💭 (U+1F4AD) is double-width on all major TUI emulators.
        assert_eq!(display_width("💭"), 2);
        // 2 (emoji) + 1 (space) + 8 ("thinking") = 11 cells, not 10 chars.
        assert_eq!(display_width("💭 thinking"), 11);
    }

    #[test]
    fn cjk_is_two_cells() {
        // 你好 = 2 wide chars = 4 cells.
        assert_eq!(display_width("你好"), 4);
    }

    #[test]
    fn combining_marks_are_zero() {
        // "a\u{0301}" (a + combining acute) renders as 1 cell.
        assert_eq!(display_width("a\u{0301}"), 1);
    }

    #[test]
    fn clamped_returns_total_when_fits() {
        assert_eq!(display_width_clamped("hello", 80), 5);
    }

    #[test]
    fn clamped_signals_overflow_early() {
        // "你" is 2 cells; max=1 should overflow after first char.
        assert!(display_width_clamped("你好", 1) > 1);
    }

    #[test]
    fn floor_char_boundary_walks_back_continuation_bytes() {
        // 50 × '文' = 150 bytes; char #40 starts at byte 120, so byte
        // 121 (a continuation byte) sits inside char #40. floor must
        // step back to 120 — NOT 119, which would put us inside
        // char #39.
        let s = "文".repeat(50);
        let end = floor_char_boundary(&s, 121);
        assert_eq!(end, 120, "should step back to byte 120 (start of char #40)");
        assert!(s.is_char_boundary(end));
        // And on an exact boundary, floor passes through.
        assert_eq!(floor_char_boundary(&s, 120), 120);
        // A byte inside char #39 (e.g. 118) is also a continuation
        // byte — floor should step back to byte 117.
        assert_eq!(floor_char_boundary(&s, 118), 117);
    }

    #[test]
    fn floor_char_boundary_passes_through_ascii() {
        let s = "hello world";
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 11), 11);
    }

    #[test]
    fn floor_char_boundary_at_or_past_end_returns_len() {
        let s = "hi";
        assert_eq!(floor_char_boundary(s, 100), 2);
    }

    // ---- truncate_cells (v0.8.9 P1-3) --------------------------------

    #[test]
    fn truncate_cells_leaves_short_text_alone() {
        assert_eq!(truncate_cells("hello", 10), "hello");
        assert!(!truncate_cells("hello", 10).contains('…'));
    }

    #[test]
    fn truncate_cells_marks_what_it_dropped() {
        let out = truncate_cells("abcdefghij", 5);
        assert_eq!(out, "abcd…");
        assert_eq!(display_width(&out), 5);
    }

    #[test]
    fn truncate_cells_respects_cell_width_not_bytes() {
        // 5 CJK chars = 10 cells but only 15 bytes. A byte-based budget
        // would have let 4 through and blown past the cell limit.
        let s = "文文文文文";
        let out = truncate_cells(s, 5);
        assert_eq!(display_width(&out), 5);
        assert_eq!(out, "文文…");
    }

    #[test]
    fn truncate_cells_degenerate_budgets() {
        assert_eq!(truncate_cells("abc", 0), "");
        assert_eq!(truncate_cells("abc", 1), "…");
        assert_eq!(truncate_cells("", 5), "");
    }

    #[test]
    fn truncate_cells_never_exceeds_the_budget() {
        // Property-ish sweep over a mix of widths and content shapes.
        let samples = [
            "plain ascii content that is quite long",
            "中文内容也很长需要被截断",
            "mixed 中文 and ascii together in one line",
            "💭 emoji 🧠 and text",
            "a",
        ];
        for s in samples {
            for max in 0..40usize {
                let out = truncate_cells(s, max);
                assert!(
                    display_width(&out) <= max,
                    "{s:?} at max={max} produced width {}",
                    display_width(&out)
                );
            }
        }
    }

    #[test]
    fn truncate_line_cells_keeps_styles_and_fits() {
        use ratatui::style::{Color, Style};
        use ratatui::text::{Line, Span};

        let line = Line::from(vec![
            Span::styled("bold-part-", Style::default().fg(Color::Red)),
            Span::styled("and-the-rest-of-it", Style::default().fg(Color::Blue)),
        ]);
        let out = truncate_line_cells(&line, 12);
        let width: usize = out
            .spans
            .iter()
            .map(|s| display_width(s.content.as_ref()))
            .sum();
        assert!(width <= 12, "line width {width} > 12");
        let joined: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(joined.contains('…'), "no ellipsis in {joined:?}");
        // The first span's style must survive.
        assert_eq!(out.spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn truncate_line_cells_passes_short_lines_through() {
        use ratatui::text::{Line, Span};
        let line = Line::from(vec![Span::raw("short")]);
        let out = truncate_line_cells(&line, 40);
        let joined: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "short");
        assert!(!joined.contains('…'));
    }
}
