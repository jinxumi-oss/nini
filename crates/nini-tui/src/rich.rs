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
use crate::width::{display_width, floor_char_boundary};
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
/// v0.8.3: Pi parity — wrap each line with `bg("userMessageBg")` so the
/// user turn stands out from assistant output. `> ` prefix is rendered
/// in `userMessageText` color, body text in `userMessageText` too.
pub fn render_user_message(text: &str, theme: &Theme) -> Vec<RLine<'static>> {
    let bg = theme.bg_style("userMessageBg");
    let fg = theme.color("userMessageText");
    let make_prefix = || Span::styled("> ", bg.fg(fg).add_modifier(ratatui::style::Modifier::BOLD));
    let make_body = |line: &str| Span::styled(line.to_string(), bg.fg(fg));
    // Break on newline so multi-line user input renders as multi-line.
    let mut out: Vec<RLine<'static>> = Vec::new();
    let mut first = true;
    for line in text.split('\n') {
        if first {
            out.push(RLine::from(vec![make_prefix(), make_body(line)]));
            first = false;
        } else {
            out.push(RLine::from(make_body(&format!("  {line}"))));
        }
    }
    if out.is_empty() {
        out.push(RLine::from(vec![make_prefix(), Span::raw(String::new())]));
    }
    out
}

/// Render a tool-call announcement: `[tool call] <name>(<args>)`.
///
/// v0.8.3: Wrap the call row with `bg("toolPendingBg")` so it visually
/// pops as a pending operation (Pi's `ToolExecutionComponent` pattern).
/// Title color uses `toolTitle` (emphasized); args in `toolOutput`.
pub fn render_tool_call(name: &str, args: &str, theme: &Theme) -> Vec<RLine<'static>> {
    // v0.8.4 (bugfix): byte-slicing at a hard index panics on
    // multi-byte UTF-8 (CJK characters are 3 bytes each). Use a
    // hand-rolled `floor_char_boundary` to truncate at the
    // largest byte index ≤ 120 that is a valid character
    // boundary. We avoid `str::floor_char_boundary` (stable
    // 1.91) because nini's MSRV is 1.85. Without this guard, a
    // tool whose `args` happen to land mid-character at byte 120
    // (very common with Chinese / Japanese / Korean text)
    // panicked the entire TUI with
    // `end byte index 120 is not a char boundary; it is inside
    // '文'`.
    let args_preview = if args.len() > 120 {
        let end = floor_char_boundary(args, 120);
        format!("{}…", &args[..end])
    } else {
        args.to_string()
    };
    let bg = theme.bg_style("toolPendingBg");
    vec![RLine::from(vec![
        Span::styled(
            "▸ ".to_string(),
            bg.fg(theme.color("toolTitle")),
        ),
        Span::styled(
            name.to_string(),
            bg.fg(theme.color("toolTitle"))
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::styled(
            format!(" {args_preview}"),
            bg.fg(theme.color("toolOutput")),
        ),
    ])]
}

/// Render a tool-result block.
///
/// Content is ANSI-stripped, hyperlinked, then truncated to
/// `TOOL_RESULT_PREVIEW_MAX_BYTES`. Pi's `ToolExecutionComponent`
/// does exactly this for tool outputs.
/// Render a unified diff string (output of `nini_tools::diff::render_unified`)
/// with +/- coloring. Lines starting with `+` are dim-error (additions
/// in red/dim context). Lines starting with `-` are green (removals).
/// Lines starting with ` ` are dim (context). Lines starting with `…`
/// indicate a hunk separator.
///
/// Pi's `ToolExecutionComponent` does this for file-edit tool calls.
pub fn render_diff(diff: &str, theme: &Theme) -> Vec<RLine<'static>> {
    let mut out: Vec<RLine<'static>> = Vec::new();
    for line in diff.lines() {
        // Pick the leading sign char (handle multi-byte UTF-8 like `…`).
        let first_char = line.chars().next().unwrap_or(' ');
        let rest = line[first_char.len_utf8()..].to_string();
        let sign = first_char;
        // v0.8.3: Use Pi-equivalent dedicated diff color slots
        // (toolDiffAdded / toolDiffRemoved / toolDiffContext) instead of
        // generic success/error/muted. This lets users tune diff colors
        // independently of general status colors.
        let style = match sign {
            '+' => theme
                .fg_style("toolDiffAdded")
                .add_modifier(ratatui::style::Modifier::BOLD),
            '-' => theme.fg_style("toolDiffRemoved"),
            ' ' | '…' => theme.fg_style("toolDiffContext"),
            _ => theme.fg_style("text"),
        };
        // Preserve the leading sign character so it lines up visually.
        out.push(RLine::from(Span::styled(
            line.to_string(),
            style,
        )));
        // Suppress unused-variable warning on `rest` (kept for clarity
        // / future per-token coloring).
        let _ = rest;
    }
    out
}

