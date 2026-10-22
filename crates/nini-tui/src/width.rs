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

/// The terminal's width in columns, or 0 when it cannot be determined.
///
/// The terminal is the only place that knows how wide a cell is; the
/// width module is the only place that knows how wide a *character* is.
/// Code that writes to the terminal needs both.
pub fn terminal_cols() -> usize {
    crossterm::terminal::size()
        .map(|(c, _)| c as usize)
        .unwrap_or(0)
}

/// Does this terminal count East-Asian **Ambiguous** characters as two
/// cells?
///
/// `unicode-width` answers 1 for them (the Unicode default), and so does
/// most of the western world. A terminal configured for a Chinese,
/// Japanese or Korean locale very often answers **2** — `─ │ ┌ ┐ └ ┘
/// ├ ┤ ┬ ┴ ┼`, `░ ▓ █`, `• ▸ ✓ ✗ ❯ ↑ ↓`, and the braille spinner are
/// *all* ambiguous, and they are *all* chrome.
///
/// That is not cosmetic. A row laid out as N cells is 2N columns wide in
/// such a terminal, so the terminal folds it — and folding one row pushes
/// **every row below it down by a line**. What is left is a wall of
/// half-rules and stray fragments: the "乱码" and the "下划线" in the
/// report. Measured on this repository's own capture at 100 columns:
/// **501 of 4920 rows overflow**, the widest at **200** columns.
///
/// The terminal cannot be asked (there is no query for "how wide do you
/// draw U+2500"), so the answer comes from two places, in this order:
///
/// 1. `NINI_AMBIGUOUS_WIDTH=wide|narrow` — an explicit override, for a
///    terminal that does not follow its locale;
/// 2. the locale (`LC_ALL`/`LC_CTYPE`/`LANG` naming zh/ja/ko).
///
/// When in doubt this assumes **wide**, because the two failures are not
/// equal: a layout that assumed wide and met a narrow terminal is merely
/// sparse, while a layout that assumed narrow and met a wide terminal is
/// a broken screen.
use std::sync::atomic::{AtomicU8, Ordering};

const UNDECIDED: u8 = 0;
const NARROW: u8 = 1;
const WIDE: u8 = 2;

static POLICY: AtomicU8 = AtomicU8::new(UNDECIDED);

/// Say how the terminal draws ambiguous characters: `"wide"`, `"narrow"`,
/// or `"auto"` to follow the locale. Unknown values mean `"auto"`.
///
/// This exists because the terminal cannot be asked, and because the
/// locale is a correlation rather than a guarantee: a CJK-locale user
/// with a narrow terminal (and the reverse) both exist.
pub fn set_ambiguous_width(mode: &str) {
    POLICY.store(
        match mode.trim().to_ascii_lowercase().as_str() {
            "wide" | "2" => WIDE,
            "narrow" | "1" => NARROW,
            _ => UNDECIDED,
        },
        Ordering::SeqCst,
    );
}

/// Whether the wide interpretation is in force.
pub fn ambiguous_width_is_wide() -> bool {
    match POLICY.load(Ordering::SeqCst) {
        NARROW => false,
        WIDE => true,
        _ => {
            // Default: **narrow**. Most terminals — including the modern
            // ones a CJK-locale user is most likely to be running — draw
            // ambiguous characters one cell wide, and a locale is a
            // correlation, not a fact. `NINI_AMBIGUOUS_WIDTH=wide` (or the
            // runtime probe, `crate::ambiguous_probe`) is how a terminal
            // that says otherwise gets its say.
            POLICY.store(NARROW, Ordering::SeqCst);
            false
        }
    }
}

pub fn locale_is_cjk() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .any(|v| {
            let v = v.to_ascii_lowercase();
            ["zh", "ja", "ko"].iter().any(|tag| v.contains(tag))
        })
}

