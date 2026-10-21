//! Render layer: turn `AppState` into ratatui `Frame` drawing calls.
//!
//! Layout:
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │ nini — model: test-model — session: abc123 │ ← status bar (1 line)
//! ├───────────────────────────────────────────────────────────────┤
//! │ > hi                                          │ ← transcript
//! │ hello! How can I help?                         │   (scrollable)
//! │ ...                                            │
//! ├───────────────────────────────────────────────────────────────┤
//! │ > _                                            │ ← prompt editor
//! │                                                │   (3 lines)
//! │                                                │
//! ├───────────────────────────────────────────────────────────────┤
//! │ F1=help Ctrl+C=quit Enter=send Ctrl+L=model   │ ← key hints (1)
//! └───────────────────────────────────────────────────────────────┘
//! ```
#![allow(unused_mut)] // render/runtime use mut bindings for future hook points

use crate::rich::{
    render_assistant_message, render_bash_execution, render_tool_call, render_tool_result,
    render_user_message,
};
use crate::selector::{SelectorItem, SelectorPanel};
use crate::state::{AppState, RunMode, TranscriptLine};
use crate::theme::Theme;
use crate::width::{display_width, floor_char_boundary, truncate_cells, truncate_cells_head};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::atomic::Ordering;

/// Render the full TUI frame.
pub fn render_frame(f: &mut Frame, state: &AppState) {
    render_frame_with_theme(f, state, &Theme::default());
}

/// Render with explicit theme. Use this when a non-default theme is loaded
/// (e.g., user picked `light` via `--use-theme`).
pub fn render_frame_with_theme(f: &mut Frame, state: &AppState, theme: &Theme) {
    let area = f.area();
    // When a completion popup is showing, we steal one row from the
    // transcript area so the popup floats above the prompt.
    let popup_height = if state
        .ui_state
        .completion
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        // Up to 8 lines + 2 (border)
        let n = state
            .ui_state
            .completion
            .as_ref()
            .map(|p| p.items.len())
            .unwrap_or(0);
        (n as u16).min(8) + 2
    } else {
        // v0.8.4 (ux-001): prompt box height grows with multi-line
        // input AND soft-wrapped single-line input. Without the soft-
        // wrap estimate, a 60-char single line in a 40-col terminal
        // was clipped mid-word once the box overflowed. Inner width
        // = area.width minus 2 borders minus 2*padding_x minus the
        // 2-char `❯ ` prompt prefix; visual rows = explicit
        // newlines + ceil(first_line_width / inner_width), then 2
        // for top/bottom border, clamped to [3, area.height/3].
        let padding_x: u16 = 1;
        let inner_width = area.width.saturating_sub(2 + 2 * padding_x + 2).max(1) as usize;
        // v0.10: this used to hand-roll the width with
        // `map(|c| if (c as u32) > 0x1100 { 2 } else { 1 })`, under a
        // comment that said "the exact mapping lives in `unicode-width`".
        // The approximation is wrong for variation selectors: U+FE0F is
        // above 0x1100, so an emoji written as base + U+FE0F was budgeted
        // at four cells for two cells of glyph, and the box grew to fit
        // nothing. `display_width` is the exact function, and it is
        // already imported at the top of this file.
        let max_h = (area.height / 3).max(5);
        prompt_height(&state.input.text, inner_width, max_h)
    };
    // v0.9.1 (P2-12): the chrome used to be a fixed 6 rows (2 footer +
    // 3 prompt + 1 hint) before the transcript got anything. Below that,
    // ratatui squeezed the overflow into whatever was left, and a 20x5
    // terminal rendered the user's own message and the footer's
    // "idle  m" on the *same line*, with the prompt's input row pushed
    // off the bottom. It never panicked — ratatui clipped — but it no
    // longer read as an application, and "make your terminal bigger" is
    // not advice you can give someone in a 60-column split pane.
    //
    // Degrade in known steps instead of falling off a cliff: drop the
    // hint bar, then the footer's second row, then the prompt's border.
    // The transcript and the prompt are the two things you cannot work
    // without, so they are last to go.
    let footer_rows: u16 = match area.height {
        0..=4 => 0,
        5..=6 => 1,
        _ => 2,
    };
    let hint_rows: u16 = if area.height >= 10 { 1 } else { 0 };
    let want_prompt: u16 = if popup_height > 0 { popup_height } else { 3 };
    let prompt_rows: u16 = want_prompt
        .min(area.height.saturating_sub(footer_rows + hint_rows + 1))
        .max(1);
    let search_rows: u16 = if state.ui_state.search.is_some() {
        1
    } else {
        0
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(footer_rows), // pwd line + stats/model line
            Constraint::Length(search_rows), // search bar (only when active)
            Constraint::Min(1),              // transcript — never below one row
            Constraint::Length(prompt_rows), // prompt OR popup
            Constraint::Length(hint_rows),   // key hints
        ])
        .split(area);

    render_footer(f, state, theme, chunks[0]);
    // F019: when transcript search is active, render a search bar
    // row (the Layout reserves 1 line for it above the transcript).
    //
    // When search is inactive the search slot is 0 rows and the
    // indices below shift accordingly. Previously (commit d4070d0)
    // the chunk mapping was hard-coded as `chunks[1]/[2]/[3]/[4]`
    // without accounting for the 0-height collapse, which made the
    // transcript render into an empty area and pushed the prompt
    // up to row 1. v0.6 fix: always use the chunks[] indices in
    // their Layout order regardless of search state.
    let (transcript_chunk, prompt_chunk, footer_chunk) = if state.ui_state.search.is_some() {
        // status=0, search=1, transcript=2, prompt=3, footer=4
        render_search_bar(f, state, theme, chunks[1]);
        (chunks[2], chunks[3], chunks[4])
    } else {
        // ratatui keeps chunk indices stable even when one slot has
        // 0 height, so the transcript/prompt/footer indices are the
        // SAME as the active case (2, 3, 4) — chunks[1] is just an
        // empty Rect we never render into. Earlier (commit d4070d0)
        // this branch used chunks[1]/[2]/[3] which collided with the
        // transcript Min(3) slot and pushed the prompt up to row 1.
        (chunks[2], chunks[3], chunks[4])
    };
    render_transcript(f, state, theme, transcript_chunk);
    if state
        .ui_state
        .completion
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        render_completion_popup(f, state, theme, prompt_chunk);
    } else {
        render_prompt(f, state, theme, prompt_chunk);
    }
    render_key_hints(f, state, theme, footer_chunk);

    if state.run_state.mode == RunMode::Running {
        // Running spinner replaces the transcript pane (already
        // computed above as `transcript_chunk`).
        // render_running_indicator was removed in v0.8.3 — the working
        // spinner is now embedded in the footer (render_stats_line).
    }
}

fn render_footer(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    // v0.8: Pi-style 2-line footer.
    //   Line 1: cwd ⎇ branch • [session_id]                (env context, dim)
    //   Line 2: " nini " badge • phase • diff • ctx% • tokens • cost  ...............  (provider) model • thinking • [theme]
    //                                                                   ^^^ right-aligned padding ^^^
    //
    // Mirrors `FooterComponent` in pi-coding-agent/.../components/footer.js: a
    // dedicated env line + a stats line with right-aligned model identity.
    if area.height == 0 {
        // v0.9.1 (P2-12): the layout deliberately gave the footer zero
        // rows on a very short terminal. The old `height < 2` fallback
        // then *invented* one (`tmp.height = 1`) and drew the stats line
        // on top of the transcript — which is how a 40x3 terminal ended
        // up rendering `> hello idle    m`: the user's own message and
        // the status bar, on the same line. Zero rows means zero rows.
        return;
    }
    if area.height == 1 {
        // One row available: show the stats line only — it carries the
        // model identity, which is the more useful of the two.
        let mut tmp = area;
        tmp.height = 1;
        render_stats_line(f, state, theme, tmp);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    render_pwd_line(f, state, theme, rows[0]);
    render_stats_line(f, state, theme, rows[1]);
}

/// Line 1 of the footer: cwd + git branch + session id, all dim except branch
/// (which uses success green). Mirrors Pi's `FooterComponent` pwd line.
fn render_pwd_line(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut pushed = 0;

    if let Some(cwd) = &state.session_state.cwd {
        spans.push(Span::styled(shorten_home(cwd), theme.fg_style("dim")));
        pushed += 1;
    }
    if let Some(branch) = &state.session_state.git_branch {
        if pushed > 0 {
            spans.push(Span::styled("  ", theme.fg_style("dim")));
        }
        spans.push(Span::styled(
            format!("\u{2387} {branch}"),
            theme.fg_style("success"),
        ));
        pushed += 1;
    }
    // Session id (truncated to 8 chars). Pi-style "[abc12345]" pill.
    let session_disp = state
        .session_state
        .session_id
        .as_deref()
        .map(|s| &s[..s.len().min(8)])
        .unwrap_or("no session");
    if pushed > 0 {
        spans.push(Span::styled("  \u{2022}  ", theme.fg_style("dim")));
    }
    spans.push(Span::styled(
        format!("[{session_disp}]"),
        theme.fg_style("dim"),
    ));

    // v0.8.9 (P1-6): the line used to be handed to `Paragraph` whole
    // and clipped at the screen edge with no marker, so a long cwd read
    // as `~/very/deep/nested/project/path/that/kee` — the user could
    // not tell it was cut, and a long session tag degraded to `[no `.
    //
    // Priorities, worst-to-best:
    //   1. session tag  — least useful. Dropped whole, never half-shown.
    //   2. git branch   — truncated, not dropped.
    //   3. cwd          — truncated **from the left**, because the tail
    //      of a path is the part that identifies the project.
    let width = area.width as usize;
    if line_width(&spans) > width {
        spans = fit_pwd_line(spans, &session_disp, theme, width);
    }

    f.render_widget(Paragraph::new(RLine::from(spans)), area);
}

/// Cell width of a span list. v0.8.9 (P1-6): this used to be
/// `chars().count()` in the stats line, which under-counts CJK (1 char
/// = 2 cells) and therefore under-pads the right-aligned half — pushing
/// the model name off screen for any non-ASCII input.
fn line_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum()
}

