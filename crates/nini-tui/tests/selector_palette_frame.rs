//! Frame-level oracle for the **command palette** overlay (Ctrl+Shift+K).
//!
//! Pattern copied from `tests/autocomplete_e2e.rs`: render one frame with
//! `ratatui::backend::TestBackend` and assert on the row text.
//!
//! Production path pinned here:
//!
//! * **open** — `keys::resolve` maps Ctrl+Shift+K to
//!   `KeyAction::OpenCommandPalette`; `runtime::apply_action` turns that into
//!   the status flag `open_selector:palette`, which the runtime loop turns
//!   into a `command_palette::CommandPalette`.
//! * **draw** — the runtime draws the app frame
//!   (`render::render_frame_with_theme`) and then overlays
//!   `render::render_selector_panel` on the transcript area. The harness
//!   below does exactly that, so the asserted frames are the frames the
//!   user sees.
//! * **navigate** — `runtime::handle_selector_key` (the Down/Esc router for
//!   an open overlay) is private, so the harness mirrors its Down branch
//!   verbatim. The repo's existing tests mirror `runtime::handle_key` the
//!   same way (`tests/agent_wire_e2e.rs`, `tests/tui_e2e.rs`).
//!
//! FIXED (v0.16): the selector panel used to leave the cells it covers
//! **uncleared** — `selector.rs` said "Explicit Clear over the inner area"
//! but only called `cell.set_style(bg)`, never reset the symbol, so the
//! transcript under the overlay stayed *readable* through the rows the
//! list did not fill (the v0.5-era "text fragments peeking out" defect
//! that the comment always claimed to prevent). The fix resets the symbol
//! too; `the_panel_covers_the_transcript_it_sits_on` (in
//! `selector_palette_frame.rs`) pins it.

use nini_tui::command_palette::CommandPalette;
use nini_tui::keys::{resolve, Key, KeyAction, KeyCode, KeyModifiers};
use nini_tui::render::{render_frame_with_theme, render_selector_panel};
use nini_tui::runtime::apply_action;
use nini_tui::selector::SelectorState;
use nini_tui::state::AppState;
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

const TITLE: &str = "Command palette — type to fuzzy-search all commands";

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

/// The app frame with the palette drawn over the transcript area, exactly
/// where the runtime draws it.
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

/// Mirror of `runtime::handle_selector_key`'s Down branch: move the cursor
/// one row down *within the visible list*, then translate the visible
/// position back into an index into the full item list.
fn press_down(sel: &mut dyn SelectorState, visible: &[usize]) {
    if visible.is_empty() {
        return;
    }
    let cur = sel.state_selected();
    let cur_pos = visible.iter().position(|&i| i == cur).unwrap_or(0);
    let new_pos = (cur_pos + 1) % visible.len();
    sel.state_set_selected(visible[new_pos]);
}

/// The palette's visible list for an empty query (the runtime uses the
/// whole item list when the query is empty).
fn all_visible(sel: &dyn SelectorState) -> Vec<usize> {
    (0..sel.state_items().len()).collect()
}

/// Ctrl+Shift+K is the palette chord, and it opens the palette.
#[test]
fn palette_open_entry_is_ctrl_shift_k() {
    let action = resolve(
        &nini_tui::keys::default_keymap(),
        Key::new(KeyCode::Char('k'), KeyModifiers::CTRL | KeyModifiers::SHIFT),
    );
    assert_eq!(action, KeyAction::OpenCommandPalette);

    let mut state = AppState::new("test-model");
    apply_action(
        &mut state,
        Key::new(KeyCode::Char('k'), KeyModifiers::CTRL | KeyModifiers::SHIFT),
    );
    assert_eq!(
        state.run_state.status, "open_selector:palette",
        "Ctrl+Shift+K must ask the runtime loop for the palette"
    );
}