/// The ASCII stand-in for an ambiguous character, when the terminal
/// draws the ambiguous one two cells wide.
///
/// # Why the frame cannot simply use fewer of them
///
/// A TUI frame fills its whole width: every row is `cols` cells, the
/// unused ones padded with spaces. In a terminal that draws an ambiguous
/// character two cells wide, a row carrying *k* of them renders as
/// `cols + k` columns — the row overflows no matter how few there are,
/// because the padding is the difference. One rule is enough to push the
/// row past the edge, and a folded row moves **everything below it** down
/// by a line.
///
/// So in that terminal the answer is not "draw fewer" but "draw none":
/// every character on screen must be one the terminal agrees on. The
/// mapping below is the usual one — box drawing to ASCII, bullets to
/// ASCII — and it is only used when the wide policy is in force.
pub fn ascii_fallback(ch: char) -> Option<char> {
    Some(match ch {
        // Box drawing.
        '\u{2500}' | '\u{2501}' | '\u{2504}' | '\u{2505}' => '-',
        '\u{2502}' | '\u{2503}' | '\u{2506}' | '\u{2507}' => '|',
        '\u{250c}' | '\u{250f}' | '\u{2510}' | '\u{2513}' | '\u{2514}' | '\u{2517}'
        | '\u{2518}' | '\u{251c}' | '\u{2523}' | '\u{252b}' | '\u{2533}' | '\u{253b}'
        | '\u{252c}' | '\u{253c}' | '\u{2534}' => '+',
        // Blocks used for bars and the scrollbar.
        '\u{2588}' | '\u{2593}' | '\u{2592}' | '\u{2591}' | '\u{2590}' => '#',
        '\u{2596}' | '\u{2597}' | '\u{2598}' => '.',
        // Marks nini and the models use.
        '\u{2022}' | '\u{00b7}' => '*',
        '\u{25cf}' | '\u{25cb}' => '*',
        '\u{2713}' | '\u{2714}' => '+',
        '\u{2717}' | '\u{2718}' => 'x',
        '\u{25b8}' | '\u{25b6}' | '\u{25c2}' => '>',
        '\u{25c0}' => '<',
        '\u{2191}' => '^',
        '\u{2193}' => 'v',
        '\u{2192}' => '>',
        '\u{2190}' => '<',
        '\u{2026}' | '\u{2025}' => '.',
        '\u{00a0}' | '\u{2007}' | '\u{2009}' | '\u{200a}' => ' ',
        '\u{00ab}' | '\u{00bb}' => '"',
        '\u{2039}' | '\u{203a}' => '\u{27}',
        '\u{2032}' | '\u{2033}' => '\'',
        '\u{00d7}' => 'x',
        '\u{2640}' | '\u{2642}' => '+',
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        '\u{2014}' | '\u{2013}' => '-',
        _ => return None,
    })
}

/// Replace every ambiguous character in `s` with its ASCII stand-in.
///
/// A no-op unless the wide policy is in force, so the common case pays
/// nothing.
pub fn ascii_safe(s: &str) -> String {
    if !ambiguous_width_is_wide() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    for ch in s.chars() {
        match ascii_fallback(ch) {
            Some(c) => {
                out.push(c);
                changed = true;
            }
            None => out.push(ch),
        }
    }
    if changed {
        out
    } else {
        s.to_string()
    }
}

/// The vertical bar the frame is drawn with, in a character every
/// terminal agrees on.
pub fn vertical_bar() -> String {
    ascii_safe("\u{2502} ")
}

/// A vertical bar without the trailing space.
pub fn vertical_edge() -> String {
    ascii_safe("\u{2502}")
}

/// `cells` columns of a bar character, in characters every terminal
/// agrees on.
pub fn bar_cells(ch: char, cells: usize) -> String {
    let ch = if ambiguous_width_is_wide() {
        ascii_fallback(ch).unwrap_or('#')
    } else {
        ch
    };
    ch.to_string().repeat(chars_for_cells(ch, cells))
}

/// The horizontal rule for `cells` columns, drawn in characters that
/// are one cell wide in **every** terminal.
///
/// Returns the string; the caller draws it.
pub fn horizontal_rule(cells: usize) -> String {
    let ch = if ambiguous_width_is_wide() {
        '-'
    } else {
        '\u{2500}'
    };
    ch.to_string().repeat(cells.max(1))
}

/// How many `ch` characters are needed to cover `cells` columns.
///
/// The chrome is drawn by repeating a character, and that character may be
/// one column wide or two. A layout that emits `cells` copies of an
/// ambiguous character covers `cells` columns in a narrow terminal and
/// `2 × cells` in a wide one — which is how the frame ends up twice as
/// wide as the terminal.
pub fn chars_for_cells(ch: char, cells: usize) -> usize {
    let w = char_width(ch).max(1);
    cells.div_ceil(w).max(1)
}

