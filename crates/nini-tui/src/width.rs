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
}
