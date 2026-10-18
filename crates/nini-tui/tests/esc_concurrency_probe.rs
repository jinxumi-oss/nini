//! Probe (2026-10-18): can a second agent be spawned while the first is
//! still winding down?
//!
//! Not a proposed test — a measurement. It reproduces the state a real
//! interrupt leaves behind and then submits again, counting how many
//! times the driver is invoked.
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
//! The only guard against a second spawn is `mode == RunMode::Running`
//! (`KeyAction::Submit`). Step 2 cleared that. So step 3 should spawn
//! driver #2 while #1 is still running, and both write to the same
//! `SharedState` and the same session file.

use nini_tui::runtime::{submit_user_input, AgentDriver, SharedState};
use nini_tui::state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// **Ignored on purpose: this is a confirmed defect, not a passing test.**
///
/// It is kept, ignored, because the mechanism is the kind that gets
/// re-derived wrongly later: the fix belongs in `runtime.rs` (split the
/// "editor is usable" fact from the "an agent is running" fact), and this
/// file is the executable version of that reasoning. Run it with
///
/// ```console
/// cargo test -p nini-tui --test esc_concurrency_probe -- --ignored --nocapture
/// ```
///
/// Measured result on v0.10.2: **`spawns=2 live=2`**.
///
/// Pi cannot reach this state — `AgentSession.isIdle` is
/// `!_isAgentRunActive` (`agent-session.js:592`), a property of the
/// *agent*, and Esc only calls `agent.abort()` without touching it. A
/// submit during streaming is steered into the running turn
/// (`streamingBehavior`), so a second agent is never spawned. nini has one
/// `RunMode` field doing both jobs, and v0.10.1 set it to `Editing` the
/// instant Esc was pressed — which is what made the editor responsive
/// again, and which also removed the only guard against a second spawn.
#[tokio::test]
#[ignore = "documents a confirmed defect; see GOALS.md P0-6"]
async fn submitting_soon_after_an_esc_starts_a_second_agent() {
    let dir = std::env::temp_dir().join(format!("nini-escwin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let shared: SharedState = {
        let mut g = AppState::new("test-model");
        g.session_init(dir.join("s.jsonl"));
        g.push_user("a question that is still streaming");
        Arc::new(std::sync::Mutex::new(g))
    };

    let live = Arc::new(AtomicUsize::new(0));
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // The driver stands in for an agent task that has been told to abort
    // but has not yet finished: it increments a counter, signals "running",
    // and waits on a channel nobody will ever close.
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
    {
        let mut g = shared.lock().unwrap();
        for c in "first prompt".chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "one turn should be running");
    assert_eq!(live.load(Ordering::SeqCst), 1, "that turn should be live");

    // ── step 2: Esc ─────────────────────────────────────────────────────
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = nini_tui::state::RunMode::Running;
        nini_tui::runtime::apply_action(
            &mut g,
            nini_tui::keys::Key::new(nini_tui::keys::KeyCode::Esc, nini_tui::keys::KeyModifiers::NONE),
        );
        assert_eq!(
            g.run_state.mode,
            nini_tui::state::RunMode::Editing,
            "Esc must hand the editor back immediately — that was v0.10.1's \
             whole point"
        );
    }
    assert_eq!(
        live.load(Ordering::SeqCst),
        1,
        "…while the agent task is still alive, because abort is a flag the \
         task observes on its own schedule"
    );

    // ── step 3: Enter ───────────────────────────────────────────────────
    {
        let mut g = shared.lock().unwrap();
        for c in "second prompt".chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let spawned = spawns.load(Ordering::SeqCst);
    let alive = live.load(Ordering::SeqCst);
    println!("spawns={spawned} live={alive}");

    assert_eq!(
        spawned, 1,
        "expected the window to be open: a second agent was spawned \
         ({spawned} spawns, {alive} live) sharing one SharedState and one \
         session file"
    );
}
