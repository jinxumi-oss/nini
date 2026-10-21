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
//! * **Nothing waits.** The guard's exit does not depend on the pipe
//!   reaching EOF: a child process that inherited the pipe's write end
//!   keeps it open forever, and waiting for EOF would hang the exit
//!   (Ctrl+D would never return the shell). The reader is stopped by an
//!   explicit signal, and the guard waits for it with a timeout.
//!
//! stdout is not touched by the guard: the renderer writes to stdout;
//! changing stdout's meaning would break the frame, not fix it. What
//! keeps the *library* off stdout is a structural test — see
//! `crates/nini-tui/tests/no_terminal_writes.rs`.
//!
//! # Lifetime
//!
//! The guard is installed in [`crate::runtime::run`] after raw mode is on
//! and before the alternate screen is entered, and dropped when the
//! event loop returns. The panic hook calls [`release_stderr`] first, so
//! a crash report still reaches the user's terminal.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, Receiver};
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

/// The slot is taken by an install that has not published its
/// descriptor yet.
///
/// The install path is: claim the slot → `dup` the real stderr →
/// point fd 2 at the pipe → publish the descriptor. The claim is a
/// single compare-and-swap, so two threads cannot both be halfway
/// through that sequence: the second one is told the guard is already
/// installed instead of saving *the pipe* as "the original stderr".
const SLOT_CLAIMED: i32 = -2;

/// How long the guard waits for the reader thread before it gives up
/// on it.
///
/// The reader is stopped by an explicit signal, so it ends in
/// microseconds. The timeout is there for the pathological case (the
/// reader is blocked in a syscall that never returns): a stuck reader
/// must not keep the process from exiting. When the timeout expires
/// the thread is **detached** — not joined — and it may no longer
/// deliver to the transcript (it puts its lines in the residual
/// instead, and the residual is written to the real stderr).
const READER_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);

/// The most lines the residual buffer keeps.
///
/// The residual holds the lines that could not be delivered: the guard
/// is shutting down, or the state was locked by the render loop. It is
/// a safety net, not a log — an unbounded one would be a memory leak
/// in a long session.
const RESIDUAL_LIMIT: usize = 1000;

/// Set by [`fail_next_install_after_publish`]: the next install fails
/// after it has published the descriptor, which is the point at which
/// the rollback has to happen.
static FAIL_NEXT_INSTALL: AtomicBool = AtomicBool::new(false);

/// Test hook: make the next [`TerminalGuard::install`] fail the way a
/// reader-thread spawn failure does.
///
/// The failure has to happen **after** the descriptor is published,
/// because that is the only point at which a stale descriptor can be
/// left behind: the slot then points at a descriptor that is about to
/// be closed, every later install is refused, and the panic hook's
/// `dup2` points fd 2 at whatever that number means by then.
///
/// This exists so the integration test can prove that a failed install
/// rolls the slot back. The runtime never calls it.
#[doc(hidden)]
pub fn fail_next_install_after_publish() {
    FAIL_NEXT_INSTALL.store(true, Ordering::SeqCst);
}

/// Hand the real stderr back to the process.
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
///
/// The descriptor is owned here (not borrowed from the guard) so that
/// it stays valid for as long as *anybody* can still need it: the
/// reader thread writes the lines it could not deliver to it, and that
/// can happen after the guard itself is gone.
struct Restore {
    saved: OwnedFd,
    done: AtomicBool,
}

impl Restore {
    fn new(saved: OwnedFd) -> Self {
        Restore {
            saved,
            done: AtomicBool::new(false),
        }
    }

    /// The descriptor the original stderr was duplicated to.
    fn saved_fd(&self) -> RawFd {
        self.saved.as_raw_fd()
    }

