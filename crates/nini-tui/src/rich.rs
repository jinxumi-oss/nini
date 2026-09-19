//! Rich transcript rendering: markdown + ANSI + hyperlink + image-paste.
//!
//! Mirrors Pi's `AssistantMessageComponent` family:
//! - Markdown headers/lists/blockquote/code/links via `markdown.rs`
//! - Inline ANSI colors via `ansi.rs::strip_ansi` (Pi strips; we strip
//!   and emit theme-styled spans)
//! - OSC 8 hyperlinks via `hyperlink.rs`
//! - Image-paste path detection via `image_paste.rs::describe_path`
//!
//! All entry points take `&Theme` so colors follow the active theme.
//! All return `Vec<RLine<'static>>` so renderers can plug them into
//! ratatui widgets directly.

use crate::markdown::render_markdown;
use crate::theme::Theme;
use ratatui::text::{Line as RLine, Span};

/// Maximum bytes to keep from a single bash-output preview.
pub const BASH_PREVIEW_MAX_BYTES: usize = 2_000;
/// Maximum lines to keep from a single bash-output preview.
pub const BASH_PREVIEW_MAX_LINES: usize = 16;
/// Maximum bytes to keep from a single tool result before truncation.
pub const TOOL_RESULT_PREVIEW_MAX_BYTES: usize = 2_000;

/// Render an assistant message: Markdown → themed ratatui lines.
///
/// If `text` looks like it contains inline ANSI escapes, they're
/// stripped (Pi behavior — TUI grids can't render 24-bit color
/// reliably across terminals). Markdown is then applied.
pub fn render_assistant_message(text: &str, theme: &Theme) -> Vec<RLine<'static>> {
    if text.is_empty() {
        return vec![RLine::from(Span::raw(""))];
    }
    // Strip ANSI escapes — Pi does the same before markdown rendering.
    let clean = crate::ansi::strip_ansi(text);
    // Auto-link bare URLs into OSC 8 hyperlinks so terminals like
    // iTerm2 / Kitty / WezTerm render them as clickable links.
    let linked = crate::hyperlink::auto_link(&clean);
    render_markdown(&linked, theme)
}

/// Render a user message: `> ` prefix + plain text.
///
/// Pure ASCII: we deliberately don't auto-link or markdownify user
/// input — the prompt editor shows literal text, and markdownifying
/// user messages would mangle pasted code.
pub fn render_user_message(text: &str, theme: &Theme) -> Vec<RLine<'static>> {
    let make_prefix = || Span::styled("> ", theme.fg_style("success"));
    // Break on newline so multi-line user input renders as multi-line.
    let mut out: Vec<RLine<'static>> = Vec::new();
    let mut first = true;
    for line in text.split('\n') {
        if first {
            out.push(RLine::from(vec![make_prefix(), Span::raw(line.to_string())]));
            first = false;
        } else {
            out.push(RLine::from(Span::raw(format!("  {line}"))));
        }
    }
    if out.is_empty() {
        out.push(RLine::from(vec![make_prefix(), Span::raw(String::new())]));
    }
    out
}

/// Render a tool-call announcement: `[tool call] <name>(<args>)`.
///
/// `args` is a JSON string truncated for display. The call name is
/// styled with `toolPendingBg` to mirror Pi's pending color.
pub fn render_tool_call(name: &str, args: &str, theme: &Theme) -> Vec<RLine<'static>> {
    let args_preview = if args.len() > 120 {
        format!("{}…", &args[..120])
    } else {
        args.to_string()
    };
    vec![RLine::from(vec![
        Span::styled("[tool call] ", theme.fg_style("toolPendingBg")),
        Span::styled(
            name.to_string(),
            theme
                .fg_style("toolPendingBg")
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::raw(format!(" {args_preview}")),
    ])]
}

