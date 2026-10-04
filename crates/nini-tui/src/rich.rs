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
use crate::width::{display_width, floor_char_boundary, truncate_cells, truncate_line_cells};
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
pub fn render_assistant_message(
    text: &str,
    theme: &Theme,
    max_width: Option<usize>,
) -> Vec<RLine<'static>> {
    if text.is_empty() {
        return vec![RLine::from(Span::raw(""))];
    }
    // Strip ANSI escapes — Pi does the same before markdown rendering.
    let clean = crate::ansi::strip_ansi(text);
    // Auto-link bare URLs into OSC 8 hyperlinks so terminals like
    // iTerm2 / Kitty / WezTerm render them as clickable links.
    let linked = crate::hyperlink::auto_link(&clean);
    let lines = render_markdown(&linked, theme);
    // v0.8.9 (P1-3): clamp to the real terminal width. The markdown
    // renderer has no idea how wide the screen is — it emitted
    // arbitrarily long lines and relied on "ratatui will clip anyway",
    // but the transcript renders through a `List`, and `List` cuts at
    // the screen edge with no marker. Clamping the finished lines
    // covers every markdown construct (headings, code fences, tables,
    // nested lists) without threading a width through markdown.rs.
    match max_width {
        Some(w) if w > 0 => lines
            .into_iter()
            .map(|l| truncate_line_cells(&l, w))
            .collect(),
        _ => lines,
    }
}

/// Render a user message: `> ` prefix + plain text.
///
/// v0.8.3: Pi parity — wrap each line with `bg("userMessageBg")` so the
/// user turn stands out from assistant output. `> ` prefix is rendered
/// in `userMessageText` color, body text in `userMessageText` too.
pub fn render_user_message(
    text: &str,
    theme: &Theme,
    max_width: Option<usize>,
) -> Vec<RLine<'static>> {
    let bg = theme.bg_style("userMessageBg");
    let fg = theme.color("userMessageText");
    let make_prefix = || Span::styled("> ", bg.fg(fg).add_modifier(ratatui::style::Modifier::BOLD));
    let make_body = |line: &str| Span::styled(line.to_string(), bg.fg(fg));
    // The `> ` prefix and the two-space continuation indent both cost
    // cells, so the body budget is narrower than the terminal.
    let body_budget = max_width.map(|w| w.saturating_sub(2));
    // Break on newline so multi-line user input renders as multi-line.
    let mut out: Vec<RLine<'static>> = Vec::new();
    let mut first = true;
    for line in text.split('\n') {
        let body = match body_budget {
            Some(b) if b > 0 => truncate_cells(line, b),
            _ => line.to_string(),
        };
        if first {
            out.push(RLine::from(vec![make_prefix(), make_body(&body)]));
            first = false;
        } else {
            out.push(RLine::from(make_body(&format!("  {body}"))));
        }
    }
    if out.is_empty() {
        out.push(RLine::from(vec![make_prefix(), Span::raw(String::new())]));
    }
    out
}

/// Cell budget for `args` when the caller has no terminal width (unit
/// tests, `inspect_*` examples). Sits where the old hard-coded 120
/// *byte* budget landed for ASCII so existing snapshots barely move —
/// but it is now the right unit, so CJK no longer overflows.
pub const TOOL_CALL_ARGS_FALLBACK_CELLS: usize = 100;

