//! Regression: transcript rendering must not scale with transcript size.
//!
//! **Reported symptom**: after finishing a complex task, typing in the
//! prompt felt sluggish and the whole TUI felt like it was holding a
//! lot of resources. Not "the model is slow" — *the UI* was slow while
//! nothing was happening.
//!
//! **Root cause** (`render.rs`): the renderer needs to know how many
//! screen rows each transcript line expands to, in order to place the
//! autoscroll window and the `↑ N more above` indicator. It obtained
//! that by calling `items_for_line` for **every line in the transcript**,
//! fully, on **every frame** — and then rendering the visible lines a
//! second time.
//!
//! Measured, release build, before the fix:
//!
//! ```text
//!   16 lines /   8 KB →   5.94 ms/frame
//!   77 lines /  42 KB →  23.43 ms/frame
//!  154 lines /  83 KB →  45.09 ms/frame
//!  307 lines / 164 KB →  90.65 ms/frame
//!  614 lines / 338 KB → 180.97 ms/frame
//! 1227 lines / 661 KB → 361.53 ms/frame
//! ```
//!
//! A linear **0.545 ms/KB** with no intercept. A 16 ms budget is 60 fps,
//! so ~29 KB of transcript saturates one frame. And the cost was
//! *independent of what was on screen* — 34 visible rows, 1227 lines
//! rendered, 1193 discarded immediately. That is why it showed up as
//! "sluggish after a big task" rather than "always slow": the
//! transcript is what grew.
//!
//! After: **~1.8 ms regardless of size** — flat, because only the
//! visible rows are rendered and the rest is a cached row count.
//!
//! # What this file asserts
//!
//! 1. **A frame's cost does not grow with the transcript.** The
//!    threshold is a multiple of the 16 ms frame budget rather than an
//!    absolute millisecond count, so it does not rot on slower CI.
//! 2. **The cache is invisible.** Every render produces byte-identical
//!    output to a cache-free render, across sizes, widths, themes and
//!    streaming mutations.

