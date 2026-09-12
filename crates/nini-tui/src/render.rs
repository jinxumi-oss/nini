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

use crate::state::{ AppState, CompletionItem, RunMode, TranscriptLine };
use ratatui::layout::{ Constraint, Direction, Layout, Rect };
use ratatui::style::{ Color, Modifier, Style };
use ratatui::text::{ Line as RLine, Span };
use ratatui::widgets::{ Block, Borders, Clear, List, ListItem, Paragraph, Wrap };
use ratatui::Frame;

/// Render the full TUI frame.
pub fn render_frame(f: &mut Frame, state: &AppState) {
    let area = f.area();
    // When a completion popup is showing, we steal one row from the
    // transcript area so the popup floats above the prompt.
    let popup_height = if state.completion.as_ref().map(|p| !p.is_empty()).unwrap_or(false) {
        // Up to 8 lines + 2 (border)
        let n = state.completion.as_ref().map(|p| p.items.len()).unwrap_or(0);
        (n as u16).min(8) + 2
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                            // status bar
            Constraint::Min(3),                               // transcript
            Constraint::Length(if popup_height > 0 { popup_height } else { 3 }), // prompt OR popup
            Constraint::Length(1),                            // key hints
        ])
        .split(area);

    render_status(f, state, chunks[0]);
    render_transcript(f, state, chunks[1]);
    if state.completion.as_ref().map(|p| !p.is_empty()).unwrap_or(false) {
        render_completion_popup(f, state, chunks[2]);
    } else {
        render_prompt(f, state, chunks[2]);
    }
    render_key_hints(f, state, chunks[3]);

    if state.mode == RunMode::Running {
        render_running_indicator(f, chunks[1]);
    }
}

fn render_status(f: &mut Frame, state: &AppState, area: Rect) {
    let mode = match state.mode {
        RunMode::Editing => "[ready]",
        RunMode::Running => "[running...]",
        RunMode::Aborted => "[aborted]",
        RunMode::Quitting => "[quitting]",
    };
    let session = state.session_id.as_deref().unwrap_or("(no session)");
    let line = RLine::from(vec![
        Span::styled(" nini ", Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" model={} session={} {mode} ", state.model, session)),
        Span::styled(
            format!("tokens: in={} out={} ", state.tokens.input, state.tokens.output),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!("{} ", state.status), Style::default().fg(Color::Yellow)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_transcript(f: &mut Frame, state: &AppState, area: Rect) {
    let items: Vec<ListItem> = state
        .transcript
        .iter()
        .map(|line| match line {
            TranscriptLine::User(text) => ListItem::new(RLine::from(vec![
                Span::styled("> ", Style::default().fg(Color::Green)),
                Span::raw(text.as_str()),
            ])),
            TranscriptLine::AssistantText(text) => {
                ListItem::new(RLine::from(Span::raw(text.as_str())))
            }
            TranscriptLine::ToolCall { name, args } => ListItem::new(RLine::from(vec![
                Span::styled("[tool call] ", Style::default().fg(Color::Magenta)),
                Span::styled(name.as_str(), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {args}")),
            ])),
            TranscriptLine::ToolResult { ok, content } => ListItem::new(RLine::from(vec![
                Span::styled(
                    if *ok { "[tool result] " } else { "[tool error] " },
                    Style::default().fg(if *ok { Color::Cyan } else { Color::Red }),
                ),
                Span::raw(content.as_str()),
            ])),
            TranscriptLine::Divider => {
                let div: String = "─".repeat(60);
                ListItem::new(RLine::from(Span::styled(
                    div,
                    Style::default().fg(Color::DarkGray),
                )))
            }
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    f.render_widget(list, area);
}

fn render_prompt(f: &mut Frame, state: &AppState, area: Rect) {
    let prompt_symbol = if state.mode == RunMode::Running { "⏵" } else { "❯" };
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
                Span::styled(format!("{prefix} "), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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

fn render_key_hints(f: &mut Frame, _state: &AppState, area: Rect) {
    let hints = RLine::from(vec![
        Span::styled(" F1 ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("help "),
        Span::styled(" Ctrl+C ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("quit "),
        Span::styled(" Ctrl+D ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("exit "),
        Span::styled(" Enter ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("send "),
        Span::styled(" Ctrl+L ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("model "),
        Span::styled(" ↑↓ ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw("history"),
    ]);
    f.render_widget(Paragraph::new(hints), area);
}

/// Render the slash-command completion popup above the prompt.
fn render_completion_popup(f: &mut Frame, state: &AppState, area: Rect) {
    let Some(popup) = &state.completion else { return };
    if popup.items.is_empty() { return; }

    let lines: Vec<RLine> = popup
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let is_selected = i == popup.selected;
            let name_style = if is_selected {
                Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            };
            let mut spans = vec![Span::styled(format!("/{}", item.name), name_style)];
            if let Some(hint) = &item.argument_hint {
                spans.push(Span::styled(
                    format!(" {hint}"),
                    Style::default().fg(if is_selected { Color::Black } else { Color::DarkGray }),
                ));
            }
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                item.description.as_str(),
                Style::default().fg(if is_selected { Color::Black } else { Color::Gray }),
            ));
            RLine::from(spans)
        })
        .collect();
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" commands (up/down select, Tab/Enter accept, Esc cancel) ")
                .border_style(Style::default().fg(Color::Cyan)),
        );
    f.render_widget(para, area);
}

fn render_running_indicator(f: &mut Frame, area: Rect) {
    // Subtle visual hint: a thin spinner row at the top-right of transcript.
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (chrono::Utc::now().timestamp_millis() / 80) as usize % spinner.len();
    let indicator = RLine::from(Span::styled(spinner[i], Style::default().fg(Color::Yellow)));
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