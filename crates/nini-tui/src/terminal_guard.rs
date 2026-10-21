//! Keep the library's stderr out of the screen the TUI is painting.
//!
//! # What this is for
//!
//! The TUI owns the terminal while it runs: raw mode, the alternate
//! screen, and a full-screen repaint on every tick. Anything else that
//! writes to that terminal lands **on top of** the painted frame.
//!
//! The library does write. `nini_core::project_trust::check` prints
//!
//! ```text
//! [nini] trust: cwd '/…' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)
//! ```
//!
//! on **every tool call** (the default trust level is `Ask` when there is
//! no `trust.json`), and the agent prints on its error paths too. Those
//! lines went straight to fd 2 — the alternate screen — and interleaved
//! with the repaint, so the user saw two different texts occupying the
//! same row:
//!
//! ```text
//!  nini   ⠼[nini] trust: cwd '/home/jin/nini2' is at Ask level; d5f3ulting 6o Allow (v0.8 …)
//! ```
//!
//! (`5`/`3` are the TUI's own `ctx 3%` / `out 662` digits, drawn over
//! the library's text.)
//!
//! # The fix
//!
//! One choke point, at the only place that owns the terminal: while the
//! TUI runs, fd 2 is a **pipe**, not a terminal. A background thread
//! reads it line by line and turns each line into a
//! [`TranscriptLine::Notice`](crate::state::TranscriptLine::Notice) in
//! the transcript — where it can be read, scrolled and exported like
//! anything else, and where it is unmistakably not model output.
//!
//! Three properties are not negotiable:
//!
//! * **Nothing is lost.** Every line the guard reads reaches the
//!   transcript. Lines still in the pipe when the guard is dropped go
//!   back to the *real* stderr, so a message is never silently
//!   swallowed.
//! * **Nothing deadlocks.** The pipe has a finite buffer. If the reader
//!   thread ever stops, the next `eprintln!` would block the whole
//!   process — so the thread restores fd 2 the moment it ends, for any
//!   reason, including a panic.
//! * **stdout is not touched.** The renderer writes to stdout; changing
//!   stdout's meaning would break the frame, not fix it.
//!
//! # Lifetime
//!
//! The guard is installed in [`crate::runtime::run`] after raw mode is on
//! and before the alternate screen is entered, and dropped when the
//! event loop returns. The panic hook calls [`release_stderr`] first, so
//! a crash report still reaches the user's terminal.

use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::runtime::SharedState;

/// The descriptor the guard takes over: stderr.
const STDERR_FD: RawFd = 2;

/// The saved stderr while a guard is installed, or `-1`.
///
/// Process-global on purpose: the panic hook has no handle to the guard
/// (the guard lives in `run`'s stack frame, and a panic may unwind
/// through a task that never touches it), but it must be able to hand
/// the real stderr back before the crash report is printed.
static SAVED_STDERR: AtomicI32 = AtomicI32::new(-1);

/// Restore fd 2 to the terminal it had before the guard was installed.
///
/// Safe to call at any time, and safe to call twice. The panic hook
/// calls it so that a crash report is not swallowed by the pipe the
/// guard installed.
pub fn release_stderr() {
    let saved = SAVED_STDERR.load(Ordering::SeqCst);
    if saved >= 0 {
        // SAFETY: `saved` is a dup of the original stderr, owned by the
        // guard for as long as it is registered. `dup2` onto fd 2 is
        // what "restore" means, and it is atomic with respect to other
        // writers of the descriptor table.
        unsafe {
            libc::dup2(saved, STDERR_FD);
        }
    }
}

/// Puts fd 2 back where it was, exactly once, whoever gets there first.
struct Restore {
    saved: RawFd,
    done: AtomicBool,
}

impl Restore {
    fn new(saved: RawFd) -> Self {
        Restore {
            saved,
            done: AtomicBool::new(false),
        }
    }

    /// Idempotent: the guard's `Drop` and the reader thread both call
    /// it, and only the first one does the `dup2`.
    fn restore(&self) {
        if !self.done.swap(true, Ordering::SeqCst) {
            // SAFETY: `self.saved` is a valid, owned dup of the original
            // stderr; it stays alive for as long as this `Restore` does.
            unsafe {
                libc::dup2(self.saved, STDERR_FD);
            }
        }
    }
}

/// Restores fd 2 when the reader thread ends, for any reason.
///
/// Without this, a reader thread that died (a panic, an I/O error)
/// would leave fd 2 pointing at a pipe nobody reads: the pipe fills up,
/// and the next `eprintln!` blocks the process forever.
struct RestoreOnExit {
    restore: Arc<Restore>,
}

impl Drop for RestoreOnExit {
    fn drop(&mut self) {
        self.restore.restore();
    }
}

/// The guard: fd 2 is a pipe, and a thread turns its lines into
/// transcript notices.
///
/// Dropping it restores fd 2, stops the reader thread, and writes back
/// the lines that were still in the pipe.
pub struct TerminalGuard {
    /// The original stderr. Writing to it reaches the real terminal
    /// even while fd 2 points at the pipe.
    saved: OwnedFd,
    /// The write end of the pipe. Held so the reader only sees EOF when
    /// the guard is dropped; fd 2 is a second reference to it.
    pipe_write: Option<OwnedFd>,
    /// The reader thread.
    reader: Option<JoinHandle<()>>,
    /// Shared with the reader thread.
    restore: Arc<Restore>,
    /// Lines that were still in the pipe when the guard was dropped.
    residual: Arc<Mutex<Vec<String>>>,
    /// Set when the guard starts shutting down: from then on, the
    /// reader stops delivering to the transcript and keeps the lines
    /// for the real stderr instead.
    closing: Arc<AtomicBool>,
}

