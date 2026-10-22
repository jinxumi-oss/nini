use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 行宽，按「中文终端」的口径：歧义字符算 2 列。
fn cjk_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width_cjk(s)
}

#[test]
fn probe() {
    let mut state = AppState::new("m");
    state.push_assistant(String::from(
        "| 项目 | 数量 | 说明 |\n|---|---|---|\n| crates | 213 | 说明 |",
    ));
    state.push_assistant(String::from(
        "• 第一条\n• 第二条\n---\n```bash\nls -la\n```",
    ));
    state.push_user(String::from("请统计 crates 下所有 Rust 源文件的数量"));
    let w = 100u16;
    let mut t = Terminal::new(TestBackend::new(w, 24)).unwrap();
    t.draw(|f| render_frame(f, &state)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut over = 0;
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            let sym = buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ");
            // 续格（空符号）不算一列
            if !sym.is_empty() {
                line.push_str(sym);
            }
        }
        let cw = cjk_width(&line);
        if cw > w as usize {
            over += 1;
            println!(
                "行 {y} 中文口径宽 {cw} > {w}: {:?}",
                &line[..line.len().min(60)]
            );
        }
    }
    println!("超宽行数 = {over}");
}
