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
    render_assistant_message, render_bash_execution, render_divider, render_tool_call,
    render_tool_result, render_user_message,
};
use crate::selector::{SelectorItem, SelectorPanel};
use crate::state::{AppState, RunMode, TranscriptLine};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap};

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
        .ui_state.completion
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        // Up to 8 lines + 2 (border)
        let n = state
            .ui_state.completion
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
        let inner_width = area
            .width
            .saturating_sub(2 + 2 * padding_x + 2)
            .max(1) as usize;
        let explicit_lines = state.input.text.matches('\n').count() as u16;
        let first_line_width = state
            .input
            .text
            .split('\n')
            .next()
            .unwrap_or("")
            .chars()
            // Approximate display width: BMP chars = 1, CJK / wide
            // / emoji (>= U+1100) = 2 cells. The exact mapping lives
            // in `unicode-width`; a 2-cell cap is safe for budgeting.
            .map(|c| if (c as u32) > 0x1100 { 2 } else { 1 })
            .sum::<usize>();
        let wrapped_rows = if first_line_width <= inner_width {
            1u16
        } else {
            (first_line_width as f32 / inner_width as f32).ceil() as u16
        };
        let total_rows = explicit_lines + wrapped_rows;
        let max_h = (area.height / 3).max(5);
        (total_rows + 2).clamp(3, max_h)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // footer (Pi-style 2-row: pwd line + stats/model line)
            Constraint::Length(if state.ui_state.search.is_some() { 1 } else { 0 }), // search bar (only when active)
            Constraint::Min(3),    // transcript
            Constraint::Length(if popup_height > 0 { popup_height } else { 3 }), // prompt OR popup
            Constraint::Length(1), // key hints
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
        .ui_state.completion
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
    if area.height < 2 {
        // Fallback: very small area — collapse to a single stats-only line.
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
        spans.push(Span::styled(
            shorten_home(cwd),
            theme.fg_style("dim"),
        ));
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

    f.render_widget(Paragraph::new(RLine::from(spans)), area);
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
        let pct = (state.run_state.context_used as f64
            / state.run_state.context_window as f64)
            * 100.0;
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
    if let Some(name) = state
        .ui_state
        .theme_name
        .as_ref()
        .filter(|n| !n.is_empty())
    {
        right.push(Span::raw("  "));
        right.push(Span::styled(
            format!("[{name}]"),
            theme.fg_style("accent"),
        ));
    }

    // ---- Compose with right-alignment ----
    let left_width: usize = left
        .iter()
        .map(|s| s.content.as_ref().chars().count())
        .sum();
    let right_width: usize = right
        .iter()
        .map(|s| s.content.as_ref().chars().count())
        .sum();
    let width = area.width as usize;
    let min_gap = 2;

    let mut spans = left;
    if left_width + right_width + min_gap <= width {
        spans.push(Span::raw(" ".repeat(width - left_width - right_width)));
    } else {
        // Not enough room: still keep a min gap so the two halves don't collide.
        spans.push(Span::raw(" ".repeat(min_gap)));
    }
    spans.extend(right);

    f.render_widget(Paragraph::new(RLine::from(spans)), area);
}

/// Render a simple ASCII progress bar for context-window usage.
fn context_bar(pct: f64) -> String {
    let width = 8;
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    format!("[{}{}]", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty))
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
    let Some(search) = &state.ui_state.search else { return };
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
    let query = Span::styled(
        search.query.clone(),
        theme.fg_style("text"),
    );
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

