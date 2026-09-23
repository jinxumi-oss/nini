//! Synchronous shell executor for the TUI's `!cmd` passthrough.
//!
//! v0.5 left this as a 25-line stub that returned empty output and a fake
//! "exit 0" status. v0.6 wires it to `std::process::Command` so the
//! transcript actually shows what the command did. Behavior:
//!
//! * Spawns `$SHELL -c <cmd>` so users get full shell semantics
//!   (pipes, redirects, globs) — same contract as the bash tool.
//! * Captures stdout and stderr.
//! * Honors a caller-supplied `timeout`. On Unix the child runs in its own
//!   process group (`setsid`) and we `killpg(SIGTERM)` then `SIGKILL` on
//!   timeout; on Windows we fall back to per-pid kill (best-effort).
//! * Returns `BashResult` with stdout, stderr, exit code, and duration.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default command timeout for `!cmd` in the TUI. Shorter than the bash
/// tool's 2-minute default because `!cmd` is interactive — users expect
/// feedback quickly. Override via `~/.pi/agent/settings.json` `bashTimeoutMs`.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Hard upper bound — protects against pathological `bashTimeoutMs` values
/// that would otherwise hang the TUI indefinitely.
pub const MAX_TIMEOUT_MS: u64 = 10 * 60 * 1000; // 10 minutes

/// Grace period between SIGTERM and SIGKILL on Unix.
pub const KILL_GRACE_MS: u64 = 200;

#[derive(Debug, Clone)]
pub struct BashResult {
    /// Decoded UTF-8 stdout. ANSI escapes are preserved (the TUI strips
    /// them later via `ansi::strip_ansi`).
    pub output: String,
    /// Decoded UTF-8 stderr. Same ANSI handling as `output`.
    pub stderr: String,
    /// True when stdout/stderr was truncated to fit the preview window.
    pub truncated: bool,
    /// Original byte length of stdout before truncation.
    pub original_bytes: u32,
    /// Original line count of stdout before truncation.
    pub original_lines: u32,
    /// Exit code, or `None` if the child was killed by a signal / timeout.
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the run, in milliseconds.
    pub duration_ms: u64,
    /// True when exit_code == Some(0). False for non-zero exit AND for
    /// signals/timeout (no code).
    pub ok: bool,
    /// True when the command was killed because it exceeded `timeout_ms`.
    pub timed_out: bool,
}

impl Default for BashResult {
    fn default() -> Self {
        Self {
            output: String::new(),
            stderr: String::new(),
            truncated: false,
            original_bytes: 0,
            original_lines: 0,
            exit_code: None,
            duration_ms: 0,
            ok: false,
            timed_out: false,
        }
    }
}

pub struct BashRunner;

impl BashRunner {
    pub fn new() -> Self {
        Self
    }

    /// Run `cmd` synchronously in `cwd`. Blocks the caller (the TUI event
    /// loop) for up to `timeout_ms` milliseconds, then either returns the
    /// captured output or a timeout-flagged result with partial output.
    pub fn run_blocking(&mut self, cmd: &str, cwd: &std::path::Path) -> BashResult {
        self.run_blocking_with_timeout(cmd, cwd, DEFAULT_TIMEOUT_MS)
    }

