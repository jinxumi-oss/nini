//! The library must not write to the terminal the TUI is painting.
//! invariant: ca97731b
//! invariant: 72dafe4a
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
//! The tests below drive the guard directly and check the properties
//! that make it safe to have:
//!
//! 1. a library write reaches the transcript, and **not** the terminal;
//! 2. the real stderr works again once the guard is dropped, and no
//!    message is lost on the way out;
//! 3. a writer that fills the pipe is not blocked — the reader thread
//!    keeps draining, and if it ever stops, fd 2 is handed back so the
//!    process cannot wedge;
//! 4. the guard's **exit** does not wait for the pipe to reach EOF —
//!    a child that inherited the write end keeps it open, and waiting
//!    for EOF hung the exit (Ctrl+D never returned the shell);
//! 5. the reader thread never waits for the render loop's lock — a
//!    blocked reader blocks every writer, and the pipe is finite;
//! 6. a failed install does not poison the process-global slot — the
//!    symptom of a poisoned slot is the original bug: the library
//!    writing to the screen the TUI is painting.

use std::os::fd::{AsRawFd, RawFd};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
/// Move the guard's pending lines into the transcript, the way the
/// render loop does on every tick.
///
/// The reader thread does not write to the transcript itself — it hands
/// the lines over through the guard's own queue, so it never takes the
/// render loop's state lock. The runtime drains that queue here.
fn drain_into(guard: &TerminalGuard, state: &SharedState) {
    for line in guard.take_pending() {
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_notice(line);
    }
}

