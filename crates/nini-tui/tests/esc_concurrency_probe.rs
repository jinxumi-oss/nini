//! P2/A1 — can a second agent be spawned while the first is still winding
//! down?
//!
//! This file began as a probe of a *confirmed* defect (v0.10.2, measured
//! `spawns=2 live=2`) and is now the regression test for the fix. It is no
//! longer `#[ignore]`d: a defect that is fixed and then re-introduced has
//! to fail a plain `cargo test`, not a remembered command line.
//!
//! The shape being checked:
//!
//! 1. a turn is running (`mode == Running`), driver #1 is live and has
//!    **not** finished;
//! 2. the user presses Esc. The runtime sets `mode = Editing` right away
//!    (`runtime.rs`, the `Abort` arm: "Pi's interrupt is synchronous … the
//!    editor is usable the instant you press Esc") and notifies the abort
//!    flag, but the agent task is still somewhere between the stream and
//!    its `Done` event;
//! 3. the user — whose prompt is already typed — presses Enter.
//!
//! The only guard against a second spawn used to be `mode == RunMode::Running`
//! (`KeyAction::Submit`). Step 2 cleared that, so step 3 spawned driver #2
//! while #1 was still running, and both wrote to the same `SharedState` and
//! the same session file.
//!
//! Pi cannot reach this state: `AgentSession.isIdle` is `!_isAgentRunActive`
//! (`agent-session.js:620`), a property of the *agent*, and Esc only calls
//! `agent.abort()` without touching it. A submit during streaming is steered
//! into the running turn (`streamingBehavior`), so a second agent is never
//! spawned. nini had one `RunMode` field doing both jobs — hence
//! `RunState::agent_alive`, which is only that one fact.
//!
//! The second half of the file guards the *other* direction. A guard that is
//! never lowered is not a fix, it is a mute button: if `agent_alive` stuck at
//! `true`, no message could ever be submitted again and every test here would
//! still be green. Those tests are the ones that make the difference between
//! "spawns at most one agent" and "spawns no agent at all" observable.

use nini_tui::runtime::{submit_user_input, AgentDriver, AgentEventLite, SharedState};
use nini_tui::state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A counter, so two tests never share a directory.
///
/// The first version named the directory after the **process**, which meant
/// every test in this file used the same one: `cargo test` runs the tests
/// of a file in parallel threads, so they were deleting and recreating
/// each other's session file. Nothing failed — the tests do not read the
/// file back — but the isolation was a lie, and a test that *does* read it
/// (see `a_queued_message_is_written_to_the_session_once`) would have
/// caught the others' deletions.
static DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fresh_state() -> (SharedState, std::path::PathBuf) {
    let n = DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nini-escwin-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shared: SharedState = {
        let mut g = AppState::new("test-model");
        g.session_init(dir.join("s.jsonl"));
        g.push_user("a question that is still streaming");
        Arc::new(std::sync::Mutex::new(g))
    };
    (shared, dir)
}

