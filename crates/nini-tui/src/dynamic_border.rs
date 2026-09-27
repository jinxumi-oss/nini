//! `DynamicBorder` — a horizontal rule that adapts to the rendered width.
//!
//! Mirrors Pi's `DynamicBorder` (pi-tui/dist/components/dynamic-border.js):
//! ```typescript
//! export class DynamicBorder {
//!     render(width) { return [this.color("\u{2500}".repeat(Math.max(1, width)))]; }
//! }
//! ```
//!
//! Usage:
//! ```ignore
//! let border = DynamicBorder::new("borderMuted");
//! let line = border.render(60, &theme);
//! // "\x1b[38;5;...m────────────────────────────────────────────────────────\x1b[0m"
//! ```

use crate::theme::Theme;
use ratatui::text::{Line, Span};

/// Horizontal rule that always fills the given width.
///
/// `color` is a theme slot name (e.g., `"borderMuted"`, `"borderAccent"`,
/// `"mdHr"`). The rendered output is a single `Span` containing `─` (U+0001)
/// repeated `width` times, styled with that color.
#[derive(Debug, Clone, Copy)]
pub struct DynamicBorder {
    /// Theme slot name for the border color.
    pub color: &'static str,
}

impl DynamicBorder {
    pub const fn new(color: &'static str) -> Self {
        Self { color }
    }

    /// Render to a `Line` (one row) suitable for ratatui widgets.
    pub fn render(&self, width: usize, theme: &Theme) -> Line<'static> {
        let w = width.max(1);
        let spans = vec![Span::styled(
            "\u{2500}".repeat(w),
            theme.fg_style(self.color),
        )];
        Line::from(spans)
    }

    /// Render to a raw ANSI string (for direct print / export).
    pub fn render_string(&self, width: usize, theme: &Theme) -> String {
        let w = width.max(1);
        theme.fg(self.color, &"\u{2500}".repeat(w))
    }

    /// A horizontal rule with a centered label, e.g. `──── code ────`.
    ///
    /// The label is placed in the middle; remaining columns are filled
    /// with `─` to the full width. Used for Pi-style ` ``` ` fenced code.
    pub fn render_with_label(&self, width: usize, label: &str, theme: &Theme) -> Line<'static> {
        let w = width.max(1);
        let label_len = label.chars().count();
        let half = w.saturating_sub(label_len + 2) / 2;
        let rest = w.saturating_sub(half).saturating_sub(label_len + 2);

        let mut spans = Vec::new();
        spans.push(Span::styled(
            "\u{2500}".repeat(half.max(1)),
            theme.fg_style(self.color),
        ));
        spans.push(Span::raw(format!(" {label} ")));
        spans.push(Span::styled(
            "\u{2500}".repeat(rest.max(1)),
            theme.fg_style(self.color),
        ));
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn render_fills_full_width() {
        let b = DynamicBorder::new("borderMuted");
        let line = b.render(40, &Theme::dark());
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(joined.chars().count(), 40);
        assert!(joined.chars().all(|c| c == '\u{2500}'));
    }

    #[test]
    fn render_min_width_is_one() {
        let b = DynamicBorder::new("borderMuted");
        let line = b.render(0, &Theme::dark());
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(joined.chars().count(), 1);
    }

    #[test]
    fn render_with_label_centers_text() {
        let b = DynamicBorder::new("mdCodeBlockBorder");
        let line = b.render_with_label(20, "rust", &Theme::dark());
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // Format: "─── rust ──────────" — dashed sides around centered label
        assert!(joined.starts_with('\u{2500}'));
        assert!(joined.contains(" rust "));
        assert!(joined.ends_with('\u{2500}'));
    }

    #[test]
    fn render_string_emits_ansi() {
        let b = DynamicBorder::new("borderMuted");
        let out = b.render_string(10, &Theme::dark());
        assert!(out.starts_with("\x1b["));
        assert!(out.ends_with("\x1b[0m"));
        // 10 dashes + reset
        assert!(out.contains("\u{2500}".repeat(10).as_str()));
    }
}