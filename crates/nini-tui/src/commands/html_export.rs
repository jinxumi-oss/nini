//! v0.8.3: Render transcript as minimal HTML document for /export.
//!
//! Pure function — input is a slice of `TranscriptLine`, output is
//! a self-contained HTML string.

use crate::state::TranscriptLine;

/// Escape HTML special characters: `&`, `<`, `>`.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render the transcript as a minimal HTML document.
///
/// Used by `/export`. Includes inline CSS, no external assets. HTML
/// escapes all user content (`&`, `<`, `>`).
pub fn render_transcript_html(lines: &[TranscriptLine]) -> String {
    let mut out = String::from(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>nini session</title>\
         <style>body{font-family:system-ui;max-width:800px;margin:2em auto;padding:0 1em;}\
         .user{color:#0a7}.assistant{color:#333}.tool{color:#a3a;font-family:monospace}\
         .divider{border-top:1px solid #ccc;margin:1em 0}</style></head><body>\n",
    );
    for l in lines {
        match l {
            TranscriptLine::User(t) => {
                out.push_str(&format!("<p class=\"user\"><b>&gt;</b> {}</p>\n", html_escape(t)));
            }
            TranscriptLine::AssistantText(t) => {
                out.push_str(&format!("<p class=\"assistant\">{}</p>\n", html_escape(t)));
            }
            TranscriptLine::ToolCall { name, args, .. } => {
                out.push_str(&format!(
                    "<p class=\"tool\">[tool call] {name} {args}</p>\n"
                ));
            }
            TranscriptLine::BashExecution { cmd, output, stderr, ok, exit_code, duration_ms, .. } => {
                let status = if *ok { "ok" } else { "fail" };
                let escaped_output = output
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                let stderr_html = if stderr.trim().is_empty() {
                    String::new()
                } else {
                    let escaped_stderr = stderr
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    format!("<pre class=\"bash-stderr\">{escaped_stderr}</pre>")
                };
                out.push_str(&format!(
                    "<p class=\"bash\">! <code>{cmd}</code> [{status}{}] in {duration_ms}ms<br><pre>{escaped_output}</pre>{stderr_html}</p>\n",
                    exit_code.map(|c| format!(" exit={c}")).unwrap_or_default(),
                ));
            }
            TranscriptLine::ToolResult { ok, content, .. } => {
                let cls = "tool";
                let label = if *ok { "tool result" } else { "tool error" };
                out.push_str(&format!("<p class=\"{cls}\">[{label}] {}</p>
", html_escape(content)));
            }
            TranscriptLine::Divider => {
                out.push_str("<hr class=\"divider\">\n");
            }
            // v0.8: omit thinking content from HTML export by
            // default — it's model-internal noise. Users can
            // re-export with --include-thinking if they want it.
            TranscriptLine::ThinkingText(_) => {
                out.push_str("<p class=\"thinking\" style=\"color:#aaa;font-style:italic\">\n[thinking elided]\n</p>\n");
            }
        }
    }
    out.push_str("</body></html>\n");
    out
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_transcript_produces_valid_html() {
        let html = render_transcript_html(&[]);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.ends_with("</body></html>\n"));
    }

    #[test]
    fn html_escapes_user_content() {
        let lines = vec![crate::state::TranscriptLine::User(
            "<script>alert('xss')</script>".into(),
        )];
        let html = render_transcript_html(&lines);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn user_and_assistant_emit_different_classes() {
        let lines = vec![
            crate::state::TranscriptLine::User("hi".into()),
            crate::state::TranscriptLine::AssistantText("hello".into()),
        ];
        let html = render_transcript_html(&lines);
        assert!(html.contains(r#"class="user""#));
        assert!(html.contains(r#"class="assistant""#));
    }
}