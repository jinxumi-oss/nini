//! The frame must fit the terminal **as the terminal measures it**.
//!
//! # The failure
//!
//! Every piece of chrome nini draws — `─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`, the
//! `░ ▓ █` progress bar, `• ▸ ✓ ❯ ↑ ↓`, the braille spinner — is an
//! East-Asian **Ambiguous** character. `unicode-width` says one cell
//! (the Unicode default) and so does most of the western world; a
//! terminal configured for a Chinese, Japanese or Korean locale very
//! often says **two**.
//!
//! That is not cosmetic. A row laid out as N cells is 2N columns wide
//! in such a terminal, so the terminal folds it — and folding one row
//! pushes **every row below it down by a line**. What is left on screen
//! is a wall of half-rules and stray fragments: the report's "乱码" and
//! its "下划线".
//!
//! Measured on this repository's own TUI capture at 100 columns: **501
//! of 4920 rows overflow**, the widest at **200** columns.
//!
//! # What is asserted
//!
//! The frame is measured with `unicode_width::width_cjk` — the width a
//! CJK terminal gives — and **no row may exceed the terminal**. The test
//! forces the CJK interpretation (see `NINI_AMBIGUOUS_WIDTH`), so it is
//! the same assertion on a developer machine that has never opened a
//! Chinese terminal.
//!
//! Two directions are checked, because only one of them is safe:
//! assuming the characters are **narrow** when they are wide breaks the
//! screen; assuming they are **wide** when they are narrow only leaves
//! the row a little sparse. So the layout must satisfy the wider of the
//! two, and a terminal that is actually narrow still renders correctly.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The width a **CJK terminal** gives one row of the frame.
///
/// A wide character occupies two cells: the grid stores it in one cell and
/// resets the next one to a space, and the terminal consumes that space
/// as the character's second half. So the space after a wide character is
/// not a column of its own — counting it is how a measurement invents an
/// overflow that is not there.
fn row_width_cjk(cells: &[&str]) -> usize {
    use unicode_width::UnicodeWidthStr;
    let mut width = 0usize;
    let mut continuation = false;
    for sym in cells {
        if *sym == "" {
            continue;
        }
        if continuation && *sym == " " {
            continuation = false;
            continue;
        }
        let w = UnicodeWidthStr::width_cjk(*sym);
        width += w;
        continuation = w == 2;
    }
    width
}

/// Every kind of content the TUI can put on screen, including the chrome
/// it draws itself: a table (box drawing), a rule, a progress bar, a
/// quote, a code fence, an emoji line.
fn hostile_states() -> Vec<AppState> {
    let bodies = [
        "| 项目 | 数量 | 说明 |\n|---|---|---|\n| crates | 213 | 说明 |\n| nini-tui | 106 | TUI |",
        "---\n\n段落：统计结果 crates 下共有二百一十三个 Rust 源文件。\n\n---\n\n结束。",
        "> 引用：终端显示宽度按**单元格**算，不是字符数。\n\n```bash\nfind crates -name '*.rs'\n```",
        "• 第一条  • 第二条  • 第三条\n\n✓ 完成  ✗ 失败  ▸ 工具  ❯ 输入  ↑ 上面  ↓ 下面  ⠋ 转圈",
        "统计结果 ✅ 🚀 🎉 —— crates 下共有二百一十三个源文件。",
    ];
    let mut out = Vec::new();
    for body in bodies {
        let mut state = AppState::new("test-model");
        state.push_assistant(String::from(body));
        state.push_user(String::from(
            "请统计 crates 下所有 Rust 源文件的数量，并解释测试分布。",
        ));
        out.push(state);
    }
    // The *running* state is the one the report is about: the spinner
    // (braille, ambiguous), the phase label, the context bar (`░ ▓ █`,
    // ambiguous), the token counts and the model name — all on one row,
    // all ambiguous, all next to the terminal's right edge.
    let mut running = AppState::new("MiniMax-M3.1-Flash-Preview");
    running.run_state.mode = nini_tui::state::RunMode::Running;
    running.run_state.context_window = 200_000;
    running.run_state.context_used = 42_000;
    running.run_state.tokens.input = 9_900;
    running.run_state.tokens.output = 662;
    running.model_state.provider = Some(String::from("openai-compat"));
    running.push_assistant(String::from("正在统计… • 已完成 3/9 ✓"));
    out.push(running);
    out
}

/// One test, because the policy is process-wide: two tests running in
/// parallel would each set it and the frames would be measured under
/// whichever policy the other one installed.
#[test]
fn the_frame_fits_a_terminal_that_doubles_ambiguous_characters() {
    // Force the wide interpretation: this test is the assertion that the
    // layout survives a terminal that doubles the ambiguous characters.
    nini_tui::width::set_ambiguous_width("wide");

    for width in [60u16, 80, 100, 120] {
        for (i, state) in hostile_states().into_iter().enumerate() {
            let mut t = Terminal::new(TestBackend::new(width, 24)).unwrap();
            t.draw(|f| render_frame(f, &state)).unwrap();
            let buf = t.backend().buffer().clone();
            for y in 0..buf.area.height {
                let cells: Vec<&str> = (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect();
                let w = row_width_cjk(&cells);
                let line: String = cells.concat();
                assert!(
                    w <= width as usize,
                    "case {i} @ {width}: row {y} is {w} columns wide in a CJK terminal \
                     (the terminal has {width}) — it would fold, and every row below it \
                     would move down a line:\n{}",
                    line.chars().take(70).collect::<String>()
                );
            }
        }
    }
    // The override itself: `wide` and `narrow` say it outright, so a
    // terminal that does not follow its locale — or does not answer the
    // probe at all — can be told what it is.
    for (mode, expect_wide) in [("wide", true), ("narrow", false)] {
        nini_tui::width::set_ambiguous_width(mode);
        assert_eq!(
            nini_tui::width::ambiguous_width_is_wide(),
            expect_wide,
            "mode {mode:?} did not take effect"
        );
    }
    // `auto` is the state before the terminal has been asked: the
    // historical behaviour (one cell), which is what every modern
    // terminal does by default. The runtime's ladder — measure, then the
    // locale — decides before the first frame is drawn.
    nini_tui::width::set_ambiguous_width("auto");
    assert!(
        !nini_tui::width::ambiguous_width_is_wide(),
        "before the terminal is asked, nini must keep the historical one-cell assumption"
    );
}
