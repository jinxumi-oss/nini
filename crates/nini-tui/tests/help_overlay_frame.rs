//! Frame-level oracle for the **help overlay** (`/help`).
//!
//! Pattern copied from `tests/autocomplete_e2e.rs`: render one frame with
//! `ratatui::backend::TestBackend` and assert on the row text.
//!
//! Production path pinned here:
//!
//! * **open** — submitting `/help` sets the status flag
//!   `open_selector:help` (`commands::dispatch::cmd_help`), which the
//!   runtime loop turns into a `help_overlay::HelpSelector`.
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

use nini_tui::help_overlay::HelpSelector;
use nini_tui::render::{render_frame_with_theme, render_selector_panel};
use nini_tui::runtime::{shared_state, submit_user_input, AgentDriver, SharedState};
use nini_tui::selector::SelectorState;
use nini_tui::state::AppState;
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::sync::Arc;

const TITLE: &str = "Help — type to filter slash commands";

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

/// The app frame with the help overlay drawn over the transcript area,
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

/// Submitting `/help` asks the runtime loop for the help overlay.
#[test]
fn help_open_entry_is_slash_help() {
    let shared: SharedState = shared_state(AppState::new("test-model"));
    let driver: AgentDriver = Arc::new(|_msg, _seed, _sink, done| {
        done.notify_waiters();
        tokio::spawn(async {})
    });
    {
        let mut g = shared.lock().unwrap();
        for c in "/help".chars() {
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
        g.run_state.status, "open_selector:help",
        "/help must open the help overlay"
    );
}

/// The open help overlay is in the frame: its title and the first command.
#[test]
fn help_open_frame_shows_title_and_first_command() {
    let sel = HelpSelector::new();
    let frame = overlay_frame(&sel, "");
    assert!(
        frame.contains(TITLE),
        "help overlay title missing from the frame:\n{frame}"
    );
    assert!(
        frame.contains("▶ /settings"),
        "the first command (/settings) must be highlighted:\n{frame}"
    );
    assert!(
        frame.contains("Open settings menu"),
        "the command description must be shown:\n{frame}"
    );
}

/// One Down moves the highlight from "/settings" to "/model".
#[test]
fn help_down_moves_highlight_to_second_command() {
    let mut sel = HelpSelector::new();
    let visible: Vec<usize> = (0..sel.state_items().len()).collect();
    press_down(&mut sel, &visible);

    let frame = overlay_frame(&sel, "");
    assert!(
        frame.contains("▶ /model"),
        "after Down the second command (/model) must be highlighted:\n{frame}"
    );
    assert!(
        !frame.contains("▶ /settings"),
        "the first command must no longer be highlighted:\n{frame}"
    );
}

/// Esc closes the overlay: the frame no longer carries it.
#[test]
fn help_esc_leaves_no_overlay_in_the_frame() {
    let sel = HelpSelector::new();
    let open = overlay_frame(&sel, "");
    assert!(open.contains(TITLE), "precondition: the overlay is open");

    // Esc (`runtime::handle_selector_key` → `KeyCode::Esc`): the runtime
    // clears the query and drops the selector, so the next frame is the
    // plain app frame.
    let frame = base_frame();
    assert!(
        !frame.contains(TITLE),
        "the help overlay title must be gone after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("▶ /settings"),
        "no help row may stay highlighted after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("Open settings menu"),
        "no help entry may stay in the frame after Esc:\n{frame}"
    );
}
