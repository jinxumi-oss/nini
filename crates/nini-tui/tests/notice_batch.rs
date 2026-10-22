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
//! `push_notice` goes through `Arc::make_mut(&mut transcript_state)`. The
//! render loop's snapshot holds another `Arc` clone of the same transcript,
//! so the refcount is ≥ 2 and **every push deep-copies the whole
//! transcript**. A tool that printed 200 lines therefore copied the entire
//! transcript 200 times, on the tokio worker, and the UI froze.
//!
//! These tests cover both halves of the fix: the batch API behaves like the
//! single-line one (order, content, and the copy-on-write semantics under a
//! live snapshot), and the runtime's drain path really uses the batch API.

use nini_tui::state::{AppState, TranscriptLine};

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

/// The copy-on-write contract must survive the batch form.
///
/// The render loop holds a snapshot of the state while it draws, and that
/// snapshot shares the transcript through an `Arc`. Mutating the live state
/// must therefore copy the transcript rather than edit the snapshot's copy —
/// and the batch form must not accidentally edit the shared buffer in place
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

// ---------------------------------------------------------------------------
// structure: the drain path uses the batch API
// ---------------------------------------------------------------------------

/// The runtime's source, read at compile time.
const RUNTIME_SRC: &str = include_str!("../src/runtime.rs");
/// The state's source, read at compile time.
const STATE_SRC: &str = include_str!("../src/state.rs");

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

/// The number of single-notice call sites must not grow because of the
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

/// The batch API must do its work in **one** copy-on-write, and in one
/// pass over the messages.
///
/// `push_notices` exists to replace N deep copies with one. If it ever
/// grows a second `Arc::make_mut`, or a loop that copies per message, it
/// is back to copying per line.
///
/// The number of deep copies is not observable from outside the type —
/// `Arc::make_mut` leaves no trace — so this is a source-level check.
/// It is deliberately narrow: exactly one `Arc::make_mut`, and no `for`
/// loop over the batch.
#[test]
fn push_notices_makes_exactly_one_copy_on_write() {
    let body = push_notices_body();
    assert_eq!(
        body.matches("Arc::make_mut").count(),
        1,
        "the batch must copy the transcript once, not once per line:\n{body}"
    );
    assert!(
        !body.contains("for "),
        "the batch must not loop over the messages — a loop that copies per \
         message is the defect this API exists to remove:\n{body}"
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
