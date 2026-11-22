//! Invariant guards for the two NONE-tier fixes that previously had **no
//! assertion** (see `docs/plans/surpass-pi-optimization-plan.md` S1).
//!
//! Both are **source-structure oracles** in the same family as
//! `crates/nini-core/tests/ledger_anchors.rs`: they assert the repaired
//! call site still carries the guard that fixed the bug. A "revert the fix"
//! mutation turns the file red, which is the only property that makes the
//! assertion an oracle rather than a comment.
//!
//! invariant: 78593581
//! invariant: 040e2d4a
//!
//! The `invariant:` annotations above are the declaration that
//! `scripts/invariants.py` reads to move these two from `NONE` to
//! `ANNOTATED`; the real defence is the assertion that can be made to fail.

/// **78593581** — the second clipboard entry point takes the guard.
///
/// `apply_action`'s `PasteImage` arm (the real Ctrl+V path) calls
/// `paste_image_with_size_from_clipboard`, **not**
/// `paste_image_from_clipboard`. The first version of the macOS clipboard
/// fix guarded only the latter, so one of every two calls was unguarded and
/// the macOS leg kept dying with a stack-overflow fault inside
/// `objc_msgSend`. This asserts the *second* entry point still takes
/// `clipboard_guard()`. Reverting the guard makes the test red.
#[test]
fn the_second_clipboard_entry_point_takes_the_clipboard_guard() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/image_paste.rs"))
        .expect("read image_paste.rs");

    let fn_at = src
        .find("pub fn paste_image_with_size_from_clipboard")
        .expect("the second clipboard entry point exists");
    let body = &src[fn_at..];
    let end = body.find("\n}\n").expect("function body end");
    let body = &body[..end];

    assert!(
        body.contains("crate::clipboard::clipboard_guard()"),
        "the second clipboard entry point no longer takes clipboard_guard() \
         — reverting 78593581 lets two threads enter NSPasteboard at once"
    );
}

/// **040e2d4a** — the watchdog drops the state lock before pushing the stop notice.
///
/// The watchdog's abnormal-end branches push a user-visible stop notice via
/// `push_notices`, which re-acquires the state lock. The state lock is a
/// **non-reentrant** `Mutex`, so the fix had to `drop(g)` before pushing;
/// without it the watchdog deadlocks and never resets `agent_alive`, turning
/// "frozen after disconnect" into a permanent freeze.
///
/// The rule asserted here: walking **backwards** from every `push_notices`
/// in the watchdog region, the most recent lock-related marker must be a
/// `drop(g)`, never a `lock_state`. Reverting any watchdog `drop(g)` makes
/// the nearest marker a `lock_state`, so the test goes red.
#[test]
fn the_watchdog_drops_the_state_lock_before_pushing_the_stop_notice() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/runtime.rs"))
        .expect("read runtime.rs");

    let watchdog_at = src
        .find("// v0.10.8 (P2/A1) — the watchdog")
        .expect("the watchdog region exists");
    let region = &src[watchdog_at..];
    // The watchdog function ends at the first line-start `}` (no indentation).
    let end = region
        .find("\n}\n")
        .expect("the watchdog region ends at a top-level brace");
    let region = &region[..end];

    let mut last_lock_marker: Option<&str> = None;
    let mut pushes = 0usize;
    for line in region.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("drop(g)") {
            last_lock_marker = Some("drop(g)");
        } else if trimmed.contains("lock_state(") {
            last_lock_marker = Some("lock_state");
        }
        if trimmed.contains("push_notices(") {
            let has_drop = matches!(last_lock_marker, Some("drop(g)"));
            assert!(
                has_drop,
                "watchdog `push_notices` is not preceded by a `drop(g)` — \
                 the nearest lock marker is {last_lock_marker:?}; reverting \
                 040e2d4a deadlocks the watchdog (non-reentrant state lock)"
            );
            pushes += 1;
        }
    }
    assert!(
        pushes >= 2,
        "expected at least two watched `push_notices` sites, found {pushes}"
    );
}