/// Drain the guard's queue into the transcript until `needle` shows up
/// (or the deadline passes). The render loop does this on every tick; a
/// test has to do it too.
fn wait_for_drained_notice(guard: &TerminalGuard, state: &SharedState, needle: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        drain_into(guard, state);
        if notices(state).iter().any(|n| n == needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

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
    let guard = TerminalGuard::install().expect("install the guard");

    // The line that produced the garbled screen, verbatim.
    let line = "[nini] trust: cwd '/home/jin/nini2' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)";
    write_to_stderr(line);

    // The render loop drains the guard's queue; a test has to do the
    // same thing.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        drain_into(&guard, &state);
        if notices(&state).iter().any(|n| n == line) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

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
        let guard = TerminalGuard::install().expect("install the guard");
        write_to_stderr("caught by the guard");
        assert!(
            wait_for_drained_notice(&guard, &state, "caught by the guard"),
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
    let _state = shared_state(AppState::new("test-model"));
    let _guard = TerminalGuard::install().expect("install the guard");

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

#[test]
fn dropping_the_guard_does_not_wait_for_a_child_that_inherited_the_pipe() {
    // The exit must not depend on the pipe reaching EOF. fd 2 is a
    // pipe while the guard is installed, and **every child process
    // inherits it** — the external editor is the one that matters: a
    // daemonising editor (`$EDITOR=code` without `--wait`,
    // `emacsclient`, `gvim -f`) keeps the write end open for as long
    // as it lives, which is forever.
    //
    // A guard that waits for EOF therefore never returns: the TUI has
    // already left the alternate screen, and the shell prompt never
    // comes back. Ctrl+D hung.
    let _terminal = terminal();
    let _stderr = StderrToFile::install();
    let _state = shared_state(AppState::new("test-model"));
    let guard = TerminalGuard::install().expect("install the guard");

    // A child that inherits fd 2 (the pipe's write end) and keeps it
    // open. It is never waited for.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn the child that inherits the pipe");

    let started = Instant::now();
    drop(guard);
    let elapsed = started.elapsed();

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        elapsed < Duration::from_secs(2),
        "dropping the guard waited {elapsed:?} for a child that inherited the \
         pipe — the exit must be driven by a stop signal, not by EOF"
    );
}

#[test]
fn a_failed_install_does_not_poison_the_global_slot() {
    // The guard keeps the saved stderr in a process-global slot, so the
    // panic hook can hand the real stderr back. A failed install that
    // left a **stale** descriptor in that slot would (a) refuse every
    // later install — the TUI would run with no guard at all, which is
    // the original bug — and (b) point fd 2 at whatever that number
    // means by then, when the panic hook fires.
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let _state = shared_state(AppState::new("test-model"));

    // Make the install fail at the last step — after the descriptor has
    // been published, which is where the rollback has to happen.
    nini_tui::terminal_guard::fail_next_install_after_publish();
    let failed = TerminalGuard::install();
    assert!(
        failed.is_err(),
        "the simulated failure did not happen: the install succeeded"
    );
    drop(failed);

    // The slot must be free again …
    let guard = TerminalGuard::install().expect("the failed install poisoned the global slot");
    drop(guard);

    // … and fd 2 must still be the real terminal, not a stale number.
    write_to_stderr("after the failed install");
    let seen = stderr.contents();
    assert!(
        seen.contains("after the failed install"),
        "fd 2 does not point at the real terminal after a failed install:\n{seen}"
    );
}

#[test]
fn a_notice_is_never_lost_when_the_state_is_locked() {
    // The reader thread and the render loop share the state. If the
    // reader *waited* for the lock, a writer that filled the pipe
    // would block — and the pipe is finite, so the whole process would
    // wedge. The reader must never wait: the line goes to the residual
    // instead, and the residual is written to the real stderr when the
    // guard is dropped.
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let guard = TerminalGuard::install().expect("install the guard");

    // Hold the lock, the way the render loop does while it runs a
    // command.
    let held = state.lock().unwrap_or_else(|e| e.into_inner());

    // A writer that fills the pipe several times over. It can only
    // finish if the reader keeps draining while the state is locked.
    let (tx, rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let payload = "y".repeat(1024);
        for _ in 0..256 {
            write_to_stderr(&payload);
        }
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(10)).is_ok(),
        "the writer blocked on a full pipe while the state was locked: the \
         reader thread is waiting for the render loop's lock"
    );
    writer.join().ok();

    // A line that is recognisable, written while the state is locked.
    write_to_stderr("written while the state was locked");
    // Give the reader time to pick it up while the lock is still held:
    // it cannot deliver it (the state is locked), so it has to keep it.
    std::thread::sleep(Duration::from_millis(300));
    drop(held);

    // The line was not delivered to the transcript (the state was
    // locked) — but it must not be lost either: the reader hands it to
    // the guard's own queue, which the render loop drains, and whatever
    // is still in the queue at the end goes to the real stderr.
    let in_queue = guard
        .take_pending()
        .iter()
        .any(|l| l == "written while the state was locked");
    let mut in_transcript = false;
    if let Some(g) = Some(&guard) {
        in_transcript = wait_for_drained_notice(g, &state, "written while the state was locked");
    }
    drop(guard);
    let seen = stderr.contents();
    assert!(
        seen.contains("written while the state was locked") || in_transcript || in_queue,
        "the line was lost: the reader did not deliver it, the queue did \
         not hold it, and the residual did not give it back:\n{seen}"
    );
}

#[test]
fn a_line_written_after_the_shutdown_begins_goes_to_the_real_stderr() {
    // The transcript disappears with the alternate screen. A line
    // delivered *after* the screen is gone would be delivered to a
    // screen that no longer exists — which is the same as losing it.
    // The runtime therefore calls `begin_shutdown()` before it leaves
    // the alternate screen.
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let guard = TerminalGuard::install().expect("install the guard");

    guard.begin_shutdown();
    write_to_stderr("after the shutdown began");

    // Give the reader a moment to pick the line up.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !notices(&state)
            .iter()
            .any(|n| n == "after the shutdown began"),
        "the line was delivered to a transcript that is about to disappear: {:?}",
        notices(&state)
    );

    drop(guard);
    let seen = stderr.contents();
    assert!(
        seen.contains("after the shutdown began"),
        "the line was swallowed: it reached neither the transcript nor the \
         real stderr:\n{seen}"
    );
}