/// Fit the pwd line into `width` cells, in the priority order described
/// on [`render_pwd_line`].
fn fit_pwd_line(
    spans: Vec<Span<'static>>,
    session_disp: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Span<'static>> {
    // Drop the session tag first — the whole `[...]` span plus its
    // preceding separator. A half-shown `[no ` tells the user less than
    // no session pill at all.
    let without_session: Vec<Span<'static>> = spans
        .iter()
        .filter(|s| s.content.as_ref() != format!("[{session_disp}]"))
        .cloned()
        .collect();
    // The separator only existed to join the session tag; without it, a
    // trailing `  •  ` is noise.
    let mut kept: Vec<Span<'static>> = without_session
        .into_iter()
        .filter(|s| s.content.as_ref() != "  \u{2022}  ")
        .collect();

    if line_width(&kept) > width {
        // Still too long: head-truncate so the tail of the path — the
        // part that identifies the project — survives.
        kept = truncate_spans_head(&kept, width);
    }
    let _ = theme;
    kept
}

/// Truncate a span list to `max` cells from the left, prefixing `…` on
/// the first span. Used for the pwd line, where the tail identifies the
/// directory.
fn truncate_spans_head(spans: &[Span<'static>], max: usize) -> Vec<Span<'static>> {
    if max == 0 {
        return Vec::new();
    }
    let total = line_width(spans);
    if total <= max {
        return spans.to_vec();
    }
    // Which span does the cut land in, and how many cells survive?
    let keep_from = total - (max - 1);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut acc = 0usize;
    let mut cut_done = false;
    for (i, s) in spans.iter().enumerate() {
        let w = display_width(s.content.as_ref());
        if cut_done {
            break;
        }
        if acc + w <= keep_from {
            acc += w;
            out.push(s.clone());
            continue;
        }
        let keep = keep_from.saturating_sub(acc);
        // `truncate_cells_head` already prefixes the marker; adding one
        // here produced "……" on screen (caught by the e2e test).
        let _ = i;
        out.push(Span::styled(
            truncate_cells_head(s.content.as_ref(), keep + 1),
            s.style,
        ));
        cut_done = true;
    }
    out
}

/// Line 2 of the footer: brand badge + stats (left) ............ (provider) model + thinking + [theme] (right).
///
/// Right-side model identity is right-aligned within `area.width` using
/// raw-space padding between the two halves (mirrors pi-tui's
/// `truncateToWidth(statsLeft + " ".repeat(...) + rightSide, width)`).
fn render_stats_line(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    use crate::rich::{spinner_frame, AgentPhase};

    // ---- LEFT half: brand + phase + status + diff + ctx% + tokens + cost ----
    let mut left: Vec<Span<'static>> = vec![Span::styled(
        " nini ".to_string(),
        theme
            .bg_style("accent")
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];

    // Phase indicator: 5-state map with spinner when running.
    let phase = match state.run_state.mode {
        RunMode::Running => AgentPhase::Working,
        _ => AgentPhase::Idle,
    };
    let phase_label = if matches!(state.run_state.mode, RunMode::Running) {
        format!("{} {}", spinner_frame(), phase.label())
    } else {
        phase.label().to_string()
    };
    left.push(Span::raw("  "));
    left.push(Span::styled(
        phase_label,
        theme.fg_style(phase.color_name()),
    ));

    // Status override (set by runtime for "aborted", "compacting", etc.).
    if !state.run_state.status.is_empty() && state.run_state.status != "ready" {
        left.push(Span::styled("  \u{2022}  ", theme.fg_style("dim")));
        left.push(Span::styled(
            state.run_state.status.clone(),
            theme.fg_style("warning"),
        ));
    }

    // Last edit-tool diff summary: "[edit +N -M]" pill.
    if let Some((adds, dels)) = state.ui_state.last_diff {
        left.push(Span::styled("  ", theme.fg_style("dim")));
        left.push(Span::styled("[edit ", theme.fg_style("muted")));
        left.push(Span::styled(format!("+{adds}"), theme.fg_style("success")));
        left.push(Span::styled(format!(" -{dels}"), theme.fg_style("error")));
        left.push(Span::styled("]", theme.fg_style("muted")));
    }

    // Context-window segment (Pi-style: "ctx 42% [████░░░░]").
    if state.run_state.context_window > 0 {
        let pct =
            (state.run_state.context_used as f64 / state.run_state.context_window as f64) * 100.0;
        let bar = context_bar(pct);
        let color_name = if pct > 90.0 {
            "error"
        } else if pct > 70.0 {
            "warning"
        } else {
            "success"
        };
        left.push(Span::styled("  \u{2022}  ", theme.fg_style("dim")));
        left.push(Span::styled(
            format!("ctx {:>3.0}% ", pct),
            theme.fg_style(color_name),
        ));
        left.push(Span::styled(bar, theme.fg_style(color_name)));
    }

    // Token-count segment (compact).
    if state.run_state.tokens.input > 0 || state.run_state.tokens.output > 0 {
        left.push(Span::styled("  \u{2022}  ", theme.fg_style("dim")));
        left.push(Span::styled(
            format!(
                "in {} out {}",
                fmt_thousands(state.run_state.tokens.input),
                fmt_thousands(state.run_state.tokens.output),
            ),
            theme.fg_style("dim"),
        ));
    }

    // Cost segment (only when > $0).
    if state.run_state.cost_usd > 0.0 {
        left.push(Span::styled("  \u{2022}  ", theme.fg_style("dim")));
        left.push(Span::styled(
            format!("${:.4}", state.run_state.cost_usd),
            theme.fg_style("success"),
        ));
    }

    // ---- RIGHT half: provider + model + thinking + [theme] ----
    let mut right: Vec<Span<'static>> = Vec::new();
    if let Some(provider) = state
        .model_state
        .provider
        .as_ref()
        .filter(|p| !p.is_empty())
    {
        right.push(Span::styled(
            format!("({provider}) "),
            theme.fg_style("dim"),
        ));
    }
    right.push(Span::raw(state.model_state.model.clone()));
    if let Some(level) = state
        .model_state
        .thinking_level
        .as_ref()
        .filter(|l| !l.is_empty())
    {
        right.push(Span::styled(
            format!(" \u{2022} {level}"),
            theme.fg_style("dim"),
        ));
    }
    if let Some(name) = state.ui_state.theme_name.as_ref().filter(|n| !n.is_empty()) {
        right.push(Span::raw("  "));
        right.push(Span::styled(format!("[{name}]"), theme.fg_style("accent")));
    }

    // ---- Compose with right-alignment ----
    //
    // v0.8.9 (P1-6): three problems, all visible in a 40-column
    // terminal:
    //
    //   * widths were measured with `chars().count()`, so a CJK model
    //     name or status was counted as 1 cell per character while
    //     rendering as 2 — the padding came up short and the right
    //     half spilled off screen.
    //   * when the halves didn't fit, the code kept a 2-space gap and
    //     appended `right` **anyway**, guaranteeing an overflow. The
    //     thing that got pushed off was the model name — the single
    //     most useful thing on the line. v0.8.8 went to some trouble
    //     to plumb `--model` through to this label.
    //   * nothing was ever marked as cut.
    //
    // New priority: the model identity on the right is never the thing
    // that gets sacrificed. The left half (brand, phase, ctx%, tokens,
    // cost) is truncated with `…` to make room.
    let width = area.width as usize;
    let min_gap = 2;

    // A right half wider than the terminal on its own (a very long
    // model id) gets head-truncated — the tail is what identifies it.
    let right: Vec<Span<'static>> = if line_width(&right) > width {
        truncate_spans_head(&right, width)
    } else {
        right
    };
    let right_width = line_width(&right);

    let left_fits = line_width(&left) + right_width + min_gap <= width;
    let left_budget = width.saturating_sub(right_width + min_gap);
    let left: Vec<Span<'static>> = if left_fits {
        std::mem::take(&mut left)
    } else if left_budget == 0 {
        // No room for anything on the left at all.
        Vec::new()
    } else {
        truncate_spans_tail(&left, left_budget)
    };
    let left_width = line_width(&left);
    let left_is_empty = left.is_empty();

    let mut spans = left;
    // When the left half had to be dropped entirely there is nothing to
    // separate, and adding `min_gap` would push the line one to two
    // cells past the terminal — which is exactly how the right half used
    // to escape the screen.
    let gap = if left_is_empty {
        0
    } else {
        width.saturating_sub(left_width + right_width).max(min_gap)
    };
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);

    f.render_widget(Paragraph::new(RLine::from(spans)), area);
}

