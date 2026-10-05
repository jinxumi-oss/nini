//! Regression: a panic anywhere must not kill the TUI permanently.
//!
//! **The failure this prevents.** The runtime had 29
//! `shared.lock().unwrap()` calls. A `Mutex` is poisoned when a thread
//! panics while holding it, and every later `lock()` on a poisoned mutex
//! returns `Err`. So `unwrap()` turns **one** panic into a permanent
//! condition: the very first `lock().unwrap()` on the render path —
//! i.e. the next frame — panics again, and again, for the rest of the
//! process. The user is left with a frozen or blank terminal and no way
//! out short of killing nini.
//!
//! That is a far worse outcome than the panic that started it, and it is
//! not hypothetical for this codebase: v0.9.1 shipped fixes for three
//! separate panics reachable from ordinary input — an integer overflow
//! in `read`, a UTF-8 boundary panic in bash output truncation, and a
//! concurrent-agent indexing panic.
//!
//! # What this asserts
//!
//! 1. `lock_state` hands back a usable guard on a poisoned mutex.
//! 2. The render path survives a poisoned lock.
//! 3. A simulated runtime tick (status update + a key action) survives.
//! 4. The global event bus survives.

#![allow(clippy::needless_return)]

use nini_tui::render::render_frame;
use nini_tui::runtime::{lock_state, shared_state};
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect()
}

/// Poison `shared` by panicking while the guard is alive.
fn poison(shared: &nini_tui::runtime::SharedState) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = shared.lock().unwrap();
        panic!("panic while holding the state lock");
    }));
    assert!(result.is_err(), "the lock was not poisoned");
    // Confirm the precondition rather than assuming it.
    assert!(
        shared.is_poisoned(),
        "mutex should report poisoned after the unwind"
    );
}

#[test]
fn lock_state_recovers_a_poisoned_mutex() {
    let shared = shared_state(AppState::new("test-model"));
    poison(&shared);

    let guard = lock_state(&shared);
    // The guard is usable: this is the whole point.
    assert_eq!(guard.model_state.model, "test-model");
    drop(guard);

    // And it keeps working, rather than panicking once and only once.
    for _ in 0..3 {
        drop(lock_state(&shared));
    }
}

#[test]
fn the_render_path_survives_a_poisoned_lock() {
    let shared = shared_state(AppState::new("test-model"));
    lock_state(&shared).push_user("before the panic".to_string());
    poison(&shared);

    // The render path is the very first thing that would have panicked
    // before the fix, because it locks on every single frame.
    for _ in 0..3 {
        let guard = lock_state(&shared);
        let rows = frame_rows(&guard, 100, 30);
        assert!(
            rows.iter().any(|r| !r.trim().is_empty()),
            "the frame rendered empty"
        );
        assert!(
            rows.iter().any(|r| r.contains("before the panic")),
            "transcript content lost after poisoning:\n{rows:?}"
        );
    }
}

#[test]
fn a_runtime_tick_survives_a_poisoned_lock() {
    // Stands in for what the runtime loop actually does each iteration:
    // read status, write status, mutate transcript, handle a key.
    let shared = shared_state(AppState::new("test-model"));
    poison(&shared);

    {
        let mut g = lock_state(&shared);
        g.run_state.status = "still alive".to_string();
    }
    {
        let g = lock_state(&shared);
        assert_eq!(g.run_state.status, "still alive");
    }
    {
        let mut g = lock_state(&shared);
        g.push_user("after the panic".to_string());
        g.push_divider();
    }
    {
        let g = lock_state(&shared);
        let rows = frame_rows(&g, 100, 30);
        assert!(
            rows.iter().any(|r| r.contains("after the panic")),
            "new content did not render:\n{rows:?}"
        );
    }
}

#[test]
fn the_global_event_bus_survives_a_poisoning_panic() {
    // A poisoned *global* outlives every session, so a single panic
    // would make the next run of nini panic on its first emitted event.
    use nini_tui::event_bus;
    use nini_tui::event_bus::AppEvent;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = event_bus::global();
        panic!("poison the global bus");
    }));
    assert!(result.is_err());

    // Must not panic on the next use.
    for _ in 0..3 {
        event_bus::global().emit(AppEvent::ThemeChanged("x".into()));
    }
}

#[test]
fn a_panic_in_one_region_does_not_take_down_unrelated_work() {
    // The point of recovering the guard rather than propagating: state
    // touched before the panic is still intact, so the TUI can carry on
    // drawing something truthful instead of dying.
    let shared = shared_state(AppState::new("test-model"));
    {
        let mut g = lock_state(&shared);
        g.push_user("written before the panic".to_string());
    }
    poison(&shared);

    let guard = lock_state(&shared);
    let lines = &guard.transcript_state.lines;
    assert_eq!(lines.len(), 1);
    assert!(
        matches!(&lines[0], nini_tui::state::TranscriptLine::User(t) if t == "written before the panic")
    );
}
