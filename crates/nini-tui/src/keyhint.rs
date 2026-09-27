//! `keyHint` / `rawKeyHint` formatters — Pi-style footer key hints.
//!
//! Pi's pattern (from `dist/modes/interactive/components/keybinding-hints.js`):
//! ```typescript
//! export function keyHint(keybinding, description) {
//!     return theme.fg("dim", keyText(keybinding)) + theme.fg("muted", ` ${description}`);
//! }
//! ```
//!
//! In nini we use `dim` for the key (e.g. `Ctrl+C`) and `muted` for the
//! description (e.g. `quit`). When concatenated in the footer, the eye
//! picks up the dim key first, then the muted explanation. This is the
//! standard "VSCode-style" hint layout.

use crate::theme::Theme;
use ratatui::text::Span;

/// Render a `[dim]key + muted] description` pair as two Spans.
///
/// Use this to build the footer hint list. `key` is the keystroke text
/// (e.g. `Ctrl+C`), `description` is the short verb (e.g. `quit`).
pub fn key_hint_spans<'a>(theme: &Theme, key: &str, description: &str) -> Vec<Span<'a>> {
    vec![
        Span::styled(key.to_string(), theme.fg_style("dim")),
        Span::styled(format!(" {description}"), theme.fg_style("muted")),
    ]
}

/// Concatenated form (for snapshot tests).
pub fn key_hint_string(theme: &Theme, key: &str, description: &str) -> String {
    format!("{}{} {}", theme.fg("dim", key), theme.fg("muted", description), "")
}

/// Trim a hint list to fit within `max_width`. When truncation happens,
/// a dim "…" is appended. The trim prefers dropping whole hints over
/// truncating one mid-line — matches Pi's `truncateToWidth(statsLeft +
/// " ".repeat(...) + rightSide, width)`.
pub fn trim_to_width(hints: &[(&str, &str)], max_width: usize, theme: &Theme) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut used = 0usize;
    for (key, desc) in hints {
        // Per-line width is key + " " + desc.
        let line_w = key.chars().count() + 1 + desc.chars().count();
        // Always keep at least the first hint.
        if !out.is_empty() && used + line_w + 1 > max_width {
            // Truncate — append " …" rendered as muted.
            out.push((" \u{2026}".to_string(), String::new()));
            let _ = theme; // keep API symmetric with potential future tint
            break;
        }
        out.push((key.to_string(), desc.to_string()));
        used += line_w + 1; // +1 for the trailing space between hints
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn key_hint_emits_two_spans() {
        let spans = key_hint_spans(&Theme::dark(), "Ctrl+C", "quit");
        assert_eq!(spans.len(), 2);
        // First span is dim color, second is muted.
        assert_eq!(spans[0].content, "Ctrl+C");
        assert_eq!(spans[1].content, " quit");
    }

    #[test]
    fn trim_to_width_keeps_all_when_fits() {
        let hints = vec![(" F1 ", "help"), (" Enter ", "send")];
        let out = trim_to_width(&hints, 80, &Theme::dark());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, " F1 ");
    }

    #[test]
    fn trim_to_width_appends_ellipsis_when_truncated() {
        let hints = vec![
            (" F1 ", "help"),
            (" Enter ", "send"),
            (" Shift+Enter ", "newline"),
            (" Ctrl+L ", "model"),
            (" Ctrl+C ", "quit"),
        ];
        // Width 20 — only " F1  help" + ellipsis fits.
        let out = trim_to_width(&hints, 20, &Theme::dark());
        let last = out.last().unwrap();
        assert!(last.0.contains('\u{2026}'), "expected ellipsis, got {last:?}");
    }

    #[test]
    fn trim_to_width_always_keeps_first_hint() {
        let hints = vec![(" F1 ", "help"), (" Enter ", "send")];
        // Even if width is 1, we still emit the first hint.
        let out = trim_to_width(&hints, 1, &Theme::dark());
        assert!(!out.is_empty());
    }
}