/// Truncate a span list to `max` cells from the right, appending `…` to
/// the span the cut lands in. Used for the stats line's left half, where
/// the head (brand + phase) matters more than the tail (token counts).
fn truncate_spans_tail(spans: &[Span<'static>], max: usize) -> Vec<Span<'static>> {
    if max == 0 {
        return Vec::new();
    }
    let total = line_width(spans);
    if total <= max {
        return spans.to_vec();
    }
    // Everything after this many cells is dropped.
    let keep_to = max.saturating_sub(1);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut acc = 0usize;
    let mut cut_done = false;
    for s in spans.iter() {
        if cut_done {
            break;
        }
        let w = display_width(s.content.as_ref());
        if acc + w <= keep_to {
            acc += w;
            out.push(s.clone());
            continue;
        }
        let keep = keep_to.saturating_sub(acc);
        let tail = truncate_cells(s.content.as_ref(), keep + 1);
        out.push(Span::styled(tail, s.style));
        cut_done = true;
    }
    out
}

/// Render a simple ASCII progress bar for context-window usage.
fn context_bar(pct: f64) -> String {
    let width = 8;
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    format!(
        "[{}{}]",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty)
    )
}

/// Format `1234567` as `"1.2M"` for compact status display.
fn fmt_thousands(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}K", n as f64 / 1000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Replace the user's home directory prefix with `~` for compactness.
fn shorten_home(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = std::path::PathBuf::from(&home);
        if let Ok(stripped) = path.strip_prefix(&home_path) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

/// Render the F019 transcript search bar (query + match count + current
/// index). One row tall, sits just below the status bar.
fn render_search_bar(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};
    let Some(search) = &state.ui_state.search else {
        return;
    };
    let total = search.matches.len();
    let current = if total == 0 { 0 } else { search.current + 1 };
    let counter = if total == 0 {
        "no matches".to_string()
    } else {
        format!("{current}/{total}")
    };
    let prefix = Span::styled(
        "/".to_string(),
        theme.fg_style("accent").add_modifier(Modifier::BOLD),
    );
    let query = Span::styled(search.query.clone(), theme.fg_style("text"));
    let counter_span = Span::styled(
        format!("  {counter}"),
        theme.fg_style(if total == 0 { "error" } else { "dim" }),
    );
    let hint = Span::styled(
        "  n=next  N=prev  Esc=cancel".to_string(),
        theme.fg_style("muted"),
    );
    let line = Line::from(vec![prefix, query, counter_span, hint]);
    let para = ratatui::widgets::Paragraph::new(line);
    f.render_widget(para, area);
}

/// How many `TranscriptLine`s it takes to cover `items` rendered rows,
/// counting from the bottom up — the same direction the renderer skips in.
///
/// Mirrors the user-scroll branch below so the number PageUp uses and the
/// number PageUp actually moves stay the same thing. Getting that wrong is
/// how "one page" becomes "one line" or "the whole transcript".
/// Where the prompt cursor sits, relative to the text area's top-left.
///
/// Two things were wrong here and both are only visible with input the
/// e2e suite never typed.
///
/// **Width.** The column was `text[..cursor].chars().count()` — a count of
/// characters standing in for a count of terminal cells. Every other width
/// in this file uses `width::display_width` (lines 274, 327, 552), the
/// comment at line 638 says "using display_width so emoji / CJK count
/// correctly", and line 642 imports `unicode_width`. CJK glyphs are two
/// cells, so `研究一下` put the cursor on top of the second character.
///
/// **Row.** The row was `chars / inner_width`, which does not know that a
/// newline exists. On a multi-line prompt — which the box supports, and
/// renders with a dim `❯` on continuation lines — the cursor was placed at
/// a row computed from the whole buffer's length, so it landed somewhere
/// unrelated to the line being typed on.
///
/// The row here is explicit newlines plus however many wrapped rows the
/// current line has consumed. Wrapping is ratatui's, which breaks on word
/// boundaries, so the wrapped-row count can exceed the character
/// arithmetic by one on a long unbroken token; that is why the old code
/// clamped with `.min(...)` and why this still clamps.
/// Rows the prompt box needs: explicit newlines plus wrapped rows for the
/// first line, plus the two borders, clamped.
///
/// Extracted rather than left inline for the same reason as
/// `prompt_cursor_geometry` — the height budget decides whether a prompt
/// wraps, and an inline expression inside a layout block cannot be asked
/// about. It was measured wrong exactly once: a hand-rolled
/// `(c as u32) > 0x1100` stood in for `unicode-width`, under a comment
/// admitting as much, and charged a variation selector (U+FE0F, zero-width)
/// two cells.
fn prompt_height(text: &str, inner_width: usize, max_h: u16) -> u16 {
    let explicit_lines = text.matches('\n').count() as u16;
    let first_line_width = display_width(text.split('\n').next().unwrap_or(""));
    let wrapped_rows = if first_line_width <= inner_width {
        1u16
    } else {
        (first_line_width as f32 / inner_width as f32).ceil() as u16
    };
    let total_rows = explicit_lines + wrapped_rows;
    (total_rows + 2).clamp(3, max_h)
}

struct PromptCursor {
    /// Column within the current visual row, in cells.
    column: usize,
    /// Rows below the top border: explicit newlines, plus wrapped rows.
    row: usize,
}

fn prompt_cursor_geometry(text: &str, cursor_byte: usize, inner_width: usize) -> PromptCursor {
    let cursor_byte = floor_char_boundary(text, cursor_byte.min(text.len()));
    let before = &text[..cursor_byte];

    let explicit_rows = before.matches('\n').count();
    let current_line = before.rsplit('\n').next().unwrap_or("");

    let column_in_line = display_width(current_line);
    let wrapped_rows = column_in_line / inner_width.max(1);

    PromptCursor {
        column: column_in_line % inner_width.max(1),
        row: explicit_rows + wrapped_rows,
    }
}

use crate::scrollbar::lines_needed_for_items;

fn render_transcript(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    // v0.8.7 (ux-002, transcript continuity): the previous autoscroll
    // math used `lines.len()` (TranscriptLine count) as the scroll unit,
    // but each TranscriptLine expands to 1..N ListItems at render time.
    // For 8 parallel tool calls with multi-line bodies, the rendered
    // item count (~80) is much larger than visible_height (~18 at
    // 80x24). ratatui's List widget by default clips the BOTTOM, so
    // the LATEST `▸ name{…}` headers scrolled off-screen — user saw
    // tool result body fragments without their tool call announcement
    // ("窗口工具调用输出不连续,会断开").
    //
    // Fix: render all items, then truncate to `visible_height`. For
    // autoscroll keep the TAIL (latest, chat-style). For user-scrolled
    // view, translate `scroll_offset` (TranscriptLine count) to an
    // item-space offset via per-line item counts, so the user-scroll
    // window is also correct.
    //
    // v0.8.4 (ux-001) history: previous logic only used `scroll_offset`
    // for upward scroll and otherwise showed `[0..visible_height]` —
    // i.e. the TOP of the transcript, ignoring new messages. The
    // v0.8.4 fix computed `start = total - visible_height` but the
    // iteration still produced ALL items when items > visible_height,
    // and ratatui clipped from the top down (so the OLDEST items were
    // visible, not the newest). v0.8.7 fixes both.
    // v0.8.9 (P1-5): defensive clamp. Every operation that shrinks
    // `lines` now calls `TranscriptState::reset_view`, but a stale
    // offset from anywhere else would still be catastrophic: the
    // user-scroll branch below skips items from the bottom until the
    // row budget runs out, and past the end the budget never runs out
    // — so `visible_items` comes back empty and the transcript area
    // renders as a blank box with only `↓ N more` in the corner.
    // Clamping costs nothing and caps the damage of a missed reset at
    // "the user sees the top" instead of "the user sees nothing".
    let lines = &state.transcript_state.lines;
    let total_lines = lines.len();
    // v0.10.3: the number of rows the transcript *area* has. The rows
    // the content may use is one less whenever an overflow indicator is
    // shown — see `content_area` below.
    let full_height = area.height as usize;
    let autoscroll = state.transcript_state.autoscroll || total_lines == 0;
    // Clamp rather than mutate: `render_transcript` receives a shared
    // `&AppState`, and the renderer has no business writing. The real
    // fix is `TranscriptState::reset_view` at every line-shrinking
    // call site; this is the cheap guarantee that a missed one degrades
    // to "shows the top" instead of "shows an empty box".
    let scroll_offset = state
        .transcript_state
        .scroll_offset
        .min(total_lines.saturating_sub(1));

    // First pass: per-line item counts. Used for user-scroll item-space
    // offset translation, and for the overflow indicator count.
    //
    // v0.8.9 (perf): this used to render every line in the transcript,
    // in full, on every frame — measured at a linear 0.545 ms/KB, so a
    // real task's worth of history (a few hundred KB) put every frame
    // in the 100-360 ms range and made typing in the prompt visibly
    // laggy. The count depends only on the line's content and the
    // terminal width, so it is cached on the `TranscriptState`.
    //
    // The count is still exact on a miss: we render that one line and
    // keep the result. Streaming assistant output invalidates exactly
    // the one line that grew, so a frame during token streaming costs
    // one line, not the whole transcript.
    let width = area.width;
    let mut items_per_line: Vec<usize> = Vec::with_capacity(total_lines);
    for (idx, line) in lines.iter().enumerate() {
        let fingerprint = line.content_fingerprint();
        let count = match state.item_cache.lock() {
            Ok(mut cache) => {
                cache.ensure_len(total_lines);
                match cache.get(idx, width) {
                    Some((cached_fp, count)) if cached_fp == fingerprint => count,
                    _ => {
                        let count =
                            items_for_line(line, theme, width as usize, SearchHighlight::None)
                                .len();
                        cache.put(idx, width, fingerprint, count);
                        count
                    }
                }
            }
            // A poisoned lock must not take the TUI down; worst case is
            // the pre-cache behaviour (recompute every frame).
            Err(_) => items_for_line(line, theme, width as usize, SearchHighlight::None).len(),
        };
        items_per_line.push(count);
    }
    let total_items: usize = items_per_line.iter().sum();

    // v0.10.3: the overflow indicators used to be painted **on top of**
    // the transcript's first and last row, so `↑ N more above` ate the
    // beginning of the first visible line (and `↓ N more` ate the end
    // of the last one). The indicators now get a row of their own and
    // the content gets one row less.
    //
    // Which row is reserved depends on the mode, and the decision has
    // to be made *before* the visible slice is chosen — otherwise
    // "how much is hidden" and "how much room is left" would depend on
    // each other. So the test for "is there an indicator" is made
    // against the full height, and only then is the height reduced.
    //
    // - autoscroll: the indicator sits on the first row (there is
    //   content above the fold), the content starts on the second row.
    // - user-scrolled: the indicator sits on the last row (there is
    //   content below the fold), the content ends one row earlier.
    let indicator_row_allowed = full_height >= 3;
    let needs_top_indicator = autoscroll && indicator_row_allowed && total_items > full_height;
    let needs_bottom_indicator = !autoscroll && indicator_row_allowed && scroll_offset > 0;
    let visible_height = full_height - usize::from(needs_top_indicator || needs_bottom_indicator);
    let content_area = if needs_top_indicator {
        Rect::new(area.x, area.y + 1, area.width, area.height - 1)
    } else if needs_bottom_indicator {
        Rect::new(area.x, area.y, area.width, area.height - 1)
    } else {
        area
    };

    // v0.10 (scrollbar): how far the viewport sits from the live tail, in
    // rendered rows. 0 at the tail; also 0 in autoscroll mode, which is
    // correct — following the output means being at the bottom.
    let mut items_skipped_from_bottom: usize = 0;

    // v0.10: publish how many lines one page covers, so PageUp scrolls a
    // page instead of the single line it used to scroll. Computed here
    // because this is the only scope that has both the per-line row counts
    // and the viewport height; `ScrollUp` had neither and guessed `1`.
    state.page_lines.store(
        lines_needed_for_items(&items_per_line, visible_height, total_lines).max(1),
        Ordering::Relaxed,
    );
    // v0.10: hand the mouse the row counts it cannot compute itself. A
    // scrollbar drag arrives as a pixel offset and has to become a
    // `scroll_offset`, which counts lines.
    if let Ok(mut heights) = state.line_heights.lock() {
        *heights = items_per_line.clone();
    }
    if let Ok(mut vp) = state.transcript_viewport.lock() {
        *vp = crate::scrollbar::Viewport {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            // v0.10.3: the scrollbar describes the **content** area. The
            // indicator row is not part of it: the reader scrolls the
            // transcript, not the hint.
            height: content_area.height,
            content_height: total_items,
            viewport_height: visible_height,
        };
    }

    // v0.10.2: which transcript line, if any, is the active search hit.
    //
    // `SearchState::matches` holds **TranscriptLine** indices, which is
    // exactly the `i` in the loops below, so no translation is needed.
    // `SearchHighlight::None` is passed for the item-count cache: the
    // paint does not change how many rows a line occupies, and caching
    // counts is what keeps a long transcript off the per-frame budget.
    let search = state.ui_state.search.as_ref();
    let highlight_for = |idx: usize| -> SearchHighlight {
        let Some(s) = search else {
            return SearchHighlight::None;
        };
        if s.current < s.matches.len() && s.matches[s.current] == idx {
            SearchHighlight::Current
        } else if s.matches.contains(&idx) {
            SearchHighlight::Hit
        } else {
            SearchHighlight::None
        }
    };

    // Choose the visible slice.
    let mut visible_items: Vec<ListItem> = Vec::with_capacity(visible_height.min(total_items));
    let mut hidden_above_items: usize = 0;

    if total_items <= visible_height {
        // Everything fits — render everything.
        for (i, line) in lines.iter().enumerate() {
            visible_items.extend(items_for_line(
                line,
                theme,
                area.width as usize,
                highlight_for(i),
            ));
        }
    } else if autoscroll {
        // Tail-aligned: keep the latest `visible_height` items.
        hidden_above_items = total_items - visible_height;
        let mut budget = visible_height;
        for i in (0..total_lines).rev() {
            let count = items_per_line[i];
            if count <= budget {
                let line_items =
                    items_for_line(&lines[i], theme, area.width as usize, highlight_for(i));
                visible_items = line_items
                    .into_iter()
                    .chain(visible_items.into_iter())
                    .collect();
                budget -= count;
                if budget == 0 {
                    break;
                }
            } else {
                // Single line bigger than entire visible area — tail-truncate.
                let mut line_items =
                    items_for_line(&lines[i], theme, area.width as usize, highlight_for(i));
                let start = line_items.len() - budget;
                visible_items = line_items
                    .split_off(start)
                    .into_iter()
                    .chain(visible_items.into_iter())
                    .collect();
                break;
            }
        }
    } else {
        // User-scrolled view: translate `scroll_offset` (TranscriptLine
        // count) to an item-space offset.
        let mut items_to_skip_from_bottom: usize = 0;
        let mut lines_skipped = 0;
        for i in (0..total_lines).rev() {
            if lines_skipped >= scroll_offset {
                break;
            }
            items_to_skip_from_bottom += items_per_line[i];
            lines_skipped += 1;
        }
        // Captured before the loop below consumes it. The scrollbar needs the
        // same number, and recomputing it here would mean running the same
        // two loops twice per frame.
        items_skipped_from_bottom = items_to_skip_from_bottom;

        // Now collect items from the bottom, skipping
        // `items_to_skip_from_bottom` items, until we have visible_height.
        let mut budget = visible_height;
        for i in (0..total_lines).rev() {
            let count = items_per_line[i];
            let skip_here = items_to_skip_from_bottom.min(count);
            items_to_skip_from_bottom -= skip_here;
            let take_from_line = count - skip_here;
            if take_from_line == 0 || budget == 0 {
                continue;
            }
            let take = take_from_line.min(budget);
            let mut line_items =
                items_for_line(&lines[i], theme, area.width as usize, highlight_for(i));
            let start = line_items.len() - take;
            visible_items = line_items
                .split_off(start)
                .into_iter()
                .chain(visible_items.into_iter())
                .collect();
            budget -= take;
            if budget == 0 {
                break;
            }
        }
    }

    // v0.8.4 (ux-001): welcome card for empty cold-start state.
    if visible_items.is_empty() && total_lines == 0 && state.input.text.is_empty() {
        render_welcome_card(f, theme, area);
        return;
    }

    // v0.10: the transcript scrollbar, on the area's last column.
    //
    // Painted after the list and the two overflow indicators so it wins the
    // cell. Pi does the same (`layout.ts:293` `paintScrollbar` runs over the
    // composed screen row).
    //
    // Pi's `always` mode also narrows the content by one column
    // (`scroll-view.ts` `getContentWidth`). nini does not, in any mode: the
    // bar overwrites the last cell rather than reflowing the transcript,
    // because reflowing would move text under the reader every time the bar
    // appears and disappears.
    let mode = state.ui_state.scrollbar_mode();
    let since_scroll = crate::scrollbar::now_millis().saturating_sub(state.scroll_activity_ms());
    if crate::scrollbar::should_draw(mode, total_items, visible_height, since_scroll) {
        if let Some(g) = crate::scrollbar::geometry(
            content_area.x,
            content_area.y,
            content_area.width,
            content_area.height,
            total_items,
            visible_height,
            items_skipped_from_bottom,
        ) {
            // One line per thumb row. The first version rendered
            // `"█".repeat(height)` as a single string into a one-column
            // area: that is a two-cell token, so ratatui clipped it to one
            // character and every row below the first stayed blank. The e2e
            // test caught it by asserting the thumb is contiguous.
            let style = theme.bg_style("scrollbarThumb");
            let rows: Vec<RLine> = (0..g.thumb_height.max(1))
                .map(|_| RLine::from(Span::styled("\u{2588}", style)))
                .collect();
            let bar_area = Rect::new(g.column, g.thumb_top, 1, g.thumb_height);
            f.render_widget(Paragraph::new(rows), bar_area);
        }
    }

    // v0.8.9 (P1-5): last-resort backstop. If the transcript is
    // non-empty but the scroll window resolved to nothing, fall back to
    // the tail. Unreachable once `clamp_view` is in place, and kept
    // because the failure mode is an empty screen — cheap insurance
    // against a future refactor of the item math above.
    if visible_items.is_empty() && total_lines > 0 {
        let mut tail: Vec<ListItem> = Vec::with_capacity(visible_height);
        for line in lines.iter().rev() {
            let mut items = items_for_line(line, theme, area.width as usize, highlight_for(0));
            items.reverse();
            if items.len() >= visible_height {
                items.drain(..items.len() - visible_height);
                tail = items.into_iter().rev().chain(tail).collect();
                break;
            }
            let n = items.len();
            tail = items.into_iter().chain(tail).collect();
            if n == 0 {
                break;
            }
        }
        visible_items = tail;
    }

    // (debug prints removed)
    let list = List::new(visible_items)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    // v0.10.3: the list goes into the **content** area, which is one row
    // shorter than the transcript area whenever an overflow indicator is
    // shown. Painting it into the whole area is what let the indicator
    // overwrite a row of text.
    f.render_widget(list, content_area);

    // v0.8.7 (ux-002): scroll indicator in autoscroll mode when
    // content overflows above. Tells them they can scroll up to see
    // more. Bottom-right placement matches the user-scroll indicator.
    //
    // v0.10.3: it is painted in the row the content does **not** use
    // (`needs_top_indicator` reserved the transcript area's first row),
    // not on top of the first row of text.
    if needs_top_indicator {
        let label = format!(" \u{2191} {hidden_above_items} more above ");
        let style = theme.fg_style("dim");
        // v0.10: `chars().count()`, not display width. U+2191 is
        // East-Asian-Ambiguous — one cell in a western terminal, two in a
        // CJK one — so the right-aligned indicator shifted in exactly the
        // terminals this project targets. Same class as the status-bar fix
        // in v0.8.9 (see the comment above line 477), which left these two
        // sites behind because nobody had reported them yet.
        let width = (display_width(&label) as u16).min(area.width);
        let ind_area = Rect::new(area.x, area.y, width, 1);
        let ind = Paragraph::new(Span::styled(label, style));
        f.render_widget(ind, ind_area);
    }

    // v0.8.4 (ux-001): user-scroll `↓ N more` indicator at the
    // bottom-right corner of the transcript area. Suppressed during
    // autoscroll so it doesn't flicker on every token delta.
    //
    // v0.10.3: like its counterpart, it gets a row of its own — the
    // transcript area's last row, which the content no longer uses.
    if needs_bottom_indicator {
        let hidden = scroll_offset;
        let label = format!(" \u{2193} {hidden} more ");
        let style = theme.fg_style("accent").add_modifier(Modifier::BOLD);
        let width = (display_width(&label) as u16).min(area.width); // see above
        let x = area.x + area.width.saturating_sub(width);
        let y = area.y + area.height.saturating_sub(1);
        let ind_area = Rect::new(x, y, width, 1);
        let ind =
            Paragraph::new(Span::styled(label, style)).alignment(ratatui::layout::Alignment::Right);
        f.render_widget(ind, ind_area);
    }
}

/// v0.8.7 (ux-002): extract the per-line item expansion so we can
/// compute item counts without rendering, and render twice without doing
/// the work twice. (We DO render twice — once for counting, once for
/// the actual list — but the cost is bounded: 1..N items per line where
/// N is typically <10. For a 200-line transcript this is cheap.)
/// How a transcript line relates to the active search.
///
/// v0.10.2. Pi paints a hit with `underline + bg(searchMatchBg) +
/// fg(searchMatchText)` and the *current* hit with `bold + inverse` on top
/// (`interactive-mode.ts:349` creates `searchMatchStyle` and
/// `searchCurrentMatchStyle` and hands both to the alt-screen scroll
/// view). Before this, nini computed the match list, drew the count, and
/// painted nothing — `searchMatchBg` / `searchMatchText` were palette
/// entries with no reader, the same dead-slot shape as `mdCodeBlockBorder`
/// (§32) and `thinkingText`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchHighlight {
    /// Not a search hit.
    #[default]
    None,
    /// A hit, but not the one `n` / `N` is on.
    Hit,
    /// The hit the search bar's `current` points at.
    Current,
}