/// Render a tool-result block.
///
/// Content is ANSI-stripped, hyperlinked, then truncated to
/// `TOOL_RESULT_PREVIEW_MAX_BYTES`. Pi's `ToolExecutionComponent`
/// does exactly this for tool outputs.
pub fn render_tool_result(ok: bool, content: &str, theme: &Theme) -> Vec<RLine<'static>> {
    let prefix_color = if ok { "toolSuccessBg" } else { "error" };
    let label = if ok { "[tool result] " } else { "[tool error] " };

    // Strip ANSI + auto-link + truncate. For multi-line content, cap
    // at a small number of lines.
    let clean = crate::ansi::strip_ansi(content);
    let linked = crate::hyperlink::auto_link(&clean);
    let lines: Vec<&str> = linked.lines().collect();
    let mut out: Vec<RLine<'static>> = Vec::new();
    out.push(RLine::from(Span::styled(label.to_string(), theme.fg_style(prefix_color))));

    // If the content references a pasted image path, surface it
    // prominently.
    if let Some(line) = lines.first() {
        if line.contains("[pasted image:") {
            out.push(RLine::from(Span::styled(
                format!("  {line}"),
                theme.fg_style("accent"),
            )));
            return out;
        }
    }

    let max_lines = 8;
    let max_chars_per_line = 200;
    for (i, line) in lines.iter().take(max_lines).enumerate() {
        let truncated: String = if line.len() > max_chars_per_line {
            format!("{}…", &line[..max_chars_per_line])
        } else {
            line.to_string()
        };
        out.push(RLine::from(Span::styled(
            format!("  {truncated}"),
            theme.fg_style("muted"),
        )));
        if i + 1 < lines.len() && i + 1 == max_lines && lines.len() > max_lines {
            out.push(RLine::from(Span::styled(
                format!("  …({} more lines)", lines.len() - max_lines),
                theme.fg_style("dim"),
            )));
        }
    }
    if linked.len() > TOOL_RESULT_PREVIEW_MAX_BYTES {
        out.push(RLine::from(Span::styled(
            format!(
                "  …(truncated, {} more bytes)",
                linked.len() - TOOL_RESULT_PREVIEW_MAX_BYTES
            ),
            theme.fg_style("dim"),
        )));
    }
    out
}

/// Render a bash-execution block: `! <cmd> [<exit>] <ms>` then output.
///
/// Mirrors Pi's `BashExecutionComponent` — banner with status, then
/// streaming output preview capped to 16 lines / 2KB.
pub fn render_bash_execution(
    cmd: &str,
    output: &str,
    ok: bool,
    exit_code: Option<i32>,
    duration_ms: u64,
    theme: &Theme,
) -> Vec<RLine<'static>> {
    let header_color = if ok { "success" } else { "error" };
    let exit_str = exit_code
        .map(|c| format!("[exit {c}]"))
        .unwrap_or_else(|| "[no exit]".to_string());
    let header = format!("$ {cmd} {exit_str} {duration_ms}ms");

    let mut out: Vec<RLine<'static>> = vec![RLine::from(vec![
        Span::styled("! ", theme.fg_style("warning").add_modifier(ratatui::style::Modifier::BOLD)),
        Span::styled(header, theme.fg_style(header_color)),
    ])];

    // Strip ANSI from captured output, then cap.
    let clean = crate::ansi::strip_ansi(output);
    let linked = crate::hyperlink::auto_link(&clean);
    let bytes_truncated = linked.len() > BASH_PREVIEW_MAX_BYTES;
    let truncated = if bytes_truncated {
        format!(
            "{}…\n[…{} bytes total, showing first {}]",
            &linked[..BASH_PREVIEW_MAX_BYTES],
            linked.len(),
            BASH_PREVIEW_MAX_BYTES
        )
    } else {
        linked.clone()
    };
    let lines: Vec<&str> = truncated.lines().collect();
    let max_lines = BASH_PREVIEW_MAX_LINES;
    for (i, line) in lines.iter().take(max_lines).enumerate() {
        out.push(RLine::from(Span::styled(
            format!("  {line}"),
            theme.fg_style("muted"),
        )));
        if i + 1 < lines.len() && i + 1 == max_lines && lines.len() > max_lines {
            out.push(RLine::from(Span::styled(
                format!("  …({} more lines)", lines.len() - max_lines),
                theme.fg_style("dim"),
            )));
        }
    }
    out
}

/// Render a divider line: 60 box-drawing chars styled `dim`.
///
/// Matches `render_transcript`'s previous behavior exactly so existing
/// snapshots don't drift.
pub fn render_divider(theme: &Theme) -> RLine<'static> {
    let div: String = "─".repeat(60);
    RLine::from(Span::styled(div, theme.fg_style("dim")))
}

/// Status-indicator spinner: cycles through 10 braille frames.
///
/// Used both for the existing `render_running_indicator` and for the
/// new `render_status` 5-state indicator.
pub fn spinner_frame() -> &'static str {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (chrono::Utc::now().timestamp_millis() / 80) as usize % FRAMES.len();
    FRAMES[i]
}

/// 5-state agent phase indicator. Mirrors Pi's `StatusIndicatorComponent`
/// with Idle / Working / Compacting / Retrying / BranchSummary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPhase {
    Idle,
    Working,
    Compacting,
    Retrying,
    BranchSummary,
}