impl TerminalGuard {
    /// Take over stderr for as long as the guard is alive.
    ///
    /// Every step that can fail restores fd 2 before returning, so a
    /// failed install leaves the process exactly as it was — a TUI that
    /// cannot install the guard is still a TUI, just a noisier one.
    pub fn install(state: SharedState) -> io::Result<TerminalGuard> {
        if SAVED_STDERR.load(Ordering::SeqCst) >= 0 {
            // Two guards would fight over fd 2: the second one would
            // pipe the pipe. Refuse instead of corrupting it.
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a terminal guard is already installed",
            ));
        }

        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` is a two-element array, as `pipe` requires.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `pipe` initialised both descriptors; taking ownership
        // of them is the only way they get closed.
        let read_end = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_end = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        // SAFETY: dup of fd 2; on failure nothing has been changed yet.
        let saved_raw = unsafe { libc::dup(STDERR_FD) };
        if saved_raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `saved_raw` is a fresh descriptor we now own.
        let saved = unsafe { OwnedFd::from_raw_fd(saved_raw) };

        // SAFETY: `write_end` is a valid descriptor. If this fails, fd 2
        // is untouched, so there is nothing to undo.
        if unsafe { libc::dup2(write_end.as_raw_fd(), STDERR_FD) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let restore = Arc::new(Restore::new(saved.as_raw_fd()));
        let residual: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let closing = Arc::new(AtomicBool::new(false));

        // Publish the saved descriptor so the panic hook can hand the
        // real stderr back.
        SAVED_STDERR.store(saved.as_raw_fd(), Ordering::SeqCst);

        let thread_restore = restore.clone();
        let thread_residual = residual.clone();
        let thread_closing = closing.clone();
        let reader = std::thread::Builder::new()
            .name("nini-stderr".to_string())
            .spawn(move || {
                // The reader thread ends for three reasons: EOF, an I/O
                // error, or a panic. In all three, fd 2 has to go back
                // to the terminal, or the next write to stderr blocks
                // on a pipe nobody reads.
                with_restore_on_exit(&thread_restore, || {
                    read_loop(read_end, &state, &thread_residual, &thread_closing);
                });
            })
            .map_err(|e| {
                // The thread never started, so nobody will read the
                // pipe: put fd 2 back before giving up.
                restore.restore();
                e
            })?;

        Ok(TerminalGuard {
            saved,
            pipe_write: Some(write_end),
            reader: Some(reader),
            restore,
            residual,
            closing,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // 1. From now on, lines the reader picks up belong to the real
        //    stderr, not to the transcript: the transcript is about to
        //    disappear with the alternate screen.
        self.closing.store(true, Ordering::SeqCst);
        // 2. Hand the terminal back. Writes to fd 2 reach the real
        //    stderr again.
        self.restore.restore();
        // 3. Unpublish the saved descriptor, but only if it is still
        //    ours — another guard may have taken over in the meantime.
        let _ = SAVED_STDERR.compare_exchange(
            self.saved.as_raw_fd(),
            -1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        // 4. Close the last reference to the pipe's write end, so the
        //    reader sees EOF and stops.
        drop(self.pipe_write.take());
        // 5. Wait for the reader to finish the lines it already had.
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        // 6. Whatever was still in the pipe goes to the real stderr, so
        //    that a line is never silently lost.
        let residual = std::mem::take(
            &mut *self
                .residual
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if !residual.is_empty() {
            let mut out = io::stderr();
            for line in residual {
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
    }
}

/// Run `body` with fd 2 restored when it ends — normally, on an error,
/// or on a panic (the guard is dropped while the panic unwinds).
fn with_restore_on_exit(restore: &Arc<Restore>, body: impl FnOnce()) {
    let guard = RestoreOnExit {
        restore: restore.clone(),
    };
    body();
    drop(guard);
}

/// The reader thread's body: turn lines into transcript notices.
fn read_loop(
    read_end: OwnedFd,
    state: &SharedState,
    residual: &Arc<Mutex<Vec<String>>>,
    closing: &Arc<AtomicBool>,
) {
    // `OwnedFd` is a descriptor, not a stream; `File` is the owned
    // stream over the same descriptor.
    let file: std::fs::File = read_end.into();
    let mut reader = BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();
    loop {
        raw.clear();
        // `read_until` rather than `read_line`: a library that writes
        // non-UTF-8 bytes must not kill the reader thread (which would
        // leave fd 2 pointing at a pipe nobody reads).
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break, // EOF: the write end was closed.
            Ok(_) => {}
            Err(_) => break,
        }
        let text = String::from_utf8_lossy(&raw);
        let line = text.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            // A blank line carries no message. Keeping it would put
            // empty rows in the transcript for no reason.
            continue;
        }
        if closing.load(Ordering::SeqCst) {
            // The guard is shutting down: this line did not make it
            // into the transcript, so it goes to the real stderr.
            residual
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(line.to_string());
            continue;
        }
        deliver(state, line);
    }
}

/// Push one line into the transcript.
fn deliver(state: &SharedState, line: &str) {
    // A poisoned lock still holds a usable state — the poison is a
    // warning about a panic elsewhere, not a broken value. Delivering
    // anyway is what keeps the promise that no line is lost.
    let mut guard = match state.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.push_notice(line.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoring_twice_is_harmless() {
        // `release_stderr` is called by the panic hook and by the
        // guard's `Drop`; both may run. It must not corrupt fd 2.
        let restore = Arc::new(Restore::new(STDERR_FD));
        restore.restore();
        restore.restore();
        assert!(restore.done.load(Ordering::SeqCst));
    }
}
