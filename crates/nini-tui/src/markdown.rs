//! Markdown rendering for the transcript.
//!
//! Supports the subset the LLM is likely to emit in chat responses:
//! - ATX headings `# … ######`
//! - Bulleted lists `- item` and ordered lists `1. item`
//! - Task lists `- [ ] todo` and `- [x] done`
//! - Blockquotes `> quoted text`
//! - Fenced code blocks ``` ```lang ... ``` ``` (multi-line, kept verbatim)
//! - Inline code `` `x` ``, bold `**x**`, italic `*x*`/`_x_`
//! - Links `[label](url)` rendered as plain text + url hint
//! - Hard line breaks (two-space suffix) and paragraphs (blank-line separated)

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Maximum line width before we soft-wrap at word boundaries.
const MAX_LINE_WIDTH: usize = 100;

/// Render markdown text into styled ratatui lines.
pub fn render_markdown(s: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code_block: Option<String> = None;
    let mut code_buffer: Vec<String> = Vec::new();

    for raw in s.lines() {
        if let Some(rest) = strip_fence_open(raw) {
            if let Some(lang) = in_code_block.take() {
                flush_code_block(&mut out, &mut code_buffer, &lang, theme);
            }
            in_code_block = Some(rest.to_string());
            code_buffer.clear();
            continue;
        }
        if let Some(true) = in_code_block.as_ref().map(|_| strip_fence_close(raw)) {
            let lang = in_code_block.take().unwrap_or_default();
            flush_code_block(&mut out, &mut code_buffer, &lang, theme);
            continue;
        }
        if in_code_block.is_some() {
            code_buffer.push(raw.to_string());
            continue;
        }

        if raw.trim().is_empty() {
            out.push(Line::from(""));
            continue;
        }

        if let Some(_rest) = raw.strip_prefix("###### ")
            .or_else(|| raw.strip_prefix("##### "))
            .or_else(|| raw.strip_prefix("#### "))
            .or_else(|| raw.strip_prefix("### "))
            .or_else(|| raw.strip_prefix("## "))
            .or_else(|| raw.strip_prefix("# "))
        {
            out.push(Line::from(vec![Span::styled(
                raw.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
            continue;
        }

        if let Some(rest) = raw.strip_prefix("> ") {
            out.push(Line::from(vec![
                Span::styled("│ ", theme.fg_style("dim")),
                Span::styled(rest.to_string(), theme.fg_style("dim")),
            ]));
            continue;
        }

        if let Some(rest) = raw.strip_prefix("- [ ] ").or_else(|| raw.strip_prefix("- [x] ")) {
            let done = raw.starts_with("- [x] ");
            let mark = if done { "☑ " } else { "☐ " };
            out.push(Line::from(vec![
                Span::raw("  ".to_string()),
                Span::styled(
                    mark.to_string(),
                    if done {
                        theme.fg_style("success")
                    } else {
                        theme.fg_style("dim")
                    },
                ),
                Span::styled(rest.to_string(), theme.fg_style("default")),
            ]));
            continue;
        }

        if let Some(rest) = raw.strip_prefix("- ") {
            let first_line = Line::from(vec![
                Span::raw("  • ".to_string()),
                Span::styled(rest.to_string(), theme.fg_style("default")),
            ]);
            out.push(first_line);
            continue;
        }

        if is_ordered_list(raw) {
            out.push(Line::from(vec![
                Span::styled(raw.to_string(), theme.fg_style("default")),
            ]));
            continue;
        }

        let line = parse_inline(raw, theme, Style::default());
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        if joined.chars().count() > MAX_LINE_WIDTH {
            out.extend(wrap_line(line, MAX_LINE_WIDTH));
        } else {
            out.push(line);
        }
    }

    if let Some(lang) = in_code_block.take() {
        flush_code_block(&mut out, &mut code_buffer, &lang, theme);
    }

    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// Detect ``` ``` ` or ``` ```lang ` open fence. Returns the language (or "").
fn strip_fence_open(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("```") {
        return None;
    }
    let rest = &trimmed[3..];
    if rest.chars().any(|c| c == '`') {
        return None;
    }
    Some(rest.trim())
}

/// Detect ``` ``` ` close fence.
fn strip_fence_close(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with("```") {
        return false;
    }
    trimmed[3..].chars().all(|c| c != '`')
}

fn flush_code_block(out: &mut Vec<Line<'static>>, buf: &mut Vec<String>, lang: &str, theme: &Theme) {
    let header = if lang.is_empty() {
        "------- code -------".to_string()
    } else {
        format!("------- {} -------", lang)
    };
    out.push(Line::from(vec![Span::styled(
        header,
        theme.fg_style("dim"),
    )]));
    let code_style = theme.fg_style("dim");
    for line in buf.drain(..) {
        let content = if line.is_empty() { " ".to_string() } else { line };
        out.push(Line::from(vec![Span::styled(content, code_style)]));
    }
    out.push(Line::from(vec![Span::styled(
        "---------------------".to_string(),
        theme.fg_style("dim"),
    )]));
}

/// Detect `1. `, `2. `, ... `99. ` ordered list marker.
fn is_ordered_list(line: &str) -> bool {
    let mut chars = line.chars();
    let mut saw_digit = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            saw_digit = true;
            continue;
        }
        if c == '.' && saw_digit {
            return chars.next() == Some(' ');
        }
        return false;
    }
    false
}

/// Scan inline markdown in `s` and produce a styled Line.
pub(crate) fn parse_inline(s: &str, theme: &Theme, base: Style) -> Line<'static> {
    let bytes = s.as_bytes();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < bytes.len() {
        // Inline code `x`
        if bytes[i] == b'`' {
            if let Some(end_rel) = find_closing_backtick(&bytes[i + 1..]) {
                let code = &s[i + 1..i + 1 + end_rel];
                flush_plain(&mut buf, &mut spans, base);
                spans.push(Span::styled(
                    code.to_string(),
                    base.patch(theme.fg_style("code")).add_modifier(Modifier::DIM),
                ));
                i = i + 1 + end_rel + 1;
                continue;
            }
        }

        // Bold **x**
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end_rel) = find_closing_double(&bytes[i + 2..]) {
                let inner = &s[i + 2..i + 2 + end_rel];
                flush_plain(&mut buf, &mut spans, base);
                spans.push(Span::styled(
                    inner.to_string(),
                    base.add_modifier(Modifier::BOLD),
                ));
                i = i + 2 + end_rel + 2;
                continue;
            }
        }

        // Italic *x* or _x_
        if bytes[i] == b'*' || bytes[i] == b'_' {
            let marker = bytes[i];
            if let Some(end_rel) = find_closing_single(&bytes[i + 1..], marker) {
                let inner = &s[i + 1..i + 1 + end_rel];
                if !inner.is_empty() && !inner.contains('\n') {
                    flush_plain(&mut buf, &mut spans, base);
                    spans.push(Span::styled(
                        inner.to_string(),
                        base.add_modifier(Modifier::ITALIC),
                    ));
                    i = i + 1 + end_rel + 1;
                    continue;
                }
            }
        }

        // Link [label](url)
        if bytes[i] == b'[' {
            if let Some((label, url)) = parse_link(&s[i..]) {
                flush_plain(&mut buf, &mut spans, base);
                spans.push(Span::styled(
                    label.to_string(),
                    base.patch(theme.fg_style("link")).add_modifier(Modifier::UNDERLINED),
                ));
                spans.push(Span::styled(
                    format!(" ({})", url),
                    base.patch(theme.fg_style("dim")),
                ));
                i += 1 + label.len() + 2 + url.len() + 1;
                continue;
            }
        }

        let ch = s[i..].chars().next().unwrap();
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush_plain(&mut buf, &mut spans, base);
    Line::from(spans)
}

fn flush_plain(buf: &mut String, spans: &mut Vec<Span<'static>>, base: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), base));
    }
}