/// Terminal display width of a single character, in cells.
#[inline]
pub fn char_width(c: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    if ambiguous_width_is_wide() {
        UnicodeWidthChar::width_cjk(c).unwrap_or(0)
    } else {
        UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

/// Terminal display width of `s` in cells, using the same policy as
/// [`char_width`].
#[inline]
pub fn str_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    if ambiguous_width_is_wide() {
        UnicodeWidthStr::width_cjk(s)
    } else {
        UnicodeWidthStr::width(s)
    }
}

/// Terminal display width of `s` in cells.
///
/// Always returns a finite `usize`; non-printable control bytes
/// contribute 0 (matches what most terminals render anyway). Empty
/// string returns 0.
#[inline]
pub fn display_width(s: &str) -> usize {
    str_width(s)
}

/// The width of the text in a **CJK** terminal.
///
/// `unicode-width` counts East-Asian-Ambiguous characters as one
/// column; a CJK terminal draws them as two. `width_cjk` is the other
/// measurement of the same string — the one a Chinese or Japanese
/// terminal actually uses.
///
/// It matters for anything that has to *fit*: a row measured with
/// `display_width` can be one column per ambiguous character too wide
/// in a CJK terminal, and a widget that clips rather than wraps (the
/// transcript's `List`) then cuts the tail off.
pub fn display_width_cjk(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width_cjk(s)
}

/// How many columns wider the text is in a CJK terminal than in a
/// western one — the number of East-Asian-Ambiguous characters it
/// contains.
pub fn cjk_extra_width(s: &str) -> usize {
    display_width_cjk(s).saturating_sub(display_width(s))
}

/// Wrap the text so that every line fits in `max_width` **columns of a
/// CJK terminal**.
///
/// [`wrap_text_to_width`] measures with the western width, where an
/// East-Asian-Ambiguous character is one column. A CJK terminal draws
/// it as two, so a line that fits the western budget can be one column
/// per ambiguous character too wide there — and a widget that clips
/// rather than wraps (the transcript's `List`) then cuts the tail off.
///
/// So the text is wrapped with a budget that leaves room for the
/// characters that will take two columns, and the result is checked and
/// re-wrapped until every line really fits.
pub fn wrap_text_to_width_cjk(text: &str, max_width: usize) -> Option<Vec<String>> {
    if max_width == 0 {
        return None;
    }
    if display_width_cjk(text) <= max_width {
        // It fits in a CJK terminal as it is.
        return None;
    }
    // The first guess: keep the same proportion of the text per line.
    let western = display_width(text);
    let cjk = display_width_cjk(text);
    let mut budget = if cjk > 0 {
        ((max_width as u64 * western as u64) / cjk as u64) as usize
    } else {
        max_width
    };
    budget = budget.clamp(1, max_width);

    for _ in 0..8 {
        let Some(parts) = wrap_text_to_width(text, budget) else {
            return None;
        };
        let worst = parts
            .iter()
            .map(|p| display_width(p) + cjk_extra_width(p))
            .max()
            .unwrap_or(0);
        if worst <= max_width {
            return Some(parts);
        }
        // Give back exactly the columns that did not fit.
        let overflow = worst - max_width;
        budget = budget.saturating_sub(overflow).max(1);
    }
    None
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
        let w = char_width(c);
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
        let cw = char_width(c);
        if consumed + cw > budget {
            break;
        }
        out.push(c);
        consumed += cw;
    }
    out.push('…');
    out
}

