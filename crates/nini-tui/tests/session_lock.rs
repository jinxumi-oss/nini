//! The session lock, and what a busy one used to mean.
//!
//! `nini-tui/src/state.rs` held the session in a `tokio::sync::Mutex` and
//! took it with `try_lock` at all eleven access points. Nothing was ever
//! held across an `.await`, so the async mutex bought nothing — but its
//! `try_lock` bought eleven different answers to ordinary contention, none
//! of them reported to anyone:
//!
//! * `session_context_messages` → an empty seed, so the model was asked the
//!   next question with no history at all
//! * `session_append_message` → the turn was never recorded
//! * `session_compact` → compaction silently did not happen
//! * `transcript_from_context` → the transcript stopped matching the session
//! * `/tree` and the branch summary → an empty tree, or a status line stuck
//!   on "computing…"
//! * `/fork` → "the session is busy — try again in a moment"
//!
//! The contention was self-inflicted: `session_flush` holds the lock across
//! `write_to_file`, which is a full serialise of every entry plus an atomic
//! rename. Every user turn flushes.
//!
//! Two properties are asserted here, both of which fail on the old code and
//! both of which fail on any future return to `try_lock`:
//!
//! 1. a reader under contention waits, and gets the history
//! 2. a writer under contention waits, and its message is recorded

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::sync::Arc;
use std::time::Duration;

fn new_state(
    tag: &str,
) -> (
    Arc<std::sync::Mutex<AppState>>,
    AgentSink,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("nini-lock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink, dir)
}

/// Hold the session lock on another thread for `hold`, so contention is a
/// fact rather than a hope.
///
/// Returns a receiver that fires once the lock is actually held. The holder
/// never waits for the test: it takes the lock, signals, sleeps, releases.
/// A reader that *waits* therefore finishes on its own; a reader that gives
/// up returns whatever its fallback was. That is the whole difference
/// between the two implementations, and it is why these tests can fail.
///
/// Without the handshake the holder thread may not even be scheduled before
/// the reader runs, and a test of contention that never contends passes on
/// the code it is meant to catch — which is exactly what the first version
/// of these tests did.
///
/// `try_lock` rather than `lock`, deliberately: both `std::sync::Mutex` and
/// `tokio::sync::Mutex` have it, so these tests compile and run against
/// either implementation. Under the old `tokio` + `try_lock` product code
/// they fail on their assertions; they do not fail to build.
fn hold_the_lock(
    state: &AppState,
    hold: Duration,
) -> (std::sync::mpsc::Receiver<()>, std::thread::JoinHandle<()>) {
    let session = state.session_state.session.clone().expect("a session");
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _guard = session
            .try_lock()
            .expect("the session is free before the test starts");
        let _ = held_tx.send(());
        std::thread::sleep(hold);
    });
    (held_rx, handle)
}

#[tokio::test]
async fn a_contended_read_yields_the_context_not_an_empty_seed() {
    let (state, _sink, dir) = new_state("read");
    state.lock().unwrap().push_user("read the config");
    state.lock().unwrap().push_assistant("first answer");

    let unlocked = state.lock().unwrap().session_context_messages();
    assert_eq!(
        unlocked.len(),
        2,
        "sanity: the seed has both turns before contention"
    );

    let (held, holder) = hold_the_lock(&state.lock().unwrap(), Duration::from_millis(120));
    held.recv_timeout(Duration::from_secs(5))
        .expect("the holder took the session lock");
    let contended = state.lock().unwrap().session_context_messages();
    holder.join().unwrap();

    assert_eq!(
        contended.len(),
        2,
        "a busy session must be waited for, not reported as an empty one"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_contended_append_is_still_recorded() {
    let (state, _sink, dir) = new_state("write");
    state.lock().unwrap().push_user("first question");

    let (held, holder) = hold_the_lock(&state.lock().unwrap(), Duration::from_millis(120));
    held.recv_timeout(Duration::from_secs(5))
        .expect("the holder took the session lock");
    state
        .lock()
        .unwrap()
        .push_assistant("written while the session was busy");
    holder.join().unwrap();

    let entries = {
        let s = state.lock().unwrap();
        let session = s.session_state.session.clone().unwrap();
        let guard = session.try_lock().expect("the holder has joined");
        guard.entries.len()
    };
    assert_eq!(
        entries, 2,
        "an append lost to a busy lock is a turn the user can never get back"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The production shape: a flush is spawned on every turn end, and the next
/// user turn reads the seed. On the old code this test measured 198 empty
/// reads out of 200 — the flush thread held the lock for the whole write and
/// every reader looked straight past it.
#[tokio::test]
async fn flushing_does_not_make_the_seed_disappear() {
    let (state, sink, dir) = new_state("flush");
    state.lock().unwrap().push_user("what did you find?");

    let mut empty = 0usize;
    for _ in 0..200 {
        sink.push(AgentEventLite::TurnEnd);
        if state.lock().unwrap().session_context_messages().is_empty() {
            empty += 1;
        }
    }
    assert_eq!(
        empty, 0,
        "{empty} of 200 seeds were empty — a flush must not take the session \
         away from the reader that follows it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