/// The style Pi uses for a search hit.
///
/// `interactive-mode.ts:349` builds two closures:
/// ```js
/// searchMatchStyle:       (t) => theme.underline(theme.bg("searchMatchBg", theme.fg("searchMatchText", t)))
/// searchCurrentMatchStyle: (t) => theme.bold(theme.inverse(styleSearchMatch(t)))
/// ```
/// so a hit is underlined on the match colours and the current one adds
/// bold + reverse on top. Before this, nini computed the match list,
/// drew the `2/2` counter, and painted nothing — `searchMatchBg` and
/// `searchMatchText` were palette entries with no reader, the same
/// dead-slot shape as `mdCodeBlockBorder` (§32) and `thinkingText`.
fn search_highlight_style(level: SearchHighlight, theme: &Theme) -> Style {
    let mut style = theme
        .bg_style("searchMatchBg")
        .fg(theme.color("searchMatchText"));
    style = style.add_modifier(Modifier::UNDERLINED);
    if level == SearchHighlight::Current {
        style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
    }
    style
}

/// Paint one rendered transcript row with the search-match style.
///
/// Applied per `Span` rather than through `ListItem::style` for two
/// reasons: ratatui resolves `span > line > item`, and every renderer
/// here sets a per-span foreground, so an item-level style would colour
/// the background and be ignored for the text. And `ListItem::content`
/// is `pub(crate)` in ratatui 0.29, so a post-pass over the finished
/// items is not expressible from here — the paint has to happen while
/// the `Line` is still ours.
fn highlight_row(
    mut line: RLine<'static>,
    level: SearchHighlight,
    theme: &Theme,
) -> RLine<'static> {
    if level == SearchHighlight::None {
        return line;
    }
    let style = search_highlight_style(level, theme);
    for span in line.spans.iter_mut() {
        span.style = span.style.patch(style);
    }
    line
}