    /// Idempotent: the guard's `Drop` and the reader thread both call
    /// it, and only the first one does the `dup2`.
    fn restore(&self) {
        if !self.done.swap(true, Ordering::SeqCst) {
            // SAFETY: `self.saved` is a valid, owned dup of the original
            // stderr; it stays alive for as long as this `Restore` does.
            unsafe {
                libc::dup2(self.saved_fd(), STDERR_FD);
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

/// The claim on the process-global slot, released if the install does
/// not finish.
///
/// Without this, a failed install would leave the slot marked as taken
/// forever: every later install would be refused with `AlreadyExists`
/// and the TUI would run with no guard at all — the symptom this whole
/// module exists to remove, back again.
struct SlotClaim;

impl Drop for SlotClaim {
    fn drop(&mut self) {
        let _ = SAVED_STDERR.compare_exchange(SLOT_CLAIMED, -1, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// The stop signal: a pipe the reader thread also watches.
///
/// The reader must not depend on the stderr pipe reaching EOF. A child
/// process that inherited the write end (the external editor does) keeps
/// it open for as long as it lives — for a daemon, forever — and the
/// guard's exit would then wait forever. The stop signal is the way
/// out: the guard writes a byte, the reader wakes up and stops.
struct StopSignal {
    write: OwnedFd,
}

impl StopSignal {
    fn new() -> std::io::Result<(StopSignal, OwnedFd)> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` is a two-element array, as `pipe` requires.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `pipe` initialised both descriptors; taking ownership
        // of them is the only way they get closed.
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        Ok((StopSignal { write }, read))
    }

    /// Wake the reader up. The byte's content does not matter; only its
    /// arrival does.
    fn signal(&self) {
        let byte = [0u8; 1];
        // SAFETY: `byte` is a valid one-byte buffer.
        unsafe {
            libc::write(
                self.write.as_raw_fd(),
                byte.as_ptr() as *const libc::c_void,
                1,
            );
        }
    }
}

/// The guard: fd 2 is a pipe, and a thread turns its lines into
/// transcript notices.
///
/// Dropping it restores fd 2, stops the reader thread, and writes back
/// the lines that were still in the pipe.
pub struct TerminalGuard {
    /// The original stderr, shared with the reader thread: the reader
    /// writes the lines it could not deliver there.
    restore: Arc<Restore>,
    /// The write end of the pipe. Held so the reader only sees EOF when
    /// the guard is dropped; fd 2 is a second reference to it.
    pipe_write: Option<OwnedFd>,
    /// The reader thread.
    reader: Option<JoinHandle<()>>,
    /// The reader's "I am done" signal.
    reader_done: Option<Receiver<()>>,
    /// Shared with the reader thread.
    residual: Arc<Mutex<Vec<String>>>,
    /// Set when the guard starts shutting down: from then on, the
    /// reader stops delivering to the transcript and keeps the lines
    /// for the real stderr instead.
    closing: Arc<AtomicBool>,
    /// How many residual lines were dropped because the buffer was
    /// full. (Held by the reader thread; the guard does not need it.)
    /// The stop signal for the reader thread.
    stop: Option<StopSignal>,
}

impl TerminalGuard {
    /// Take over stderr for as long as the guard is alive.
    ///
    /// Every step that can fail restores fd 2 before returning, so a
    /// failed install leaves the process exactly as it was — a TUI that
    /// cannot install the guard is still a TUI, just a noisier one.
    pub fn install(state: SharedState) -> std::io::Result<TerminalGuard> {
        // One atomic step claims the process-global slot. A
        // check-then-act here would be a race: two threads could both
        // see "no guard installed", both `dup` fd 2 — and the second
        // one would save *the pipe* as the original stderr, which
        // means the guard's `Drop` would point fd 2 at a pipe that is
        // about to close, and every later `eprintln!` would fail.
        if SAVED_STDERR
            .compare_exchange(-1, SLOT_CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Two guards would fight over fd 2: the second one would
            // pipe the pipe. Refuse instead of corrupting it.
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "a terminal guard is already installed",
            ));
        }
        // From here on, every early return has to give the slot back.
        // The binding exists for its `Drop`; it is deliberately not
        // `_`-prefixed, because an underscore-prefixed binding reads as
        // a discarded value (and rustc would warn about an unused one).
        #[allow(unused_variables)]
        let claim = SlotClaim;

        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` is a two-element array, as `pipe` requires.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `pipe` initialised both descriptors; taking ownership
        // of them is the only way they get closed.
        let read_end = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_end = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        // SAFETY: dup of fd 2; on failure nothing has been changed yet.
        let saved_raw = unsafe { libc::dup(STDERR_FD) };
        if saved_raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `saved_raw` is a fresh descriptor we now own.
        let saved = unsafe { OwnedFd::from_raw_fd(saved_raw) };

        // SAFETY: `write_end` is a valid descriptor. If this fails, fd 2
        // is untouched, so there is nothing to undo.
        if unsafe { libc::dup2(write_end.as_raw_fd(), STDERR_FD) } < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let restore = Arc::new(Restore::new(saved));
        let residual: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let closing = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));

        // Publish the saved descriptor so the panic hook can hand the
        // real stderr back.
        SAVED_STDERR.store(restore.saved_fd(), Ordering::SeqCst);

        let (stop, stop_read) = match StopSignal::new() {
            Ok(pair) => pair,
            Err(e) => {
                // fd 2 points at the pipe and nobody will read it: put
                // it back before giving up, and give the slot back.
                restore.restore();
                let _ = SAVED_STDERR.compare_exchange(
                    restore.saved_fd(),
                    -1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                return Err(e);
            }
        };

        if FAIL_NEXT_INSTALL.swap(false, Ordering::SeqCst) {
            // The reader thread cannot be started: nobody will read
            // the pipe, so fd 2 has to go back to the terminal, and the
            // slot has to be released — a stale descriptor in the slot
            // would refuse every later install and would point the
            // panic hook at a closed descriptor.
            restore.restore();
            let _ = SAVED_STDERR.compare_exchange(
                restore.saved_fd(),
                -1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            return Err(std::io::Error::other(
                "simulated failure to start the reader thread",
            ));
        }

        let (done_tx, done_rx) = channel::<()>();
        let thread_restore = restore.clone();
        let thread_residual = residual.clone();
        let thread_closing = closing.clone();
        let thread_dropped = dropped.clone();
        let reader = std::thread::Builder::new()
            .name("nini-stderr".to_string())
            .spawn(move || {
                // The reader thread ends for four reasons: the stop
                // signal, EOF, an I/O error, or a panic. In all of
                // them, fd 2 has to go back to the terminal, or the
                // next write to stderr blocks on a pipe nobody reads.
                with_restore_on_exit(&thread_restore, || {
                    read_loop(
                        read_end,
                        stop_read,
                        &state,
                        &thread_residual,
                        &thread_closing,
                        &thread_dropped,
                    );
                    // The lines that could not be delivered go to the
                    // *real* stderr, not to the transcript: the
                    // transcript is about to disappear with the
                    // alternate screen, and a line that vanishes with
                    // it was swallowed.
                    flush_residual(&thread_restore, &thread_residual, &thread_dropped);
                });
                let _ = done_tx.send(());
            })
            .map_err(|e| {
                // The thread never started, so nobody will read the
                // pipe: put fd 2 back before giving up, and release
                // the slot — a poisoned slot would leave the TUI
                // without a guard for the rest of the session.
                restore.restore();
                let _ = SAVED_STDERR.compare_exchange(
                    restore.saved_fd(),
                    -1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                e
            })?;

        Ok(TerminalGuard {
            restore,
            pipe_write: Some(write_end),
            reader: Some(reader),
            reader_done: Some(done_rx),
            residual,
            closing,
            stop: Some(stop),
        })
    }

    /// Stop delivering to the transcript.
    ///
    /// From this moment on, every line the reader picks up goes to the
    /// *real* stderr instead of the transcript. The runtime calls this
    /// **before** it leaves the alternate screen: the transcript is
    /// about to be erased, so a line delivered after that point would
    /// be delivered to a screen that no longer exists — which is the
    /// same as losing it.
    ///
    /// Calling it twice is harmless.
    pub fn begin_shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

/// Hand the terminal back for the duration of the external editor.
///
/// The editor runs in the user's **real** terminal, not in the TUI's.
/// With the guard installed, the editor's diagnostics — `vim: error
/// reading …`, `code: command not found` — would be swallowed into a
/// transcript that is *not on screen* at that moment, so the user
/// would never see them.
///
/// Dropping the guard hands fd 2 back to the real terminal and stops
/// the reader thread; the lines the reader could not deliver are
/// written to the real stderr, where the user can see them.
pub fn suspend_for_external_editor(guard: &mut Option<TerminalGuard>) {
    drop(guard.take());
}

/// Put the guard back after the external editor.
///
/// The TUI is painting again, so the library's stderr must not land on
/// the frame. If the guard cannot be installed, the TUI keeps running
/// without it — noisier, but not broken — and the caller says so in
/// the transcript.
pub fn resume_after_external_editor(
    guard: &mut Option<TerminalGuard>,
    state: SharedState,
) -> std::io::Result<()> {
    let installed = TerminalGuard::install(state)?;
    *guard = Some(installed);
    Ok(())
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // 1. From now on, lines the reader picks up belong to the real
        //    stderr, not to the transcript: the transcript is about to
        //    disappear with the alternate screen.
        self.begin_shutdown();
        // 2. Hand the terminal back. Writes to fd 2 reach the real
        //    stderr again.
        self.restore.restore();
        // 3. Unpublish the saved descriptor, but only if it is still
        //    ours — another guard may have taken over in the meantime.
        let _ = SAVED_STDERR.compare_exchange(
            self.restore.saved_fd(),
            -1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        // 4. Tell the reader to stop. This is a *signal*, not a wait
        //    for EOF: a child process that inherited the pipe's write
        //    end keeps the pipe open, and waiting for EOF would hang
        //    the exit forever.
        if let Some(stop) = self.stop.take() {
            stop.signal();
        }
        // 5. Close the last reference to the pipe's write end, so the
        //    reader sees EOF if it is still looking at the pipe.
        drop(self.pipe_write.take());
        // 6. Wait for the reader — with a timeout. The reader ends as
        //    soon as it sees the stop signal; the timeout only exists
        //    so that a wedged reader cannot keep the process from
        //    exiting. On timeout the thread is detached, not joined:
        //    it may no longer deliver to the transcript (the guard is
        //    closing, so it puts its lines in the residual instead).
        if let Some(done) = self.reader_done.take() {
            if done.recv_timeout(READER_JOIN_TIMEOUT).is_err() {
                // The reader did not finish in time: detach it. Its
                // `JoinHandle` is dropped, which does not wait for it.
                self.reader.take();
            }
        }
        // 7. Whatever was still in the pipe goes to the real stderr, so
        //    that a line is never silently lost. The reader usually
        //    did this already (it owns the residual); this is the
        //    belt-and-braces path for a reader that was detached.
        let residual = std::mem::take(
            &mut *self
                .residual
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if !residual.is_empty() {
            write_lines(&self.restore, &residual);
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

/// Write the lines that could not be delivered to the real stderr.
///
/// The destination is the *saved* descriptor, not fd 2: fd 2 may have
/// been taken over by something else by now, and the saved descriptor
/// is the terminal the user was looking at when the guard was
/// installed.
fn write_lines(restore: &Arc<Restore>, lines: &[String]) {
    // The destination is the *saved* descriptor, not fd 2: fd 2 may
    // have been taken over by something else by now, and the saved
    // descriptor is the terminal the user was looking at when the
    // guard was installed.
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    // SAFETY: `text` is a valid buffer and the length matches it. The
    // descriptor is the guard's own dup of the original stderr, and it
    // is still open: whoever calls this holds the `Restore`.
    unsafe {
        libc::write(
            restore.saved_fd(),
            text.as_ptr() as *const libc::c_void,
            text.len(),
        );
    }
}

/// Write the residual to the saved descriptor and clear it.
fn flush_residual(
    restore: &Arc<Restore>,
    residual: &Arc<Mutex<Vec<String>>>,
    dropped: &AtomicBool,
) {
    let lines = std::mem::take(
        &mut *residual
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    if lines.is_empty() {
        return;
    }
    write_lines(restore, &lines);
    if dropped.load(Ordering::SeqCst) {
        // Say that something was dropped: a silent drop is a lie about
        // "nothing is lost".
        let notice =
            b"[nini] terminal guard: the residual buffer was full; some lines were dropped\n";
        // SAFETY: `notice` is a valid buffer and the length matches it.
        unsafe {
            libc::write(
                restore.saved_fd(),
                notice.as_ptr() as *const libc::c_void,
                notice.len(),
            );
        }
    }
}

/// The reader thread's body: turn lines into transcript notices.
///
/// It watches **two** descriptors: the stderr pipe (the lines) and the
/// stop signal (the way out). The stop signal is what makes the exit
/// independent of the pipe reaching EOF — see [`StopSignal`].
fn read_loop(
    read_end: OwnedFd,
    stop_read: OwnedFd,
    state: &SharedState,
    residual: &Arc<Mutex<Vec<String>>>,
    closing: &Arc<AtomicBool>,
    dropped: &AtomicBool,
) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        // Wait for either the pipe or the stop signal.
        let mut fds = [
            libc::pollfd {
                fd: read_end.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stop_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a two-element array and `nfds` is 2.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if ready < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        // The stop signal wins: the guard is going away and the lines
        // that are still in the pipe belong to the real stderr.
        if fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            break;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            continue;
        }
        // SAFETY: `buf` is a valid buffer of the length we pass.
        let n = unsafe {
            libc::read(
                read_end.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n == 0 {
            break; // EOF: the write end was closed.
        }
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        pending.extend_from_slice(&buf[..n as usize]);
        // Split the buffer into lines. A line without its newline is
        // kept until the rest of it arrives.
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = pending.drain(..=pos).collect();
            handle_line(&raw, state, residual, closing, dropped);
        }
    }
    // A last line without a newline is still a line.
    if !pending.is_empty() {
        let raw = std::mem::take(&mut pending);
        handle_line(&raw, state, residual, closing, dropped);
    }
}

/// Turn one raw line (with or without its newline) into a notice.
fn handle_line(
    raw: &[u8],
    state: &SharedState,
    residual: &Arc<Mutex<Vec<String>>>,
    closing: &Arc<AtomicBool>,
    dropped: &AtomicBool,
) {
    // `from_utf8_lossy` rather than `from_utf8`: a library that writes
    // non-UTF-8 bytes must not kill the reader thread (which would
    // leave fd 2 pointing at a pipe nobody reads).
    let text = String::from_utf8_lossy(raw);
    let line = text.trim_end_matches(['\n', '\r']);
    if line.trim().is_empty() {
        // A blank line carries no message. Keeping it would put
        // empty rows in the transcript for no reason.
        return;
    }
    if closing.load(Ordering::SeqCst) {
        // The guard is shutting down: this line did not make it into
        // the transcript, so it goes to the real stderr.
        push_residual(residual, line, dropped);
        return;
    }
    deliver(state, line, residual, dropped);
}

/// Push one line into the transcript — or into the residual, if the
/// state is locked.
///
/// The reader thread must never **wait** for the render loop: the
/// render loop holds the state lock while it runs a command, and a
/// writer that fills the pipe would then block until the command
/// finished. `try_lock` keeps the reader thread free: the line goes
/// into the residual (and from there to the real stderr) instead of
/// waiting.
fn deliver(
    state: &SharedState,
    line: &str,
    residual: &Arc<Mutex<Vec<String>>>,
    dropped: &AtomicBool,
) {
    match state.try_lock() {
        Ok(mut guard) => guard.push_notice(line.to_string()),
        Err(_) => {
            // The state is locked by the render loop. The line is not
            // lost: it goes to the residual, and the residual is
            // written to the real stderr when the guard is dropped.
            push_residual(residual, line, dropped);
        }
    }
}

/// Add a line to the residual, keeping the buffer bounded.
fn push_residual(residual: &Arc<Mutex<Vec<String>>>, line: &str, dropped: &AtomicBool) {
    let mut guard = residual
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() >= RESIDUAL_LIMIT {
        // The oldest line goes: the buffer is a safety net, and an
        // unbounded one is a leak.
        guard.remove(0);
        dropped.store(true, Ordering::SeqCst);
    }
    guard.push(line.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A file that stands in for the terminal.
    struct FakeTerminal {
        path: std::path::PathBuf,
        /// Held so the descriptor fd 2 points at stays open.
        _file: std::fs::File,
        saved: RawFd,
    }

    impl FakeTerminal {
        fn install() -> FakeTerminal {
            let path = std::env::temp_dir().join(format!(
                "nini-restore-probe-{}-{}",
                std::process::id(),
                line!()
            ));
            let file = std::fs::File::create(&path).expect("temp file");
            // SAFETY: dup of fd 2; the descriptor is ours.
            let saved = unsafe { libc::dup(STDERR_FD) };
            assert!(saved >= 0, "could not dup stderr");
            // SAFETY: `file` owns a valid descriptor; `dup2` moves the
            // process's stderr onto it.
            unsafe { libc::dup2(file.as_raw_fd(), STDERR_FD) };
            FakeTerminal {
                path,
                _file: file,
                saved,
            }
        }

        fn read_to_string(&self) -> String {
            let mut text = String::new();
            let mut file = std::fs::File::open(&self.path).expect("reopen");
            let _ = file.read_to_string(&mut text);
            text
        }
    }

    impl Drop for FakeTerminal {
        fn drop(&mut self) {
            // SAFETY: `saved` is a dup of the original stderr.
            unsafe {
                libc::dup2(self.saved, STDERR_FD);
                libc::close(self.saved);
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn restoring_twice_puts_the_original_stderr_back() {
        // `release_stderr` is called by the panic hook and by the
        // guard's `Drop`; both may run. It must not corrupt fd 2 — and
        // it must actually put the *original* descriptor back, not
        // merely flip a flag.
        let terminal = FakeTerminal::install();
        let saved = unsafe { libc::dup(STDERR_FD) };
        assert!(saved >= 0, "could not dup stderr");

        // Something else takes over fd 2 …
        let other = std::fs::File::create(
            std::env::temp_dir().join(format!("nini-restore-other-{}", std::process::id())),
        )
        .expect("temp file");
        // SAFETY: `other` owns a valid descriptor.
        unsafe { libc::dup2(other.as_raw_fd(), STDERR_FD) };

        // … and the restore puts the original one back.
        let restore = Arc::new(Restore::new(unsafe { OwnedFd::from_raw_fd(saved) }));
        restore.restore();
        // The second call must be harmless.
        restore.restore();

        let seen = terminal.read_to_string();
        assert!(
            seen.is_empty(),
            "the restore did not put the original stderr back: {seen:?}"
        );
        // SAFETY: `seen` is a valid buffer.
        let n = unsafe { libc::write(STDERR_FD, b"probe\n".as_ptr() as *const libc::c_void, 6) };
        assert_eq!(n, 6, "could not write to fd 2");
        let seen = terminal.read_to_string();
        assert_eq!(
            seen, "probe\n",
            "fd 2 did not point at the original terminal after the restore"
        );
    }
}
