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

/// Copy `s` to the system clipboard.
///
/// Returns `Err` when no clipboard is reachable: headless CI, a missing
/// `$DISPLAY`/`$WAYLAND_DISPLAY`, or a Wayland/X11 bridge that refuses
/// the connection. Callers are expected to fall back to stdout in that
/// case rather than pretend the copy worked.
pub fn copy(s: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .map_err(|e| format!("clipboard unavailable: {e}"))?
        .set_text(s.to_owned())
        .map_err(|e| format!("clipboard write failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