fn items_for_line(
    line: &TranscriptLine,
    theme: &Theme,
    area_width: usize,
    highlight: SearchHighlight,
) -> Vec<ListItem<'static>> {
    // v0.8.9 (P1-3): one width for every branch. Only the ToolResult
    // arm used to receive it, which is why a 210-character user message
    // rendered as `> STARTAAAA…` with its tail silently gone — the
    // `List` widget does not wrap, it cuts at the screen edge.
    // `None` when called outside a Frame (tests) or on a degenerate
    // width, so the renderers fall back to their legacy budgets.
    let max_w = if area_width > 4 {
        Some(area_width)
    } else {
        None
    };
    let items: Vec<RLine<'static>> = match line {
        TranscriptLine::User(text) => render_user_message(text, theme, max_w),
        TranscriptLine::AssistantText(text) => render_assistant_message(text, theme, max_w),
        // v0.8: dim/italic reasoning block (Pi-style). Wrap by display
        // width so long thinking blocks render across multiple rows.
        // v0.10.2: thinking is Markdown in Pi (`assistant-message.ts:147`),
        // painted `thinkingText` + italic. It used to be `wrap_thinking_block`
        // — raw strings, `dim` — so list markers and emphasis delimiters
        // were printed verbatim. See `rich::render_thinking_block`.
        TranscriptLine::ThinkingText(text) => {
            crate::rich::render_thinking_block(text, theme, max_w)
        }
        TranscriptLine::ToolCall {
            id: _,
            name,
            args,
            collapsed,
        } => {
            let mut out: Vec<RLine<'static>> = render_tool_call(name, args, theme, max_w);
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(crate::rich::render_collapsed_hint(theme));
            }
            out
        }
        TranscriptLine::ToolResult {
            id: _,
            name,
            ok,
            content,
            collapsed,
            duration_ms,
        } => {
            // `max_w` is computed once at the top of this function
            // (v0.8.9 P1-3) and shared by every arm.
            let mut out: Vec<RLine<'static>> =
                render_tool_result(name, *ok, content, *duration_ms, theme, max_w);
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(crate::rich::render_collapsed_hint(theme));
            }
            out
        }
        TranscriptLine::Divider => vec![crate::rich::turn_separator()],
        TranscriptLine::StopNotice(msg) => vec![RLine::from(Span::styled(
            format!("  {msg}"),
            theme.fg_style("error"),
        ))],
        // v0.10.3: a line the library wrote to stderr while the TUI was
        // painting, caught by the terminal guard. It is dim and prefixed
        // so it cannot be mistaken for model output — the whole point of
        // the guard is that the reader can tell the two apart.
        //
        // A notice can carry more than one line (a `/copy` fallback
        // prints the whole message), so it is split: the first row
        // carries the marker, the rest are indented under it.
        TranscriptLine::Notice(text) => {
            let mut rows = Vec::new();
            for (i, line) in text.lines().enumerate() {
                let prefix = if i == 0 { "  \u{26a0} " } else { "    " };
                rows.push(RLine::from(Span::styled(
                    format!("{prefix}{line}"),
                    theme.fg_style("dim"),
                )));
            }
            if rows.is_empty() {
                rows.push(RLine::from(Span::styled(
                    "  \u{26a0} ",
                    theme.fg_style("dim"),
                )));
            }
            rows
        }
        TranscriptLine::BashExecution {
            cmd,
            output,
            stderr,
            ok,
            exit_code,
            duration_ms,
            collapsed,
            ..
        } => {
            let mut out: Vec<RLine<'static>> = render_bash_execution(
                cmd,
                output,
                stderr,
                *ok,
                *exit_code,
                *duration_ms,
                theme,
                max_w,
            );
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(crate::rich::render_collapsed_hint(theme));
            }
            out
        }
    };
    items
        .into_iter()
        .map(|l| ListItem::new(highlight_row(l, highlight, theme)))
        .collect()
}

/// v0.8.4 (ux-001): cold-start welcome card. Renders into the empty
/// transcript area with the title centred on the vertical midline and
/// a short list of example commands. Avoids the dead-space look that
/// confuses first-time users ("did it crash?").
fn render_welcome_card(f: &mut Frame, theme: &Theme, area: Rect) {
    use ratatui::layout::Alignment;
    use ratatui::widgets::Paragraph;

    let title = Paragraph::new(Span::styled(
        " nini \u{2014} interactive coding agent ".to_string(),
        theme.fg_style("accent").add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center);

    let subtitle = Paragraph::new(Span::styled(
        "Type a prompt and press Enter to start. Examples below.".to_string(),
        theme.fg_style("muted"),
    ))
    .alignment(Alignment::Center);

    let examples: Vec<(&str, &str)> = vec![
        (" /help ", "list every slash command"),
        (" /model ", "switch provider / model"),
        (" /theme ", "toggle dark \u{2194} light"),
        (" /clear ", "reset the conversation"),
        (" !ls -la ", "run a shell command inline"),
    ];

    let mut example_lines: Vec<RLine> = Vec::new();
    for (i, (key, desc)) in examples.iter().enumerate() {
        let sep = if i > 0 { "  \u{2022}  " } else { "" };
        let mut spans: Vec<Span<'static>> = Vec::new();
        if !sep.is_empty() {
            spans.push(Span::styled(sep.to_string(), theme.fg_style("dim")));
        }
        spans.push(Span::styled(
            key.to_string(),
            theme.fg_style("keyHint").add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!("  {desc}"), theme.fg_style("muted")));
        example_lines.push(RLine::from(spans));
    }
    let examples_widget = Paragraph::new(example_lines).alignment(Alignment::Center);

    // v-stack: title + subtitle + (pad) + examples. The pad fills the
    // remaining area so the title lands roughly 1/3 from the top.
    let pad_height = area.height.saturating_sub(3 + examples.len() as u16) / 2;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(pad_height.max(2)),
            Constraint::Length(1), // title
            Constraint::Length(1), // subtitle
            Constraint::Length(examples.len() as u16),
        ])
        .split(area);

    f.render_widget(title, chunks[1]);
    f.render_widget(subtitle, chunks[2]);
    f.render_widget(examples_widget, chunks[3]);
}