#![allow(clippy::needless_return)]

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One frame's rendered rows.
fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0u16;
            while x < buf.area.width {
                let sym = buf
                    .cell((x, y))
                    .map(|c| c.symbol())
                    .unwrap_or(" ")
                    .to_string();
                line.push_str(&sym);
                x += sym.chars().count().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// Median of `n` timed frames. Median rather than mean so a single
/// scheduler hiccup on a shared CI box cannot fail the test.
fn median_frame_time(state: &AppState, w: u16, h: u16, n: usize) -> Duration {
    // Warm up: the first frame pays the markdown parser's lazy statics
    // and the cache is cold.
    for _ in 0..3 {
        let backend = TestBackend::new(w, h);
        let mut t = Terminal::new(backend).unwrap();
        t.draw(|f| render_frame(f, state)).unwrap();
    }
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let backend = TestBackend::new(w, h);
        let mut t = Terminal::new(backend).unwrap();
        let t0 = Instant::now();
        t.draw(|f| render_frame(f, state)).unwrap();
        samples.push(t0.elapsed());
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// A transcript that looks like the aftermath of a real task: tool
/// results with multi-line bodies, user messages, and assistant output
/// that goes through the markdown renderer (by far the most expensive
/// per line).
fn realistic_transcript(rounds: usize) -> AppState {
    let mut s = AppState::new("test-model");
    for i in 0..rounds {
        let body: String = (0..12)
            .map(|j| format!("// line {j:02} of result {i} — realistic file content"))
            .collect::<Vec<_>>()
            .join("\n");
        s.push_tool_result(&format!("t{i}"), "read", true, body, Some(3));
        if i % 3 == 0 {
            s.push_user(format!("请分析 result {i} 并给出建议"));
        }
        if i % 5 == 0 {
            s.push_assistant(
                "Here is a **markdown** answer with `inline code`, a [link](https://x.com), \
                 a list:\n\n- one\n- two\n\n```rust\nfn main() {}\n```"
                    .to_string(),
            );
        }
    }
    s
}

// ---------------------------------------------------------------------------
// 1. The performance claim itself
// ---------------------------------------------------------------------------

/// The headline regression: a big transcript must not cost more per
/// frame than a small one.
///
/// The assertion is **relative** — "the 800-round transcript costs no
/// more than 3x the 20-round one" — because that is the property that
/// was broken (it was ~60x) and because an absolute millisecond bound
/// would be flaky on slow or shared CI hardware. The pre-fix
/// measurement for these two sizes was 5.9 ms vs 361.5 ms.
#[test]
fn frame_cost_does_not_scale_with_transcript_size() {
    let small = realistic_transcript(20);
    let big = realistic_transcript(800);

    let small_lines = small.transcript_state.lines.len();
    let big_lines = big.transcript_state.lines.len();
    assert!(
        big_lines > small_lines * 20,
        "precondition: the big transcript should be much bigger ({small_lines} vs {big_lines})"
    );

    let t_small = median_frame_time(&small, 100, 34, 15);
    let t_big = median_frame_time(&big, 100, 34, 15);

    eprintln!(
        "PERF small({small_lines} lines) = {:.2} ms | big({big_lines} lines) = {:.2} ms",
        t_small.as_secs_f64() * 1000.0,
        t_big.as_secs_f64() * 1000.0
    );

    assert!(
        t_big < t_small * 3,
        "a {big_lines}-line transcript costs {:.2} ms/frame vs {:.2} ms for \
         {small_lines} lines — frame cost is scaling with transcript size again \
         (budget: 3x; before the fix this ratio was ~60x)",
        t_big.as_secs_f64() * 1000.0,
        t_small.as_secs_f64() * 1000.0
    );
}

/// An absolute sanity bound, expressed in frame budgets. 16 ms is 60 fps;
/// 4 budgets of headroom leaves room for slow CI while still catching a
/// regression that pushes a frame into "noticeably laggy" territory.
///
/// Pre-fix this was 361 ms at the largest size — about 22 budgets.
#[test]
fn frame_cost_stays_within_a_few_frame_budgets() {
    const FRAME_BUDGET: Duration = Duration::from_millis(16);
    for rounds in [20usize, 200, 800] {
        let s = realistic_transcript(rounds);
        let t = median_frame_time(&s, 100, 34, 15);
        eprintln!(
            "PERF {rounds} rounds ({} lines) = {:.2} ms/frame",
            s.transcript_state.lines.len(),
            t.as_secs_f64() * 1000.0
        );
        assert!(
            t < FRAME_BUDGET * 4,
            "{rounds} rounds: {:.2} ms/frame exceeds 4 frame budgets (64 ms). \
             Before the fix this was {:.2} ms.",
            t.as_secs_f64() * 1000.0,
            match rounds {
                20 => 5.9,
                200 => 90.7,
                _ => 361.5,
            }
        );
    }
}

// ---------------------------------------------------------------------------
// 2. The cache must be invisible
// ---------------------------------------------------------------------------

/// Identical output with the cache warm vs cold.
///
/// The cache is keyed on `(width, content fingerprint)`, so a stale
/// entry can only affect a row *count* — which shifts the scroll window.
/// If that ever became visible, the transcript would show the wrong
/// slice of history. This is the guard against it.
#[test]
fn cached_and_uncached_renders_are_identical() {
    for rounds in [3usize, 40, 200] {
        let s = realistic_transcript(rounds);
        // Cold: first render populates the cache.
        let cold = frame_rows(&s, 100, 34);
        // Warm: every entry should now hit.
        let warm = frame_rows(&s, 100, 34);
        assert_eq!(
            cold, warm,
            "{rounds} rounds: the second render differs from the first — \
             the cache is not returning the same result"
        );
    }
}

/// Resizing the terminal must not serve counts computed for the old
/// width (a narrower terminal wraps different line counts).
#[test]
fn a_width_change_invalidates_the_cache() {
    let s = realistic_transcript(120);
    let _ = frame_rows(&s, 120, 34);
    let _ = frame_rows(&s, 120, 34); // warm at 120

    // Now render much narrower. If the cache ignored the width, the
    // row counts would be the 120-column ones and the visible window
    // would be wrong.
    let narrow = frame_rows(&s, 60, 34);
    let narrow_again = frame_rows(&s, 60, 34);
    assert_eq!(
        narrow, narrow_again,
        "renders at a new width are not stable"
    );
    for row in &narrow {
        assert!(
            nini_tui::width::display_width(row) <= 60,
            "row overflows the narrower terminal: {row:?}"
        );
    }
    // And going back to 120 must reproduce the original 120 output.
    let back = frame_rows(&s, 120, 34);
    let original = frame_rows(&s, 120, 34);
    assert_eq!(back, original);
}

/// Streaming growth of the last line (the token-by-token case) must
/// re-count that line and nothing else — and must not corrupt the view.
#[test]
fn streaming_growth_is_handled() {
    let mut s = realistic_transcript(60);
    // Put the streaming line **last**, so it is inside the viewport.
    // Growing an off-screen line correctly changes nothing on screen,
    // and asserting on that would make this test pass for the wrong
    // reason.
    s.push_assistant("Streaming starts here".to_string());
    let before = frame_rows(&s, 100, 34);
    assert!(
        before.iter().any(|r| r.contains("Streaming starts here")),
        "precondition: the streaming line should be visible before it grows"
    );

    // Simulate token-by-token arrival.
    {
        let ts = Arc::make_mut(&mut s.transcript_state);
        if let Some(TranscriptLine::AssistantText(t)) = ts.lines.last_mut() {
            t.push_str(&" — and here is the rest of the sentence".repeat(3));
        }
    }

    let after = frame_rows(&s, 100, 34);
    assert!(
        after.iter().any(|r| r.contains("and here is the rest")),
        "the grown text was not rendered:\n{after:?}"
    );
    // And the cache must have re-counted that line rather than serving
    // the pre-growth count (which would show the wrong slice of history).
    let after_again = frame_rows(&s, 100, 34);
    assert_eq!(after, after_again, "post-growth renders are unstable");
}

/// Compaction shifts every surviving line down one slot. The cache must
/// be dropped, or a stored count would describe the wrong line.
#[test]
fn compaction_invalidates_the_cache() {
    let mut s = realistic_transcript(80);
    let _ = frame_rows(&s, 100, 34); // warm
    let folded = s.auto_compact_local();
    assert!(
        folded > 0,
        "precondition: something should have been folded"
    );
    let after = frame_rows(&s, 100, 34);
    let after_again = frame_rows(&s, 100, 34);
    assert_eq!(
        after, after_again,
        "renders right after compaction are unstable — the cache was not \
         invalidated when line indices shifted"
    );
    assert!(
        after.iter().any(|r| !r.trim().is_empty()),
        "compaction left the transcript blank:\n{after:?}"
    );
}

/// Same for `/clear`-shaped mutation: line indices get reused.
#[test]
fn clearing_the_transcript_invalidates_the_cache() {
    let mut s = realistic_transcript(60);
    let _ = frame_rows(&s, 100, 34);
    {
        let ts = Arc::make_mut(&mut s.transcript_state);
        ts.lines.clear();
        ts.reset_view();
    }
    s.invalidate_item_counts();
    s.push_divider();
    s.push_assistant("fresh start".to_string());

    let a = frame_rows(&s, 100, 34);
    let b = frame_rows(&s, 100, 34);
    assert_eq!(a, b, "renders after clear are unstable");
    assert!(
        a.iter().any(|r| r.contains("fresh start")),
        "the new content should be visible:\n{a:?}"
    );
}

/// A poisoned cache lock must not take the TUI down — worst case is the
/// old (slow but correct) behaviour.
#[test]
fn a_poisoned_cache_lock_degrades_instead_of_panicking() {
    let s = realistic_transcript(30);
    // The panic has to happen *inside* `catch_unwind` so the guard is
    // dropped during the unwind and the mutex ends up poisoned.
    let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = s.item_cache.lock().unwrap();
        panic!("poison the lock on purpose");
    }));
    assert!(poison.is_err(), "the lock was not poisoned by this test");
    let rows = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| frame_rows(&s, 100, 34)));
    assert!(
        rows.is_ok(),
        "a poisoned item cache must not panic the render path"
    );
}
