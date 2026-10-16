//! A markdown table is a table, not the word "[table]".
//!
//! `ENABLE_TABLES` has been on in `markdown.rs` since the parser swap, and
//! the module's own header table lists "Tables (basic)" as supported — but
//! `Tag::Table` pushed the literal string `[table]` and the cell text fell
//! through to the paragraph path. In the terminal a three-column table came
//! out as:
//!
//! ```text
//! [table]
//! 模块
//! 行数
//! 风险
//! mod_0
//! 120
//! 高
//! ```
//!
//! one cell per line, with the marker in front.
//!
//! Pi: `renderTable(token, availableWidth, …)`,
//! `@mariozechner/pi-tui` `dist/components/markdown.js:543` — box-drawing
//! frame, columns sized between a natural and a minimum width, cell text
//! wrapped to its column, and a fallback to the raw source when even one
//! cell per column will not fit.
//!
//! These call `render_markdown`, whose signature is unchanged by the fix.

use nini_tui::markdown::render_markdown;
use nini_tui::theme::Theme;
use nini_tui::width::display_width;

const TABLE: &str =
    "| 模块 | 行数 | 风险 |\n|---|---|---|\n| mod_0 | 120 | 高 |\n| mod_1 | 157 | 中 |\n";

fn lines(width: usize) -> Vec<String> {
    render_markdown(TABLE, &Theme::dark(), Some(width))
        .into_iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn joined(width: usize) -> String {
    lines(width).join("\n")
}

#[test]
fn a_table_is_not_the_word_table() {
    let out = joined(80);
    assert!(
        !out.contains("[table]"),
        "the renderer still emits its placeholder: {out}"
    );
}

#[test]
fn the_cells_survive_in_readable_positions() {
    let out = joined(80);
    for cell in ["模块", "行数", "风险", "mod_0", "120", "高", "mod_1", "157"] {
        assert!(out.contains(cell), "cell {cell:?} is missing: {out}");
    }
    // A header and a body row are on the same visual row, not stacked.
    let header_row = out
        .lines()
        .find(|l| l.contains("模块") && l.contains("行数"))
        .unwrap_or_else(|| panic!("header cells are not on one row:\n{out}"));
    assert!(
        header_row.contains("风险"),
        "the header row lost a column: {header_row:?}"
    );
    let body_row = out
        .lines()
        .find(|l| l.contains("mod_0") && l.contains("120"))
        .unwrap_or_else(|| panic!("body cells are not on one row:\n{out}"));
    assert!(
        body_row.contains("高"),
        "the body row lost a column: {body_row:?}"
    );
}

#[test]
fn the_table_is_drawn_with_a_frame() {
    let out = joined(80);
    assert!(
        out.contains('┌') && out.contains('┐'),
        "no top frame:\n{out}"
    );
    assert!(
        out.contains('└') && out.contains('┘'),
        "no bottom frame:\n{out}"
    );
    assert!(out.contains('├'), "no header separator:\n{out}");
    assert!(out.contains('│'), "no column separators:\n{out}");
}

#[test]
fn no_table_row_exceeds_the_width() {
    for width in [30usize, 40, 60, 80, 120] {
        for line in lines(width) {
            assert!(
                display_width(&line) <= width,
                "width {width}: row is {} cells: {line:?}",
                display_width(&line)
            );
        }
    }
}

/// A table with more text than fits has to shrink its columns, not lose
/// cells — this is the whole point of the column arithmetic.
#[test]
fn a_wide_table_shrinks_columns_and_keeps_every_cell() {
    let wide = "\
| module | description | risk |
|---|---|---|
| render | the transcript renderer and its width handling | high |
| session | the on-disk JSONL session | low |
";
    for width in [40usize, 60, 100] {
        let out = render_markdown(wide, &Theme::dark(), Some(width))
            .into_iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for cell in [
            "module",
            "description",
            "risk",
            "render",
            "session",
            "high",
            "low",
        ] {
            assert!(
                out.contains(cell),
                "width {width}: {cell:?} was dropped:\n{out}"
            );
        }
    }
}

/// A table narrower than its own frame cannot be framed. Pi falls back to
/// the raw source (`markdown.js:557`); nini has no raw text to fall back to,
/// so the requirement is the weaker but still meaningful one: it must not
/// panic, must not overflow, and must not invent rows.
#[test]
fn an_impossibly_narrow_table_degrades_instead_of_overflowing() {
    for width in [4usize, 6, 8, 10] {
        let out: Vec<String> = render_markdown(TABLE, &Theme::dark(), Some(width))
            .into_iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        for line in &out {
            assert!(
                display_width(line) <= width,
                "width {width}: row is {} cells: {line:?}",
                display_width(line)
            );
        }
    }
}

/// The frame must survive a CJK table, where every cell is two cells wide
/// and an off-by-one would show up as a ragged right edge.
#[test]
fn a_cjk_table_is_rectangular() {
    let out = lines(60);
    let boxed: Vec<&String> = out
        .iter()
        .filter(|l| l.contains('│') || l.contains('┌') || l.contains('├') || l.contains('└'))
        .collect();
    assert!(
        boxed.len() >= 6,
        "expected a framed table, got:\n{}",
        out.join("\n")
    );
    let widths: Vec<usize> = boxed.iter().map(|l| display_width(l)).collect();
    let first = widths[0];
    for (line, w) in boxed.iter().zip(&widths) {
        assert_eq!(
            *w, first,
            "ragged table: {line:?} is {w} cells, the frame is {first}"
        );
    }
}

/// Ordinary markdown around a table must not be swallowed by it.
#[test]
fn text_around_a_table_survives() {
    let md = "before\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter";
    let out = render_markdown(md, &Theme::dark(), Some(60))
        .into_iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        out.contains("before"),
        "the paragraph before was lost:\n{out}"
    );
    assert!(
        out.contains("after"),
        "the paragraph after was lost:\n{out}"
    );
    assert!(out.contains('│'), "and the table itself is gone:\n{out}");
}
