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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

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
        .completion
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        // Up to 8 lines + 2 (border)
        let n = state
            .completion
            .as_ref()
            .map(|p| p.items.len())
            .unwrap_or(0);
        (n as u16).min(8) + 2
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status bar
            Constraint::Min(3),    // transcript
            Constraint::Length(if popup_height > 0 { popup_height } else { 3 }), // prompt OR popup
            Constraint::Length(1), // key hints
        ])
        .split(area);

    render_status(f, state, theme, chunks[0]);
    render_transcript(f, state, theme, chunks[1]);
    if state
        .completion
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false)
    {
        render_completion_popup(f, state, theme, chunks[2]);
    } else {
        render_prompt(f, state, theme, chunks[2]);
    }
    render_key_hints(f, state, theme, chunks[3]);

    if state.mode == RunMode::Running {
        render_running_indicator(f, theme, chunks[1]);
    }
}

fn render_status(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let mode = match state.mode {
        RunMode::Editing => "[ready]",
        RunMode::Running => "[running...]",
        RunMode::Aborted => "[aborted]",
        RunMode::Quitting => "[quitting]",
    };
    let session = state.session_id.as_deref().unwrap_or("(no session)");
    let line = RLine::from(vec![
        Span::styled(
            " nini ",
            theme
                .bg_style("accent")
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " model={} session={} {mode} ",
            state.model, session
        )),
        Span::styled(
            format!(
                "tokens: in={} out={} (est) ",
                state.tokens.input, state.tokens.output
            ),
            theme.fg_style("dim"),
        ),
        Span::styled(
            format!("{} ", state.status),
            theme.fg_style("warning"),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_transcript(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    // Apply scroll: when scroll_offset > 0, show the slice ending at
    // (total - scroll_offset). When autoscroll is on or scroll_offset is
    // 0, show the entire transcript (capped by area.height).
    let total = state.transcript.len();
    let visible_height = area.height as usize;
    let (start, end) = if state.scroll_offset == 0 || total == 0 {
        let end = total.min(visible_height);
        (0, end)
    } else {
        // Show the last (visible_height) lines ending at (total - scroll_offset).
        let end = total.saturating_sub(state.scroll_offset);
        let start = end.saturating_sub(visible_height);
        (start, end)
    };
    // Build ListItems. Each TranscriptLine may map to 1..N rows:
    //   - User: 1 row (or N rows for multi-line)
    //   - AssistantText: 1..N rows from Markdown rendering
    //   - ToolCall/ToolResult: 1..N rows (header + body lines)
    //   - BashExecution: 1 banner row + N output rows
    //   - Divider: 1 row
    //
    // We flatten per-line expansion into a Vec<ListItem> by emitting
    // multiple items for a single TranscriptLine. Then truncate to
    // `visible_height` based on the cumulative tail.
    let mut items: Vec<ListItem> = Vec::new();
    for line in state.transcript.iter().skip(start).take(end.saturating_sub(start)) {
        let new_items: Vec<ListItem> = match line {
            TranscriptLine::User(text) => render_user_message(text, theme)
                .into_iter()
                .map(ListItem::new)
                .collect(),
            TranscriptLine::AssistantText(text) => render_assistant_message(text, theme)
                .into_iter()
                .map(ListItem::new)
                .collect(),
            TranscriptLine::ToolCall { name, args } => {
                let lines = render_tool_call(name, args, theme);
                lines.into_iter().map(ListItem::new).collect()
            }
            TranscriptLine::ToolResult { ok, content } => {
                let lines = render_tool_result(*ok, content, theme);
                lines.into_iter().map(ListItem::new).collect()
            }
            TranscriptLine::Divider => vec![ListItem::new(render_divider(theme))],
            TranscriptLine::BashExecution {
                cmd,
                output,
                ok,
                exit_code,
                duration_ms,
                ..
            } => {
                let lines = render_bash_execution(
                    cmd,
                    output,
                    *ok,
                    *exit_code,
                    *duration_ms,
                    theme,
                );
                lines.into_iter().map(ListItem::new).collect()
            }
        };
        items.extend(new_items);
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    f.render_widget(list, area);
}

fn render_prompt(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let prompt_symbol = if state.mode == RunMode::Running {
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

    // Build a Paragraph with line-by-line rendering, then position cursor manually.
    let mut line_widgets: Vec<RLine> = Vec::new();
    if lines.is_empty() {
        line_widgets.push(RLine::from(Span::raw(" ")));
    } else {
        for (i, l) in lines.iter().enumerate() {
            let prefix = if i == 0 { prompt_symbol } else { " " };
            line_widgets.push(RLine::from(vec![
                Span::styled(
                    format!("{prefix} "),
                    theme
                        .fg_style("success")
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(l.as_str()),
            ]));
        }
    }
    let para = Paragraph::new(line_widgets)
        .block(Block::default().borders(Borders::TOP).title(" input "))
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);

    // Render cursor
    if state.mode != RunMode::Running && area.height >= 2 && area.width >= 2 {
        let cursor_x = area.x + 2 + (cursor_char as u16 % area.width.saturating_sub(2));
        let line_idx = (cursor_char as u16) / area.width.saturating_sub(2);
        let cursor_y = area.y + 1 + line_idx.min(area.height.saturating_sub(2) - 1);
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_key_hints(f: &mut Frame, _state: &AppState, theme: &Theme, area: Rect) {
    let hints = RLine::from(vec![
        Span::styled(" F1 ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("help "),
        Span::styled(" Ctrl+C ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("quit "),
        Span::styled(" Ctrl+D ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("exit "),
        Span::styled(" Enter ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("send "),
        Span::styled(" Ctrl+L ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("model "),
        Span::styled(" ↑↓ ", theme.bg_style("dim").fg(Color::White)),
        Span::raw("history"),
    ]);
    f.render_widget(Paragraph::new(hints), area);
}

/// Render the slash-command completion popup above the prompt.
fn render_completion_popup(f: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let Some(popup) = &state.completion else {
        return;
    };
    if popup.items.is_empty() {
        return;
    }

    let lines: Vec<RLine> = popup
        .items
        .iter()
        .enumerate()
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
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" commands (up/down select, Tab/Enter accept, Esc cancel) ")
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

fn render_running_indicator(f: &mut Frame, theme: &Theme, area: Rect) {
    // Subtle visual hint: a thin spinner row at the top-right of transcript.
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (chrono::Utc::now().timestamp_millis() / 80) as usize % spinner.len();
    let indicator = RLine::from(Span::styled(spinner[i], theme.fg_style("warning")));
    let indicator_area = Rect {
        x: area.x + area.width.saturating_sub(3),
        y: area.y,
        width: 3,
        height: 1,
    };
    // Clear the cell first to avoid overlay artifacts.
    f.render_widget(Clear, indicator_area);
    f.render_widget(Paragraph::new(indicator), indicator_area);
}