fn find_closing_backtick(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_double(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_single(bytes: &[u8], marker: u8) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == marker {
            // CommonMark: an emphasis opener must NOT be preceded by
            // whitespace, OR followed by whitespace/punctuation. We
            // accept either side (CommonMark's left-flanking/right-flanking
            // rules simplified): reject only if BOTH sides are whitespace.
            let prev_space = i == 0 || bytes[i - 1].is_ascii_whitespace();
            let next_space = i + 1 >= bytes.len() || bytes[i + 1].is_ascii_whitespace();
            if !(prev_space && next_space) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn parse_link(s: &str) -> Option<(&str, &str)> {
    if !s.starts_with('[') {
        return None;
    }
    let close_bracket = s.find(']')?;
    if close_bracket + 1 >= s.len() || s.as_bytes()[close_bracket + 1] != b'(' {
        return None;
    }
    let close_paren = s[close_bracket + 2..].find(')')?;
    let label = &s[1..close_bracket];
    let url = &s[close_bracket + 2..close_bracket + 2 + close_paren];
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url))
}

fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_len = 0usize;

    for span in line.spans.into_iter() {
        let text = span.content.into_owned();
        let style = span.style;
        for word in text.split_whitespace() {
            let word_len = word.chars().count() + 1;
            if current_len + word_len > width && !current_spans.is_empty() {
                out.push(Line::from(std::mem::take(&mut current_spans)));
                current_len = 0;
            }
            if current_len > 0 {
                current_spans.push(Span::raw(" ".to_string()));
                current_len += 1;
            }
            current_spans.push(Span::styled(word.to_string(), style));
            current_len += word.chars().count();
        }
    }
    if !current_spans.is_empty() {
        out.push(Line::from(current_spans));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn renders_heading() {
        let lines = render_markdown("# Title\n", &theme());
        assert!(!lines.is_empty());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.starts_with("# "));
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn renders_bullet_with_marker() {
        let lines = render_markdown("- item 1\n- item 2\n", &theme());
        assert!(lines.len() >= 2);
        let first: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(first.contains("• item 1"));
    }

    #[test]
    fn renders_bold_inline() {
        let lines = render_markdown("this is **bold** text\n", &theme());
        let has_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_bold, "expected a bold span");
    }

    #[test]
    fn renders_italic_inline() {
        let lines = render_markdown("this is *italic* text\n", &theme());
        let has_italic = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
        assert!(has_italic, "expected an italic span");
    }

    #[test]
    fn renders_italic_with_underscore() {
        let lines = render_markdown("this is _also italic_\n", &theme());
        let has_italic = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
        assert!(has_italic, "underscore italic should also work");
    }

    #[test]
    fn renders_inline_code() {
        let lines = render_markdown("use `foo()` to call\n", &theme());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("foo()"), "inline code lost: {joined}");
    }

    #[test]
    fn renders_inline_link() {
        let lines = render_markdown("see [docs](https://example.com) here\n", &theme());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("docs"));
        assert!(joined.contains("https://example.com"));
    }

    #[test]
    fn renders_task_list_done_and_pending() {
        let lines = render_markdown("- [ ] pending\n- [x] done\n", &theme());
        assert!(lines.len() >= 2);
        let pending: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let done: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(pending.contains("\u{2610}"));
        assert!(done.contains("\u{2611}"));
    }

    #[test]
    fn renders_blockquote() {
        let lines = render_markdown("> quoted\n", &theme());
        let joined: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("quoted"));
    }

    #[test]
    fn renders_fenced_code_block() {
        let lines = render_markdown("```rust\nfn main() {}\n```\n", &theme());
        let joined: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("code"), "code fence header missing");
        assert!(joined.contains("fn main() {}"), "code body missing");
    }

    #[test]
    fn renders_ordered_list() {
        let lines = render_markdown("1. first\n2. second\n", &theme());
        assert!(lines.len() >= 2);
        let first: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(first.contains("1. first"));
    }

    #[test]
    fn empty_input_yields_blank_line() {
        let lines = render_markdown("", &theme());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn heading_levels_recognized() {
        let lines = render_markdown(
            "# h1\n## h2\n### h3\n#### h4\n##### h5\n###### h6\n",
            &theme(),
        );
        assert_eq!(lines.len(), 6);
        for line in &lines {
            assert!(
                line.spans[0].style.add_modifier.contains(Modifier::BOLD),
                "every heading level should be bold"
            );
        }
    }
}


fn has_modifier(style: &Style, want: Modifier) -> bool {
    style.add_modifier.contains(want)
}
