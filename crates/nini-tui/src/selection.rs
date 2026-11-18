//! Mouse text selection for the transcript.
//!
//! v0.11: nini's transcript is rendered as a `List` of one `ListItem` per
//! screen row, each built from a `TranscriptLine` by `render::items_for_line`.
//! Selection works in **screen-row space**: the renderer publishes the plain
//! text of every visible screen row, the mouse hit-tests a `(row, col)`
//! against those rows, and the selected text is the concatenation of the
//! rows between the anchor and the focus.
//!
//! The rule this module enforces: **select what is on screen.** A row is
//! one screen row, a column is a character offset within that row's text
//! (mapped from the terminal column through [`col_to_char_offset`, which
//! counts CJK characters as the width the terminal actually gives them).
//! Wrapped continuations are their own rows, so a drag that crosses a
//! wrap boundary selects both parts with a `\n` between them — exactly
//! what the reader sees.
//!
//! Everything here is a pure function of its inputs so the selection
//! geometry can be tested without a terminal.

/// A point in screen-row space.
///
/// `row` is the index into the published `screen_text_rows` (0-based,
/// counting from the top of the transcript content area). `col` is a
/// **character offset** into that row's text (0..=char_count), not a
/// terminal column — see [`col_to_char_offset`] for the mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectionPoint {
    pub row: usize,
    pub col: usize,
}

/// The granular unit a selection operates on.
///
/// Mirrors Pi's `SelectionGranularity` (`tui-alt-screen.ts:213`): a click
/// selects a character, a double-click a word, a triple-click a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGranularity {
    /// A single character at the pointer.
    Character,
    /// The whole word under the pointer. Paths, `kebab-case` tokens and
    /// `snake_case` tokens stay whole (see [`word_bounds`]).
    Word,
    /// The whole screen row the pointer is on.
    Line,
}

/// An active selection: a fixed anchor and a moving focus.
///
/// The anchor stays where the press started; the focus follows the pointer
/// during a drag. Either may be the "top" or "bottom" of the selection —
/// the order is determined when the text is extracted, so dragging upward
/// works the same as dragging downward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: SelectionPoint,
    pub focus: SelectionPoint,
}

impl Selection {
    /// The two endpoints, ordered so the first is the top-left corner.
    pub fn ordered(&self) -> (SelectionPoint, SelectionPoint) {
        if (self.anchor.row, self.anchor.col) <= (self.focus.row, self.focus.col) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    /// True when the anchor and focus are the same point (no drag, or a
    /// zero-width click).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

/// Is this character part of a "word" for word-granularity selection?
///
/// Letters, digits and CJK ideographs (`is_alphanumeric` covers all three
/// in Unicode) plus the joiners that keep paths and identifiers whole:
/// `-`, `_`, `/`, `.`, `+`, `=`. Whitespace and punctuation are separators.
///
/// This is the Rust equivalent of Pi's `TERMINAL_WORD_SELECTION_JOINERS`
/// (`tui-alt-screen.ts:81-90`): a double-click on `foo/bar_baz` selects the
/// whole token, not just `foo`.
fn is_word_or_joiner(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '/' | '.' | '+' | '=')
}

/// Map a terminal **column** to a character offset within `text`.
///
/// Terminal columns count cells, and a CJK character occupies two cells.
/// Walking the text and accumulating display widths places the pointer:
/// a column that lands inside a character's cells maps to that character's
/// offset, and a column past the end maps to the character count.
///
/// The unit is a **character index** (not a byte offset) — the same unit
/// [`SelectionPoint::col`] and every range in this module use. Mixing the
/// two silently mis-selects every row that contains a non-ASCII character.
pub fn col_to_char_offset(text: &str, screen_col: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut width = 0usize;
    for (char_idx, c) in text.chars().enumerate() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        // The column is "at" this character if it falls in [width, width+cw).
        if width >= screen_col {
            return char_idx;
        }
        if width + cw > screen_col {
            return char_idx;
        }
        width += cw;
    }
    text.chars().count()
}