/// The open palette is in the frame: its title and its first command.
#[test]
fn palette_open_frame_shows_title_and_first_command() {
    let palette = CommandPalette::new();
    let frame = overlay_frame(&palette, "");
    assert!(
        frame.contains(TITLE),
        "palette title missing from the frame:\n{frame}"
    );
    assert!(
        frame.contains("▶ /settings"),
        "the first palette row (/settings) must be highlighted:\n{frame}"
    );
    assert!(
        frame.contains("/model"),
        "the palette must list the /model command:\n{frame}"
    );
}

/// One Down moves the highlight from the first row to the second one.
#[test]
fn palette_down_moves_highlight_to_second_command() {
    let mut palette = CommandPalette::new();
    let visible = all_visible(&palette);
    press_down(&mut palette, &visible);

    let frame = overlay_frame(&palette, "");
    assert!(
        frame.contains("▶ /model"),
        "after Down the second command (/model) must be highlighted:\n{frame}"
    );
    assert!(
        !frame.contains("▶ /settings"),
        "the first command must no longer be highlighted:\n{frame}"
    );
}

/// Typing filters the list: the empty-query frame and the "quit" frame
/// differ, and only the matching commands survive.
#[test]
fn palette_query_filters_the_list() {
    let palette = CommandPalette::new();
    let empty = overlay_frame(&palette, "");
    let filtered = overlay_frame(&palette, "quit");

    assert_ne!(empty, filtered, "a query must change the rendered frame");
    assert!(
        filtered.contains("› quit"),
        "the typed query must be shown in the input row:\n{filtered}"
    );
    assert!(
        filtered.contains("▶ /quit"),
        "the first match (/quit) must be highlighted:\n{filtered}"
    );
    assert!(
        filtered.contains("Quit nini"),
        "the meta action 'Quit nini' must match too:\n{filtered}"
    );
    assert!(
        !filtered.contains("/settings"),
        "a non-matching command must be filtered out:\n{filtered}"
    );
    assert!(
        empty.contains("/settings"),
        "the unfiltered frame must still list /settings"
    );
}

/// Esc closes the palette: the frame no longer carries the overlay.
#[test]
fn palette_esc_leaves_no_overlay_in_the_frame() {
    let palette = CommandPalette::new();
    let open = overlay_frame(&palette, "");
    assert!(open.contains(TITLE), "precondition: the palette is open");

    // Esc (`runtime::handle_selector_key` → `KeyCode::Esc`): the query and
    // the visible list are cleared and the runtime drops the selector, so
    // the next frame is the plain app frame.
    let frame = base_frame();
    assert!(
        !frame.contains(TITLE),
        "the palette title must be gone after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("▶ /settings"),
        "no palette row may stay highlighted after Esc:\n{frame}"
    );
    assert!(
        !frame.contains("Quit nini"),
        "no palette entry may stay in the frame after Esc:\n{frame}"
    );
}

/// The panel must **cover** the transcript it sits on.
///
/// v0.5-era defect ("text fragments peeking out between selector rows"),
/// still true in v0.15: `selector.rs` said "Explicit Clear over the inner
/// area" but only called `cell.set_style(bg)` — it repainted the
/// background and left the **symbols**, so the welcome card under the
/// overlay stayed readable through the rows the list did not fill.
///
/// The fix resets the symbol as well. This assertion is what keeps it
/// fixed: it is the difference between "the panel is drawn" and "the
/// panel is opaque".
#[test]
fn the_panel_covers_the_transcript_it_sits_on() {
    let sel = CommandPalette::new();
    let frame = overlay_frame(&sel, "");
    assert!(
        frame.contains(TITLE),
        "the palette must be on screen:\n{frame}"
    );
    // The bleed-through shows up as **fragments**: the welcome card is
    // wider than the list rows, so the parts of it that fall between two
    // list items stayed on screen ("…rt. Examples below."). So the
    // assertion has to look for a fragment, not for the whole line — the
    // whole line is covered by the panel's own rows.
    assert!(
        !frame.contains("Examples below."),
        "the welcome card under the panel must not show through the rows the \
         list does not fill:\n{frame}"
    );
    assert!(
        !frame.contains("Type a prompt and press Enter"),
        "the welcome card under the panel must not show through the rows the \
         list does not fill:\n{frame}"
    );
}