impl AgentPhase {
    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            AgentPhase::Idle => "idle",
            AgentPhase::Working => "working…",
            AgentPhase::Compacting => "compacting…",
            AgentPhase::Retrying => "retrying…",
            AgentPhase::BranchSummary => "summarizing branch…",
        }
    }
    /// Theme color name for the phase.
    pub fn color_name(&self) -> &'static str {
        match self {
            AgentPhase::Idle => "success",
            AgentPhase::Working => "warning",
            AgentPhase::Compacting => "accent",
            AgentPhase::Retrying => "warning",
            AgentPhase::BranchSummary => "accent",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn render_assistant_message_simple_text() {
        let lines = render_assistant_message("Hello!", &theme());
        assert!(!lines.is_empty());
    }

    #[test]
    fn render_assistant_message_with_markdown() {
        let lines = render_assistant_message("# Heading\n\n- item 1\n- item 2", &theme());
        // Headings + list items render as multiple lines.
        assert!(lines.len() >= 3, "expected >=3 lines, got {}", lines.len());
    }

    #[test]
    fn render_assistant_message_strips_ansi() {
        // CSI escape sequence; should be stripped before markdown render.
        let lines = render_assistant_message("\x1b[31mred text\x1b[0m", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // No raw escape bytes in the output.
        assert!(!joined.contains('\x1b'), "found raw escape: {joined:?}");
    }

    #[test]
    fn render_assistant_message_autolinks_urls() {
        let lines = render_assistant_message("see https://example.com here", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("https://example.com"));
    }

    #[test]
    fn render_user_message_single_line() {
        let lines = render_user_message("hi", &theme());
        assert_eq!(lines.len(), 1);
        // Prefix "> "
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content.contains('>')));
    }

    #[test]
    fn render_user_message_multiline_indents() {
        let lines = render_user_message("a\nb\nc", &theme());
        assert_eq!(lines.len(), 3);
        // Lines after the first are indented with two spaces.
        assert!(lines[1].spans.iter().any(|s| s.content.starts_with("  b")));
    }

    #[test]
    fn render_tool_call_truncates_long_args() {
        let args = "x".repeat(500);
        let lines = render_tool_call("bash", &args, &theme());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // Should truncate with ellipsis.
        assert!(joined.contains('…'), "missing ellipsis: {joined}");
        assert!(joined.len() < 200, "should be truncated");
    }

    #[test]
    fn render_tool_result_success_color() {
        let lines = render_tool_result(true, "ok output", &theme());
        assert!(!lines.is_empty());
        assert!(lines[0].spans.iter().any(|s| s.content.contains("[tool result]")));
    }

    #[test]
    fn render_tool_result_error_label() {
        let lines = render_tool_result(false, "fail", &theme());
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content.contains("[tool error]")));
    }

    #[test]
    fn render_tool_result_image_path_promoted() {
        let content = "[pasted image: /tmp/abc.png]\nsome more text";
        let lines = render_tool_result(true, content, &theme());
        // Image line should appear prominently (accent color).
        assert!(lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content.contains("pasted image"))
        }));
    }

    #[test]
    fn render_tool_result_truncates_long_content() {
        let content = "line\n".repeat(100);
        let lines = render_tool_result(true, &content, &theme());
        // Should not render 100 lines.
        assert!(lines.len() < 15, "got {} lines", lines.len());
        // Should have a "more lines" marker.
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains("more lines") || joined.contains("more bytes"),
            "expected truncation marker in: {joined}"
        );
    }

    #[test]
    fn render_bash_execution_with_status() {
        let lines = render_bash_execution("ls", "file1\nfile2", true, Some(0), 42, &theme());
        // Header + output lines.
        assert!(lines.len() >= 2);
        // First line is the banner.
        let header: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(header.contains("$ ls"));
        assert!(header.contains("[exit 0]"));
        assert!(header.contains("42ms"));
    }

    #[test]
    fn render_bash_execution_truncates_long_output() {
        let mut out = String::new();
        for i in 0..100 {
            out.push_str(&format!("line {i}\n"));
        }
        let lines = render_bash_execution("cat big", &out, true, Some(0), 100, &theme());
        assert!(lines.len() <= 18); // banner + 16 max + truncation marker
    }

    #[test]
    fn render_bash_execution_strips_ansi() {
        let lines = render_bash_execution(
            "echo",
            "\x1b[32mgreen\x1b[0m text",
            true,
            Some(0),
            5,
            &theme(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!joined.contains('\x1b'));
    }

    #[test]
    fn render_divider_is_sixty_dashes() {
        let line = render_divider(&theme());
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(joined.chars().count(), 60);
    }

    #[test]
    fn phase_labels_distinct() {
        let phases = [
            AgentPhase::Idle,
            AgentPhase::Working,
            AgentPhase::Compacting,
            AgentPhase::Retrying,
            AgentPhase::BranchSummary,
        ];
        let mut labels: Vec<&str> = phases.iter().map(|p| p.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 5, "labels should all be unique");
    }

    #[test]
    fn spinner_returns_valid_braille() {
        let s = spinner_frame();
        // Each frame is a single braille char (3 bytes UTF-8).
        assert_eq!(s.chars().count(), 1);
    }
}