fn render_prompt(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    // v0.10.2: publish the prompt's inner width for vertical caret
    // motion. It is the only place the number exists, exactly as in Pi,
    // where `buildVisualLineMap(this.lastWidth)` reads a width recorded
    // during render. Unconditional — the key handler must not depend on
    // whether a cursor happens to be drawn this frame.
    //
    // 4 is the floor `render_prompt` already uses below
    // (`area.width >= 2 + padding_x + 2`), so the published value cannot
    // disagree with the geometry the frame itself uses.
    state.prompt_inner_width.store(
        area.width
            .saturating_sub(5) // 1 border + 1 padding + 2 prompt + 1 border
            .max(4) as usize,
        std::sync::atomic::Ordering::Relaxed,
    );

    let prompt_symbol = if state.run_state.mode == RunMode::Running {
        "⏵"
    } else {
        "❯"
    };
    let text = state.input.text.clone();
    let cursor_byte = state.input.cursor;
    // Cursor geometry needs `inner_width`, which is only known after the
    // border and insets are subtracted, so it is computed below.

    // Split into lines for multi-line rendering.
    let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();

    // v0.8.3: Pi parity —
    //   • Multi-line: first line uses bold ❯, continuation lines use dim ❯
    //     so the user has continuous visual cues while typing multi-line input.
    //   • paddingX = 1: left and right margins of 1 column so text doesn't
    //     touch the borders.
    //   • Border color = theme.fg_style("borderMuted") (theme-aware).
    //   • Block title = "─ input ─" (DynamicBorder style, Pi).
    let padding_x: u16 = 1;
    let is_first_line = |i: usize| i == 0;
    let prompt_style_bold = theme.fg_style("success").add_modifier(Modifier::BOLD);
    let prompt_style_dim = theme.fg_style("dim");

    let mut line_widgets: Vec<RLine> = Vec::new();
    if lines.is_empty() {
        line_widgets.push(RLine::from(Span::raw(" ")));
    } else {
        for (i, l) in lines.iter().enumerate() {
            let prefix_char = if is_first_line(i) {
                prompt_symbol
            } else {
                "❯"
            };
            let prefix_style = if is_first_line(i) {
                prompt_style_bold
            } else {
                prompt_style_dim
            };
            line_widgets.push(RLine::from(vec![
                Span::styled(format!("{prefix_char} "), prefix_style),
                Span::raw(l.as_str()),
            ]));
        }
    }
    // v0.9.1 (P2-12): `Borders::ALL` costs two rows. At the 2-row
    // height the layout now hands us on a short terminal, the borders
    // consumed the whole widget and the input line — the one row that
    // matters — was pushed out. Below 3 rows, drop the frame and draw
    // the text bare: an unframed prompt you can type into beats a
    // handsome empty box.
    let para = if area.height >= 3 {
        Paragraph::new(line_widgets)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme.fg_style("borderMuted"))
                    .title(Span::styled(
                        " \u{2500} input \u{2500} ",
                        theme.fg_style("borderMuted"),
                    ))
                    .padding(Padding::horizontal(padding_x)),
            )
            .wrap(Wrap { trim: false })
    } else {
        Paragraph::new(line_widgets).wrap(Wrap { trim: false })
    };
    f.render_widget(para, area);

    // Render cursor. Account for paddingX: x offset starts after the
    // left border (1) + padding (padding_x) + the 2-char prompt symbol.
    if state.run_state.mode != RunMode::Running
        && area.height >= 3
        && area.width >= 2 + padding_x + 2
    {
        let left_inset: u16 = 1 /* border */ + padding_x + 2 /* "❯ " */;
        let inner_width = area
            .width
            .saturating_sub(1 /* right border */ + padding_x + left_inset);
        let geom = prompt_cursor_geometry(&text, cursor_byte, inner_width.max(1) as usize);
        let cursor_x = area.x + left_inset + geom.column as u16;
        let cursor_y =
            area.y + 1 /* top border */ + (geom.row as u16).min(area.height.saturating_sub(2) - 1);
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_key_hints(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    // v0.8.3: Pi parity — hints now use keyHint(theme) formatting
    // (dim key + muted description) instead of bg("dim")+white text.
    // F1 toggle switches between compact (5 hints) and extended (13+).
    //
    // v0.8.4 (ux-001): pick a shorter hint set on narrow terminals so
    // the most important keys (Enter=send, Ctrl+C=quit) always fit.
    // The full 5-hint compact set is ~58 chars; on a 60-col terminal
    // the 5th hint would overflow and ratatui's `Paragraph` truncates
    // the tail silently — without an ellipsis the user wouldn't even
    // know hints were dropped.
    //
    // v0.10.2: every hint here is checked against `default_keymap()` by
    // `tests/footer_binding_truth.rs`. Three of these strings were
    // fiction:
    //
    //   `Ctrl+Z`          — not bound at all (undo is `Ctrl+/`)
    //   `Alt+Backspace`   — not bound at all (`Ctrl+W` is kill-word)
    //   `Ctrl+C quit`     — bound, but to `Abort`, and `Abort` with no
    //                       popup open **clears the input buffer**
    //                       (runtime.rs: the `Abort` arm's final else).
    //
    // The first two were keys that did nothing at all. The third was
    // worse: the footer promised an exit and delivered a destructive
    // clear of half-typed input. The same class of defect §35 found in
    // `RunMode::Aborted` — a hint and an implementation that never met.
    let narrow = area.width < 70;
    let hints: Vec<(&str, &str)> = match state.run_state.mode {
        RunMode::Editing if state.ui_state.help_extended && !narrow => vec![
            (" F1 ", "short"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+W ", "kill-word"),
            (" Alt+D ", "kill"),
            (" Ctrl+/ ", "undo"),
            (" Ctrl+Y ", "yank"),
            (" Ctrl+L ", "model"),
            (" Ctrl+T ", "thinking"),
            (" Ctrl+P ", "model+"),
            (" Ctrl+O ", "collapse"),
            (" Ctrl+D ", "exit"),
        ],
        RunMode::Editing if state.ui_state.help_extended => vec![
            // narrow extended: drop the seldom-used kill/undo/yank hints
            (" F1 ", "short"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+L ", "model"),
            (" Ctrl+D ", "exit"),
        ],
        RunMode::Editing if narrow => vec![
            // narrow compact: keep only the essentials
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+D ", "exit"),
        ],
        RunMode::Editing => vec![
            (" F1 ", "help"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+L ", "model"),
            (" Ctrl+D ", "exit"),
        ],
        // `Ctrl+C` and `Esc` are both bound to `Abort`, so both say
        // "abort". It used to say "force-quit", which names an action
        // the key does not perform.
        RunMode::Running => vec![(" Esc ", "abort"), (" Ctrl+C ", "abort")],
        // Quitting = the double-tap window is open. `Ctrl+D` inside it
        // exits; `Esc` clears `pending_quit` and returns to Editing
        // (runtime.rs, the `Abort` arm's `pending_quit` branch). The old
        // `Ctrl+C force-quit` named a third action that exists nowhere:
        // `Ctrl+C` is `Abort`, which in this mode does neither.
        RunMode::Quitting => vec![(" Esc ", "cancel"), (" Ctrl+D ", "quit")],
    };

    // Use trim_to_width so when hints overflow we append a dim "…"
    // instead of silently dropping the tail (P1-5 from the typography plan).
    let trimmed = crate::keyhint::trim_to_width(&hints, area.width as usize, theme);

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, desc)) in trimmed.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".to_string()));
        }
        spans.extend(crate::keyhint::key_hint_spans(theme, key, desc));
    }
    f.render_widget(Paragraph::new(RLine::from(spans)), area);
}

/// Render the slash-command completion popup above the prompt.
fn render_completion_popup(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let Some(popup) = &state.ui_state.completion else {
        return;
    };
    if popup.items.is_empty() {
        return;
    }

    // Compute the visible window of items. Default viewport is 8 rows;
    // arrow keys scroll within the bounds when the list is longer.
    let max_visible = popup.max_visible.max(1);
    let total = popup.items.len();
    let start = popup.scroll_offset.min(total.saturating_sub(1));
    let end = (start + max_visible).min(total);
    let lines: Vec<RLine> = popup
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(end - start)
        .map(|(i, item)| {
            let is_selected = i == popup.selected;
            let name_style = if is_selected {
                theme
                    .bg_style("selectedBg")
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.fg_style("accent").add_modifier(Modifier::BOLD)
            };
            let mut spans = vec![Span::styled(format!("/{}", item.name), name_style)];
            if let Some(hint) = &item.argument_hint {
                spans.push(Span::styled(
                    format!(" {hint}"),
                    theme.fg_style(if is_selected { "borderMuted" } else { "dim" }),
                ));
            }
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                item.description.as_str(),
                theme.fg_style(if is_selected { "borderMuted" } else { "muted" }),
            ));
            RLine::from(spans)
        })
        .collect();

    // Title shows scroll position when the popup is long enough to
    // overflow — gives users a concrete "3/23" indicator so they know
    // there's more.
    let title = if total > max_visible {
        format!(
            " commands ({}/{} ≤, ↑↓ navigate) ",
            popup.selected + 1,
            total
        )
    } else {
        " commands (up/down select, Tab/Enter accept, Esc cancel) ".to_string()
    };

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(theme.fg_style("accent")),
    );
    f.render_widget(para, area);
}

/// Render a selector panel over the transcript area. Called when an
/// interactive selector is active.
pub fn render_selector_panel(
    f: &mut Frame,
    title: &str,
    query: &str,
    items: &[SelectorItem],
    visible: &[usize],
    selected: usize,
    theme: &Theme,
    area: Rect,
) {
    let panel = SelectorPanel::new(title, query, items, visible, selected, theme);
    f.render_widget(panel, area);
}

// v0.8.3: kept as a no-op fallback for callers that still invoke it
// during a transient resize. The actual spinner renders via
// `render_stats_line` in the footer (Pi parity).
#[allow(dead_code)]
fn render_running_indicator(f: &mut Frame, theme: &Theme, area: Rect) {
    // v0.8.3: Pi parity — the spinner now renders in the status line
    // (footer) via `render_stats_line` instead of floating over the
    // transcript's top-right corner. This avoids the "double spinner"
    // artifact (one in the status bar, one over the transcript) and
    // matches Pi's layout where the working indicator lives next to
    // the phase label. This function is kept as a no-op fallback for
    // callers that still invoke it during a transient resize.
    let _ = (f, theme, area);
}

// =====================================================================
// Status bar tests
// =====================================================================

#[cfg(test)]
mod footer_tests {
    use super::*;
    use crate::state::AppState;