/// Render a tool-call announcement: `[tool call] <name>(<args>)`.
///
/// v0.8.3: Wrap the call row with `bg("toolPendingBg")` so it visually
/// pops as a pending operation (Pi's `ToolExecutionComponent` pattern).
/// Title color uses `toolTitle` (emphasized); args in `toolOutput`.
///
/// v0.8.9 (P1-3): the old budget was a flat 120 **bytes** of `args`,
/// which is wrong twice over. 120 bytes of ASCII is 120 cells — on an
/// 80-column terminal the `…` we appended landed at column 120, i.e.
/// *off-screen*, so the truncation marker was itself truncated away and
/// the user saw a hard cut with no signal. And 120 bytes of CJK is 40
/// chars = 80 cells, which overflows anything narrower. The unit test
/// that "covered" this asserted `joined.contains('…')` on the returned
/// string, so it stayed green while the screen showed no ellipsis at
/// all. The budget now derives from the actual width minus `▸ ` and
/// the tool name, so the name is always fully visible and the `…` is
/// always the last cell on screen.
pub fn render_tool_call(
    name: &str,
    args: &str,
    theme: &Theme,
    max_width: Option<usize>,
) -> Vec<RLine<'static>> {
    let bg = theme.bg_style("toolPendingBg");
    // "▸ " + name + " " — cells consumed before args start.
    let fixed: usize = 2 + display_width(name) + 1;
    let args_budget: usize = match max_width {
        Some(w) if w > fixed => w - fixed,
        // No room for args at all (very narrow terminal, or a
        // pathologically long tool name): keep one cell for the marker
        // so the line still reads as "there is more here".
        Some(_) => 1,
        None => TOOL_CALL_ARGS_FALLBACK_CELLS,
    };
    let args_preview = truncate_cells(args, args_budget);
    vec![RLine::from(vec![
        Span::styled("▸ ".to_string(), bg.fg(theme.color("toolTitle"))),
        Span::styled(
            name.to_string(),
            bg.fg(theme.color("toolTitle"))
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::styled(format!(" {args_preview}"), bg.fg(theme.color("toolOutput"))),
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
        out.push(RLine::from(Span::styled(line.to_string(), style)));
        // Suppress unused-variable warning on `rest` (kept for clarity
        // / future per-token coloring).
        let _ = rest;
    }
    out
}

pub fn render_tool_result(
    name: &str,
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
    // v0.8.8 (tool-identity): show the tool name in the result
    // header so the user can tell which of N parallel results this
    // belongs to. Renders as `✓ bash` or `✗ read` (Pi parity).
    let label = if ok {
        format!("✓ {} ", name)
    } else {
        format!("✗ {} ", name)
    };

    // Strip ANSI + auto-link + truncate. For multi-line content, cap
    // at a small number of lines.
    let clean = crate::ansi::strip_ansi(content);
    let linked = crate::hyperlink::auto_link(&clean);
    let lines: Vec<&str> = linked.lines().collect();
    let mut out: Vec<RLine<'static>> = Vec::new();
    // First line: `✓ <name>` (bold) + optional duration pill
    // `Took N.Ns` (Pi-style). v0.8.8 changed label from `✓ ` to
    // `✓ <name> ` so the user knows which tool produced this result.
    let mut first_spans: Vec<Span<'static>> = vec![Span::styled(
        label,
        bg.fg(fg_title).add_modifier(ratatui::style::Modifier::BOLD),
    )];
    if let Some(ms) = duration_ms {
        let dur_label = if ms >= 1000 {
            format!("Took {:.2}s ", ms as f64 / 1000.0)
        } else {
            format!("Took {ms}ms ")
        };
        first_spans.push(Span::styled(dur_label, bg.fg(theme.color("muted"))));
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
    max_width: Option<usize>,
) -> Vec<RLine<'static>> {
    let header_color = if ok { "success" } else { "error" };
    let exit_str = exit_code
        .map(|c| format!("[exit {c}]"))
        .unwrap_or_else(|| "[no exit]".to_string());
    let header = format!("$ {cmd} {exit_str} {duration_ms}ms");

    // v0.8.9 (P1-3): clamp to the terminal width. The 2 KB / 16-line
    // budgets below bound *volume*, not *width* — a single 1500-byte
    // line was rendered whole and then silently cut at the screen edge
    // by the `List`, with nothing to tell the user it had been clipped.
    let header_text = match max_width {
        Some(w) if w > 2 => truncate_cells(&header, w.saturating_sub(2)),
        _ => header,
    };
    // Body lines carry a 2-space indent.
    let body_budget: Option<usize> = max_width.map(|w| w.saturating_sub(2));
    let clamp_body = |line: &str| -> String {
        match body_budget {
            Some(b) if b > 0 => truncate_cells(line, b),
            _ => line.to_string(),
        }
    };

    let mut out: Vec<RLine<'static>> = vec![RLine::from(vec![
        Span::styled(
            "! ",
            theme
                .fg_style("warning")
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::styled(header_text, theme.fg_style(header_color)),
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
            format!("  {}", clamp_body(line)),
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
            theme
                .fg_style("error")
                .add_modifier(ratatui::style::Modifier::BOLD),
        )));
        for line in stderr_lines.iter().take(max_lines) {
            out.push(RLine::from(Span::styled(
                format!("  {}", clamp_body(line)),
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
        let lines = render_assistant_message("Hello!", &theme(), None);
        assert!(!lines.is_empty());
    }

    #[test]
    fn render_assistant_message_with_markdown() {
        let lines = render_assistant_message("# Heading\n\n- item 1\n- item 2", &theme(), None);
        // Headings + list items render as multiple lines.
        assert!(lines.len() >= 3, "expected >=3 lines, got {}", lines.len());
    }

    #[test]
    fn render_assistant_message_strips_ansi() {
        // CSI escape sequence; should be stripped before markdown render.
        let lines = render_assistant_message("\x1b[31mred text\x1b[0m", &theme(), None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // No raw escape bytes in the output.
        assert!(!joined.contains('\x1b'), "found raw escape: {joined:?}");
    }

    #[test]
    fn render_assistant_message_autolinks_urls() {
        let lines = render_assistant_message("see https://example.com here", &theme(), None);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("https://example.com"));
    }

    #[test]
    fn render_user_message_single_line() {
        let lines = render_user_message("hi", &theme(), None);
        assert_eq!(lines.len(), 1);
        // Prefix "> "
        assert!(lines[0].spans.iter().any(|s| s.content.contains('>')));
    }

    #[test]
    fn render_user_message_multiline_indents() {
        let lines = render_user_message("a\nb\nc", &theme(), None);
        assert_eq!(lines.len(), 3);
        // Lines after the first are indented with two spaces.
        assert!(lines[1].spans.iter().any(|s| s.content.starts_with("  b")));
    }

    #[test]
    fn render_tool_call_truncates_long_args() {
        let args = "x".repeat(500);
        let lines = render_tool_call("bash", &args, &theme(), None);
        let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        // Should truncate with ellipsis.
        assert!(joined.contains('…'), "missing ellipsis: {joined}");
        assert!(joined.len() < 200, "should be truncated");
    }

    #[test]
    fn render_tool_result_success_color() {
        let lines = render_tool_result("bash", true, "ok output", None, &theme(), None);
        assert!(!lines.is_empty());
        assert!(lines[0].spans.iter().any(|s| s.content.contains("✓ ")));
    }

    #[test]
    fn render_tool_result_error_label() {
        let lines = render_tool_result("bash", false, "fail", None, &theme(), None);
        assert!(lines[0].spans.iter().any(|s| s.content.contains("✗ ")));
    }

    #[test]
    fn render_tool_result_shows_duration_pill_when_set() {
        // v0.8: Pi-style "Took 1.23s" / "Took 850ms" pill on the
        // first line of the tool result.
        let lines = render_tool_result("bash", true, "ok", Some(1230), &theme(), None);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content.contains("Took 1.23s")),
            "expected 'Took 1.23s' pill, got: {:?}",
            lines[0].spans,
        );
        let lines_ms = render_tool_result("bash", true, "ok", Some(850), &theme(), None);
        assert!(
            lines_ms[0]
                .spans
                .iter()
                .any(|s| s.content.contains("Took 850ms")),
            "expected 'Took 850ms' pill, got: {:?}",
            lines_ms[0].spans,
        );
    }

    #[test]
    fn render_tool_result_omits_duration_pill_when_none() {
        // v0.8: when the tool doesn't measure its own duration
        // (duration_ms = None), no "Took ..." pill appears.
        let lines = render_tool_result("bash", true, "ok", None, &theme(), None);
        assert!(
            !lines[0]
                .spans
                .iter()
                .any(|s| s.content.starts_with("Took ")),
            "unexpected duration pill, got: {:?}",
            lines[0].spans,
        );
    }

    #[test]
    fn render_tool_result_image_path_promoted() {
        let content = "[pasted image: /tmp/abc.png]\nsome more text";
        let lines = render_tool_result("bash", true, content, None, &theme(), None);
        // Image line should appear prominently (accent color).
        assert!(lines
            .iter()
            .any(|l| { l.spans.iter().any(|s| s.content.contains("pasted image")) }));
    }

    #[test]
    fn render_tool_result_truncates_long_content() {
        let content = "line\n".repeat(100);
        let lines = render_tool_result("bash", true, &content, None, &theme(), None);
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
            render_bash_execution("ls", "file1\nfile2", "", true, Some(0), 42, &theme(), None);
        // Header + output lines.
        assert!(lines.len() >= 2);
        // First line is the banner.
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
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
        let lines = render_bash_execution("cat big", &out, "", true, Some(0), 100, &theme(), None);
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
            None,
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
            None,
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
        let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
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
            let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
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
        let lines = render_tool_call("bash", &args, &Theme::default(), Some(40));
        let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        // Should not panic and should include an ellipsis to signal truncation.
        assert!(joined.contains('…'), "missing ellipsis: {joined:?}");
        // v0.8.9 (P1-3): the budget is a cell count derived from the
        // terminal width, not a flat byte count. 50 CJK chars is 100
        // cells, so a 40-cell line must cut and fill exactly 40.
        assert_eq!(
            display_width(&joined),
            40,
            "line must fit the terminal exactly: {joined:?}"
        );
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

/// v0.8.9 (P1-3) — width-budget unit tests.
///
/// These pin the *cell arithmetic* of each renderer. They are necessary
/// but not sufficient: the old `render_tool_call_truncates_long_args`
/// asserted `contains('…')` on the returned string and stayed green
/// while the screen showed no ellipsis at all. The frame-level
/// assertions that actually prove the user sees the marker live in
/// `tests/transcript_width_e2e.rs`.
#[cfg(test)]
mod width_tests {
    use super::*;
    use crate::width::display_width;

    fn theme() -> Theme {
        Theme::default()
    }

    fn joined(lines: &[RLine<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect()
    }

    fn line_width(l: &RLine<'static>) -> usize {
        l.spans
            .iter()
            .map(|s| display_width(s.content.as_ref()))
            .sum()
    }

    #[test]
    fn user_message_clamps_to_width_with_ellipsis() {
        let long = format!("START{}END", "A".repeat(200));
        let lines = render_user_message(&long, &theme(), Some(40));
        let text = joined(&lines);
        assert!(text.contains('…'), "no marker in {text:?}");
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40: {l:?}", line_width(l));
        }
        // The `> ` prefix must survive so the row still reads as a user turn.
        assert!(text.starts_with("> "), "lost the prefix: {text:?}");
    }

    #[test]
    fn user_message_none_means_no_clamp() {
        let long = "A".repeat(500);
        let lines = render_user_message(&long, &theme(), None);
        assert!(
            !joined(&lines).contains('…'),
            "None must mean no width clamp"
        );
    }

    #[test]
    fn user_message_clamps_every_continuation_line() {
        let text = format!("first\n{}", "b".repeat(300));
        let lines = render_user_message(&text, &theme(), Some(40));
        assert_eq!(lines.len(), 2);
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40: {l:?}", line_width(l));
        }
        assert!(joined(&lines).contains('…'), "second line not marked");
    }

    #[test]
    fn tool_call_keeps_the_name_and_marks_the_args() {
        // The regression this fixes: at 80 columns, 120 bytes of ASCII
        // args put the `…` at column 120 — off-screen. The user saw a
        // hard cut with no marker.
        let args = "x".repeat(200);
        let lines = render_tool_call("bash", &args, &theme(), Some(80));
        let text = joined(&lines);
        assert!(
            text.starts_with("▸ bash "),
            "name must stay visible: {text:?}"
        );
        assert!(text.contains('…'), "no marker: {text:?}");
        assert_eq!(line_width(&lines[0]), 80, "must fill exactly: {text:?}");
    }

    #[test]
    fn tool_call_respects_cjk_cell_width() {
        // 60 CJK args = 180 bytes = 120 cells. On an 80-cell line only
        // 73 cells of args fit after the 7-cell "▸ grep " prefix.
        let args = "文".repeat(60);
        let lines = render_tool_call("grep", &args, &theme(), Some(80));
        let text = joined(&lines);
        assert!(text.contains('…'), "no marker: {text:?}");
        assert_eq!(
            line_width(&lines[0]),
            80,
            "CJK must be measured in cells, not bytes: width {} in {text:?}",
            line_width(&lines[0])
        );
    }

    #[test]
    fn tool_call_at_starvation_width_still_marks_args() {
        // Terminal narrower than "▸ " + a long tool name: we must not
        // silently drop the args without a marker.
        let args = "x".repeat(200);
        let lines = render_tool_call("a_very_long_tool_name", &args, &theme(), Some(8));
        assert!(
            joined(&lines).contains('…'),
            "no marker: {joined:?}",
            joined = joined(&lines)
        );
    }

    #[test]
    fn tool_call_short_args_get_no_marker() {
        let lines = render_tool_call("read", r#"{"path":"a.rs"}"#, &theme(), Some(80));
        let text = joined(&lines);
        assert!(
            !text.contains('…'),
            "must not mark untruncated args: {text:?}"
        );
    }

    #[test]
    fn assistant_message_clamps_long_paragraphs() {
        let long = format!("START {} END", "c".repeat(300));
        let lines = render_assistant_message(&long, &theme(), Some(40));
        assert!(!lines.is_empty());
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40", line_width(l));
        }
        assert!(joined(&lines).contains('…'), "no marker");
    }

    #[test]
    fn assistant_message_clamps_markdown_constructs() {
        // Code fences are the widest thing markdown emits; make sure
        // they get clamped too and don't blow the budget.
        let md = "```rust\nfn main() { println!(\"hello world, this is long\"); }\n```";
        let lines = render_assistant_message(md, &theme(), Some(30));
        for l in &lines {
            assert!(line_width(l) <= 30, "width {} > 30: {l:?}", line_width(l));
        }
    }

    #[test]
    fn assistant_message_none_means_no_clamp() {
        let long = "c".repeat(300);
        let lines = render_assistant_message(&long, &theme(), None);
        assert!(!joined(&lines).contains('…'), "None must mean no clamp");
    }

    #[test]
    fn bash_execution_clamps_header_and_body() {
        let out = "q".repeat(1500);
        let lines =
            render_bash_execution("echo hi", &out, "", true, Some(0), 5, &theme(), Some(40));
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40: {l:?}", line_width(l));
        }
        assert!(joined(&lines).contains('…'), "no marker");
    }

    #[test]
    fn bash_execution_clamps_stderr_too() {
        let lines = render_bash_execution(
            "cmd",
            "ok",
            &"e".repeat(500),
            false,
            Some(1),
            5,
            &theme(),
            Some(40),
        );
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40: {l:?}", line_width(l));
        }
    }

    #[test]
    fn bash_execution_clamps_a_long_command_header() {
        let lines = render_bash_execution(
            &"c".repeat(300),
            "out",
            "",
            true,
            Some(0),
            1,
            &theme(),
            Some(40),
        );
        for l in &lines {
            assert!(line_width(l) <= 40, "width {} > 40: {l:?}", line_width(l));
        }
        assert!(joined(&lines).contains('…'), "no marker on the header");
    }
}
