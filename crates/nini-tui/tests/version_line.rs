//! invariant: v1.0.0 — 每次打开 nini TUI 都显示版本号（TUI 顶部版本行，始终存在）。
//!
//! 版本行是 TUI 帧顶部的一行，每次渲染都画：冷启动（空会话）与已有会话
//! 时再次打开，都要显示当前运行的版本号。版本号读自 `env!("CARGO_PKG_VERSION")`，
//! 与 `nini --version` 的版本段同源（`crates/nini-cli/src/main.rs` 的 `version_line()`
//! 同样用 `env!("CARGO_PKG_VERSION")`），不硬编码版本字符串。
//!
//! 断言的是用户看到的内容：渲染出的帧里，顶部那一行包含 `nini {版本号}`；
//! 冷启动（空会话）与有会话两种情况下都出现。

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn frame_text(width: u16, height: u16, state: &AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// 冷启动（空会话）：顶部版本行必须出现，且版本号与 `--version` 同源。
#[test]
fn cold_start_shows_the_version_line() {
    let state = AppState::new("test-model");
    for width in [60u16, 80, 120, 200] {
        let frame = frame_text(width, 24, &state);
        let version = env!("CARGO_PKG_VERSION");
        let expected = format!("nini {version}");
        assert!(
            frame.contains(&expected),
            "at width {width}, cold start lost the version line \"nini {version}\":\n{frame}"
        );
        // 版本行必须在顶部第一行，才是“每次打开 TUI 就看到”。
        let first_line = frame.lines().next().unwrap();
        assert!(
            first_line.contains(&expected),
            "at width {width}, the version line is not on the first row of the frame:\n{first_line}"
        );
    }
}

/// 短终端（高度 5，最窄可跑的 TUI）也必须有版本行：版本行是固定的一行，
/// 不能依赖终端够大才出现（锁「始终存在」）。
#[test]
fn the_version_line_shows_on_short_terminals_too() {
    let state = AppState::new("test-model");
    let frame = frame_text(80, 5, &state);
    let version = env!("CARGO_PKG_VERSION");
    let expected = format!("nini {version}");
    assert!(
        frame.contains(&expected),
        "height 5 cold start lost the version line \"nini {version}\":\n{frame}"
    );
    let first_line = frame.lines().next().unwrap();
    assert!(
        first_line.contains(&expected),
        "height 5 the version line is not on the first row:\n{first_line}"
    );
}

/// 已有会话时再次打开 TUI：版本行同样必须出现（不是只在冷启动欢迎卡里）。
#[test]
fn a_live_session_still_shows_the_version_line() {
    let mut state = AppState::new("test-model");
    state.push_user("how does the version show up");
    state.push_assistant("it shows on the top line asini version");

    let frame = frame_text(120, 30, &state);
    let version = env!("CARGO_PKG_VERSION");
    let expected = format!("nini {version}");
    assert!(
        frame.contains(&expected),
        "a live session lost the version line \"nini {version}\":\n{frame}"
    );
}

/// 版本号必须来自编译期的 crate 版本，而不是硬编码字符串：改版本后测试仍跟着变。
#[test]
fn the_version_line_comes_from_cargo_pkg_version() {
    // 这里直接断言渲染出的版本行与编译期版本一致（与 above 同源）；
    // 若有人把版本写死，这里会让“写死 ≠ crate 版本”露出来。
    let state = AppState::new("test-model");
    let frame = frame_text(100, 24, &state);
    let crate_version = env!("CARGO_PKG_VERSION");
    let expected = format!("nini {crate_version}");
    assert_eq!(
        frame.contains(&expected),
        true,
        "the version line must be rendered from CARGO_PKG_VERSION (= {crate_version}):\n{frame}"
    );
}