/// The `[start, end)` character range of the word containing `col` in
/// `text`.
///
/// `col` is a **character index** (it comes from [`col_to_char_offset`]).
/// The word is the maximal run of word-characters and joiners **around**
/// the character at `col` — see [`is_word_or_joiner`]. If the character at
/// `col` is not a word character (e.g. whitespace or punctuation), the
/// range is zero-width at `col`.
pub fn word_bounds(text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let char_idx = col.min(n);

    // Only expand if the character under the pointer is a word character.
    if char_idx < n && is_word_or_joiner(chars[char_idx]) {
        let mut s = char_idx;
        while s > 0 && is_word_or_joiner(chars[s - 1]) {
            s -= 1;
        }
        let mut e = char_idx + 1;
        while e < n && is_word_or_joiner(chars[e]) {
            e += 1;
        }
        return (s, e);
    }
    // Not on a word character: zero-width range at the boundary.
    (char_idx, char_idx)
}

/// The `[start, end)` range of the whole row: the entire text.
pub fn line_bounds(text: &str) -> (usize, usize) {
    (0, text.chars().count())
}

/// Compute the selection range for a pointer at `point` given the
/// `granularity`, against the text of `row`.
///
/// Returns `None` for word granularity when the pointer is not on a
/// word-character (e.g. on whitespace), so the caller falls back to a
/// character selection.
pub fn range_for_point(
    row_text: &str,
    point: SelectionPoint,
    granularity: SelectionGranularity,
) -> Option<(usize, usize)> {
    match granularity {
        SelectionGranularity::Character => {
            let n = row_text.chars().count();
            let col = point.col.min(n);
            if col >= n {
                return Some((col, col));
            }
            // One character: the next index is the next character.
            Some((col, col + 1))
        }
        SelectionGranularity::Word => {
            let n = row_text.chars().count();
            let col = point.col.min(n);
            let (s, e) = word_bounds(row_text, col);
            if s == e {
                // No word character under the pointer.
                return None;
            }
            Some((s, e))
        }
        SelectionGranularity::Line => Some(line_bounds(row_text)),
    }
}

