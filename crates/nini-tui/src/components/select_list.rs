//! `SelectList` — the shared selection list used by autocomplete, command
//! palette, and 6 selector overlays.
//!
//! Mirrors Pi's `SelectList` (pi-tui/dist/components/select-list.js):
//!
//! ```typescript
//! export class SelectList {
//!     constructor(items, maxVisible, theme, layout = {}) { ... }
//!     render(width) { ... }
//!     handleInput(keyData) { ... }
//! }
//! ```
//!
//! Layout:
//!   - Two-column layout when items have a `description`: primary column
//!     (item label) + description column (indented). PRIMARY_COLUMN_WIDTH
//!     = 32 chars, gap = 2, min description width = 10 (Pi defaults).
//!   - Selected row uses `accent` foreground; description in `muted`.
//!   - Scroll indicator `(N/M)` when items overflow.
//!   - Wrap-around Up/Down navigation.

use ratatui::text::Span;
use serde::{Deserialize, Serialize};

/// Default primary column width — matches Pi's `DEFAULT_PRIMARY_COLUMN_WIDTH`.
pub const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
/// Gap between primary and description columns.
pub const PRIMARY_COLUMN_GAP: usize = 2;
/// Minimum description column width (otherwise collapsed).
pub const MIN_DESCRIPTION_WIDTH: usize = 10;

/// A single item in the select list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectItem {
    /// Unique identifier (e.g. command id, model id).
    pub value: String,
    /// Display label (e.g. `/model`, "claude-sonnet").
    pub label: String,
    /// Optional secondary line shown dim after the label.
    pub description: Option<String>,
    /// True when this item is the currently-active value.
    pub is_current: bool,
}

impl SelectItem {
    /// Convenience constructor.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
            is_current: false,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn current(mut self) -> Self {
        self.is_current = true;
        self
    }
}

/// Render hint returned by `render()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListRender {
    pub lines: Vec<Vec<Span<'static>>>,
    pub selected_index: usize,
    pub total: usize,
    pub scroll_info: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SelectList {
    items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    scroll_offset: usize,
}

