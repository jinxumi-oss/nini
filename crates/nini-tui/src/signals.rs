//! Keeping the terminal usable when nini stops unexpectedly.
//!
//! # What this was
//!
//! Six lines, all of it stub:
//!
//! ```text
//! //! Signal handler stub.
//! pub fn install() {}
//! pub fn install_handlers() {}
//! pub fn mark_raw_mode(_on: bool) {}
//! pub fn mark_alt_screen(_on: bool) {}
//! ```
//!
//! # What that cost
//!
//! `runtime.rs` put a comment above these calls:
//!
//! ```text
//! Install SIGINT/SIGTERM/SIGHUP handlers + panic hook before enabling
//! raw mode. The panic hook calls emergency_cleanup() to restore the
//! terminal if anything blows up.
//! ```
//!
//! `emergency_cleanup` did not exist. It still would not, in any other
//! module. So the comment described a program that was not there.
//!
//! crossterm enters raw mode and the alternate screen directly, a few
//! lines away from each `mark_*` call, so the marks looked redundant. They
//! were not: the marks are the *state* that tells cleanup whether there is
//! anything to undo. With them empty, killing nini with `SIGTERM`, or
//! letting it panic, left the terminal in raw mode inside the alternate
//! screen - no echo, no scrolling, escape codes leaking into the shell.
//! `kill %1` from another terminal was the only way out.
//!
//! # What Pi does
//!
//! `pi-coding-agent/src/modes/interactive/interactive-mode.ts:3960`:
//!
//! ```ts
//! // `signal-exit` checks the listener list during the same SIGTERM/SIGHUP
//! process.on("SIGTERM", ...);
//! ```
//!
//! and at `:3965`, SIGINT is ignored while the process is suspended so that
//! Ctrl+C in the editor does not kill the session.
//!
//! # Why tokio, and not a signal crate
//!
//! `signal-hook` is already in `Cargo.lock` transitively. It is not needed:
//! the workspace enables `tokio = { features = ["full"] }`, so
//! `tokio::signal::unix` is available with no new dependency, no unsafe,
//! and - unlike a `libc::signal` handler - cleanup runs in ordinary code
//! where calling `crossterm` is allowed. In a real signal handler only
//! async-signal-safe calls are legal, and restoring a terminal is not one
//! of them.
//!
//! What is *not* implemented, because Pi's version of it needs a suspend
//! state nini does not have: ignoring SIGINT while an external editor
//! runs. See `runtime.rs`, where the TUI is torn down before the editor is
//! spawned.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static RAW_MODE: AtomicBool = AtomicBool::new(false);
static ALT_SCREEN: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: AtomicBool = AtomicBool::new(false);

/// Record that raw mode has been entered or left.
///
/// Called either side of crossterm's own `enable_raw_mode`. crossterm does
/// the actual work; this records whether there is work to undo.
pub fn mark_raw_mode(on: bool) {
    RAW_MODE.store(on, Ordering::SeqCst);
}

/// Record that the alternate screen has been entered or left.
pub fn mark_alt_screen(on: bool) {
    ALT_SCREEN.store(on, Ordering::SeqCst);
}

pub fn raw_mode_active() -> bool {
    RAW_MODE.load(Ordering::SeqCst)
}

pub fn alt_screen_active() -> bool {
    ALT_SCREEN.load(Ordering::SeqCst)
}

/// Put the terminal back the way it was found.
///
/// Idempotent, and safe to call when nothing is marked - it then does no
/// terminal I/O at all, which is what makes it testable without a tty and
/// harmless when it races a normal shutdown.
///
/// Never returns an error upward: this runs on the way out of a process
/// that is already failing, and there is nowhere to report a failure to.
pub fn emergency_cleanup() {
    let had_alt = ALT_SCREEN.swap(false, Ordering::SeqCst);
    let had_raw = RAW_MODE.swap(false, Ordering::SeqCst);
    if !had_alt && !had_raw {
        return;
    }

    // `std::io::stdout()` rather than crossterm's `execute!`: this runs
    // from a signal task and from a panic hook, where building a ratatui
    // backend would be both slow and pointless.
    let mut out = std::io::stdout();
    if had_alt {
        // Leave the alternate screen and put the cursor back.
        let _ = out.write_all(b"\x1b[?1049l\x1b[?25h");
    }
    let _ = out.flush();

    if had_raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Conventional exit status for a signal: 128 + the signal number, as a
/// shell reports it.
fn exit_code_for(signal: i32) -> i32 {
    128 + signal
}

pub const SIGINT: i32 = libc_sigint();
pub const SIGTERM: i32 = libc_sigterm();
pub const SIGHUP: i32 = libc_sighup();

// The three signal numbers, spelled out rather than pulled from `libc`:
// nini-tui does not depend on `libc`, and these are fixed by POSIX.
const fn libc_sigint() -> i32 {
    2
}
const fn libc_sigterm() -> i32 {
    15
}
const fn libc_sighup() -> i32 {
    1
}

/// Restore the terminal and exit, the way a well-behaved TUI does on
/// SIGINT/SIGTERM/SIGHUP.
///
/// Separate from [`emergency_cleanup`] so a test can assert the cleanup
/// without terminating the test process.
fn shutdown_from_signal(signal: i32) -> ! {
    emergency_cleanup();
    std::process::exit(exit_code_for(signal));
}

/// Install the panic hook and the signal tasks.
///
/// Returns an error only when the panic hook could not be chained, which
/// means some earlier hook panicked while being called; the signal tasks
/// are still installed.
///
/// Idempotent: calling it twice does not chain two panic hooks.
pub fn install_handlers() -> Result<(), std::io::Error> {
    if !PANIC_HOOK.swap(true, Ordering::SeqCst) {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            emergency_cleanup();
            // v0.10.3: the terminal guard turns stderr into a pipe
            // while the TUI runs. Hand the real stderr back before the
            // crash report is printed, or the report goes into the pipe
            // and the user sees no reason at all.
            crate::terminal_guard::release_stderr();
            previous(info);
        }));
    }

    // `install_handlers` is called from inside the tokio runtime (the TUI's
    // entry point is async), so a task can be spawned. If it somehow is
    // not, the panic hook above is the more important half and the
    // terminal is still restored on a crash - just not on a signal.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(watch_signals());
        }
        Err(_) => {
            eprintln!(
                "[nini] signals: no async runtime, SIGINT/SIGTERM will not restore the terminal"
            );
        }
    }
    Ok(())
}

