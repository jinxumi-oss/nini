use std::sync::Arc;
use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn main() {
    let mut s = AppState::new("test-model");
    s.push_user("Run ls /tmp and list 5 files you see");
    Arc::make_mut(&mut s.transcript_state).lines.push(TranscriptLine::ThinkingText(
        "The user wants me to run ls /tmp and list 5 files I see. \
         I'll use the bash tool to execute the ls command and then \
         report back the first 5 file names from the output. \
         This is a simple, safe request - listing files in /tmp \
         is read-only and doesn't modify anything. Let me run the \
         command to see what files are available."
            .to_string(),
    ));
    Arc::make_mut(&mut s.transcript_state).lines.push(TranscriptLine::ToolCall {
        name: "bash".to_string(),
        args: r#"{"command": "ls /tmp"}"#.to_string(),
        collapsed: false,
    });
    Arc::make_mut(&mut s.transcript_state).lines.push(TranscriptLine::ToolResult {
        ok: true,
        content: "file1\nfile2\nfile3\nfile4\nfile5\nfile6\nfile7\nfile8\nfile9\nfile10".to_string(),
        collapsed: false,
        duration_ms: Some(42),
    });
    Arc::make_mut(&mut s.transcript_state).lines.push(TranscriptLine::AssistantText(
        "Here are 5 files from /tmp:\n1. file1\n2. file2\n3. file3\n4. file4\n5. file5".to_string(),
    ));

    let backend = TestBackend::new(200, 50);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, &s)).unwrap();
    let buf = terminal.backend().buffer().clone();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                print!("{}", c.symbol());
            }
        }
        println!();
    }
}