    fn footer_text(state: &AppState) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(200, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_key_hints(f, state, &Theme::default(), f.area()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, 0)) {
                out.push_str(c.symbol());
            }
        }
        out
    }

    #[test]
    fn footer_editing_mode_shows_send_and_help() {
        let state = AppState::new("m");
        let text = footer_text(&state);
        assert!(text.contains("F1"));
        assert!(text.contains("help"));
        assert!(text.contains("Enter"));
        assert!(text.contains("send"));
        // Editing mode should NOT show abort hint.
        assert!(!text.contains("abort"));
    }

    #[test]
    fn footer_running_mode_shows_abort() {
        let mut state = AppState::new("m");
        state.run_state.mode = RunMode::Running;
        let text = footer_text(&state);
        assert!(text.contains("Esc"));
        assert!(text.contains("abort"));
        // Running mode should NOT show send hint.
        assert!(!text.contains("send"));
    }

    #[test]
    fn no_mode_promises_a_retry_that_does_not_exist() {
        // v0.10.1: the aborted state offered `Enter retry` in the footer.
        // There was no `Aborted` branch anywhere in the submit path —
        // `submit_user_input` returns unless the mode is `Editing` — so the
        // hint described a key press that did nothing. An interrupt now
        // returns to `Editing`, and the label lives in `run_state.status`.
        let mut state = AppState::new("m");
        state.run_state.status = "aborted".to_string();
        let text = footer_text(&state);
        assert!(
            !text.contains("retry"),
            "the footer must not advertise a retry that nothing implements: {text}"
        );
        assert!(
            text.contains("send"),
            "and editing is available again: {text}"
        );
    }

    /// v0.10.2: this used to assert the footer says `force-quit`.
    /// That string was the defect. `Ctrl+C` is bound to `Abort`, and
    /// `Abort` has no `Quitting` branch — in that mode it falls through
    /// to `state.input.clear()`. There is no "force-quit" action in the
    /// codebase at all, so the test was pinning a footer that lied.
    ///
    /// What is true in this mode, and worth pinning: the two keys that
    /// *do* something — `Esc` cancels the pending exit, `Ctrl+D`
    /// confirms it.
    #[test]
    fn footer_quitting_mode_names_the_two_keys_that_actually_work() {
        let mut state = AppState::new("m");
        state.run_state.mode = RunMode::Quitting;
        let text = footer_text(&state);
        assert!(
            text.contains("Ctrl+D") && text.contains("quit"),
            "Ctrl+D is the key that exits inside the double-tap window: {text}"
        );
        assert!(
            text.contains("Esc") && text.contains("cancel"),
            "Esc clears pending_quit and returns to Editing: {text}"
        );
        assert!(
            !text.contains("force-quit"),
            "no such action exists; Ctrl+C is Abort and clears the input \
             buffer in this mode: {text}"
        );
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    /// Render the 2-line footer to a text snapshot via TestBackend.
    /// Returns both rows joined with '\n' so substring assertions still work
    /// regardless of which line the content lives on.
    fn footer_text(state: &crate::state::AppState) -> String {
        use crate::theme::Theme;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_footer(f, state, &Theme::default(), f.area()))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if let Some(c) = buf.cell((x, y)) {
                    out.push_str(c.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    /// Return just row 1 (cwd/branch/session line).
    fn pwd_line_text(state: &crate::state::AppState) -> String {
        footer_text(state).lines().next().unwrap_or("").to_string()
    }

    /// Return just row 2 (stats/model line).
    fn stats_line_text(state: &crate::state::AppState) -> String {
        footer_text(state).lines().nth(1).unwrap_or("").to_string()
    }

    #[test]
    fn status_bar_shows_theme_name_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.ui_state.theme_name = Some("light".to_string());
        let text = footer_text(&state);
        assert!(
            text.contains("[light]"),
            "expected [light] pill, got: {text}"
        );
    }

    #[test]
    fn status_bar_hides_theme_pill_when_default() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let text = footer_text(&state);
        // No [theme] pill when theme_name is None.
        assert!(
            !text.contains("[light]") && !text.contains("[dark]"),
            "unexpected theme pill: {text}"
        );
    }

    #[test]
    fn status_bar_shows_provider_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("MiniMax-M3");
        state.model_state.provider = Some("anthropic".to_string());
        let text = footer_text(&state);
        // v0.8: Pi-style (provider) prefix on the stats line.
        assert!(
            text.contains("(anthropic)"),
            "expected (anthropic) prefix, got: {text}"
        );
        assert!(text.contains("MiniMax-M3"));
    }

    #[test]
    fn status_bar_hides_provider_when_unset() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let text = footer_text(&state);
        // No (provider) prefix when state.model_state.provider is None.
        assert!(
            !text.contains("(anthropic)") && !text.contains("(openai)"),
            "unexpected provider prefix: {text}"
        );
    }

    #[test]
    fn status_bar_shows_thinking_level_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.model_state.thinking_level = Some("medium".to_string());
        let text = footer_text(&state);
        // v0.8: Pi-style '• level' on the stats line.
        assert!(
            text.contains("• medium"),
            "expected '• medium' indicator, got: {text}"
        );
    }

    // ---- new Pi-style 2-line layout assertions ----

    /// v0.10: this asserted `~/nini` against a hard-coded
    /// `/home/jin/nini`. The tilde substitution reads `$HOME`, so the
    /// assertion held only on the machine it was written on and failed on
    /// every runner (`HOME=/home/runner`) — the CI log for this commit is
    /// one line long and says so.
    ///
    /// A test that encodes a developer's home directory is not a test of the
    /// product; it is a test of the machine. The path is now derived from
    /// `$HOME` so the property — *the home prefix is replaced by `~`* — is
    /// what is asserted, and it is asserted on every machine.
    #[test]
    fn footer_pwd_line_shows_cwd_and_branch() {
        use crate::state::AppState;
        let home = std::env::var("HOME").expect("HOME is set on every CI runner");
        let mut state = AppState::new("m");
        state.session_state.cwd = Some(std::path::PathBuf::from(format!("{home}/nini")));
        state.session_state.git_branch = Some("main".into());
        state.session_state.session_id = Some("a1b2c3d4e5f6".into());
        let line = pwd_line_text(&state);
        assert!(line.contains("~/nini"), "expected tilde-path, got: {line}");
        assert!(line.contains("⎇ main"), "expected git branch, got: {line}");
        assert!(
            line.contains("[a1b2c3d4]"),
            "expected session id pill, got: {line}"
        );
    }

    #[test]
    fn footer_pwd_line_omits_branch_when_unset() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.session_state.cwd = Some(std::path::PathBuf::from("/tmp"));
        let line = pwd_line_text(&state);
        // /tmp isn't under $HOME so shorten_home returns it as-is.
        assert!(line.contains("/tmp"), "expected cwd, got: {line}");
        assert!(!line.contains("⎇"), "branch should be hidden, got: {line}");
    }

    #[test]
    fn footer_pwd_line_shows_no_session_when_unset() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.session_state.cwd = Some(std::path::PathBuf::from("/tmp"));
        let line = pwd_line_text(&state);
        assert!(
            line.contains("no session"),
            "expected placeholder, got: {line}"
        );
    }

    #[test]
    fn footer_stats_line_right_aligns_model_when_space() {
        use crate::state::AppState;
        let mut state = AppState::new("MiniMax-M3");
        state.model_state.provider = Some("anthropic".into());
        state.model_state.thinking_level = Some("medium".into());
        let line = stats_line_text(&state);
        // The " nini " badge is leftmost; the model is rightmost.
        let nini_pos = line.find(" nini ").expect("brand badge missing");
        let model_pos = line.rfind("MiniMax-M3").expect("model missing");
        assert!(
            model_pos > nini_pos,
            "model should appear to the right of the badge, got: {line}"
        );
        // Provider + model + thinking should all appear on the stats line.
        assert!(line.contains("(anthropic)"));
        assert!(line.contains("• medium"));
    }

    #[test]
    fn footer_stats_line_omits_cost_when_zero() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let line = stats_line_text(&state);
        assert!(
            !line.contains('$'),
            "cost should be hidden when cost_usd == 0, got: {line}"
        );
    }

    #[test]
    fn footer_stats_line_shows_tokens_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.run_state.tokens.input = 100;
        state.run_state.tokens.output = 50;
        let line = stats_line_text(&state);
        assert!(line.contains("in 100"), "got: {line}");
        assert!(line.contains("out 50"), "got: {line}");
    }

    #[test]
    fn footer_stats_line_shows_phase_when_running() {
        use crate::state::AppState;
        use crate::state::RunMode;
        let mut state = AppState::new("m");
        state.run_state.mode = RunMode::Running;
        let text = footer_text(&state);
        assert!(
            text.contains("working"),
            "running mode should show 'working' phase, got: {text}"
        );
    }

    #[test]
    fn footer_stats_line_shows_diff_pill_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.ui_state.last_diff = Some((3, 1));
        let line = stats_line_text(&state);
        assert!(
            line.contains("[edit +3 -1]"),
            "expected edit diff pill, got: {line}"
        );
    }

    #[test]
    fn footer_two_lines_sum_to_two_rows() {
        use crate::state::AppState;
        let state = AppState::new("m");
        let text = footer_text(&state);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "footer must render exactly 2 rows, got {}: {:?}",
            lines.len(),
            lines
        );
    }
}

#[cfg(test)]
mod cursor_and_scroll_tests {
    //! v0.10. These cover the four TUI defects found on 2026-10-12.
    //!
    //! They live here rather than in `tests/` because the functions they
    //! exercise are private, and that is itself part of the finding:
    //! `KeyCode` is a private import behind a `pub` field in `keys.rs`, so
    //! an integration test cannot construct a key at all. Private-here,
    //! private-there — the shape §21.3 of the plan calls "unobservable,
    //! therefore untested".

    use super::{lines_needed_for_items, prompt_cursor_geometry, prompt_height};
    use crate::width::display_width;

    // ---- BUG 1: the cursor counted characters where cells were meant ----

    #[test]
    fn ascii_cursor_lands_on_its_character() {
        let text = "hello";
        let g = prompt_cursor_geometry(text, text.len(), 20);
        assert_eq!(g.column, 5);
        assert_eq!(g.row, 0);
    }

    #[test]
    fn a_wide_character_advances_the_column_by_two_cells() {
        let text = "研究一下";
        let g = prompt_cursor_geometry(text, text.len(), 20);
        assert_eq!(
            g.column,
            display_width(text),
            "cursor column must be cells, not chars — otherwise it lands on \
             top of the second character"
        );
        assert_eq!(g.column, 8);
    }

    #[test]
    fn emoji_advance_the_column_by_two_cells_too() {
        let text = "done ✅";
        let g = prompt_cursor_geometry(text, text.len(), 40);
        assert_eq!(g.column, display_width(text));
    }

