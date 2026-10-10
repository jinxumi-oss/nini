//! Does a real SIGTERM actually restore the terminal?
//!
//! `signals.rs`'s unit tests prove the state machine: mark, cleanup,
//! clear. They cannot prove that a signal reaches the cleanup - sending
//! SIGTERM to this process would kill it and take every other test with it.
//!
//! So the child half re-executes the test binary, installs the real
//! handlers in it, and the parent signals *that*. The evidence is the
//! alternate-screen escape sequence: `emergency_cleanup` writes it to
//! stdout, the child's stdout is a pipe, so a real signal producing a real
//! restore leaves that byte sequence in the captured output. Combined with
//! exit status 143, that is signal -> handler -> cleanup, end to end.
//!
//! The child never talks to a terminal. It marks raw mode and the alternate
//! screen as the real TUI does before entering them, and cleanup's
//! terminal writes are best-effort by design.
//!
//! An earlier version of this test had the child poll
//! `raw_mode_active()` and print once it went false. That cannot work:
//! `shutdown_from_signal` cleans up and calls `process::exit` in the same
//! breath, so the poller never runs. Asserting on output the signal path
//! itself produces avoids depending on a window that does not exist.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "NINI_SIGNAL_TEST_CHILD";

/// What `emergency_cleanup` writes when it leaves the alternate screen.
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

#[test]
fn a_real_sigterm_restores_the_terminal_and_exits_143() {
    if std::env::var(CHILD_ENV).is_ok() {
        child_body();
    }

    let exe = std::env::current_exe().expect("current test binary");
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "a_real_sigterm_restores_the_terminal_and_exits_143",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("child spawns");

    // Wait until the child reports it is installed and armed. Signalling
    // earlier lands on a process with default handlers and proves nothing
    // about ours.
    let stdout = child.stdout.take().expect("piped");
    let mut reader = BufReader::new(stdout);
    // Not necessarily the first line: the test harness prints its own
    // banner ("running 1 test", the test name) before the child's output
    // reaches the pipe, so read until the marker or a bounded number of
    // lines. Signalling before ARMED would land on a process that has not
    // installed anything yet.
    let mut seen = String::new();
    let mut armed = false;
    for _ in 0..20 {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        seen.push_str(&line);
        if line.contains("ARMED") {
            armed = true;
            break;
        }
    }
    assert!(
        armed,
        "child never reported ready. Its output so far: {seen:?}"
    );

    // `kill` rather than libc::kill: nini-tui does not depend on libc, and
    // this keeps the dependency set unchanged for one line of test code.
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success(), "could not signal the child");

    let mut rest = String::new();
    let _ = reader.read_to_string(&mut rest);
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().expect("child exits");

    assert!(
        rest.contains(LEAVE_ALT_SCREEN),
        "SIGTERM did not reach the terminal cleanup. Child stdout: {:?}\nstderr: {:?}",
        rest,
        err
    );
    assert_eq!(
        status.code(),
        Some(143),
        "expected 128+15 for SIGTERM, got {:?}\nstderr: {err:?}",
        status.code()
    );
}

/// The victim. A separate process, so the signal under test cannot reach
/// the one running the assertions.
fn child_body() -> ! {
    use nini_tui::signals;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        signals::install_handlers().expect("install handlers");
        // Stand in for the TUI, which by this point has entered both.
        signals::mark_raw_mode(true);
        signals::mark_alt_screen(true);
        println!("ARMED");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        // The signal task restores the terminal and exits. Nothing after
        // this line runs, by design.
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
}