async fn type_and_submit(
    shared: &SharedState,
    driver: &AgentDriver,
    tx: tokio::sync::watch::Sender<bool>,
    text: &str,
) {
    {
        let mut g = shared.lock().unwrap();
        for c in text.chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        shared,
        driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

// ── the defect itself ──────────────────────────────────────────────────

#[tokio::test]
async fn submitting_soon_after_an_esc_never_starts_a_second_agent() {
    let (shared, _dir) = fresh_state();

    let live = Arc::new(AtomicUsize::new(0));
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // The driver stands in for an agent task that has been told to abort
    // but has not yet finished: it increments a counter, signals "running",
    // and waits on a future that never resolves.
    let driver: AgentDriver = {
        let live = Arc::clone(&live);
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, _sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            live.fetch_add(1, Ordering::SeqCst);
            let live = Arc::clone(&live);
            tokio::spawn(async move {
                // Never completes: this *is* the window.
                std::future::pending::<()>().await;
                live.fetch_sub(1, Ordering::SeqCst);
            })
        })
    };

    // ── step 1: a turn is running ───────────────────────────────────────
    type_and_submit(&shared, &driver, tx.clone(), "first prompt").await;
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        1,
        "one turn should be running"
    );
    assert_eq!(live.load(Ordering::SeqCst), 1, "that turn should be live");

    // ── step 2: Esc ─────────────────────────────────────────────────────
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = nini_tui::state::RunMode::Running;
        nini_tui::runtime::apply_action(
            &mut g,
            nini_tui::keys::Key::new(
                nini_tui::keys::KeyCode::Esc,
                nini_tui::keys::KeyModifiers::NONE,
            ),
        );
        assert_eq!(
            g.run_state.mode,
            nini_tui::state::RunMode::Editing,
            "Esc must hand the editor back immediately — that was v0.10.1's \
             whole point, and the fix must not take it away"
        );
        assert!(
            !g.run_state.mode.eq(&nini_tui::state::RunMode::Quitting),
            "sanity: the interrupt did not start a shutdown"
        );
    }
    assert_eq!(
        live.load(Ordering::SeqCst),
        1,
        "…while the agent task is still alive, because abort is a flag the \
         task observes on its own schedule"
    );

    // ── step 3: Enter ───────────────────────────────────────────────────
    type_and_submit(&shared, &driver, tx.clone(), "second prompt").await;

    let spawned = spawns.load(Ordering::SeqCst);
    let alive = live.load(Ordering::SeqCst);
    println!("spawns={spawned} live={alive}");

    assert_eq!(
        spawned, 1,
        "a second agent was spawned ({spawned} spawns, {alive} live) while \
         the first was still running — both are sharing one SharedState and \
         one session file"
    );
    assert_eq!(alive, 1, "and both are live");
    assert!(
        shared.lock().unwrap().run_state.agent_alive,
        "Esc must not lower the spawn guard either: the first task is still \
         alive, and it is still the one holding the session file"
    );

    // …and the message the user typed was not thrown away: Pi steers a
    // submit during streaming into the running turn.
    let g = shared.lock().unwrap();
    let queued = g.run_state.pending_next_turn_messages.join(" | ");
    assert!(
        queued.contains("second prompt"),
        "the submit was refused instead of steered; the user's text is \
         gone: {queued:?}"
    );
}

// ── the other direction: the guard has to come back down ───────────────

#[tokio::test]
async fn a_done_event_reopens_the_editor_for_the_next_turn() {
    let (shared, _dir) = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            let sink = sink.clone();
            tokio::spawn(async move {
                sink.push(AgentEventLite::TextDelta("hi".into()));
                // The normal end of a turn.
                sink.push(AgentEventLite::Done);
            })
        })
    };

    type_and_submit(&shared, &driver, tx.clone(), "first").await;
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    type_and_submit(&shared, &driver, tx.clone(), "second").await;
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "a finished agent must clear `agent_alive`: the terminal Done event \
         is the only thing that lifts the spawn guard, and if it does not, \
         no message can ever be submitted again"
    );
}

#[tokio::test]
async fn a_driver_that_never_says_done_still_reopens_the_editor() {
    let (shared, _dir) = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // The turn's task ends without a terminal event at all: a panic, an
    // early `return`, a discarded event behind a poisoned lock — any of
    // those reaches the same place. `Done` is a promise made by wiring code
    // outside this crate, so it cannot be the only way down.
    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, _sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async {})
        })
    };

    type_and_submit(&shared, &driver, tx.clone(), "first").await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !shared.lock().unwrap().run_state.agent_alive,
        "a turn that ended without a Done event must still clear the guard"
    );

    type_and_submit(&shared, &driver, tx.clone(), "second").await;
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "the watchdog on the driver's JoinHandle is what keeps a silent \
         death from muting the TUI for the rest of the session"
    );
}

#[tokio::test]
async fn the_done_event_itself_clears_the_guard() {
    // The two tests above go through `submit_user_input`, which installs the
    // watchdog — so they cannot tell "Done cleared the flag" apart from
    // "the watchdog cleared it", and commenting the `Done` arm out would
    // leave them green. This one pushes the event straight at the sink, with
    // no driver and no watchdog behind it, so the terminal event is pinned
    // as the thing that lifts the guard. That matters because `Done` is
    // still the *normal* path: it is what fires on every ordinary turn.
    let (shared, _dir) = fresh_state();
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = nini_tui::state::RunMode::Running;
        g.run_state.agent_alive = true;
    }
    let sink =
        nini_tui::runtime::AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
    sink.push(AgentEventLite::Done);

    let g = shared.lock().unwrap();
    assert_eq!(g.run_state.mode, nini_tui::state::RunMode::Editing);
    assert!(
        !g.run_state.agent_alive,
        "the Done event must clear the spawn guard"
    );
}