/// Extract the text covered by a selection from the published screen rows.
///
/// Rows between the anchor and the focus are taken whole; the anchor and
/// focus rows are taken from the appropriate column to the edge (or to the
/// other endpoint when both are on the same row). Rows are joined with
/// `\n`, matching what the reader sees when a selection crosses a wrap
/// boundary.
///
/// Missing rows (the selection ran past the published rows, e.g. the
/// transcript shrank mid-drag) contribute nothing rather than panicking.
pub fn selection_text(screen_rows: &[String], selection: &Selection) -> String {
    let (start, end) = selection.ordered();
    let mut parts: Vec<String> = Vec::new();

    for row in start.row..=end.row {
        let Some(text) = screen_rows.get(row) else {
            continue;
        };
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();

        let (cs, ce) = if start.row == end.row {
            // Single row: between the two columns.
            (start.col.min(n), end.col.min(n))
        } else if row == start.row {
            // First row: from the start column to the end of the row.
            (start.col.min(n), n)
        } else if row == end.row {
            // Last row: from the start of the row to the end column.
            (0, end.col.min(n))
        } else {
            // Middle row: the whole row.
            (0, n)
        };

        // Convert char indices back to byte offsets for slicing.
        let sb = chars.iter().take(cs).map(|c| c.len_utf8()).sum::<usize>();
        let eb = chars.iter().take(ce).map(|c| c.len_utf8()).sum::<usize>();
        parts.push(text[sb..eb].to_string());
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_to_char_offset_ascii() {
        // ASCII: one cell per char, so the offset equals the column.
        assert_eq!(col_to_char_offset("hello", 0), 0);
        assert_eq!(col_to_char_offset("hello", 2), 2);
        assert_eq!(col_to_char_offset("hello", 5), 5);
        // Past the end clamps to len.
        assert_eq!(col_to_char_offset("hello", 99), 5);
    }

    #[test]
    fn col_to_char_offset_cjk() {
        // "你" occupies 2 cells, "好" occupies 2 cells. The unit is a
        // **character index**: column 2 is the start of 好 (character 1).
        let text = "你好";
        assert_eq!(col_to_char_offset(text, 0), 0);
        assert_eq!(col_to_char_offset(text, 1), 0, "column 1 is inside 你");
        assert_eq!(
            col_to_char_offset(text, 2),
            1,
            "column 2 is the start of 好 (character 1)"
        );
        assert_eq!(col_to_char_offset(text, 3), 1, "column 3 is inside 好");
        assert_eq!(col_to_char_offset(text, 4), 2, "past the end");
    }

    #[test]
    fn word_bounds_simple_word() {
        assert_eq!(word_bounds("hello world", 2), (0, 5));
        // Clicking on the space is not a word.
        assert_eq!(word_bounds("hello world", 5), (5, 5));
    }

    #[test]
    fn word_bounds_keeps_path_and_identifiers_whole() {
        // Path: slashes are joiners.
        assert_eq!(word_bounds("foo/bar_baz", 5), (0, 11));
        // kebab-case: hyphens are joiners.
        assert_eq!(word_bounds("foo-bar-baz", 5), (0, 11));
        // snake_case: underscores are joiners.
        assert_eq!(word_bounds("foo_bar_baz", 5), (0, 11));
        // A word surrounded by punctuation.
        assert_eq!(word_bounds("(hello)", 3), (1, 6));
    }

    #[test]
    fn word_bounds_cjk() {
        // CJK characters are alphanumeric, so they form words.
        // "你好世界" = 4 characters.
        assert_eq!(word_bounds("你好世界", 3), (0, 4));
        // Clicking past the end.
        assert_eq!(word_bounds("你好世界", 99), (4, 4));
    }

    #[test]
    fn range_for_point_word_falls_back_on_separator() {
        let text = "hello world";
        // On "hello": the whole word.
        let r = range_for_point(
            text,
            SelectionPoint { row: 0, col: 2 },
            SelectionGranularity::Word,
        );
        assert_eq!(r, Some((0, 5)));
        // On the space: no word, returns None.
        let r = range_for_point(
            text,
            SelectionPoint { row: 0, col: 5 },
            SelectionGranularity::Word,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn range_for_point_line_is_whole_row() {
        let text = "hello world";
        let r = range_for_point(
            text,
            SelectionPoint { row: 0, col: 2 },
            SelectionGranularity::Line,
        );
        assert_eq!(r, Some((0, 11)));
    }

    #[test]
    fn selection_text_single_row() {
        let rows = vec!["hello world".to_string()];
        let sel = Selection {
            anchor: SelectionPoint { row: 0, col: 2 },
            focus: SelectionPoint { row: 0, col: 7 },
        };
        assert_eq!(selection_text(&rows, &sel), "llo w");
    }

    #[test]
    fn selection_text_crosses_rows_with_newline() {
        let rows = vec![
            "first line".to_string(),
            "second line".to_string(),
            "third line".to_string(),
        ];
        // Select from col 2 of row 0 to col 4 of row 2.
        let sel = Selection {
            anchor: SelectionPoint { row: 0, col: 2 },
            focus: SelectionPoint { row: 2, col: 4 },
        };
        assert_eq!(selection_text(&rows, &sel), "rst line\nsecond line\nthir");
    }

    #[test]
    fn selection_text_dragging_upward_matches_downward() {
        let rows = vec!["alpha".to_string(), "beta".to_string()];
        let down = Selection {
            anchor: SelectionPoint { row: 0, col: 0 },
            focus: SelectionPoint { row: 1, col: 3 },
        };
        let up = Selection {
            anchor: SelectionPoint { row: 1, col: 3 },
            focus: SelectionPoint { row: 0, col: 0 },
        };
        assert_eq!(selection_text(&rows, &down), selection_text(&rows, &up));
    }

    #[test]
    fn selection_text_skips_missing_rows() {
        // The transcript shrank mid-drag: row 1 is gone.
        let rows = vec!["only row".to_string()];
        let sel = Selection {
            anchor: SelectionPoint { row: 0, col: 0 },
            focus: SelectionPoint { row: 5, col: 0 },
        };
        // Rows 1-5 are missing, so only row 0 contributes.
        assert_eq!(selection_text(&rows, &sel), "only row");
    }

    #[test]
    fn selection_ordered_handles_upward_drag() {
        let sel = Selection {
            anchor: SelectionPoint { row: 2, col: 5 },
            focus: SelectionPoint { row: 0, col: 1 },
        };
        let (s, e) = sel.ordered();
        assert_eq!(s, SelectionPoint { row: 0, col: 1 });
        assert_eq!(e, SelectionPoint { row: 2, col: 5 });
    }
}