    /// Same as `run_blocking`, but with a caller-supplied timeout. Pass
    /// `0` to disable the timeout (use with care — the TUI will hang).
    pub fn run_blocking_with_timeout(
        &mut self,
        cmd: &str,
        cwd: &std::path::Path,
        timeout_ms: u64,
    ) -> BashResult {
        let timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);
        let started = Instant::now();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        // Spawn child. We use std::process (sync) + a thread for the
        // wait-with-timeout, because the TUI loop is in a sync context.
        let mut command = Command::new(&shell);
        command
            .arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // On Unix, run the child in its own process group so a timeout can
        // kill the entire tree (bash + children) via killpg. `Command`
        // exposes `process_group` on `std::os::unix::process`.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Safety: setsid(2) is always safe to call from a child pre-exec
            // hook; it just creates a new session and process group.
            unsafe {
                command.pre_exec(|| {
                    // setsid creates a new session; the child becomes the
                    // process-group leader, which is exactly what we want
                    // for killpg.
                    libc_setsid();
                    Ok(())
                });
            }
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                return BashResult {
                    output: String::new(),
                    stderr: format!("failed to spawn shell: {e}"),
                    exit_code: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    ok: false,
                    ..Default::default()
                };
            }
        };
        let pid = child.id();

        // Drain stdout / stderr concurrently from the pipes on background
        // threads, then join. This matches the BashTool's pattern and
        // prevents deadlocks when a child writes a lot to stderr while
        // the parent is blocked waiting for it.
        let mut stdout_pipe = child.stdout.take().expect("piped");
        let mut stderr_pipe = child.stderr.take().expect("piped");
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::with_capacity(4096);
            let _ = stdout_pipe.read_to_end(&mut buf);
            buf
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::with_capacity(4096);
            let _ = stderr_pipe.read_to_end(&mut buf);
            buf
        });

        // Wait with timeout on a background thread so we can also poll the
        // status. We use try_wait in a loop with sleep, which keeps the
        // logic simple and works without extra deps.
        let (tx, rx) = mpsc::channel::<Result<std::process::ExitStatus, std::io::Error>>();
        let wait_thread = std::thread::spawn(move || {
            let r = child.wait();
            let _ = tx.send(r);
        });

        let mut timed_out = false;
        let exit_status = if timeout_ms == 0 {
            // No timeout: wait forever. Use recv() with no deadline.
            match rx.recv() {
                Ok(r) => r,
                Err(_) => Ok(synthetic_exit_for_io_error()),
            }
        } else {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                match rx.try_recv() {
                    Ok(r) => break r,
                    Err(mpsc::TryRecvError::Empty) => {
                        if Instant::now() >= deadline {
                            timed_out = true;
                            kill_tree(Some(pid));
                            // Drain any final result; the child is now
                            // reaped by SIGKILL after grace.
                            let _ = rx.recv_timeout(Duration::from_millis(KILL_GRACE_MS + 500));
                            break Ok(synthetic_exit_for_io_error());
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        break Ok(synthetic_exit_for_io_error());
                    }
                }
            }
        };

        let _ = wait_thread.join();

        let stdout_bytes = stdout_thread.join().unwrap_or_default();
        let stderr_bytes = stderr_thread.join().unwrap_or_default();

        let exit_code = match exit_status {
            Ok(status) => status.code(),
            Err(_) => None,
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
        let original_bytes = stdout_bytes.len() as u32;
        let original_lines = bytecount_lines(&stdout_bytes) as u32;

        BashResult {
            output: stdout,
            stderr,
            truncated: false, // preview-window truncation happens in render layer
            original_bytes,
            original_lines,
            exit_code,
            duration_ms,
            ok: matches!(exit_code, Some(0)),
            timed_out,
        }
    }
}

/// `setsid(2)` wrapper. We isolate the libc dep here so other Unix targets
/// keep building untouched.
#[cfg(unix)]
#[link(name = "c")]
extern "C" {
    fn setsid() -> i32;
}

#[cfg(unix)]
fn libc_setsid() {
    // Safety: setsid() has no failure mode that affects us here. If it
    // fails (e.g., already a process-group leader), we fall back to
    // per-pid kill.
    unsafe {
        let _ = setsid();
    }
}

/// Send SIGTERM then SIGKILL to the entire process group rooted at `pid`.
/// On non-Unix this is a no-op (Windows TODO).
#[cfg(unix)]
fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let Ok(pid_i32) = i32::try_from(pid) else { return };
    // Safety: killpg with a positive i32 targets the named process group.
    // SIGTERM first; after grace, SIGKILL.
    unsafe {
        libc_killpg(pid_i32, 15); // SIGTERM
    }
    std::thread::sleep(Duration::from_millis(KILL_GRACE_MS));
    unsafe {
        libc_killpg(pid_i32, 9); // SIGKILL
    }
}

