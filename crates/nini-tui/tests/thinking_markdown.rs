//! Oracle: a thinking block is Markdown, not a string.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! Pi renders each run of thinking blocks as Markdown
//! (`assistant-message.ts:147`):
//!
//! ```js
//! new Markdown(thinkingBlocks.join("\n\n"), …, this.markdownTheme,
//!             { color: (text) => theme.fg("thinkingText", text), italic: true })
//! ```
//!
//! nini's `wrap_thinking_block` (`render.rs`) splits the text on `\n`,
//! prefixes each line with `  💭 ` and hands the **string** to the
//! transcript. Consequences, both of which a model produces constantly:
//!
//! - a list inside a reasoning block shows its raw `- ` / `1. ` markers
//! - `**emphasis**`, `` `code` `` and fenced blocks show their delimiters
//! - the body is painted `dim`, while Pi paints `thinkingText`, a slot
//!   nini defines (`theme.rs`) and reads nowhere — the same dead-slot
//!   shape as `mdQuote` and `searchMatchBg` in this same session.
//!
//! **Test discipline (v0.10 §35.6)**: driven through `push_thinking_raw`
//! and `render_frame`, both unchanged in signature, and read out of the
//! frame cells. Nothing here calls a function that does not exist yet —
//! which is why this file compiles against the unfixed code.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;

fn render(state: &AppState) -> ratatui::buffer::Buffer {
    let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
    t.draw(|f| render_frame(f, state)).unwrap();
    t.backend().buffer().clone()
}

fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                s.push_str(c.symbol());
            }
        }
        s.push('\n');
    }
    s
}

fn thinking(text: &str) -> AppState {
    let mut state = AppState::new("test-model");
    state.push_thinking_raw(text);
    state
}

#[test]
fn a_list_inside_a_thinking_block_renders_as_a_list() {
    let state = thinking("- first consideration\n- second consideration");
    let text = frame_text(&render(&state));

    assert!(
        !text.contains("- first"),
        "the raw `- ` marker survived, so the block is not being parsed as \
         markdown:\n{text}"
    );
    assert!(
        text.contains("first consideration"),
        "the content itself must still be on screen:\n{text}"
    );
    assert!(
        text.contains('•') || text.contains('●') || text.contains('·'),
        "no list bullet was drawn in its place:\n{text}"
    );
}

#[test]
fn emphasis_markers_do_not_survive() {
    let state = thinking("this is **important** and this is `code`");
    let text = frame_text(&render(&state));

    assert!(
        !text.contains("**important**"),
        "bold delimiters are visible, so the text was not markdown-parsed:\n{text}"
    );
    assert!(
        !text.contains("`code`"),
        "inline-code backticks are visible:\n{text}"
    );
    assert!(
        text.contains("important") && text.contains("code"),
        "the content must still be there:\n{text}"
    );
}

#[test]
fn the_thinking_body_is_italic_and_coloured_from_the_thinking_slot() {
    // Pi: `{ color: theme.fg("thinkingText", …), italic: true }`
    // (`assistant-message.ts:151`).
    //
    // The first draft of this test asserted only "is it italic", which
    // passed against the broken code too — nini's old `dim + ITALIC`
    // path was italic. A test named for a slot it never checks is the
    // §31.4 shape again, so this one looks up the real hex of both slots
    // and asserts the body is neither `Reset` nor `dim`.
    let theme = Theme::dark();
    let want = theme.color("thinkingText");
    let dim = theme.color("dim");

    let state = thinking("a plain reasoning sentence");
    let buf = render(&state);

    // Row-scoped, not cell-scoped. An earlier draft here iterated every
    // cell and asserted on any cell containing "n", "g" or "s" — those
    // letters occur all over the chrome (the input border, the status
    // bar), so the assertion was measuring the frame, not the block.
    // `reasoning` is a word this fixture owns.
    // The row has to be found by **joining the row first**: a ratatui
    // cell holds a single glyph, so `cell.symbol().contains("reasoning")`
    // is never true. (That was the previous draft's mistake, and the test
    // failed with "the reasoning row must be on screen" — a measurement
    // bug read as a rendering bug.)
    let row_text = |y: u16| -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol()))
            .collect()
    };
    let row = (0..buf.area.height)
        .find(|y| row_text(*y).contains("reasoning"))
        .unwrap_or_else(|| {
            panic!(
                "the reasoning row must be on screen; frame was:\n{}",
                frame_text(&buf)
            )
        });

    // Only the cells holding the model's words are in scope. The
    // `  💭 ` gutter is nini's own chrome and is deliberately plain; a
    // `trim().is_empty()` filter does not exclude it, because the glyph
    // is not whitespace and occupies two cells.
    let needle = "reasoning";
    let first_col = row_text(row)
        .find(needle)
        .expect("the fixture word is on this row") as u16;
    let last_col = first_col + needle.chars().count() as u16;

    let mut saw_thinking_colour = false;
    for x in first_col..last_col.min(buf.area.width) {
        let Some(c) = buf.cell((x, row)) else { continue };
        assert!(
            c.modifier.contains(Modifier::ITALIC),
            "Pi renders thinking blocks italic; cell {x} is {:?}",
            c.modifier
        );
        if c.fg == want {
            saw_thinking_colour = true;
        }
        assert_ne!(c.fg, dim, "the body must use `thinkingText`, not `dim`");
        assert_ne!(
            c.fg,
            ratatui::style::Color::Reset,
            "the body must carry a colour at all"
        );
    }
    assert!(
        saw_thinking_colour,
        "no cell in the reasoning block carries the `thinkingText` colour \
         {:?}",
        want
    );
}

#[test]
fn a_fenced_block_inside_thinking_keeps_its_fences() {
    // Fences ARE Markdown syntax and Pi renders them as fences, so this
    // is the one marker that should survive — it pins the boundary
    // between "parsed as markdown" and "printed verbatim".
    let state = thinking("```rust\nlet x = 1;\n```");
    let text = frame_text(&render(&state));

    assert!(
        text.contains("```"),
        "Pi renders a fenced block with fences (markdown.ts:522)"
    );
    assert!(
        text.contains("let x = 1;"),
        "the code itself must be shown:\n{text}"
    );
}