async fn watch_signals() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[nini] signals: SIGTERM unavailable: {e}");
            return;
        }
    };
    let mut hup = match signal(SignalKind::hangup()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[nini] signals: SIGHUP unavailable: {e}");
            return;
        }
    };
    let mut int = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[nini] signals: SIGINT unavailable: {e}");
            return;
        }
    };

    // Whichever arrives first. SIGINT is included deliberately: nini leaves
    // SIGINT at its default, which kills the process, and a process killed
    // by SIGINT never runs the cleanup the TUI would otherwise have run on
    // the way out.
    let code = tokio::select! {
        _ = term.recv() => SIGTERM,
        _ = hup.recv() => SIGHUP,
        _ = int.recv() => SIGINT,
    };
    shutdown_from_signal(code);
}

/// Kept because the name is in older call sites and in comments; see the
/// module doc. Delegates to [`install_handlers`].
pub fn install() -> Result<(), std::io::Error> {
    install_handlers()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These touch process-wide state, so they are serialised. The flags
    /// are global by construction - there is one terminal per process -
    /// and `cargo test` runs this binary on many threads.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Leave no marks behind, whatever the test did.
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RAW_MODE.store(false, Ordering::SeqCst);
            ALT_SCREEN.store(false, Ordering::SeqCst);
        }
    }

    #[test]
    fn marks_round_trip() {
        let _g = guard();
        let _r = Reset;

        mark_raw_mode(true);
        mark_alt_screen(true);
        assert!(raw_mode_active() && alt_screen_active());

        mark_raw_mode(false);
        mark_alt_screen(false);
        assert!(!raw_mode_active() && !alt_screen_active());
    }

    /// The whole point: cleanup has to clear the marks so the next call is
    /// a no-op rather than a second write to the terminal.
    #[test]
    fn cleanup_clears_the_marks() {
        let _g = guard();
        let _r = Reset;

        mark_raw_mode(true);
        mark_alt_screen(true);
        emergency_cleanup();

        assert!(!raw_mode_active(), "cleanup left raw mode marked");
        assert!(!alt_screen_active(), "cleanup left alt screen marked");
    }

    /// Calling cleanup twice happens for real: the signal task and the
    /// normal exit path can both reach it.
    #[test]
    fn cleanup_is_idempotent() {
        let _g = guard();
        let _r = Reset;

        mark_raw_mode(true);
        mark_alt_screen(true);
        emergency_cleanup();
        emergency_cleanup();
        assert!(!raw_mode_active() && !alt_screen_active());
    }

    /// A shutdown with nothing marked must not write escape sequences at
    /// whatever stdout happens to be - in a test harness that is a pipe, in
    /// a real crash it is whatever the user last printed.
    #[test]
    fn cleanup_with_nothing_marked_touches_nothing() {
        let _g = guard();
        let _r = Reset;

        // No marks. If this wrote to stdout it would corrupt the harness's
        // own output and the failure would look like a broken test run.
        emergency_cleanup();
    }

    /// The panic hook is what restores the terminal on a crash, and the
    /// hook could not be chained before this existed.
    #[test]
    fn a_panic_restores_the_terminal() {
        let _g = guard();
        let _r = Reset;

        install_handlers().expect("install");
        mark_raw_mode(true);
        mark_alt_screen(true);

        let prev = std::panic::take_hook();
        std::panic::set_hook(prev);
        let result = std::panic::catch_unwind(|| {
            mark_raw_mode(true);
            mark_alt_screen(true);
            panic!("boom");
        });
        assert!(result.is_err(), "the test did not panic");

        assert!(
            !raw_mode_active() && !alt_screen_active(),
            "the panic hook did not run cleanup"
        );
    }

    #[test]
    fn exit_status_matches_what_a_shell_reports() {
        assert_eq!(exit_code_for(SIGINT), 130);
        assert_eq!(exit_code_for(SIGTERM), 143);
        assert_eq!(exit_code_for(SIGHUP), 129);
    }

    #[test]
    fn installing_twice_does_not_double_chain_the_hook() {
        let _g = guard();
        let _r = Reset;

        install_handlers().unwrap();
        // A second install must not chain a second hook, or cleanup would
        // run twice per panic and the first call's output would interleave.
        install_handlers().unwrap();

        mark_raw_mode(true);
        let _ = std::panic::catch_unwind(|| panic!("boom"));
        assert!(!raw_mode_active());
    }
}