pub fn render_tool_result(
    ok: bool,
    content: &str,
    duration_ms: Option<u64>,
    theme: &Theme,
    max_width: Option<usize>,
) -> Vec<RLine<'static>> {
    // v0.8.3: Pi parity — tool result gets a background-color box.
    // Success → bg(toolSuccessBg), Error → bg(toolErrorBg).
    let bg_slot = if ok { "toolSuccessBg" } else { "toolErrorBg" };
    let bg = theme.bg_style(bg_slot);
    let fg_title = theme.color("toolTitle");
    let fg_output = theme.color("toolOutput");
    // v0.8.4 (ux-001): replace the bracketed labels with icon glyphs so
        // the tool boundary reads instantly without parsing ASCII. The
        // triangle / check / cross are standard in IDEs and are also
        // what most TUI dashboards (lazystart, github-cli) use.
        let label = if ok { "✓ " } else { "✗ " };

    // Strip ANSI + auto-link + truncate. For multi-line content, cap
    // at a small number of lines.
    let clean = crate::ansi::strip_ansi(content);
    let linked = crate::hyperlink::auto_link(&clean);
    let lines: Vec<&str> = linked.lines().collect();
    let mut out: Vec<RLine<'static>> = Vec::new();
    // First line: prefix label + optional duration pill (Pi-style).
    let mut first_spans: Vec<Span<'static>> = vec![Span::styled(
        label.to_string(),
        bg.fg(fg_title).add_modifier(ratatui::style::Modifier::BOLD),
    )];
    if let Some(ms) = duration_ms {
        let label = if ms >= 1000 {
            format!("Took {:.2}s ", ms as f64 / 1000.0)
        } else {
            format!("Took {ms}ms ")
        };
        first_spans.push(Span::styled(
            label,
            bg.fg(theme.color("muted")),
        ));
    }
    out.push(RLine::from(first_spans));

    // If the content references a pasted image path, surface it
    // prominently.
    if let Some(line) = lines.first() {
        if line.contains("[pasted image:") {
            out.push(RLine::from(Span::styled(
                format!("  {line}"),
                bg.fg(theme.color("accent")),
            )));
            return out;
        }
    }

