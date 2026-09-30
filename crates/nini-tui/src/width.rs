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

/// v0.8.4 (bugfix): like [`floor_char_boundary`] but for the start
/// of a slice (`at` is a *byte* index where we want to start the
/// next slice, so we may need to step *forward* if `at` is mid-char).
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
}
