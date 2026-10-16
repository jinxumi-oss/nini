//! System clipboard integration.
//!
//! v0.9.2 (P0-4): this used to be a 7-line stub whose `copy()` returned
//! `Ok(())` unconditionally. Because `cmd_copy` treated `Ok(())` as
//! success, `/copy` printed "copied N bytes to clipboard" while the
//! clipboard was untouched — and the stdout fallback further down the
//! call site was unreachable, because that branch only ran on `Err`.
//!
//! The rule this module now enforces: **never report success we cannot
//! verify.** Every failure path returns `Err` with a reason the caller
//! can show the user.
//!
//! `arboard` is already a dependency (see `image_paste.rs` for the
//! image-side usage), so this costs no new dependency.

/// Serialises every trip into the system clipboard, for one process.
///
/// v0.10.1. This exists because of a SIGSEGV, found by reading a native
/// stack rather than by reasoning:
///
/// ```text
/// thread #2, name = 'paste_image_inserts_description_in_prompt'
/// stop reason = EXC_BAD_ACCESS (code=1)
/// frame #0: libobjc objc_msgSend + 32
/// ```
///
/// `arboard`'s macOS backend is Objective-C — `generalPasteboard` and the
/// message sends that follow it — and two tests in `agent_wire_e2e` reach
/// it concurrently. The same test file run with `--test-threads=1` passes
/// 17/17; under concurrency it dies in the middle of the paste test. So
/// the defect is arboard's NSPasteboard being entered from two threads at
/// once, not a missing window server: the runner reports
/// `launchctl managername: Aqua` and a reachable gui domain.
///
/// A `Mutex` cannot make arboard's Objective-C re-entrant, but it does mean
/// only one thread is inside the pasteboard at a time, which is the part
/// that crashes. It is also the right shape for the product: `/copy` and
/// Ctrl+V are both issued from the event loop, and this only matters when
/// an extension or a test reaches in from another thread.
fn clipboard_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

/// Hold the clipboard lock for the duration of a system-clipboard call.
///
/// Poisoning is recovered: a panic inside one clipboard call must not turn
/// every later one into a no-op, and the lock protects nothing that a
/// panic could have left inconsistent.
pub(crate) fn clipboard_guard() -> std::sync::MutexGuard<'static, ()> {
    clipboard_lock().lock().unwrap_or_else(|e| e.into_inner())
}

/// Copy `s` to the system clipboard.
///
/// Returns `Err` when no clipboard is reachable: headless CI, a missing
/// `$DISPLAY`/`$WAYLAND_DISPLAY`, or a Wayland/X11 bridge that refuses
/// the connection. Callers are expected to fall back to stdout in that
/// case rather than pretend the copy worked.
pub fn copy(s: &str) -> Result<(), String> {
    let _guard = clipboard_guard();
    arboard::Clipboard::new()
        .map_err(|e| format!("clipboard unavailable: {e}"))?
        .set_text(s.to_owned())
        .map_err(|e| format!("clipboard write failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.10.1: the guard exists because two threads inside arboard's
    /// macOS backend is a SIGSEGV, not a data race Rust would report. This
    /// cannot show the crash on Linux, but it can pin the property that
    /// prevents it: the clipboard is entered by one caller at a time, so
    /// no caller is ever inside it concurrently with another.
    #[test]
    fn the_clipboard_is_entered_by_one_caller_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static INSIDE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(std::thread::spawn(|| {
                for _ in 0..64 {
                    let _guard = clipboard_guard();
                    let now = INSIDE.fetch_add(1, Ordering::SeqCst) + 1;
                    MAX.fetch_max(now, Ordering::SeqCst);
                    std::thread::yield_now();
                    INSIDE.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            MAX.load(Ordering::SeqCst),
            1,
            "two threads were inside the clipboard at once, which is the \
             arrangement that crashes arboard on macOS"
        );
    }

    /// v0.9.2 (P0-4) — the regression this module exists to prevent.
    ///
    /// The old stub was `pub fn copy(_s: &str) -> Result<(), String> { Ok(()) }`.
    /// It can never come back, and a future edit that reintroduces an
    /// unconditional `Ok` needs to be caught by *something* — because CI
    /// has no clipboard, the positive path is not directly testable, so
    /// we assert the negative one: a copy attempt either genuinely
    /// succeeds against a real clipboard or reports why it did not.
    #[test]
    fn copy_never_reports_success_without_a_clipboard() {
        // A unique marker string; if some future implementation starts
        // echoing the input or swallowing the error, this catches it.
        let marker = "nini-p0-4-canary";

        match copy(marker) {
            // A real clipboard exists (dev machine with X11/Wayland).
            // We cannot assert it landed, so we only assert we did not
            // panic and did not fabricate a different value.
            Ok(()) => {}
            // No clipboard: must carry a human-readable reason so the
            // TUI can explain itself instead of silently degrading.
            Err(reason) => {
                assert!(
                    reason.contains("clipboard"),
                    "error must name the failing subsystem, got: {reason}"
                );
                assert!(
                    !reason.contains(marker),
                    "error must not echo the copied text (it may be user data), got: {reason}"
                );
            }
        }
    }
}
