//! Oracle: **转义序列不能变成 cell 文本**（v0.10.8，TUI 破框缺陷）。
//!
//! # 现象（真实会话）
//! 一个 agent 会话的 transcript 里出现 JS 堆栈，每行形如
//! `    at processSessionCache.connect (file:///home/jin/.nvm/…/run.js:277:37):`。
//! 屏幕上那一行**比终端还宽**，后面的行整体错位 —— 框破了。
//!
//! # 根因
//! nini 会把裸 URL **自动包成 OSC 8 超链接**（`hyperlink::auto_link`）——
//! 那是 Pi 的做法（Pi 自己往终端写字节，能正确发出这个序列）。
//!
//! 但 nini 的 transcript 是经 **ratatui 的 cell 缓冲区**画的，而 ratatui 0.29
//! **没有** hyperlink 支持（没有 `Modifier::HYPERLINK`）。于是那段转义序列
//! 变成**字面的 cell 文本**：终端收到 `ESC ]8;;` 后把它当 OSC 开始，把后面的
//! URL 当**可见文字**画出来 —— 那一行比屏幕宽，**框破了**。
//!
//! 而且宽度计算把零宽的转义当成了可见字符，于是行被**提前截断**
//! （实测吃掉 139 格，内容凭空消失）。
//!
//! # 两条判据
//! 1. **cell 里不能有转义字节**（ESC）——有就是把控制序列当文字画了；
//! 2. **宽度计算必须忽略转义序列**——它们是零宽的。

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 一条真实的堆栈行（超长、不间断、含 file:// 路径）。
const STACK_LINE: &str = "    at processSessionCache.connect (file:///home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/ncp/dist/run.js:277:37):";

fn frame_with_result(body: &str, width: u16, height: u16) -> Vec<Vec<String>> {
    let mut state = AppState::new("test-model");
    {
        let ts = std::sync::Arc::make_mut(&mut state.transcript_state);
        ts.lines.push(TranscriptLine::ToolResult {
            id: "t1".into(),
            name: "bash".into(),
            ok: false,
            content: body.to_string(),
            collapsed: false,
            duration_ms: Some(12),
        });
    }
    let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
    t.draw(|f| render_frame(f, &state)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| {
                    buf.cell((x, y))
                        .map(|c| c.symbol().to_string())
                        .unwrap_or_else(|| " ".to_string())
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_long_url_does_not_put_escape_bytes_into_the_frame() {
    // 终端支持 OSC 8（WezTerm）——正是这条路径当初注入了转义序列。
    std::env::set_var("TERM_PROGRAM", "WezTerm");

    let rows = frame_with_result(STACK_LINE, 80, 24);

    for (y, row) in rows.iter().enumerate() {
        let line: String = row.concat();
        assert!(
            !line.contains('\u{1b}'),
            "第 {y} 行里有 ESC 字节 —— 转义序列被当成 cell 文本画出来了：{line:?}"
        );
        assert!(
            !line.contains("]8;;"),
            "第 {y} 行里残留了 OSC 8 序列：{line:?}"
        );
    }
}

#[test]
fn a_long_url_does_not_break_the_frame() {
    std::env::set_var("TERM_PROGRAM", "WezTerm");

    let rows = frame_with_result(STACK_LINE, 80, 24);
    for (y, row) in rows.iter().enumerate() {
        // 每一行的 cell 数就是终端宽度——但**屏幕上真正显示的宽度**可能更大，
        // 因为终端会把 cell 里的转义当控制序列、把后面的文字当可见字符。
        // 上一条断言已经排除了转义字节；这里再钉住「内容没被吃掉」：
        // 那一行必须仍然看得到路径的开头。
        let line: String = row.concat();
        if line.contains("at processSessionCache.connect") {
            assert!(
                line.contains("file:///home/jin/.nvm"),
                "第 {y} 行的内容被提前截断了（URL 应当以纯文本可见）：{line:?}"
            );
        }
    }
}

#[test]
fn the_width_of_a_string_ignores_escape_sequences() {
    // 转义序列是**零宽**的：算进去会让含转义的行被误判为超宽，
    // 于是渲染器提前截断（实测吃掉 139 格）。
    let plain = "at processSessionCache.connect (file:///x/y/z.js:277:37):";
    let with_osc8 = format!("\u{1b}]8;;file:///x/y/z.js\u{1b}\\{plain}\u{1b}]8;;\u{1b}\\");
    assert_eq!(
        nini_tui::width::display_width(&with_osc8),
        nini_tui::width::display_width(plain),
        "含转义的字符串与去掉转义后的字符串，显示宽度必须相同"
    );
    assert_eq!(
        nini_tui::width::display_width(&format!("\u{1b}[31mred\u{1b}[0m")),
        3,
        "ANSI 颜色序列是零宽的"
    );
}
