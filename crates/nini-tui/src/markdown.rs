//! Markdown rendering stub — minimal implementation that splits text into
//! lines and renders headings/bullet items with style spans.

use ratatui::text::{Line, Span};

pub fn render_markdown(s: &str, _theme: &crate::theme::Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for raw in s.lines() {
        let line = raw.trim_end();
        if line.is_empty() { continue; }
        if line.starts_with("# ") {
            lines.push(Line::from(vec![Span::raw(line.to_string())]));
        } else if line.starts_with("- ") {
            let item = line.trim_start_matches("- ");
            lines.push(Line::from(vec![
                Span::raw("• ".to_string()),
                Span::raw(item.to_string()),
            ]));
        } else {
            lines.push(Line::from(line.to_string()));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}
