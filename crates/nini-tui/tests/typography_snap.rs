//! Typography snapshot harness: renders the nini TUI to text so we can
//! eyeball the new Pi-style 2-line footer alongside the markdown/prompt
//! rendering. Snapshots go to /tmp/nini-snapshots/.

use nini_tui::render::render_frame_with_theme;
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use nini_tui::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::fs;
use std::path::Path;

fn frame_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    let area = buffer.area;
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            } else {
                line.push(' ');
            }
        }
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out
}

fn render_to_file(state: &AppState, width: u16, height: u16, path: &Path) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_frame_with_theme(f, state, &Theme::dark()))
        .unwrap();
    let text = frame_text(&terminal);
    fs::write(path, text).unwrap();
    println!("wrote {}", path.display());
}

#[test]
fn snapshot_empty_state() {
    let out_dir = Path::new("/tmp/nini-snapshots-v2");
    fs::create_dir_all(out_dir).unwrap();

    let mut state = AppState::new("MiniMax-M3");
    state.model_state.provider = Some("anthropic".into());
    state.model_state.thinking_level = Some("medium".into());
    state.session_state.cwd = Some(std::path::PathBuf::from("/home/jin/nini"));
    state.session_state.git_branch = Some("main".into());
    state.session_state.session_id = Some("a1b2c3d4e5f6".into());

    render_to_file(&state, 120, 30, &out_dir.join("01_empty_120x30.txt"));
    render_to_file(&state, 80, 24, &out_dir.join("02_empty_80x24.txt"));
    render_to_file(&state, 200, 40, &out_dir.join("03_empty_200x40.txt"));
}

#[test]
fn snapshot_conversation() {
    let out_dir = Path::new("/tmp/nini-snapshots-v2");
    fs::create_dir_all(out_dir).unwrap();

    let mut state = AppState::new("MiniMax-M3");
    state.model_state.provider = Some("anthropic".into());
    state.model_state.thinking_level = Some("high".into());
    state.session_state.cwd = Some(std::path::PathBuf::from("/home/jin/nini"));
    state.session_state.git_branch = Some("main".into());

    state.push_user("Please refactor the auth middleware to use JWT instead of session cookies.");
    state.push_divider();
    state.transcript_state.lines.push(TranscriptLine::ThinkingText(
        "The user wants to migrate from session cookies to JWT. I should first read the current middleware, identify the session helpers, and propose a JWT strategy with refresh-token rotation.".into(),
    ));
    state.push_assistant(
        "I'll read the auth middleware first, then propose a JWT plan.\n\n\
         # Plan\n\n\
         1. Survey current session usage\n\
         2. Pick a JWT library (`jsonwebtoken`)\n\
         3. Replace session reads with JWT verification\n\
         4. Add refresh-token rotation\n\n\
         Here is the diff:\n\n\
         ```rust\n\
         fn verify(token: &str) -> Result<Claims, Error> {\n\
             decode::<Claims>(\n\
                 token,\n\
                 &DecodingKey::from_secret(SECRET),\n\
                 &Validation::default(),\n\
             )\n\
             .map(|d| d.claims)\n\
         }\n\
         ```\n\n\
         See [docs](https://docs.rs/jsonwebtoken) for the full API."
            .to_string(),
    );
    state.push_divider();
    state.push_tool_call("read", "{\"path\": \"src/middleware/auth.rs\"}");
    state.push_tool_result(true, "use actix_web::*;\n\npub async fn auth(req: ServiceRequest) -> ...", Some(23));
    state.push_divider();
    state.transcript_state.lines.push(TranscriptLine::BashExecution {
        id: "abc123".into(),
        cmd: "cargo build".into(),
        output: "Compiling auth v0.1.0\nFinished release [optimized] in 4.5s".into(),
        stderr: "warning: unused variable `x`".into(),
        ok: false,
        exit_code: Some(101),
        duration_ms: 4_521,
        collapsed: false,
    });
    state.push_divider();
    state.push_user("Looks good. Run the tests too.");
    state.push_assistant("Running the test suite now.");

    render_to_file(&state, 120, 40, &out_dir.join("04_conversation_120x40.txt"));
    render_to_file(&state, 80, 24, &out_dir.join("05_conversation_80x24.txt"));
}

#[test]
fn snapshot_running_state() {
    let out_dir = Path::new("/tmp/nini-snapshots-v2");
    fs::create_dir_all(out_dir).unwrap();

    let mut state = AppState::new("MiniMax-M3");
    state.model_state.provider = Some("openai".into());
    state.session_state.cwd = Some(std::path::PathBuf::from("/home/jin/nini"));
    state.session_state.git_branch = Some("feat/tui-typography".into());
    state.session_state.session_id = Some("abc12345".into());
    state.run_state.mode = RunMode::Running;
    state.run_state.context_window = 200_000;
    state.run_state.context_used = 80_000;
    state.run_state.tokens.input = 12_345;
    state.run_state.tokens.output = 4_567;
    state.run_state.cost_usd = 0.0421;

    state.push_user("summarize the auth module");
    state.push_assistant("Reading the file structure…\n\n- auth.rs\n- jwt.rs\n- session.rs");

    render_to_file(&state, 120, 30, &out_dir.join("06_running_120x30.txt"));
}

#[test]
fn snapshot_thinking_visible() {
    let out_dir = Path::new("/tmp/nini-snapshots-v2");
    fs::create_dir_all(out_dir).unwrap();

    let mut state = AppState::new("MiniMax-M3");
    state.model_state.provider = Some("anthropic".into());
    state.model_state.thinking_level = Some("max".into());
    state.session_state.cwd = Some(std::path::PathBuf::from("/home/jin/nini"));
    state.session_state.git_branch = Some("main".into());
    state.session_state.session_id = Some("xyz98765".into());

    state.push_user("explain monads in 3 sentences");
    state.transcript_state.lines.push(TranscriptLine::ThinkingText(
        "Monads are wrappers around values that compose sequential operations while handling effects (state, errors, I/O) in a pure functional style.".into(),
    ));
    state.push_assistant(
        "A monad is a triple (M, return, >>=) satisfying three laws: left identity, right identity, and associativity. \
         In Rust, `Option<T>` is the simplest monad — `return` is `Some` and `>>=` is `and_then`. \
         They're useful when you want to chain operations that may fail or produce side effects without leaving pure code."
            .to_string(),
    );

    render_to_file(&state, 100, 24, &out_dir.join("07_thinking_100x24.txt"));
}