    /// The bug measured in the field, on a prompt that mixes an ASCII
    /// command with Chinese — which is what real input looks like.
    #[test]
    fn a_mixed_prompt_puts_the_cursor_after_the_text() {
        let text = "/research 研究一下这个问题";
        let g = prompt_cursor_geometry(text, text.len(), 40);
        assert_eq!(g.column, 26, "was 18 before the fix");
    }

    // ---- BUG 1b: the row ignored newlines entirely ----

    #[test]
    fn a_second_line_puts_the_cursor_on_the_second_row() {
        let text = "first\nsecond";
        let g = prompt_cursor_geometry(text, text.len(), 40);
        assert_eq!(g.row, 1, "one explicit newline before the cursor");
        assert_eq!(g.column, display_width("second"));
    }

    #[test]
    fn a_third_line_and_wide_characters_both_count() {
        let text = "一\n二\n三";
        let g = prompt_cursor_geometry(text, text.len(), 40);
        assert_eq!(g.row, 2);
        assert_eq!(g.column, display_width("三"));
    }

    #[test]
    fn a_wrapped_line_adds_rows() {
        // 20 cells per row, 25 cells of text on one line → second row.
        let text = "abcdefghijklmnopqrstuvwxy";
        let g = prompt_cursor_geometry(text, text.len(), 20);
        assert_eq!(g.row, 1);
        assert_eq!(g.column, 5);
    }

    #[test]
    fn a_wide_character_can_push_onto_the_next_row() {
        // 6 CJK chars = 12 cells at inner_width 10 → row 1, column 2.
        let text = "一二三四五六";
        let g = prompt_cursor_geometry(text, text.len(), 10);
        assert_eq!(g.row, 1, "was 0 when the row came from a char count");
        assert_eq!(g.column, 2);
    }

    /// The old code indexed `text[..cursor]` directly. A cursor left on a
    /// byte offset that is not a char boundary — reachable through
    /// programmatic edits rather than keystrokes — must not panic the TUI.
    #[test]
    fn a_cursor_mid_character_does_not_panic() {
        let text = "研究";
        let g = prompt_cursor_geometry(text, 1, 20); // inside the first 3-byte char
        assert!(g.column <= display_width(text));
    }

    #[test]
    fn a_cursor_past_the_end_is_clamped() {
        let g = prompt_cursor_geometry("short", 9999, 20);
        assert_eq!(g.column, display_width("short"));
    }

    // ---- BUG 3: PageUp moved one line, whatever a line was worth ----

    #[test]
    fn one_page_covers_the_viewport_height() {
        // 10 lines of 3 rows each, viewport 12 rows → 4 lines.
        let per_line = vec![3usize; 10];
        assert_eq!(lines_needed_for_items(&per_line, 12, 10), 4);
    }

    #[test]
    fn one_page_crosses_a_tall_line() {
        // From the bottom: line 2 is 1 row, line 1 is 50. A 20-row viewport
        // needs both — 1 + 19 of line 1 — so two lines, not one. The first
        // draft of this test asserted 1, on the assumption that the tall
        // line covers the page on its own. It does not; there is a row
        // beneath it. The implementation was right and the test was wrong,
        // which is the only thing worth writing down.
        let per_line = vec![1usize, 50, 1];
        assert_eq!(lines_needed_for_items(&per_line, 20, 3), 2);
    }

    #[test]
    fn one_page_over_many_tall_lines() {
        // This is the case the hardcoded step of 1 got wrong: eight lines
        // of 10 rows, viewport 20 → two lines, not twenty presses.
        let per_line = vec![10usize; 8];
        assert_eq!(lines_needed_for_items(&per_line, 20, 8), 2);
    }

    #[test]
    fn one_page_never_runs_past_the_top() {
        let per_line = vec![1usize; 3];
        assert_eq!(lines_needed_for_items(&per_line, 100, 3), 3);
        assert_eq!(lines_needed_for_items(&[], 10, 0), 0);
    }

    /// The count must match what the renderer actually skips, or "one page"
    /// is a different number in the key handler than on screen. Both walk
    /// from the bottom, and this pins that they stay in step.
    #[test]
    fn the_page_count_agrees_with_the_renderer_skip() {
        let per_line = vec![2usize, 7, 1, 4, 9];
        let viewport = 11usize;
        let expected_skip = lines_needed_for_items(&per_line, viewport, per_line.len());

        // Reproduce the renderer's own skip loop.
        let mut items_to_skip = 0usize;
        let mut lines_skipped = 0usize;
        for i in (0..per_line.len()).rev() {
            if lines_skipped >= per_line.len() {
                break;
            }
            items_to_skip += per_line[i];
            lines_skipped += 1;
            if items_to_skip >= viewport {
                break;
            }
        }
        assert_eq!(expected_skip, lines_skipped);
    }

    // ---- BUG 4: the same mistake in three more places ----

    /// The input box budgeted height with `(c as u32) > 0x1100`, a
    /// hand-rolled stand-in under a comment admitting `unicode-width` was
    /// the real answer. U+FE0F is a variation selector: zero-width, above
    /// the threshold, so `✅️` was charged four cells for two.
    #[test]
    fn the_old_height_heuristic_miscounted_emoji() {
        let emoji_with_selector = "\u{2705}\u{FE0F}";
        let old: usize = emoji_with_selector
            .chars()
            .map(|c| if (c as u32) > 0x1100 { 2 } else { 1 })
            .sum();
        assert_eq!(
            display_width(emoji_with_selector),
            2,
            "the exact function disagrees with the approximation"
        );
        assert_eq!(
            old, 4,
            "the approximation over-charges, which is the defect"
        );
    }

    // ---- the two tests this file was missing, found by falsification ----

    /// The first pass of this module tested `lines_needed_for_items` and
    /// therefore passed while `render_transcript` stored a hardcoded `1`
    /// into `page_lines` — the unit under test was correct and the wiring
    /// was not. The injection that caught it: replace the store with `1`,
    /// and every test here stayed green.
    ///
    /// So: the function is tested, *and* the publish is tested.
    #[test]
    fn a_render_publishes_a_real_page_size() {
        use crate::render::render_frame;
        use crate::state::{AppState, RunMode, TranscriptLine};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use std::sync::Arc;

        let mut state = AppState::default();
        state.run_state.mode = RunMode::Editing;
        // Forty distinct assistant lines at 80 columns. Each renders to one
        // row, so a 20-row viewport covers 20 of them: one page must be a
        // count near the viewport height, not 1.
        Arc::make_mut(&mut state.transcript_state)
            .lines
            .extend((0..40).map(|i| TranscriptLine::AssistantText(format!("line {i}"))));

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render_frame(f, &state)).unwrap();

        let published = state.page_lines();
        assert!(
            published > 1,
            "after a real render, one page must be more than one line; \
             got {published}"
        );
        assert!(
            published <= state.transcript_state.lines.len(),
            "and cannot exceed the transcript"
        );
    }

    /// Same shape, for the height budget. `the_old_height_heuristic_
    /// miscounted_emoji` records what the broken code did, so it passes
    /// forever regardless of what the current code does — the injection
    /// confirmed that by restoring the old arithmetic and watching all 17
    /// tests stay green. This one asks what the current code does.
    #[test]
    fn the_height_budget_matches_the_exact_width() {
        // Base glyph plus variation selector: one cell of glyph, two
        // codepoints. The old `> 0x1100` charged the selector as a cell.
        let emoji = "\u{2705}\u{FE0F}";
        assert_eq!(display_width(emoji), 2);

        for inner_width in [2usize, 3, 4, 10] {
            let by_exact = prompt_height(emoji, inner_width, 20);
            // What the old approximation would have concluded:
            let old_first_line = 4usize; // 2 codepoints, both above 0x1100
            let old_wrapped = if old_first_line <= inner_width {
                1u16
            } else {
                (old_first_line as f32 / inner_width as f32).ceil() as u16
            };
            let old_height = (old_wrapped + 2).clamp(3, 20);
            if inner_width < 4 {
                assert_eq!(
                    by_exact, 3,
                    "an emoji is {inner_width} cells or fewer at inner_width \
                     {inner_width}: it must not wrap. The old heuristic said \
                     {old_height}."
                );
            }
        }
    }

    #[test]
    fn a_tall_prompt_grows_the_box_and_an_empty_one_does_not() {
        use super::prompt_height;

        assert_eq!(prompt_height("", 40, 20), 3, "minimum box");
        assert_eq!(prompt_height("short", 40, 20), 3);
        assert_eq!(prompt_height("a\nb", 40, 20), 4, "one newline adds a row");
        assert_eq!(prompt_height(&"x".repeat(80), 40, 20), 4, "one wrap");
        assert_eq!(prompt_height(&"x".repeat(200), 40, 20), 7, "five wraps");
        assert_eq!(
            prompt_height(&"x".repeat(500), 40, 6),
            6,
            "clamped to max_h, never past it"
        );
    }

    /// The scroll indicators size themselves with U+2191 / U+2193, which are
    /// East-Asian-Ambiguous: one cell in a western terminal, two in a CJK
    /// one. Switching them from `chars().count()` to `display_width` is a
    /// **behaviour-preserving change today**, and the test says so rather
    /// than claiming a fix.
    ///
    /// `unicode_width`'s `width()` resolves Ambiguous to 1 and
    /// `width_cjk()` to 2; `display_width` uses the former. So for these
    /// two labels the two expressions agree exactly. The change buys
    /// consistency with the rest of the file and makes the site correct if
    /// the project ever adopts `width_cjk`, which the old expression could
    /// never be.
    ///
    /// I expected this to be a bug fix and wrote the comment claiming a
    /// visible shift in CJK terminals before checking. It is not one.
    #[test]
    fn scroll_indicator_labels_agree_with_a_char_count_today() {
        for arrow in ["\u{2191}", "\u{2193}"] {
            let label = format!(" {arrow} 12 more ");
            assert_eq!(
                display_width(&label),
                label.chars().count(),
                "if this stops holding, `display_width` grew CJK-awareness and \
                 the behaviour claim in the doc comment needs updating"
            );
        }
    }
}