#[tokio::test]
async fn an_erroring_turn_reopens_the_editor() {
    let (shared, _dir) = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // Provider blew up mid-stream: `Error` then the terminal `Done`.
    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            let sink = sink.clone();
            tokio::spawn(async move {
                sink.push(AgentEventLite::Error("429 rate limited".into()));
                sink.push(AgentEventLite::Done);
            })
        })
    };

    type_and_submit(&shared, &driver, tx.clone(), "first").await;
    type_and_submit(&shared, &driver, tx.clone(), "second").await;
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "a failed turn must release the guard exactly like a successful one"
    );
}

// ── a late `Done` must not disarm a newer turn ──────────────────────────

/// v0.10.8 (P2/A1): a turn that was aborted can push its terminal event
/// **late** — after the user has already started the next one. An
/// unconditional reset would then clear the *new* turn's guard, which is
/// exactly the window this whole file exists to close.
///
/// The sink knows which turn it belongs to (`AgentSink::with_seq`), so a
/// stale terminal event is recognised and ignored.
#[tokio::test]
async fn a_late_done_from_an_older_turn_does_not_clear_the_newer_turns_guard() {
    let (shared, _dir) = fresh_state();
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = nini_tui::state::RunMode::Running;
        g.run_state.agent_alive = true;
        g.run_state.run_seq = 2; // the **current** turn is #2
    }

    // A sink belonging to turn #1 — the turn that was interrupted.
    let sink = nini_tui::runtime::AgentSink::with_seq(
        shared.clone(),
        tokio::sync::watch::channel(false).0,
        1,
    );
    sink.push(AgentEventLite::Done);

    let g = shared.lock().unwrap();
    assert!(
        g.run_state.agent_alive,
        "a Done event from an interrupted turn cleared the guard of the \
         turn that is running now — the window A1 closed is open again"
    );
    assert_eq!(
        g.run_state.mode,
        nini_tui::state::RunMode::Running,
        "and it also dragged the editor back to Editing while a turn is \
         running"
    );
}

// ── a queued message must land in the session exactly once ──────────────

/// v0.10.8 (P2/A1): the queue used to write the message into the session
/// **twice** — once when it was queued (`push_user`) and once when it was
/// delivered (the queue flush). The model saw it twice and `/resume`
/// showed it twice. The queueing writes to the transcript only; the
/// delivery writes to the session.
#[tokio::test]
async fn a_queued_message_is_written_to_the_session_once() {
    let (shared, dir) = fresh_state();
    let session_file = dir.join("s.jsonl");
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // The turn stays alive for a moment, so the second submit really lands
    // while the first turn is still running (that is the state under test).
    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            let sink = sink.clone();
            tokio::spawn(async move {
                sink.push(AgentEventLite::TextDelta("answer".into()));
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                // `TurnEnd` is what flushes the session to disk; `Done` is
                // what lifts the spawn guard. The real driver sends both.
                sink.push(AgentEventLite::TurnEnd);
                sink.push(AgentEventLite::Done);
            })
        })
    };

    // turn 1
    type_and_submit(&shared, &driver, tx.clone(), "first prompt").await;
    // submitted while the first turn is still alive → queued
    type_and_submit(&shared, &driver, tx.clone(), "queued prompt").await;
    {
        // It must be **queued**, not dropped and not merged into whatever
        // the user types next. An earlier version of the steer path only
        // queued while `mode == Editing`, so a submit during a *running*
        // turn (mode == Running) was silently dropped — the text stayed in
        // the input buffer and came out as part of the next message.
        let g = shared.lock().unwrap();
        let queued = g.run_state.pending_next_turn_messages.join(" | ");
        assert!(
            queued.contains("queued prompt"),
            "a submit during a running turn must be queued, not dropped: \
             {queued:?}"
        );
        assert_eq!(
            g.input.text, "",
            "the queued text must leave the input buffer: {:?}",
            g.input.text
        );
    }
    // let the first turn finish
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    // turn 2 → the queue is flushed here
    type_and_submit(&shared, &driver, tx.clone(), "third prompt").await;
    // The session is written to disk on the turn's terminal event, so the
    // test has to wait for the third turn to end — not just for it to
    // start.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    assert_eq!(spawns.load(Ordering::SeqCst), 2);
    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state.pending_next_turn_messages.is_empty(),
            "the queue is drained when the message is delivered: {:?}",
            g.run_state.pending_next_turn_messages
        );
    }
    let raw = std::fs::read_to_string(&session_file).unwrap_or_default();
    let count = raw.matches("queued prompt").count();
    assert_eq!(
        count, 1,
        "the queued message must be written to the session exactly once, \
         not once when it was queued and again when it was delivered. \
         session file:\n{raw}"
    );
}