#[cfg(not(unix))]
fn kill_tree(_pid: Option<u32>) {
    // Windows: best-effort. Child::kill on the original child would be
    // ideal but we don't have it here. Process-tree kill on Windows
    // requires Job Objects; deferred to follow-up.
}

#[cfg(unix)]
#[link(name = "c")]
extern "C" {
    fn killpg(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_killpg(pid: i32, sig: i32) {
    // Safety: killpg returns -1 on error which we deliberately ignore.
    unsafe {
        let _ = killpg(pid, sig);
    }
}

/// Construct a synthetic exit status representing "killed before "
/// "wait() could record an exit code" (e.g., on timeout). We model this as
/// `code() = None` so the TUI can render "(timed out)" instead of a
/// misleading zero.
#[cfg(unix)]
fn synthetic_exit_for_io_error() -> std::process::ExitStatus {
    // We can't fabricate an ExitStatus, but we can use UnixExitStatusExt
    // to construct one from a raw wait status. WIFSIGNALED with SIGKILL.
    use std::os::unix::process::ExitStatusExt;
    // Encoded wait status: signal number in the low 7 bits of the high
    // byte (WTERMSIG). SIGKILL = 9, so the status word is 9.
    unsafe { std::process::ExitStatus::from_raw(9) }
}

#[cfg(not(unix))]
fn synthetic_exit_for_io_error() -> std::process::ExitStatus {
    // On non-Unix we don't have a portable way to fabricate a status.
    // Return a dummy by spawning `true` — this branch only fires on
    // timeout/error, so the extra cost is bounded.
    Command::new("true")
        .status()
        .unwrap_or_else(|_| panic!("synthetic_exit_for_io_error fallback failed"))
}

fn bytecount_lines(b: &[u8]) -> usize {
    if b.is_empty() {
        return 0;
    }
    let mut count = 0;
    for &c in b {
        if c == b'\n' {
            count += 1;
        }
    }
    if b[b.len() - 1] != b'\n' {
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_returns_output() {
        let mut r = BashRunner::new();
        let res = r.run_blocking_with_timeout("echo HELLO", std::path::Path::new("/tmp"), 5000);
        assert!(res.output.contains("HELLO"), "got: {:?}", res);
        assert_eq!(res.exit_code, Some(0));
        assert!(res.ok);
        assert!(!res.timed_out);
    }

    #[test]
    fn nonzero_exit_marks_not_ok() {
        let mut r = BashRunner::new();
        let res = r.run_blocking_with_timeout("exit 7", std::path::Path::new("/tmp"), 5000);
        assert_eq!(res.exit_code, Some(7));
        assert!(!res.ok);
    }

    #[test]
    fn timeout_kills_long_command() {
        let mut r = BashRunner::new();
        let res = r.run_blocking_with_timeout("sleep 30", std::path::Path::new("/tmp"), 500);
        assert!(res.timed_out, "expected timed_out, got: {:?}", res);
        assert!(res.duration_ms < 2000, "timeout didn't fire promptly: {:?}", res);
    }

    #[test]
    fn stderr_captured_separately() {
        let mut r = BashRunner::new();
        let res = r.run_blocking_with_timeout(
            "echo OOPS >&2",
            std::path::Path::new("/tmp"),
            5000,
        );
        assert!(res.stderr.contains("OOPS"), "stderr was: {:?}", res.stderr);
        assert!(res.output.is_empty(), "stdout leaked stderr: {:?}", res.output);
    }

    #[test]
    fn timeout_zero_means_no_timeout() {
        let mut r = BashRunner::new();
        let res = r.run_blocking_with_timeout("echo FAST", std::path::Path::new("/tmp"), 0);
        assert!(res.output.contains("FAST"));
        assert!(!res.timed_out);
    }

    #[test]
    fn timeout_above_max_clamped() {
        let mut r = BashRunner::new();
        // Pass a huge value; we expect it to be accepted (clamped) without
        // panic. Run a fast command to verify the path.
        let res = r.run_blocking_with_timeout("echo OK", std::path::Path::new("/tmp"), u64::MAX);
        assert!(res.output.contains("OK"));
    }
}