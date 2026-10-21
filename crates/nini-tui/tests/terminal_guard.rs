//! The library must not write to the terminal the TUI is painting.
//! invariant: ca97731b
//!
//! What this is about: the TUI owns the terminal while it runs — raw
//! mode, the alternate screen, a full repaint every tick. The library
//! writes to stderr anyway: `nini_core::project_trust::check` prints a
//! trust warning **on every tool call** (the default trust level is
//! `Ask`), and the agent prints on its error paths. Those lines landed on
//! the alternate screen and interleaved with the repaint, so the user
//! read two different texts fighting over one row:
//!
//! ```text
//!  nini   ⠼[nini] trust: cwd '/home/jin/nini2' is at Ask level; d5f3ulting 6o Allow (v0.8 …)
//! ```
//!
//! (`5` and `3` are the TUI's own `ctx 3%` / `out 662` digits, drawn
//! over the library's text.)
//!
//! The fix is a choke point: while the TUI runs, fd 2 is a pipe, and the
//! lines in it become transcript notices.
//!
//! The tests below drive the guard directly and check the three
//! properties that make it safe to have:
//!
//! 1. a library write reaches the transcript, and **not** the terminal;
//! 2. the real stderr works again once the guard is dropped, and no
//!    message is lost on the way out;
//! 3. a writer that fills the pipe is not blocked — the reader thread
//!    keeps draining, and if it ever stops, fd 2 is handed back so the
//!    process cannot wedge.

use std::os::fd::{AsRawFd, RawFd};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use nini_tui::runtime::{shared_state, SharedState};
use nini_tui::state::{AppState, TranscriptLine};
use nini_tui::terminal_guard::TerminalGuard;

/// One terminal per process: the tests below take fd 2 over, so they must
/// not run at the same time as each other.
static TERMINAL: Mutex<()> = Mutex::new(());
fn terminal() -> MutexGuard<'static, ()> {
    TERMINAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Point the process's stderr at a file, and give the real one back on
/// drop.
///
/// Everything the process writes to stderr while this is alive lands in
/// the file. That is how the test tells "the guard caught it" from "it
/// reached the terminal".
struct StderrToFile {
    file: tempfile::NamedTempFile,
    saved: RawFd,
}

impl StderrToFile {
    fn install() -> StderrToFile {
        let file = tempfile::NamedTempFile::new().expect("temp file for stderr");
        // SAFETY: dup of fd 2. The descriptor is ours, and `Drop` closes
        // it.
        let saved = unsafe { libc::dup(2) };
        assert!(saved >= 0, "could not dup stderr");
        // SAFETY: `file` owns a valid descriptor; `dup2` moves the
        // process's stderr onto it.
        unsafe { libc::dup2(file.as_file().as_raw_fd(), 2) };
        StderrToFile { file, saved }
    }

    /// What reached the real stderr so far.
    fn contents(&self) -> String {
        std::fs::read_to_string(self.file.path()).unwrap_or_default()
    }
}

impl Drop for StderrToFile {
    fn drop(&mut self) {
        // SAFETY: `saved` is a dup of the original stderr, and it is
        // still open.
        unsafe {
            libc::dup2(self.saved, 2);
            libc::close(self.saved);
        }
    }
}

/// Write to fd 2 the way the library does.
///
/// `eprintln!` cannot be used here: the test harness captures the
/// output of the `print!` family at the Rust level, so it never reaches
/// fd 2 at all. The library's writes do reach fd 2 in a real process —
/// that is the whole reason the garbled screen happened — and the guard
/// works at that level, so the test writes there directly.
fn write_to_stderr(msg: &str) {
    let mut bytes = msg.as_bytes().to_vec();
    bytes.push(b'\n');
    // SAFETY: `bytes` is a valid buffer and the length matches it.
    let n = unsafe { libc::write(2, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
    assert_eq!(n, bytes.len() as isize, "could not write to fd 2");
}

/// The notice lines the transcript holds.
fn notices(state: &SharedState) -> Vec<String> {
    let guard = state.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .transcript_state
        .lines
        .iter()
        .filter_map(|line| match line {
            TranscriptLine::Notice(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Wait for a notice to show up. The reader thread is asynchronous, so
/// the test has to give it a moment — but not forever: a missing notice
/// is a failure, not something to wait for.
fn wait_for_notice(state: &SharedState, needle: &str) -> bool {
    for _ in 0..200 {
        if notices(state).iter().any(|n| n == needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn a_library_write_to_stderr_becomes_a_transcript_notice() {
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let guard = TerminalGuard::install(state.clone()).expect("install the guard");

    // The line that produced the garbled screen, verbatim.
    let line = "[nini] trust: cwd '/home/jin/nini2' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)";
    write_to_stderr(line);

    assert!(
        wait_for_notice(&state, line),
        "the library's stderr never reached the transcript: {:?}",
        notices(&state)
    );
    assert!(
        !stderr.contents().contains("defaulting to Allow"),
        "the line also reached the terminal, which is the bug this guard exists to fix:\n{}",
        stderr.contents()
    );

    drop(guard);
}

#[test]
fn the_real_stderr_works_again_after_the_guard_is_dropped() {
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));

    {
        let guard = TerminalGuard::install(state.clone()).expect("install the guard");
        write_to_stderr("caught by the guard");
        assert!(
            wait_for_notice(&state, "caught by the guard"),
            "the line never reached the transcript: {:?}",
            notices(&state)
        );
        drop(guard);
    }

    write_to_stderr("after the guard");
    let seen = stderr.contents();
    assert!(
        seen.contains("after the guard"),
        "fd 2 was not handed back when the guard was dropped:\n{seen}"
    );
    assert!(
        !seen.contains("caught by the guard"),
        "the line was written to the real stderr as well, so the user would \
         see it twice:\n{seen}"
    );
}

#[test]
fn a_writer_that_fills_the_pipe_is_not_blocked() {
    let _terminal = terminal();
    let _stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let _guard = TerminalGuard::install(state.clone()).expect("install the guard");

    // A pipe holds 64 KiB on Linux. This writes four times that, so the
    // writer can only finish if the reader thread keeps draining.
    let (tx, rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let payload = "x".repeat(1024);
        for _ in 0..256 {
            write_to_stderr(&payload);
        }
        let _ = tx.send(());
    });

    assert!(
        rx.recv_timeout(Duration::from_secs(30)).is_ok(),
        "the writer blocked on a full pipe: the reader thread is not draining"
    );
    writer.join().ok();
}

#[test]
fn a_notice_is_a_row_in_the_transcript() {
    // The guard is not the only thing that has to change: the notice has
    // to be *visible*. A system line that the reader cannot see has not
    // been fixed, it has been hidden.
    let mut state = AppState::new("test-model");
    state.push_notice("hello from the library");

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|f| nini_tui::render::render_frame(f, &state))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let frame: String = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        frame.contains("hello from the library"),
        "the notice is not on screen:\n{frame}"
    );
}