/// v0.8.4 (bugfix, ux-001): the previous version stuffed the
/// accumulated thinking text into a single RLine, so long blocks
/// were clipped to the viewport tail (e.g. `💭 ectory.` for a
/// 400-char reasoning). Hard-wrap each logical line at INNER_MAX
/// cells (using display_width so emoji / CJK count correctly) and
/// prefix every continuation with the same indent so the 💭 gutter
/// stays aligned.
fn wrap_thinking_block(text: &str) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    const FIRST_PREFIX: &str = "  \u{1F4AD} ";
    const CONT_PREFIX: &str = "   ";
    const FIRST_W: usize = 5; // 2 + 2 + 1
    const CONT_W: usize = 3;
    const INNER_MAX: usize = 180;

    let mut out: Vec<String> = Vec::new();
    for logical in text.split('\n') {
        if logical.is_empty() {
            out.push(format!("{FIRST_PREFIX}"));
            continue;
        }
        let mut first = true;
        let mut rest = logical;
        loop {
            if rest.is_empty() {
                break;
            }
            let (prefix, pw) = if first { (FIRST_PREFIX, FIRST_W) } else { (CONT_PREFIX, CONT_W) };
            first = false;
            let total_w: usize = pw
                + rest.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum::<usize>();
            if total_w <= INNER_MAX {
                out.push(format!("{prefix}{rest}"));
                break;
            }
            // Hard-wrap at the largest char index whose prefix+index
            // still fits in INNER_MAX. Simple char-count is fine here
            // — word-boundary wrapping isn't worth the complexity for
            // a dim/italic ephemeral block, and the trade-off is
            // occasionally splitting a long word vs. an unreadable
            // tail clip.
            let max_tail = INNER_MAX - pw;
            let mut used = 0usize;
            let mut cut = rest.len();
            for (i, c) in rest.char_indices() {
                let w = UnicodeWidthChar::width(c).unwrap_or(0);
                if used + w > max_tail {
                    cut = i;
                    break;
                }
                used += w;
            }
            let (head, tail) = rest.split_at(cut);
            out.push(format!("{prefix}{head}"));
            rest = tail;
        }
    }
    if out.is_empty() {
        out.push(format!("{FIRST_PREFIX}"));
    }
    out
}

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
    let lines = &state.transcript_state.lines;
    let total_lines = lines.len();
    let visible_height = area.height as usize;
    let autoscroll = state.transcript_state.autoscroll || total_lines == 0;
    let scroll_offset = state.transcript_state.scroll_offset;

    // First pass: per-line item counts. Used for user-scroll item-space
    // offset translation, and for the overflow indicator count.
    let items_per_line: Vec<usize> = lines
        .iter()
        .map(|line| items_for_line(line, theme, area.width as usize).len())
        .collect();
    let total_items: usize = items_per_line.iter().sum();

    // Choose the visible slice.
    let mut visible_items: Vec<ListItem> = Vec::with_capacity(visible_height.min(total_items));
    let mut hidden_above_items: usize = 0;

    if total_items <= visible_height {
        // Everything fits — render everything.
        for line in lines.iter() {
            visible_items.extend(items_for_line(line, theme, area.width as usize));
        }
    } else if autoscroll {
        // Tail-aligned: keep the latest `visible_height` items.
        hidden_above_items = total_items - visible_height;
        let mut budget = visible_height;
        for i in (0..total_lines).rev() {
            let count = items_per_line[i];
            if count <= budget {
                let line_items = items_for_line(&lines[i], theme, area.width as usize);
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
                let mut line_items = items_for_line(&lines[i], theme, area.width as usize);
                let start = line_items.len() - budget;
                visible_items = line_items.split_off(start)
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
            let mut line_items = items_for_line(&lines[i], theme, area.width as usize);
            let start = line_items.len() - take;
            visible_items = line_items.split_off(start)
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

    // (debug prints removed)
    let list = List::new(visible_items)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    f.render_widget(list, area);

    // v0.8.7 (ux-002): scroll indicator in autoscroll mode when
    // content overflows above. Tells them they can scroll up to see
    // more. Bottom-right placement matches the user-scroll indicator.
    if autoscroll && hidden_above_items > 0 && area.height >= 3 {
        let label = format!(" \u{2191} {hidden_above_items} more above ");
        let style = theme.fg_style("dim");
        let width = (label.chars().count() as u16).min(area.width);
        let ind_area = Rect::new(area.x, area.y, width, 1);
        let ind = Paragraph::new(Span::styled(label, style));
        f.render_widget(ind, ind_area);
    }

    // v0.8.4 (ux-001): user-scroll `↓ N more` indicator at the
    // bottom-right corner of the transcript area. Suppressed during
    // autoscroll so it doesn't flicker on every token delta.
    if !autoscroll && scroll_offset > 0 && area.height >= 3 {
        let hidden = scroll_offset;
        let label = format!(" \u{2193} {hidden} more ");
        let style = theme.fg_style("accent").add_modifier(Modifier::BOLD);
        let width = (label.chars().count() as u16).min(area.width);
        let x = area.x + area.width.saturating_sub(width);
        let y = area.y + area.height.saturating_sub(1);
        let ind_area = Rect::new(x, y, width, 1);
        let ind = Paragraph::new(Span::styled(label, style))
            .alignment(ratatui::layout::Alignment::Right);
        f.render_widget(ind, ind_area);
    }
}

/// v0.8.7 (ux-002): extract the per-line item expansion so we can
/// compute item counts without rendering, and render twice without doing
/// the work twice. (We DO render twice — once for counting, once for
/// the actual list — but the cost is bounded: 1..N items per line where
/// N is typically <10. For a 200-line transcript this is cheap.)
fn items_for_line(
    line: &TranscriptLine,
    theme: &Theme,
    area_width: usize,
) -> Vec<ListItem<'static>> {
    match line {
        TranscriptLine::User(text) => render_user_message(text, theme)
            .into_iter()
            .map(ListItem::new)
            .collect(),
        TranscriptLine::AssistantText(text) => render_assistant_message(text, theme)
            .into_iter()
            .map(ListItem::new)
            .collect(),
        // v0.8: dim/italic reasoning block (Pi-style). Wrap by display
        // width so long thinking blocks render across multiple rows.
        TranscriptLine::ThinkingText(text) => wrap_thinking_block(text)
            .into_iter()
            .map(|line| {
                ListItem::new(RLine::from(Span::styled(
                    line,
                    theme
                        .fg_style("dim")
                        .add_modifier(Modifier::ITALIC),
                )))
            })
            .collect(),
        TranscriptLine::ToolCall { name, args, collapsed } => {
            let lines = render_tool_call(name, args, theme);
            let mut out: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(ListItem::new(crate::rich::render_collapsed_hint(theme)));
            }
            out
        }
        TranscriptLine::ToolResult { ok, content, collapsed, duration_ms } => {
            // v0.8.7 (ux-002): pass the actual area width so per-line
            // truncation accounts for the visible terminal width. When
            // the renderer is called outside a Frame (tests), area.width
            // is 0 and we fall back to the legacy 200-byte budget.
            let max_w = if area_width > 4 { Some(area_width) } else { None };
            let lines = render_tool_result(*ok, content, *duration_ms, theme, max_w);
            let mut out: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(ListItem::new(crate::rich::render_collapsed_hint(theme)));
            }
            out
        }
        TranscriptLine::Divider => vec![ListItem::new(render_divider(theme))],
        TranscriptLine::StopNotice(msg) => vec![ListItem::new(RLine::from(Span::styled(
            format!("  {msg}"),
            theme.fg_style("error"),
        )))],
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
            let lines = render_bash_execution(
                cmd,
                output,
                stderr,
                *ok,
                *exit_code,
                *duration_ms,
                theme,
            );
            let mut out: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
            if *collapsed {
                if !out.is_empty() {
                    out.truncate(1);
                }
                out.push(ListItem::new(crate::rich::render_collapsed_hint(theme)));
            }
            out
        }
    }
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
        theme
            .fg_style("accent")
            .add_modifier(Modifier::BOLD),
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
        spans.push(Span::styled(
            format!("  {desc}"),
            theme.fg_style("muted"),
        ));
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
    let prompt_symbol = if state.run_state.mode == RunMode::Running {
        "⏵"
    } else {
        "❯"
    };
    let text = state.input.text.clone();
    let cursor_byte = state.input.cursor;

    // Compute display cursor position (chars, not bytes, for proper rendering).
    let cursor_char = text[..cursor_byte.min(text.len())].chars().count();

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
            let prefix_char = if is_first_line(i) { prompt_symbol } else { "❯" };
            let prefix_style = if is_first_line(i) { prompt_style_bold } else { prompt_style_dim };
            line_widgets.push(RLine::from(vec![
                Span::styled(format!("{prefix_char} "), prefix_style),
                Span::raw(l.as_str()),
            ]));
        }
    }
    let para = Paragraph::new(line_widgets)
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
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);

    // Render cursor. Account for paddingX: x offset starts after the
    // left border (1) + padding (padding_x) + the 2-char prompt symbol.
    if state.run_state.mode != RunMode::Running
        && area.height >= 3
        && area.width >= 2 + padding_x + 2
    {
        let left_inset: u16 = 1 /* border */ + padding_x + 2 /* "❯ " */;
        let inner_width = area.width.saturating_sub(1 /* right border */ + padding_x + left_inset);
        let cursor_x = area.x + left_inset + (cursor_char as u16 % inner_width.max(1));
        let line_idx = (cursor_char as u16) / inner_width.max(1);
        let cursor_y = area.y + 1 /* top border */ + line_idx.min(area.height.saturating_sub(2) - 1);
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
    let narrow = area.width < 70;
    let hints: Vec<(&str, &str)> = match state.run_state.mode {
        RunMode::Editing if state.ui_state.help_extended && !narrow => vec![
            (" F1 ", "short"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Alt+Backspace ", "kill-word"),
            (" Alt+D ", "kill"),
            (" Ctrl+Z ", "undo"),
            (" Ctrl+Y ", "yank"),
            (" Ctrl+L ", "model"),
            (" Ctrl+T ", "thinking"),
            (" Ctrl+P ", "model+"),
            (" Ctrl+O ", "collapse"),
            (" Ctrl+C ", "quit"),
            (" Ctrl+D ", "exit"),
        ],
        RunMode::Editing if state.ui_state.help_extended => vec![
            // narrow extended: drop the seldom-used kill/undo/yank hints
            (" F1 ", "short"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+L ", "model"),
            (" Ctrl+C ", "quit"),
        ],
        RunMode::Editing if narrow => vec![
            // narrow compact: keep only the essentials
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+C ", "quit"),
        ],
        RunMode::Editing => vec![
            (" F1 ", "help"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+L ", "model"),
            (" Ctrl+C ", "quit"),
        ],
        RunMode::Running => vec![
            (" Esc ", "abort"),
            (" Ctrl+C ", "force-quit"),
        ],
        RunMode::Aborted => vec![
            (" Enter ", "retry"),
            (" Esc ", "clear"),
        ],
        RunMode::Quitting => vec![(" Ctrl+C ", "force-quit")],
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
                theme
                    .fg_style("accent")
                    .add_modifier(Modifier::BOLD)
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
    fn footer_aborted_mode_shows_retry() {
        let mut state = AppState::new("m");
        state.run_state.mode = RunMode::Aborted;
        let text = footer_text(&state);
        assert!(text.contains("retry"));
    }

    #[test]
    fn footer_quitting_mode_shows_force_quit() {
        let mut state = AppState::new("m");
        state.run_state.mode = RunMode::Quitting;
        let text = footer_text(&state);
        assert!(text.contains("force-quit"));
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
        assert!(text.contains("[light]"), "expected [light] pill, got: {text}");
    }

    #[test]
    fn status_bar_hides_theme_pill_when_default() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let text = footer_text(&state);
        // No [theme] pill when theme_name is None.
        assert!(!text.contains("[light]") && !text.contains("[dark]"),
            "unexpected theme pill: {text}");
    }

    #[test]
    fn status_bar_shows_provider_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("MiniMax-M3");
        state.model_state.provider = Some("anthropic".to_string());
        let text = footer_text(&state);
        // v0.8: Pi-style (provider) prefix on the stats line.
        assert!(text.contains("(anthropic)"),
                "expected (anthropic) prefix, got: {text}");
        assert!(text.contains("MiniMax-M3"));
    }

    #[test]
    fn status_bar_hides_provider_when_unset() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let text = footer_text(&state);
        // No (provider) prefix when state.model_state.provider is None.
        assert!(!text.contains("(anthropic)") && !text.contains("(openai)"),
                "unexpected provider prefix: {text}");
    }

    #[test]
    fn status_bar_shows_thinking_level_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.model_state.thinking_level = Some("medium".to_string());
        let text = footer_text(&state);
        // v0.8: Pi-style '• level' on the stats line.
        assert!(text.contains("• medium"),
                "expected '• medium' indicator, got: {text}");
    }

    // ---- new Pi-style 2-line layout assertions ----

    #[test]
    fn footer_pwd_line_shows_cwd_and_branch() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.session_state.cwd = Some(std::path::PathBuf::from("/home/jin/nini"));
        state.session_state.git_branch = Some("main".into());
        state.session_state.session_id = Some("a1b2c3d4e5f6".into());
        let line = pwd_line_text(&state);
        assert!(line.contains("~/nini"), "expected tilde-path, got: {line}");
        assert!(line.contains("⎇ main"), "expected git branch, got: {line}");
        assert!(line.contains("[a1b2c3d4]"), "expected session id pill, got: {line}");
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
        assert!(line.contains("no session"),
                "expected placeholder, got: {line}");
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
        assert!(model_pos > nini_pos,
                "model should appear to the right of the badge, got: {line}");
        // Provider + model + thinking should all appear on the stats line.
        assert!(line.contains("(anthropic)"));
        assert!(line.contains("• medium"));
    }

    #[test]
    fn footer_stats_line_omits_cost_when_zero() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        let line = stats_line_text(&state);
        assert!(!line.contains('$'),
                "cost should be hidden when cost_usd == 0, got: {line}");
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
        assert!(text.contains("working"),
                "running mode should show 'working' phase, got: {text}");
    }

    #[test]
    fn footer_stats_line_shows_diff_pill_when_set() {
        use crate::state::AppState;
        let mut state = AppState::new("m");
        state.ui_state.last_diff = Some((3, 1));
        let line = stats_line_text(&state);
        assert!(line.contains("[edit +3 -1]"),
                "expected edit diff pill, got: {line}");
    }

    #[test]
    fn footer_two_lines_sum_to_two_rows() {
        use crate::state::AppState;
        let state = AppState::new("m");
        let text = footer_text(&state);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2,
                  "footer must render exactly 2 rows, got {}: {:?}",
                  lines.len(), lines);
    }
}
