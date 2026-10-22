//! The guard's pending lines are drained as a **batch**.
//!
//! v0.10.x. The terminal guard collects the lines a tool printed to stderr
//! and hands them to the render loop, which turned each of them into a
//! transcript notice:
//!
//! ```text
//! for line in g.take_pending() {
//!     push_notice(&shared, line);
//! }
//! ```
//!
//! `push_notice` goes through `Arc::make_mut(&mut transcript_state)` —
//! once per line. The batch form does it once per batch.
//!
//! How much that saves is **not** a measured number, and the difference
//! matters: `Arc::make_mut` copies the transcript only when something
//! else still holds a clone of it, and in the drain path nothing does
//! (the previous frame's snapshot has been dropped by then) — so the
//! per-line form copied nothing in practice either. An earlier version
//! of this file claimed the opposite, and claimed a 200-line drain
//! "froze the UI". That premise did not hold.
//!
//! What the batch form does buy is an upper bound (at most one
//! copy-on-write per drain instead of one per line) and one operation on
//! the transcript instead of N.
//!
//! These tests cover both halves: the batch API behaves like the
//! single-line one (order, content, and the copy-on-write semantics under
//! a live snapshot), and the runtime's drain path really uses the batch
//! API.

//! invariant: 266bc74
use nini_tui::state::{AppState, TranscriptLine};

/// `state.rs`, read so that the *shape* of the batch API can be checked.
const STATE_SRC: &str = include_str!("../src/state.rs");
/// `runtime.rs`, read so that the drain path can be checked.
const RUNTIME_SRC: &str = include_str!("../src/runtime.rs");

/// The text of a notice line, or a panic that says what it found instead.
fn notice_text(line: &TranscriptLine) -> &str {
    match line {
        TranscriptLine::Notice(text) => text,
        other => panic!("expected a Notice line, found {other:?}"),
    }
}

#[test]
fn push_notices_appends_every_notice_in_order() {
    let mut state = AppState::default();
    state.push_notice("before");

    state.push_notices(vec![
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
    ]);

    let lines = &state.transcript_state.lines;
    assert_eq!(lines.len(), 4, "the batch adds one line per message");
    assert_eq!(
        notice_text(&lines[0]),
        "before",
        "the earlier notice is still there"
    );
    assert_eq!(notice_text(&lines[1]), "one");
    assert_eq!(notice_text(&lines[2]), "two");
    assert_eq!(notice_text(&lines[3]), "three");
}

#[test]
fn push_notices_accepts_an_empty_batch_without_touching_the_transcript() {
    let mut state = AppState::default();
    state.push_notice("only");
    state.push_notices(Vec::new());
    assert_eq!(state.transcript_state.lines.len(), 1);
    assert_eq!(notice_text(&state.transcript_state.lines[0]), "only");
}

#[test]
fn push_notices_accepts_any_iterator() {
    let mut state = AppState::default();
    state.push_notices((0..3).map(|i| format!("line {i}")));
    let lines = &state.transcript_state.lines;
    assert_eq!(lines.len(), 3);
    assert_eq!(notice_text(&lines[2]), "line 2");
}

/// (which would corrupt the frame being drawn).
#[test]
fn push_notices_copies_rather_than_edits_a_live_snapshot() {
    let mut state = AppState::default();
    state.push_notice("first");

    // The snapshot the render loop takes every frame.
    let snapshot = state.clone();

    state.push_notices(vec!["second".to_string(), "third".to_string()]);

    assert_eq!(
        state.transcript_state.lines.len(),
        3,
        "the live state sees the whole batch"
    );
    assert_eq!(
        snapshot.transcript_state.lines.len(),
        1,
        "the snapshot must not see the batch — it is a copy-on-write copy"
    );
    assert_eq!(notice_text(&snapshot.transcript_state.lines[0]), "first");
    assert_eq!(notice_text(&state.transcript_state.lines[2]), "third");
}

/// The block in `runtime.rs` that drains the guard's pending lines.
fn drain_block(src: &str) -> &str {
    let idx = src
        .find("take_pending()")
        .expect("runtime.rs must drain the guard's pending lines");
    let rest = &src[idx..];
    // The block ends at the first closing brace on its own line, which is
    // how the drain is written today. If that ever stops being true the
    // test says so rather than silently checking the wrong span.
    let end = rest
        .find("\n        }")
        .map(|e| e + "\n        }".len())
        .expect("the drain block must end with a closing brace");
    &rest[..end]
}

#[test]
fn the_drain_path_pushes_the_whole_batch_in_one_call() {
    let block = drain_block(RUNTIME_SRC);
    assert!(
        block.contains("push_notices("),
        "the drain must push the pending lines as one batch:\n{block}"
    );
    assert!(
        !block.contains("push_notice("),
        "the drain must not push the pending lines one at a time — each \
         push is a full deep copy of the transcript:\n{block}"
    );
}

/// drain: every `push_notice(` in the file lives outside the drain block.
#[test]
fn the_drain_adds_no_single_notice_call_sites() {
    let block = drain_block(RUNTIME_SRC);
    let idx = RUNTIME_SRC.find("take_pending()").unwrap();
    let outside = RUNTIME_SRC[..idx].matches("push_notice(").count()
        + RUNTIME_SRC[idx + block.len()..]
            .matches("push_notice(")
            .count();
    let total = RUNTIME_SRC.matches("push_notice(").count();
    assert_eq!(
        total, outside,
        "the drain block must not contain a single-notice call site"
    );
}

/// The batch must be **one** operation on the transcript.
///
/// The number of deep copies is not observable from outside the type —
/// `Arc::make_mut` leaves no trace — and in the drain path there is
/// nothing else holding a clone of the transcript, so the per-line form
/// did not copy anything either. What *is* observable is the shape: one
/// `Arc::make_mut`, one `extend`, and no loop over the messages.
///
/// (An earlier version of this test claimed the batch avoided "N deep
/// copies". That was a claim about a premise that did not hold, and a
/// test that locks a false premise is worse than no test: it sends the
/// next reader looking for a cost that was never paid.)
#[test]
fn push_notices_appends_the_batch_in_one_operation() {
    let body = push_notices_body();
    assert_eq!(
        body.matches("Arc::make_mut").count(),
        1,
        "the batch must touch the transcript once, not once per line:\n{body}"
    );
    assert!(
        !body.contains("for "),
        "the batch must not loop over the messages — a loop that touches \
         the transcript per message is the shape this API exists to \
         remove:\n{body}"
    );
    assert!(
        body.contains(".extend("),
        "the batch must append the notices in one extend:\n{body}"
    );
}

/// The body of `AppState::push_notices`, as written in `state.rs`.
fn push_notices_body() -> &'static str {
    let start = STATE_SRC
        .find("pub fn push_notices(")
        .expect("state.rs must define push_notices");
    let rest = &STATE_SRC[start..];
    let end = rest
        .find("\n    }")
        .map(|e| e + "\n    }".len())
        .expect("push_notices must have a body");
    &rest[..end]
}
