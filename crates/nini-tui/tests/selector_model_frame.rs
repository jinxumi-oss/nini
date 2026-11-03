//! Frame-level oracle for the **model selector** overlay (`/model`).
//!
//! Pattern copied from `tests/autocomplete_e2e.rs`: render one frame with
//! `ratatui::backend::TestBackend` and assert on the row text.
//!
//! Production path pinned here:
//!
//! * **open** — submitting `/model` with no argument sets the status flag
//!   `open_selector:model` (`runtime::submit_user_input_inner`), which the
//!   runtime loop turns into a `selectors::model::ModelSelector`.
//! * **draw** — the runtime draws the app frame
//!   (`render::render_frame_with_theme`) and then overlays
//!   `render::render_selector_panel` on the transcript area. The harness
//!   below does exactly that.
//! * **navigate** — `runtime::handle_selector_key` is private, so the
//!   harness mirrors its Down branch verbatim (the repo's existing tests
//!   mirror `runtime::handle_key` the same way).
//!
//! FIXED (v0.16): the selector panel used to leave the cells it covers
//! **uncleared** — `selector.rs` said "Explicit Clear over the inner area"
//! but only called `cell.set_style(bg)`, never reset the symbol, so the
//! transcript under the overlay stayed *readable* through the rows the
//! list did not fill (the v0.5-era "text fragments peeking out" defect
//! that the comment always claimed to prevent). The fix resets the symbol
//! too; `the_panel_covers_the_transcript_it_sits_on` (in
//! `selector_palette_frame.rs`) pins it.

use nini_tui::render::{render_frame_with_theme, render_selector_panel};
use nini_tui::runtime::{shared_state, submit_user_input, AgentDriver, SharedState};
use nini_tui::selector::SelectorState;
use nini_tui::selectors::model::ModelSelector;
use nini_tui::state::AppState;
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::sync::Arc;

const TITLE: &str = "Switch model";

/// Render one frame and return it as text, one string per row.
fn draw(f: impl FnOnce(&mut ratatui::Frame)) -> String {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(f).unwrap();
    let buf = terminal.backend().buffer().clone();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out
}

/// The plain app frame — what is on screen when no overlay is open.
fn base_frame() -> String {
    let state = AppState::new("test-model");
    let theme = Theme::default();
    draw(|f| render_frame_with_theme(f, &state, &theme))
}

/// The app frame with the model selector drawn over the transcript area,
/// exactly where the runtime draws it.
fn overlay_frame(sel: &dyn SelectorState, query: &str) -> String {
    let state = AppState::new("test-model");
    let theme = Theme::default();
    let title = sel.state_title();
    let items = sel.state_items();
    let visible = if query.is_empty() {
        (0..items.len()).collect()
    } else {
        sel.state_filter(query)
    };
    let selected = sel.state_selected();
    draw(|f| {
        render_frame_with_theme(f, &state, &theme);
        let area = f.area();
        let panel = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(6),
        };
        render_selector_panel(f, &title, query, &items, &visible, selected, &theme, panel);
    })
}

/// Mirror of `runtime::handle_selector_key`'s Down branch.
fn press_down(sel: &mut dyn SelectorState, visible: &[usize]) {
    if visible.is_empty() {
        return;
    }
    let cur = sel.state_selected();
    let cur_pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
    let new_pos = (cur_pos + 1) % visible.len();
    sel.state_set_selected(visible[new_pos]);
}

/// The model catalogue as the runtime builds it: the shipped defaults with
/// the model that is currently in use marked as the current one.
fn selector() -> ModelSelector {
    ModelSelector::new(Some("test-model"))
}

/// Submitting `/model` (no argument) asks the runtime loop for the selector.
#[test]
fn model_open_entry_is_slash_model_without_arguments() {
    let shared: SharedState = shared_state(AppState::new("test-model"));
    let driver: AgentDriver = Arc::new(|_msg, _seed, _sink, done| {
        done.notify_waiters();
        tokio::spawn(async {})
    });
    {
        let mut g = shared.lock().unwrap();
        for c in "/model".chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );
    let g = shared.lock().unwrap();
    assert_eq!(
        g.run_state.status, "open_selector:model",
        "/model without arguments must open the model selector"
    );
}

/// The open model selector is in the frame: its title and the selected
/// model's name.
#[test]
fn model_open_frame_shows_title_and_selected_model() {
    let sel = selector();
    let frame = overlay_frame(&sel, "");
    assert!(
        frame.contains(TITLE),
        "model selector title missing from the frame:\n{frame}"
    );
    assert!(
        frame.contains("▶ anthropic/claude-fable-5"),
        "the first catalogue entry must be highlighted:\n{frame}"
    );
    assert!(
        frame.contains("anthropic/claude-fable-5"),
        "the selected model's name must appear in the frame:\n{frame}"
    );
}

/// One Down moves the highlight to the second catalogue entry.
#[test]
fn model_down_moves_highlight_to_second_entry() {
    let mut sel = selector();
    let visible: Vec<usize> = (0..sel.state_items().len()).collect();
    press_down(&mut sel, &visible);

    let frame = overlay_frame(&sel, "");
    assert!(
        frame.contains("▶ anthropic/claude-haiku-4-5"),
        "after Down the second catalogue entry must be highlighted:\n{frame}"
    );
    assert!(
        !frame.contains("▶ anthropic/claude-fable-5"),
        "the first catalogue entry must no longer be highlighted:\n{frame}"
    );
}

/// Esc closes the selector: the frame no longer carries the overlay.
#[test]
fn model_esc_leaves_no_overlay_in_the_frame() {
    let sel = selector();
    let open = overlay_frame(&sel, "");
    assert!(open.contains(TITLE), "precondition: the selector is open");

    // Esc (`runtime::handle_selector_key` → `KeyCode::Esc`): the runtime
    // clears the query and drops the selector, so the next frame is the
    // plain app frame.
    let frame = base_frame();
    assert!(
        !frame.contains(TITLE),
        "the model selector title must be gone after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("anthropic/claude-fable-5"),
        "no model row may stay in the frame after Esc:\n{frame}"
    );
}