#[test]
fn the_editors_stderr_is_not_swallowed_while_the_tui_is_suspended() {
    // The external editor runs in the user's **real** terminal, not in
    // the TUI's. The guard would swallow `vim: error reading ...` into
    // a transcript that is not on screen at that moment, so the user
    // would never see the error. The runtime therefore hands the real
    // stderr back before the editor runs.
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let _guard = TerminalGuard::install().expect("install the guard");

    nini_tui::terminal_guard::release_stderr();
    write_to_stderr("vim: error reading stdin");

    std::thread::sleep(Duration::from_millis(200));
    let seen = stderr.contents();
    assert!(
        seen.contains("vim: error reading stdin"),
        "the editor's error never reached the real terminal:\n{seen}"
    );
    assert!(
        !notices(&state)
            .iter()
            .any(|n| n == "vim: error reading stdin"),
        "the editor's error was swallowed into a transcript that is not on \
         screen: {:?}",
        notices(&state)
    );
}

#[test]
fn the_guard_comes_back_after_the_external_editor() {
    // The external editor is a hole in the TUI's ownership of the
    // terminal: the editor runs in the user's real terminal, and the
    // guard is taken down for its duration. When the editor is done,
    // the TUI paints again — so the guard has to be back, or the
    // library's stderr lands on the frame.
    let _terminal = terminal();
    let stderr = StderrToFile::install();
    let state = shared_state(AppState::new("test-model"));
    let mut guard = Some(TerminalGuard::install().expect("install the guard"));

    // The editor runs: the guard is down, the editor's error is
    // visible in the real terminal.
    nini_tui::terminal_guard::suspend_for_external_editor(&mut guard);
    write_to_stderr("vim: error reading stdin");
    assert!(
        stderr.contents().contains("vim: error reading stdin"),
        "the editor's error did not reach the real terminal:\n{}",
        stderr.contents()
    );

    // The editor is done: the guard is back, and the library's stderr
    // goes to the transcript again.
    nini_tui::terminal_guard::resume_after_external_editor(&mut guard)
        .expect("reinstall the guard");
    write_to_stderr("after the editor");
    let guard_ref = guard.as_ref().expect("the guard must be back");
    assert!(
        wait_for_drained_notice(guard_ref, &state, "after the editor"),
        "the guard did not come back: the library's stderr would land on the \
         frame the TUI is painting: {:?}",
        notices(&state)
    );
}

/// The guard must stop delivering to the transcript **before** the
/// alternate screen goes away.
///
/// The transcript is drawn on the alternate screen, so a line delivered
/// after `LeaveAlternateScreen` goes to a screen that no longer exists
/// — which is the same as losing it. The order is a fact about two
/// statements in `run()`, and a fact like that needs an assertion: the
/// call was once missing, and nothing noticed.
#[test]
fn the_guard_stops_delivering_before_the_screen_goes_away() {
    let runtime = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime.rs"),
    )
    .expect("runtime.rs");

    // The body of `run()` — the function that owns the screen's
    // lifetime. The editor dance has a LeaveAlternateScreen of its
    // own, in a different function, and comparing across functions
    // would compare the wrong two things.
    let start = runtime
        .find("pub async fn run<F>")
        .expect("runtime.rs must have run()");
    let body = &runtime[start..];
    let end = body.find("\nasync fn run_loop").unwrap_or(body.len());
    let body = &body[..end];

    let shutdown = body
        .find("begin_shutdown()")
        .expect("run() must call begin_shutdown()");
    let leave = body
        .find("LeaveAlternateScreen")
        .expect("run() must leave the alternate screen");

    assert!(
        shutdown < leave,
        "the guard keeps delivering to the transcript until the alternate screen is \
         gone (begin_shutdown at {shutdown}, LeaveAlternateScreen at {leave}): every \
         line delivered in between goes to a screen that no longer exists"
    );
}