impl SelectList {
    pub fn new(items: Vec<SelectItem>, max_visible: usize) -> Self {
        Self {
            items,
            selected_index: 0,
            max_visible: max_visible.max(1),
            scroll_offset: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn selected(&self) -> Option<&SelectItem> {
        self.items.get(self.selected_index)
    }

    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    /// Move the cursor by `delta` with wrap-around.
    pub fn move_by(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as i32;
        let mut idx = self.selected_index as i32 + delta;
        if idx < 0 {
            idx += len;
        } else if idx >= len {
            idx -= len;
        }
        self.selected_index = idx as usize;
        self.recompute_scroll();
    }

    /// Jump to a specific index (clamped).
    pub fn set_selected(&mut self, index: usize) {
        if self.items.is_empty() {
            return;
        }
        self.selected_index = index.min(self.items.len() - 1);
        self.recompute_scroll();
    }

    fn recompute_scroll(&mut self) {
        let max = self.max_visible;
        // Keep the selected index within the visible window, biased
        // toward the center when possible.
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + max {
            self.scroll_offset = self.selected_index + 1 - max;
        }
    }

    /// Render the list into styled spans. Each item is one row (one
    /// Vec<Span>). Caller wraps each row in a `Line` and feeds to a
    /// `Paragraph` widget.
    ///
    /// `selected_color` and `description_color` are theme slot names.
    pub fn render_spans(
        &self,
        width: usize,
        theme: &crate::theme::Theme,
        selected_color: &str,
        description_color: &str,
        prefix_color: &str,
        scroll_info_color: &str,
    ) -> SelectListRender {
        if self.items.is_empty() {
            return SelectListRender {
                lines: vec![vec![Span::styled(
                    "  No matching items".to_string(),
                    theme.fg_style("muted"),
                )]],
                selected_index: 0,
                total: 0,
                scroll_info: None,
            };
        }

        let has_any_desc = self.items.iter().any(|i| i.description.is_some());
        // Pick the primary column width: 32 by default, shrink if
        // there's not enough space.
        let primary_width = if has_any_desc {
            width.saturating_sub(PRIMARY_COLUMN_GAP + MIN_DESCRIPTION_WIDTH)
                .min(DEFAULT_PRIMARY_COLUMN_WIDTH)
                .max(8)
        } else {
            width.saturating_sub(2)
        };

        let total = self.items.len();
        let start = self.scroll_offset;
        let end = (start + self.max_visible).min(total);
        let mut lines = Vec::with_capacity(end - start + 1);

        for (i, item) in self.items.iter().enumerate().take(end).skip(start) {
            let is_selected = i == self.selected_index;
            let mut row: Vec<Span<'static>> = Vec::new();
            // Prefix: "  " for unselected, "▶ " or "> " for selected.
            let prefix = if is_selected { "\u{25b6} " } else { "  " };
            row.push(Span::styled(
                prefix.to_string(),
                if is_selected {
                    theme.fg_style(selected_color).add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    theme.fg_style(prefix_color)
                },
            ));
            // Label (truncate to primary_width if needed).
            let label_disp = if item.label.chars().count() > primary_width {
                let mut s: String = item.label.chars().take(primary_width.saturating_sub(1)).collect();
                s.push('\u{2026}');
                s
            } else {
                item.label.clone()
            };
            let label_style = if is_selected {
                theme
                    .fg_style(selected_color)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                theme.fg_style("text")
            };
            row.push(Span::styled(label_disp, label_style));
            // Description (if present).
            if let Some(desc) = &item.description {
                // Pad to align columns.
                let pad = primary_width.saturating_sub(item.label.chars().count()) + PRIMARY_COLUMN_GAP;
                row.push(Span::raw(" ".repeat(pad)));
                let desc_disp = if desc.chars().count() > width.saturating_sub(primary_width + 6) {
                    let mut s: String = desc.chars().take(width.saturating_sub(primary_width + 5)).collect();
                    s.push('\u{2026}');
                    s
                } else {
                    desc.clone()
                };
                row.push(Span::styled(desc_disp, theme.fg_style(description_color)));
            }
            lines.push(row);
        }

        let scroll_info = if start > 0 || end < total {
            Some(format!("  ({}/{})", self.selected_index + 1, total))
        } else {
            None
        };
        let scroll_span = scroll_info
            .as_ref()
            .map(|s| Span::styled(s.clone(), theme.fg_style(scroll_info_color)));

        SelectListRender {
            lines,
            selected_index: self.selected_index,
            total,
            scroll_info: scroll_span.map(|_| scroll_info.unwrap_or_default()),
        }
    }

    /// Helper: consume the scroll_info and return it as an Option<Vec<Span>>.
    pub fn scroll_info_span(&self, theme: &crate::theme::Theme, color: &str) -> Option<Vec<Span<'static>>> {
        if self.items.is_empty() {
            return None;
        }
        let total = self.items.len();
        let start = self.scroll_offset;
        let end = (start + self.max_visible).min(total);
        if start == 0 && end == total {
            return None;
        }
        Some(vec![Span::styled(
            format!("  ({}/{})", self.selected_index + 1, total),
            theme.fg_style(color),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn items() -> Vec<SelectItem> {
        vec![
            SelectItem {
                value: "model".into(),
                label: "/model".into(),
                description: Some("switch model".into()),
                is_current: false,
            },
            SelectItem {
                value: "help".into(),
                label: "/help".into(),
                description: Some("show help".into()),
                is_current: false,
            },
        ]
    }

    #[test]
    fn empty_list_no_panic() {
        let list = SelectList::new(vec![], 5);
        let out = list.render_spans(80, &Theme::dark(), "accent", "muted", "dim", "muted");
        assert_eq!(out.lines.len(), 1);
    }

    #[test]
    fn render_emits_one_row_per_visible_item() {
        let list = SelectList::new(items(), 5);
        let out = list.render_spans(80, &Theme::dark(), "accent", "muted", "dim", "muted");
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.total, 2);
    }

    #[test]
    fn wrap_around_up_from_top() {
        let mut list = SelectList::new(items(), 5);
        list.move_by(-1);
        assert_eq!(list.selected_index(), 1);
    }

    #[test]
    fn wrap_around_down_from_bottom() {
        let mut list = SelectList::new(items(), 5);
        list.set_selected(1);
        list.move_by(1);
        assert_eq!(list.selected_index(), 0);
    }

    #[test]
    fn selected_index_clamped() {
        let mut list = SelectList::new(items(), 5);
        list.set_selected(99);
        assert_eq!(list.selected_index(), 1);
    }

    #[test]
    fn scroll_info_appears_when_overflowing() {
        let many: Vec<SelectItem> = (0..20)
            .map(|i| SelectItem {
                value: format!("item{i}"),
                label: format!("item{i}"),
                description: None,
                is_current: false,
            })
            .collect();
        let list = SelectList::new(many, 5);
        let scroll = list.scroll_info_span(&Theme::dark(), "muted");
        assert!(scroll.is_some(), "expected scroll info when items > max_visible");
        let s = scroll.unwrap();
        let joined: String = s.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(joined.contains("(1/20)"));
    }
}