// v0.8.7 (ux-002): when caller passes `max_width`, budget
    // per-line to `max_width - 2` cells (subtracting the 2-space
    // indent) so the `…` indicator stays visible. Otherwise fall
    // back to the legacy 200-byte budget.
    //
    // v0.8.7 also uses display-cell width instead of byte length for
    // the truncation decision. Previously a 200-byte line of CJK
    // content (~67 cells) would get cut to the byte-budget, wasting
    // visible width. With cell-aware truncation we pack the full
    // visible area. The `floor_char_boundary` guard from v0.8.4 is
    // preserved for the resulting byte slice.
    let max_lines: usize = 8;
    // Budget accounts for the 2-space indent + 1 cell for the `…`
    // indicator so the indicator stays inside the visible area when
    // the terminal width equals the budget exactly.
    let max_chars_per_line: usize = match max_width {
        Some(n) if n >= 5 => n.saturating_sub(2 + 1),
        _ => 199,
    };
    for line in lines.iter().take(max_lines) {
        let truncated: String = if display_width(line) > max_chars_per_line {
            // Walk chars accumulating display width, slice at the
            // boundary that fits in max_chars_per_line cells.
            let mut consumed = 0usize;
            let mut cut_byte = line.len();
            for (byte_idx, c) in line.char_indices() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if consumed + cw > max_chars_per_line {
                    cut_byte = byte_idx;
                    break;
                }
                consumed += cw;
            }
            let safe = floor_char_boundary(line, cut_byte);
            format!("{}…", &line[..safe])
        } else {
            line.to_string()
        };
        out.push(RLine::from(Span::styled(
            format!("  {truncated}"),
            bg.fg(fg_output),
        )));
    }
    // v0.8.7 (ux-002): combined truncation indicator. Old behavior
    // produced two rows ("more lines" then "more bytes") which was
    // visually noisy. Single combined row keeps the tail tidy.
    let hidden_lines = lines.len().saturating_sub(max_lines);
    let hidden_bytes = linked.len().saturating_sub(TOOL_RESULT_PREVIEW_MAX_BYTES);
    if hidden_lines > 0 || hidden_bytes > 0 {
        let mut parts: Vec<String> = Vec::new();
        if hidden_lines > 0 {
            parts.push(format!("{hidden_lines} more lines"));
        }
        if hidden_bytes > 0 {
            if hidden_bytes >= 1024 {
                parts.push(format!("{} KB hidden", hidden_bytes / 1024));
            } else {
                parts.push(format!("{hidden_bytes} more bytes"));
            }
        }
        out.push(RLine::from(Span::styled(
            format!("  …({})", parts.join(", ")),
            bg.fg(theme.color("dim")),
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
    stderr: &str,
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
    // v0.8.4 (bugfix): byte-slicing `&linked[..BASH_PREVIEW_MAX_BYTES]`
    // panics on multi-byte UTF-8 (CJK is 3 bytes). Use
    // `floor_char_boundary` to truncate at the largest safe
    // boundary. bash output containing Chinese / Japanese / Korean
    // longer than ~666 chars (666 × 3 = 1998 bytes) used to crash
    // the TUI when `BASH_PREVIEW_MAX_BYTES` (2000) landed
    // mid-character.
    let truncated = if bytes_truncated {
        let end = floor_char_boundary(&linked, BASH_PREVIEW_MAX_BYTES);
        format!(
            "{}…\n[…{} bytes total, showing first {}]",
            &linked[..end],
            linked.len(),
            end
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

    // Render stderr in error color, separated by a "stderr:" header so
    // users can distinguish from stdout. Only emitted when non-empty.
    let stderr_trim = stderr.trim();
    if !stderr_trim.is_empty() {
        let stderr_lines: Vec<&str> = stderr_trim.lines().collect();
        out.push(RLine::from(Span::styled(
            "  stderr:".to_string(),
            theme.fg_style("error").add_modifier(ratatui::style::Modifier::BOLD),
        )));
        for line in stderr_lines.iter().take(max_lines) {
            out.push(RLine::from(Span::styled(
                format!("  {line}"),
                theme.fg_style("error"),
            )));
        }
        if stderr_lines.len() > max_lines {
            out.push(RLine::from(Span::styled(
                format!("  …({} more stderr lines)", stderr_lines.len() - max_lines),
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
        let lines = render_tool_result(true, "ok output", None, &theme(), None);
        assert!(!lines.is_empty());
        assert!(lines[0].spans.iter().any(|s| s.content.contains("✓ ")));
    }

    #[test]
    fn render_tool_result_error_label() {
        let lines = render_tool_result(false, "fail", None, &theme(), None);
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content.contains("✗ ")));
    }

    #[test]
    fn render_tool_result_shows_duration_pill_when_set() {
        // v0.8: Pi-style "Took 1.23s" / "Took 850ms" pill on the
        // first line of the tool result.
        let lines = render_tool_result(true, "ok", Some(1230), &theme(), None);
        assert!(
            lines[0].spans.iter().any(|s| s.content.contains("Took 1.23s")),
            "expected 'Took 1.23s' pill, got: {:?}",
            lines[0].spans,
        );
        let lines_ms = render_tool_result(true, "ok", Some(850), &theme(), None);
        assert!(
            lines_ms[0].spans.iter().any(|s| s.content.contains("Took 850ms")),
            "expected 'Took 850ms' pill, got: {:?}",
            lines_ms[0].spans,
        );
    }

    #[test]
    fn render_tool_result_omits_duration_pill_when_none() {
        // v0.8: when the tool doesn't measure its own duration
        // (duration_ms = None), no "Took ..." pill appears.
        let lines = render_tool_result(true, "ok", None, &theme(), None);
        assert!(
            !lines[0].spans.iter().any(|s| s.content.starts_with("Took ")),
            "unexpected duration pill, got: {:?}",
            lines[0].spans,
        );
    }

    #[test]
    fn render_tool_result_image_path_promoted() {
        let content = "[pasted image: /tmp/abc.png]\nsome more text";
        let lines = render_tool_result(true, content, None, &theme(), None);
        // Image line should appear prominently (accent color).
        assert!(lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content.contains("pasted image"))
        }));
    }

    #[test]
    fn render_tool_result_truncates_long_content() {
        let content = "line\n".repeat(100);
        let lines = render_tool_result(true, &content, None, &theme(), None);
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
        let lines =
            render_bash_execution("ls", "file1\nfile2", "", true, Some(0), 42, &theme());
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
        let lines =
            render_bash_execution("cat big", &out, "", true, Some(0), 100, &theme());
        assert!(lines.len() <= 18); // banner + 16 max + truncation marker
    }

    #[test]
    fn render_bash_execution_strips_ansi() {
        let lines = render_bash_execution(
            "echo",
            "\x1b[32mgreen\x1b[0m text",
            "",
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
    fn render_bash_execution_with_stderr() {
        let lines = render_bash_execution(
            "bash",
            "ok",
            "warning: something",
            true,
            Some(0),
            1,
            &theme(),
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("ok"), "stdout missing");
        assert!(joined.contains("warning: something"), "stderr missing");
        assert!(joined.contains("stderr"), "stderr header missing");
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

#[cfg(test)]
mod diff_render_tests {
    use super::*;

    #[test]
    fn render_diff_colors_added_and_removed() {
        let d = " line1\n-old\n+new\n line2";
        let lines = render_diff(d, &Theme::default());
        assert_eq!(lines.len(), 4);
        // Each line preserves its prefix; colors come from the theme.
        // We can at least assert the prefix text is unchanged.
        for (i, line) in lines.iter().enumerate() {
            let joined: String = line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(joined, d.lines().nth(i).unwrap(), "line {i} mismatch");
        }
    }

    #[test]
    fn render_diff_handles_ellipsis_separator() {
        let d = " a\n…\n b";
        let lines = render_diff(d, &Theme::default());
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn render_diff_empty() {
        let lines = render_diff("", &Theme::default());
        assert!(lines.is_empty());
    }

    #[test]
    fn render_diff_uses_dedicated_diff_color_slots() {
        // v0.8.3: Pi parity — diff + uses toolDiffAdded (not success).
        let d = "+added\n-removed\n context\n";
        let lines = render_diff(d, &Theme::dark());
        let dark_added = Theme::dark().color("toolDiffAdded");
        let dark_removed = Theme::dark().color("toolDiffRemoved");
        let dark_context = Theme::dark().color("toolDiffContext");
        assert_eq!(lines[0].spans[0].style.fg, Some(dark_added));
        assert_eq!(lines[1].spans[0].style.fg, Some(dark_removed));
        assert_eq!(lines[2].spans[0].style.fg, Some(dark_context));
    }

    #[test]
    fn render_tool_call_does_not_panic_on_multibyte_args() {
        // v0.8.4 regression: byte-slicing `&args[..120]` panicked
        // when byte 120 fell inside a 3-byte CJK character. The
        // TUI would crash with `end byte index 120 is not a char
        // boundary; it is inside '文'` on any tool whose args
        // exceeded 120 bytes of multi-byte UTF-8 text.
        // Build a string where byte 120 is mid-character.
        let args = "文".repeat(50); // 50 * 3 = 150 bytes; byte 120 inside char #40 (bytes 119..122)
        let lines = render_tool_call("bash", &args, &Theme::default());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // Should not panic and should include an ellipsis to signal truncation.
        assert!(joined.contains('…'), "missing ellipsis: {joined:?}");
    }
}


/// Render a dim "[Ctrl+O to expand]" hint shown under a collapsed
/// tool-output / tool-call / bash-execution block.
pub fn render_collapsed_hint(theme: &Theme) -> RLine<'static> {
    RLine::from(Span::styled(
        "    ▸ Ctrl+O to expand",
        theme.fg_style("dim"),
    ))
}
