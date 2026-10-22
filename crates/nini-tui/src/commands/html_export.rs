//! v0.8.3: Render transcript as minimal HTML document for /export.
//!
//! Pure function — input is a slice of `TranscriptLine`, output is
//! a self-contained HTML string.

use crate::state::TranscriptLine;

/// Escape HTML special characters: `&`, `<`, `>`, `"`, `'`.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
                out.push_str(&format!(
                    "<p class=\"user\"><b>&gt;</b> {}</p>\n",
                    html_escape(t)
                ));
            }
            TranscriptLine::AssistantText(t) => {
                out.push_str(&format!("<p class=\"assistant\">{}</p>\n", html_escape(t)));
            }
            TranscriptLine::ToolCall { name, args, .. } => {
                // The tool name and the arguments come from the model:
                // a `<script>` in the arguments is markup, not text.
                out.push_str(&format!(
                    "<p class=\"tool\">[tool call] {} {}</p>\n",
                    html_escape(name),
                    html_escape(args)
                ));
            }
            TranscriptLine::BashExecution {
                cmd,
                output,
                stderr,
                ok,
                exit_code,
                duration_ms,
                ..
            } => {
                let status = if *ok { "ok" } else { "fail" };
                let escaped_output = html_escape(output);
                let stderr_html = if stderr.trim().is_empty() {
                    String::new()
                } else {
                    let escaped_stderr = html_escape(stderr);
                    format!("<pre class=\"bash-stderr\">{escaped_stderr}</pre>")
                };
                // The command line is the model's own text: it is
                // escaped like every other piece of content that reaches
                // the export.
                let escaped_cmd = html_escape(cmd);
                let exit = exit_code.map(|c| format!(" exit={c}")).unwrap_or_default();
                out.push_str(&format!(
                    "<p class=\"bash\">! <code>{escaped_cmd}</code> [{status}{exit}] in {duration_ms}ms<br><pre>{escaped_output}</pre>{stderr_html}</p>\n"
                ));
            }
            TranscriptLine::ToolResult { ok, content, .. } => {
                let cls = "tool";
                let label = if *ok { "tool result" } else { "tool error" };
                out.push_str(&format!(
                    "<p class=\"{cls}\">[{label}] {}</p>
",
                    html_escape(content)
                ));
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
            // v0.8.4 (ux-001): surface stop notices in the exported
            // HTML so an exported session record reflects aborted /
            // truncated turns honestly.
            TranscriptLine::StopNotice(msg) => {
                // The stop reason comes from the provider: it is
                // external input, and the export is a document the user
                // opens in a browser.
                out.push_str(&format!(
                    "<p class=\"stop-notice\" style=\"color:#c33\">⚠ {}</p>\n",
                    html_escape(msg)
                ));
            }
            // v0.10.3: a system notice the TUI produced. Exported like
            // a stop notice, but in its own class so a reader of the
            // export can tell "the TUI said this" from "the model said
            // this".
            TranscriptLine::Notice(msg) => {
                // A notice can carry model output (the `/copy` fallback
                // puts the assistant's text in one), so it is escaped
                // like every other piece of content that reaches the
                // export. Without this, a `<script>` in a model answer
                // would land in the exported HTML as markup.
                out.push_str(&format!(
                    "<p class=\"notice\" style=\"color:#888\">⚠ {}</p>\n",
                    html_escape(msg)
                ));
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

    /// The export is a document the user opens in a browser. Every
    /// piece of content that reaches it is either the user's, the
    /// model's, or the provider's — and all three can carry markup.
    ///
    /// The arms that were missed are the ones that carry *tool* text:
    /// the command the model asked for, the arguments it passed, and
    /// the stop reason the provider sent. A model that is asked to
    /// `echo '<script>...'` produces exactly that string, and the
    /// export used to carry it as markup.
    #[test]
    fn html_escapes_every_arm_that_carries_external_text() {
        let lines = vec![
            crate::state::TranscriptLine::ToolCall {
                id: "1".into(),
                name: "bash".into(),
                args: r#"{"command":"echo '<script>alert(1)</script>'"}"#.into(),
                collapsed: false,
            },
            crate::state::TranscriptLine::BashExecution {
                id: "1".into(),
                cmd: "echo '<script>alert(2)</script>'".into(),
                output: "<b>out</b>".into(),
                stderr: "<i>err</i>".into(),
                ok: true,
                exit_code: Some(0),
                duration_ms: 1,
                collapsed: false,
            },
            crate::state::TranscriptLine::StopNotice("<script>alert(3)</script>".into()),
        ];
        let html = render_transcript_html(&lines);
        assert!(
            !html.contains("<script>"),
            "the export carries live markup:\n{html}"
        );
        assert!(
            !html.contains("<b>out</b>") && !html.contains("<i>err</i>"),
            "the tool output was not escaped:\n{html}"
        );
        assert!(html.contains("&lt;script&gt;"));
    }

    /// A quote is not markup, but it ends an HTML attribute. The
    /// escape covers it so that content which later moves into an
    /// attribute cannot break out of it.
    #[test]
    fn html_escape_covers_quotes() {
        let html = render_transcript_html(&[crate::state::TranscriptLine::User(
            "he said \"hi\" and 'bye'".into(),
        )]);
        assert!(html.contains("&quot;hi&quot;"));
        assert!(html.contains("&#39;bye&#39;"));
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
