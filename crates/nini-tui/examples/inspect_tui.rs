use nini_tui::render::render_frame;
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn snapshot(state: &AppState, w: u16, h: u16, label: &str) {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    out.push_str(&format!("=== {label} ({w}x{h}) ===\n"));
    let area = buf.area;
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    println!("{out}");
}

fn main() {
    // multi-turn state like multi_turn_agent_via_sink test
    let mut s = AppState::new("test-model");
    s.transcript_state.lines.push(TranscriptLine::User("hi".into()));
    s.transcript_state.lines.push(TranscriptLine::Divider);
    s.transcript_state.lines.push(TranscriptLine::AssistantText("first reply".into()));
    s.transcript_state.lines.push(TranscriptLine::Divider);
    s.transcript_state.lines.push(TranscriptLine::User("use bash".into()));
    s.transcript_state.lines.push(TranscriptLine::Divider);
    s.transcript_state.lines.push(TranscriptLine::AssistantText("using bash".into()));
    s.transcript_state.lines.push(TranscriptLine::ToolCall {
        name: "bash".into(),
        args: "{}".into(),
        collapsed: false,
    });
    s.transcript_state.lines.push(TranscriptLine::ToolResult {
        ok: true,
        content: "hi".into(),
        collapsed: false,
        duration_ms: Some(10),
    });
    s.transcript_state.lines.push(TranscriptLine::AssistantText("after tool".into()));
    s.run_state.mode = RunMode::Editing;
    snapshot(&s, 100, 30, "MULTI-TURN state, 100x30");

    // Now run frame_text style trim
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, &s)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut frame = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        frame.push_str(line.trim_end_matches(' '));
        frame.push('\n');
    }
    println!("=== TRIMMED FRAME ===");
    println!("{frame}");
    println!("=== ASSERTIONS ===");
    println!("contains '> hi':       {}", frame.contains("> hi"));
    println!("contains '▸ bash':     {}", frame.contains("▸ bash"));
    println!("contains 'first reply':{}", frame.contains("first reply"));
    println!("contains 'after tool': {}", frame.contains("after tool"));
}