/// Truncate `s` to at most `max` cells **from the left**, prefixing
/// `…`.
///
/// Use this where the *tail* is the informative end: filesystem paths
/// (`~/a/b/c/nini` → `…/b/c/nini`) and model identifiers
/// (`anthropic/claude-sonnet-4-5` → `…claude-sonnet-4-5`). Cutting
/// these from the right throws away exactly the part that distinguishes
/// them.
pub fn truncate_cells_head(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let budget = max - 1;
    // Walk backwards accumulating width, then cut on a char boundary.
    let mut end = s.len();
    let mut consumed = 0usize;
    for (byte_idx, c) in s.char_indices().rev() {
        let cw = char_width(c);
        if consumed + cw > budget {
            end = byte_idx + c.len_utf8();
            break;
        }
        consumed += cw;
    }
    format!("…{}", &s[ceil_char_boundary(s, end)..])
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
    fn truncate_cells_head_keeps_the_tail() {
        // Paths and model ids: the tail is the informative end.
        assert_eq!(truncate_cells_head("~/a/b/c/nini", 20), "~/a/b/c/nini");
        assert_eq!(truncate_cells_head("~/a/b/c/nini", 10), "…/b/c/nini"); // 12 chars -> keep last 9
        assert_eq!(
            truncate_cells_head("anthropic/claude-sonnet-4-5", 12),
            "…-sonnet-4-5"
        ); // keep last 11
    }

    #[test]
    fn truncate_cells_head_degenerate_budgets() {
        assert_eq!(truncate_cells_head("abcdef", 0), "");
        assert_eq!(truncate_cells_head("abcdef", 1), "…");
        assert_eq!(truncate_cells_head("", 5), "");
    }

    #[test]
    fn truncate_cells_head_never_exceeds_the_budget() {
        let samples = [
            "plain ascii that is long enough to cut",
            "中文路径很长需要从左边截断",
            "mixed 中文 and ascii together",
            "💭 emoji 🧠 tail",
        ];
        for s in samples {
            for max in 0..40usize {
                let out = truncate_cells_head(s, max);
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

/// Fold one rendered line to `max_width` cells, keeping styles.
///
/// Pi: `wrapSingleLine(line, width)` — `@mariozechner/pi-tui`
/// `dist/utils.js:580`, reached through `wrapTextWithAnsi` (`:562`).
///
/// **Why this exists.** The transcript renders through ratatui's `List`,
/// which does not wrap: it cuts at the screen edge. So a reply wider than
/// the terminal used to be handed to `truncate_cells` and the tail was
/// dropped. Measured in tmux with a scripted provider, a CJK reply came out
/// as its first screenful plus `…` at 40, 60 and 100 columns, and intact
/// only at 160 — which is why short fixtures never saw it.
///
/// Pi's shape, kept as-is:
///
/// * fold on token boundaries — a run of non-whitespace and a run of
///   whitespace are separate tokens, and the line breaks *between* tokens
/// * a token that is wider than `max_width` is broken by grapheme
/// * the tail of every line is trimmed, and no line starts with whitespace
///   (`utils.js:619-628`)
///
/// Unlike Pi, no ANSI tracking is needed: nini's `Line` carries styles as
/// structured `Span`s, so splitting a span preserves them by construction.
pub fn wrap_line(
    line: ratatui::text::Line<'static>,
    max_width: usize,
) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    use unicode_segmentation::UnicodeSegmentation;

    if max_width == 0 {
        return vec![line];
    }
    let total: usize = line
        .spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    if total <= max_width {
        return vec![line];
    }

    // Grapheme per unit, so a wide glyph or an emoji with a skin tone never
    // gets cut in half.
    let mut units: Vec<(String, Style)> = Vec::with_capacity(total);
    for span in &line.spans {
        for g in UnicodeSegmentation::graphemes(span.content.as_ref(), true) {
            units.push((g.to_string(), span.style));
        }
    }

    let is_ws = |u: &(String, Style)| u.0.chars().all(char::is_whitespace);

    let mut out: Vec<ratatui::text::Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;

    // Push `cur` as a line, trimming the whitespace that a fold leaves at
    // the end. Pi does the same (`line.trimEnd()`, `utils.js:644`).
    let flush = |cur: &mut Vec<Span<'static>>, cur_w: &mut usize, out: &mut Vec<_>| {
        while cur
            .last()
            .is_some_and(|s| s.content.chars().all(char::is_whitespace))
        {
            cur.pop();
        }
        out.push(ratatui::text::Line::from(std::mem::take(cur)));
        *cur_w = 0;
    };

    let mut i = 0usize;
    while i < units.len() {
        // One token: a maximal run of whitespace, or of non-whitespace.
        let ws = is_ws(&units[i]);
        let mut j = i;
        let mut tok_w = 0usize;
        while j < units.len() && is_ws(&units[j]) == ws {
            tok_w += display_width(&units[j].0);
            j += 1;
        }
        let token = &units[i..j];
        i = j;

        if cur_w + tok_w <= max_width {
            for (t, st) in token {
                cur.push(Span::styled(t.clone(), *st));
            }
            cur_w += tok_w;
            continue;
        }

        // A token that cannot fit on any line of its own — break it by
        // grapheme, which is Pi's `breakLongWord` (`utils.js:657`).
        if tok_w > max_width {
            flush(&mut cur, &mut cur_w, &mut out);
            for (t, st) in token {
                let w = display_width(t);
                if cur_w + w > max_width && cur_w > 0 {
                    flush(&mut cur, &mut cur_w, &mut out);
                }
                cur.push(Span::styled(t.clone(), *st));
                cur_w += w;
            }
            continue;
        }

        // It would fit on a fresh line: fold here.
        flush(&mut cur, &mut cur_w, &mut out);
        if ws {
            // "Don't start new line with whitespace" (`utils.js:626`).
            continue;
        }
        for (t, st) in token {
            cur.push(Span::styled(t.clone(), *st));
        }
        cur_w += tok_w;
    }

    if !cur.is_empty() {
        while cur
            .last()
            .is_some_and(|s| s.content.chars().all(char::is_whitespace))
        {
            cur.pop();
        }
        out.push(ratatui::text::Line::from(cur));
    }

    if out.is_empty() {
        out.push(ratatui::text::Line::from(""));
    }
    out
}

/// Fold plain text to `max_width` cells, returning `None` when it does not
/// need folding.
///
/// Pi's table cells are plain strings by the time they are laid out —
/// `wrapCellText(text, columnWidths[i])` (`@mariozechner/pi-tui`
/// `dist/components/markdown.js:640`) — so they need the wrapping rules
/// without the `Span` bookkeeping [`wrap_line`] does for a styled line.
pub fn wrap_text_to_width(text: &str, max_width: usize) -> Option<Vec<String>> {
    if max_width == 0 {
        return None;
    }
    if display_width(text) <= max_width {
        return None;
    }
    let line = ratatui::text::Line::from(ratatui::text::Span::raw(text.to_string()));
    Some(
        wrap_line(line, max_width)
            .into_iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect(),
    )
}
