use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
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
    // Long transcript that overflows the viewport
    let mut s = AppState::new("test-model");
    for i in 0..30 {
        s.transcript_state
            .lines
            .push(TranscriptLine::User(format!("user message #{i}")));
        s.transcript_state
            .lines
            .push(TranscriptLine::AssistantText(format!("assistant reply to #{i}")));
    }
    snapshot(&s, 100, 30, "A: 60 lines in 100x30 (autoscroll)");

    // User scrolled up
    let mut s2 = s.clone();
    s2.transcript_state.autoscroll = false;
    s2.transcript_state.scroll_offset = 15;
    snapshot(&s2, 100, 30, "B: scrolled up 15 lines (should show ↓ 15 more)");

    // User scrolled way up
    let mut s3 = s.clone();
    s3.transcript_state.autoscroll = false;
    s3.transcript_state.scroll_offset = 50;
    snapshot(&s3, 100, 30, "C: scrolled up 50 lines (should show ↓ 50 more)");

    // Short transcript (fits in viewport)
    let mut s4 = AppState::new("test-model");
    s4.transcript_state.lines.push(TranscriptLine::User("hi".into()));
    s4.transcript_state
        .lines
        .push(TranscriptLine::AssistantText("hello there".into()));
    snapshot(&s4, 100, 30, "D: short transcript (2 lines, autoscroll)");
}